//! Swarm cluster calls on a manager's engine.

use crate::{error::map_err, LocalDocker};
use async_trait::async_trait;
use bollard::models::{Node, NodeSpecRoleEnum, Service, Task};
use bollard::query_parameters::{
    ListConfigsOptions, ListNetworksOptionsBuilder, ListNodesOptions, ListSecretsOptions,
    ListServicesOptionsBuilder, ListTasksOptions, LogsOptionsBuilder,
};
use fleetops_core::streams::{BoxStream, DockerEvent, LogLine, LogOptions, StreamOps};
use fleetops_core::swarm::{
    ConfigSummary, NodeDetail, NodeRole, NodeSummary, SecretSummary, ServiceDetail,
    ServiceSummary, SwarmInfo, SwarmNetwork, SwarmOps, TaskSummary, STACK_LABEL,
};
use fleetops_core::{Error, Result};
use futures_util::StreamExt;
use std::collections::{BTreeMap, HashMap};

/// Unix seconds of a Docker timestamp, whatever type bollard gives it.
fn unix<T: serde::Serialize>(d: &Option<T>) -> i64 {
    d.as_ref()
        .and_then(|d| serde_json::to_value(d).ok())
        .and_then(|v| v.as_str()?.parse::<jiff::Timestamp>().ok())
        .map(|t| t.as_second())
        .unwrap_or(0)
}

/// Lowercase name of a bollard enum, as Docker spells it.
fn name<T: serde::Serialize>(v: &Option<T>) -> Option<String> {
    v.as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.is_empty())
}

fn stack_of(labels: &Option<HashMap<String, String>>) -> Option<String> {
    labels.as_ref().and_then(|l| l.get(STACK_LABEL).cloned())
}

/// Who mounts what, by id: secrets, configs and networks per service name.
#[derive(Default)]
struct Uses {
    secrets: HashMap<String, Vec<String>>,
    configs: HashMap<String, Vec<String>>,
    networks: HashMap<String, Vec<String>>,
}

impl Uses {
    fn of(services: &[Service]) -> Self {
        let mut u = Uses::default();
        for svc in services {
            let Some(spec) = &svc.spec else { continue };
            let name = spec.name.clone().unwrap_or_default();
            let tmpl = spec.task_template.as_ref();
            let cs = tmpl.and_then(|t| t.container_spec.as_ref());
            for s in cs.and_then(|c| c.secrets.as_ref()).into_iter().flatten() {
                if let Some(id) = &s.secret_id {
                    u.secrets.entry(id.clone()).or_default().push(name.clone());
                }
            }
            for c in cs.and_then(|c| c.configs.as_ref()).into_iter().flatten() {
                if let Some(id) = &c.config_id {
                    u.configs.entry(id.clone()).or_default().push(name.clone());
                }
            }
            let nets = tmpl
                .and_then(|t| t.networks.as_ref())
                .or(spec.networks.as_ref());
            for n in nets.into_iter().flatten() {
                if let Some(id) = &n.target {
                    u.networks.entry(id.clone()).or_default().push(name.clone());
                }
            }
        }
        u
    }
}

fn service_summary(svc: &Service) -> ServiceSummary {
    let spec = svc.spec.clone().unwrap_or_default();
    let mode = spec.mode.clone().unwrap_or_default();
    let (mode_name, replicas) = if let Some(r) = &mode.replicated {
        ("replicated", Some(r.replicas.unwrap_or(0).max(0) as u64))
    } else if mode.global.is_some() {
        ("global", None)
    } else if mode.replicated_job.is_some() {
        ("replicated-job", None)
    } else if mode.global_job.is_some() {
        ("global-job", None)
    } else {
        ("replicated", Some(1))
    };
    let image = spec
        .task_template
        .as_ref()
        .and_then(|t| t.container_spec.as_ref())
        .and_then(|c| c.image.clone())
        .unwrap_or_default();
    let image = image.split('@').next().unwrap_or_default().to_string();
    let ports = svc
        .endpoint
        .as_ref()
        .and_then(|e| e.ports.clone())
        .or_else(|| spec.endpoint_spec.as_ref().and_then(|e| e.ports.clone()))
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            format!(
                "{}:{}/{}",
                p.published_port.unwrap_or(0),
                p.target_port.unwrap_or(0),
                name(&p.protocol).unwrap_or_else(|| "tcp".into())
            )
        })
        .collect();
    let status = svc.service_status.clone().unwrap_or_default();
    ServiceSummary {
        id: svc.id.clone().unwrap_or_default(),
        name: spec.name.clone().unwrap_or_default(),
        stack: stack_of(&spec.labels),
        mode: mode_name.into(),
        replicas_desired: replicas,
        tasks_running: status.running_tasks.unwrap_or(0),
        tasks_desired: status.desired_tasks.unwrap_or(0),
        image,
        ports,
        update_state: svc.update_status.as_ref().and_then(|u| name(&u.state)),
        updated_at: unix(&svc.updated_at),
    }
}

fn task_summary(t: Task, services: &HashMap<String, String>) -> TaskSummary {
    let status = t.status.unwrap_or_default();
    let service_id = t.service_id.unwrap_or_default();
    TaskSummary {
        service_name: services.get(&service_id).cloned().unwrap_or_default(),
        service_id,
        id: t.id.unwrap_or_default(),
        slot: t.slot.map(|s| s.max(0) as u64),
        node_id: t.node_id.filter(|n| !n.is_empty()),
        desired_state: name(&t.desired_state).unwrap_or_default(),
        state: name(&status.state).unwrap_or_default(),
        message: status.message.unwrap_or_default(),
        error: status.err.filter(|e| !e.is_empty()),
        container_id: status
            .container_status
            .and_then(|c| c.container_id)
            .filter(|c| !c.is_empty()),
        updated_at: unix(&t.updated_at),
        version: t.version.and_then(|v| v.index).unwrap_or(0),
    }
}

fn summary(n: Node) -> NodeSummary {
    let spec = n.spec.unwrap_or_default();
    NodeSummary {
        id: n.id.unwrap_or_default(),
        hostname: n.description.and_then(|d| d.hostname).unwrap_or_default(),
        role: match spec.role {
            Some(NodeSpecRoleEnum::MANAGER) => NodeRole::Manager,
            _ => NodeRole::Worker,
        },
        state: n
            .status
            .and_then(|s| s.state)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".into()),
        availability: spec.availability.map(|a| a.to_string()).unwrap_or_default(),
        leader: n.manager_status.and_then(|m| m.leader).unwrap_or(false),
    }
}

impl LocalDocker {
    async fn list_services(&self) -> Result<Vec<Service>> {
        let opts = ListServicesOptionsBuilder::new().status(true).build();
        self.client
            .list_services(Some(opts))
            .await
            .map_err(map_err)
    }
}

#[async_trait]
impl SwarmOps for LocalDocker {
    async fn swarm_info(&self) -> Result<SwarmInfo> {
        let s = self.client.inspect_swarm().await.map_err(map_err)?;
        let trust_root = s
            .tls_info
            .and_then(|t| t.trust_root)
            .filter(|r| !r.is_empty())
            .ok_or_else(|| Error::Internal("the swarm reported no root CA".into()))?;
        Ok(SwarmInfo {
            cluster_id: s.id.unwrap_or_default(),
            trust_root,
        })
    }

    async fn nodes(&self) -> Result<Vec<NodeSummary>> {
        let nodes = self
            .client
            .list_nodes(None::<ListNodesOptions>)
            .await
            .map_err(map_err)?;
        Ok(nodes.into_iter().map(summary).collect())
    }

    async fn node_detail(&self, id: &str) -> Result<Option<NodeDetail>> {
        let n = match self.client.inspect_node(id).await.map_err(map_err) {
            Ok(n) => n,
            Err(Error::Upstream { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        let d = n.description.clone().unwrap_or_default();
        let res = d.resources.clone().unwrap_or_default();
        let platform = d.platform.clone().unwrap_or_default();
        let ms = n.manager_status.clone();
        Ok(Some(NodeDetail {
            labels: n
                .spec
                .as_ref()
                .and_then(|s| s.labels.clone())
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            engine: d
                .engine
                .as_ref()
                .and_then(|e| e.engine_version.clone())
                .unwrap_or_default(),
            os: platform.os.unwrap_or_default(),
            arch: platform.architecture.unwrap_or_default(),
            nano_cpus: res.nano_cpus.unwrap_or(0),
            memory_bytes: res.memory_bytes.unwrap_or(0),
            addr: n
                .status
                .as_ref()
                .and_then(|s| s.addr.clone())
                .unwrap_or_default(),
            manager_addr: ms.as_ref().and_then(|m| m.addr.clone()),
            reachability: ms.as_ref().and_then(|m| name(&m.reachability)),
            summary: summary(n),
        }))
    }

    async fn services(&self) -> Result<Vec<ServiceSummary>> {
        Ok(self.list_services().await?.iter().map(service_summary).collect())
    }

    async fn service(&self, id: &str) -> Result<Option<ServiceDetail>> {
        let svc = match self.client.inspect_service(id, None).await.map_err(map_err) {
            Ok(s) => s,
            Err(Error::Upstream { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        let spec = svc.spec.clone().unwrap_or_default();
        Ok(Some(ServiceDetail {
            summary: service_summary(&svc),
            labels: spec.labels.clone().unwrap_or_default().into_iter().collect(),
            spec: serde_json::to_value(&spec).map_err(|e| Error::Internal(e.to_string()))?,
            update_status: svc
                .update_status
                .as_ref()
                .and_then(|u| serde_json::to_value(u).ok()),
        }))
    }

    async fn tasks(&self) -> Result<Vec<TaskSummary>> {
        let names: HashMap<String, String> = self
            .list_services()
            .await?
            .into_iter()
            .filter_map(|s| Some((s.id?, s.spec?.name?)))
            .collect();
        let tasks = self
            .client
            .list_tasks(None::<ListTasksOptions>)
            .await
            .map_err(map_err)?;
        Ok(tasks.into_iter().map(|t| task_summary(t, &names)).collect())
    }

    async fn secrets(&self) -> Result<Vec<SecretSummary>> {
        let uses = Uses::of(&self.list_services().await?);
        let list = self
            .client
            .list_secrets(None::<ListSecretsOptions>)
            .await
            .map_err(map_err)?;
        Ok(list
            .into_iter()
            .map(|s| {
                let id = s.id.clone().unwrap_or_default();
                let spec = s.spec.clone().unwrap_or_default();
                SecretSummary {
                    used_by: uses.secrets.get(&id).cloned().unwrap_or_default(),
                    id,
                    name: spec.name.unwrap_or_default(),
                    stack: stack_of(&spec.labels),
                    created_at: unix(&s.created_at),
                    updated_at: unix(&s.updated_at),
                }
            })
            .collect())
    }

    async fn configs(&self) -> Result<Vec<ConfigSummary>> {
        let uses = Uses::of(&self.list_services().await?);
        let list = self
            .client
            .list_configs(None::<ListConfigsOptions>)
            .await
            .map_err(map_err)?;
        Ok(list
            .into_iter()
            .map(|c| {
                let id = c.id.clone().unwrap_or_default();
                let spec = c.spec.clone().unwrap_or_default();
                ConfigSummary {
                    used_by: uses.configs.get(&id).cloned().unwrap_or_default(),
                    id,
                    name: spec.name.unwrap_or_default(),
                    stack: stack_of(&spec.labels),
                    created_at: unix(&c.created_at),
                    updated_at: unix(&c.updated_at),
                }
            })
            .collect())
    }

    async fn networks(&self) -> Result<Vec<SwarmNetwork>> {
        let uses = Uses::of(&self.list_services().await?);
        let filters = HashMap::from([("scope", vec!["swarm"])]);
        let opts = ListNetworksOptionsBuilder::new().filters(&filters).build();
        let list = self
            .client
            .list_networks(Some(opts))
            .await
            .map_err(map_err)?;
        Ok(list
            .into_iter()
            .filter(|n| n.scope.as_deref() == Some("swarm"))
            .map(|n| {
                let id = n.id.clone().unwrap_or_default();
                SwarmNetwork {
                    used_by: uses.networks.get(&id).cloned().unwrap_or_default(),
                    id,
                    name: n.name.clone().unwrap_or_default(),
                    driver: n.driver.clone().unwrap_or_default(),
                    stack: stack_of(&n.labels),
                    attachable: n.attachable.unwrap_or(false),
                    internal: n.internal.unwrap_or(false),
                    encrypted: n
                        .options
                        .as_ref()
                        .is_some_and(|o| o.contains_key("encrypted")),
                    subnets: n
                        .ipam
                        .and_then(|i| i.config)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|c| c.subnet)
                        .collect(),
                }
            })
            .collect())
    }

    fn service_logs(&self, id: &str, opts: &LogOptions) -> BoxStream<LogLine> {
        let tail = opts
            .tail
            .map(|n| n.to_string())
            .unwrap_or_else(|| "all".into());
        let o = LogsOptionsBuilder::new()
            .follow(opts.follow)
            .stdout(opts.stdout)
            .stderr(opts.stderr)
            .timestamps(opts.timestamps)
            .since(opts.since.unwrap_or(0).clamp(0, i32::MAX as i64) as i32)
            .tail(&tail)
            .build();
        let s = self
            .client
            .service_logs(id, Some(o))
            .map(|r| r.map(crate::streams::output_bytes).map_err(map_err));
        crate::streams::lines(s)
    }

    fn events(&self) -> BoxStream<DockerEvent> {
        StreamOps::events(self)
    }

    async fn node(&self, id: &str) -> Result<Option<NodeSummary>> {
        match self.client.inspect_node(id).await.map_err(map_err) {
            Ok(n) => Ok(Some(summary(n))),
            Err(Error::Upstream { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
