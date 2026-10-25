//! Swarm cluster calls on a manager's engine.

use crate::{error::map_err, LocalDocker};
use async_trait::async_trait;
use base64::Engine;
use bollard::models::{
    ConfigSpec, Ipam, IpamConfig, NetworkCreateRequest, Node, NodeSpecAvailabilityEnum,
    NodeSpecRoleEnum, SecretSpec, Service, ServiceSpec, Task,
};
use bollard::query_parameters::{
    DeleteNodeOptions, ListConfigsOptions, ListNetworksOptionsBuilder, ListNodesOptions,
    ListSecretsOptions, ListServicesOptionsBuilder, ListTasksOptions, LogsOptionsBuilder,
    UpdateNodeOptionsBuilder, UpdateServiceOptionsBuilder,
};
use fleetops_core::streams::{BoxStream, DockerEvent, LogLine, LogOptions, StreamOps};
use fleetops_core::swarm::{
    Availability, ConfigSummary, NewSwarmNetwork, NodeChange, NodeDetail, NodeRole, NodeSummary,
    SecretSummary, ServiceChange, ServiceDetail, ServiceSummary, SwarmInfo, SwarmNetwork, SwarmOps,
    TaskSummary, STACK_LABEL,
};
use fleetops_core::{Error, Result};
use futures_util::StreamExt;
use serde_json::Value;
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
        labels: spec
            .labels
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect(),
    }
}

fn task_summary(t: Task, services: &HashMap<String, String>) -> TaskSummary {
    let status = t.status.unwrap_or_default();
    let service_id = t.service_id.unwrap_or_default();
    let stack = t
        .spec
        .as_ref()
        .and_then(|s| s.container_spec.as_ref())
        .and_then(|c| stack_of(&c.labels));
    TaskSummary {
        stack,
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

/// `spec` with `n` replicas; only replicated services scale.
fn scaled(mut spec: Value, n: u64) -> Result<Value> {
    match spec.pointer_mut("/Mode/Replicated") {
        Some(r) if r.is_object() => {
            r["Replicas"] = n.into();
            Ok(spec)
        }
        _ => {
            let name = spec["Name"].as_str().unwrap_or_default();
            let mode = spec["Mode"]
                .as_object()
                .and_then(|m| m.keys().next())
                .map(|k| k.to_lowercase())
                .unwrap_or_else(|| "not replicated".into());
            Err(Error::Invalid(format!(
                "only replicated services scale; '{name}' is {mode}"
            )))
        }
    }
}

/// `spec` with its tasks forced to restart.
fn forced(mut spec: Value) -> Value {
    if !spec["TaskTemplate"].is_object() {
        spec["TaskTemplate"] = serde_json::json!({});
    }
    let n = spec["TaskTemplate"]["ForceUpdate"].as_u64().unwrap_or(0);
    spec["TaskTemplate"]["ForceUpdate"] = (n + 1).into();
    spec
}

fn service_spec(v: Value) -> Result<ServiceSpec> {
    serde_json::from_value(v).map_err(|e| Error::Invalid(format!("not a service spec: {e}")))
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Error::Internal(e.to_string()))
}

/// `Ok(None)` for Docker's 404.
fn found<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(Error::Upstream { status: 404, .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Docker names a container spec's healthcheck `Healthcheck`; bollard's
/// model reads and writes `HealthCheck`. Moved over, so it survives being
/// written back through the model.
fn healthcheck_for_bollard(service: &mut Value) {
    if let Some(cs) = service["Spec"]["TaskTemplate"]["ContainerSpec"].as_object_mut() {
        if let Some(h) = cs.remove("Healthcheck") {
            cs.entry("HealthCheck").or_insert(h);
        }
    }
}

impl LocalDocker {
    /// The service as Docker sent it, read raw where it can be so nothing
    /// bollard's model lacks is lost; `None` for Docker's 404.
    async fn inspect_service_json(&self, id: &str) -> Result<Option<Value>> {
        let Some(get) = &self.raw else {
            return found(self.client.inspect_service(id, None).await.map_err(map_err))?
                .map(|s| to_value(&s))
                .transpose();
        };
        let (status, body) = get(format!("/services/{id}")).await?;
        let parsed: Option<Value> = serde_json::from_slice(&body).ok();
        match status {
            200..=299 => {
                let mut v = parsed.ok_or_else(|| {
                    Error::Internal("docker sent a service that is not JSON".into())
                })?;
                healthcheck_for_bollard(&mut v);
                Ok(Some(v))
            }
            404 => Ok(None),
            status => Err(Error::Upstream {
                status,
                message: parsed
                    .and_then(|v| v["message"].as_str().map(str::to_string))
                    .unwrap_or_else(|| format!("docker answered {status}")),
            }),
        }
    }

    async fn inspect_service_raw(&self, id: &str) -> Result<(Value, u64)> {
        let v = self
            .inspect_service_json(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("service {id}")))?;
        let version = v["Version"]["Index"].as_u64().unwrap_or(0);
        Ok((v["Spec"].clone(), version))
    }

    async fn put_service(&self, id: &str, spec: Value, version: u64, rollback: bool) -> Result<()> {
        let mut o = UpdateServiceOptionsBuilder::new().version(version as i32);
        if rollback {
            o = o.rollback("previous");
        }
        match self
            .client
            .update_service(id, service_spec(spec)?, o.build(), None)
            .await
            .map_err(map_err)
        {
            Ok(_) => Ok(()),
            // Someone changed it since this version was read.
            Err(Error::Upstream { message, .. }) if message.contains("out of sequence") => {
                Err(Error::Conflict(format!(
                    "the service changed since it was read; reload and try again ({message})"
                )))
            }
            Err(e) => Err(e),
        }
    }

    async fn list_services(&self) -> Result<Vec<Service>> {
        let opts = ListServicesOptionsBuilder::new().status(true).build();
        self.client.list_services(Some(opts)).await.map_err(map_err)
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
        Ok(self
            .list_services()
            .await?
            .iter()
            .map(service_summary)
            .collect())
    }

    async fn service(&self, id: &str) -> Result<Option<ServiceDetail>> {
        let Some(raw) = self.inspect_service_json(id).await? else {
            return Ok(None);
        };
        let svc: Service = serde_json::from_value(raw.clone())
            .map_err(|e| Error::Internal(format!("docker sent an unreadable service: {e}")))?;
        Ok(Some(ServiceDetail {
            summary: service_summary(&svc),
            spec: if raw["Spec"].is_object() {
                raw["Spec"].clone()
            } else {
                serde_json::json!({})
            },
            update_status: svc
                .update_status
                .as_ref()
                .and_then(|u| serde_json::to_value(u).ok()),
            version: svc.version.as_ref().and_then(|v| v.index).unwrap_or(0),
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
                    labels: spec.labels.unwrap_or_default().into_iter().collect(),
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
                    labels: spec.labels.unwrap_or_default().into_iter().collect(),
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
                    labels: n.labels.clone().unwrap_or_default().into_iter().collect(),
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

    async fn update_node(&self, id: &str, change: &NodeChange) -> Result<()> {
        let n = self.client.inspect_node(id).await.map_err(map_err)?;
        let version = n.version.and_then(|v| v.index).unwrap_or(0);
        let mut spec = n.spec.unwrap_or_default();
        if let Some(a) = change.availability {
            spec.availability = Some(match a {
                Availability::Active => NodeSpecAvailabilityEnum::ACTIVE,
                Availability::Pause => NodeSpecAvailabilityEnum::PAUSE,
                Availability::Drain => NodeSpecAvailabilityEnum::DRAIN,
            });
        }
        if let Some(r) = change.role {
            spec.role = Some(match r {
                NodeRole::Manager => NodeSpecRoleEnum::MANAGER,
                NodeRole::Worker => NodeSpecRoleEnum::WORKER,
            });
        }
        if let Some(l) = &change.labels {
            spec.labels = Some(l.clone().into_iter().collect());
        }
        let o = UpdateNodeOptionsBuilder::new()
            .version(version as i64)
            .build();
        self.client.update_node(id, spec, o).await.map_err(map_err)
    }

    async fn remove_node(&self, id: &str) -> Result<()> {
        self.client
            .delete_node(id, None::<DeleteNodeOptions>)
            .await
            .map_err(map_err)
    }

    async fn create_service(&self, spec: &Value) -> Result<String> {
        let r = self
            .client
            .create_service(service_spec(spec.clone())?, None)
            .await
            .map_err(map_err)?;
        Ok(r.id.unwrap_or_default())
    }

    async fn update_service(&self, id: &str, change: ServiceChange) -> Result<()> {
        match change {
            ServiceChange::Spec { spec, version } => {
                self.put_service(id, spec, version, false).await
            }
            ServiceChange::Scale(n) => {
                let (spec, version) = self.inspect_service_raw(id).await?;
                self.put_service(id, scaled(spec, n)?, version, false).await
            }
            ServiceChange::Redeploy => {
                let (spec, version) = self.inspect_service_raw(id).await?;
                self.put_service(id, forced(spec), version, false).await
            }
            ServiceChange::Rollback => {
                let (spec, version) = self.inspect_service_raw(id).await?;
                self.put_service(id, spec, version, true).await
            }
        }
    }

    async fn remove_service(&self, id: &str) -> Result<()> {
        self.client.delete_service(id).await.map_err(map_err)
    }

    async fn create_secret(
        &self,
        name: &str,
        data: &[u8],
        labels: &BTreeMap<String, String>,
    ) -> Result<String> {
        let spec = SecretSpec {
            name: Some(name.into()),
            labels: Some(labels.clone().into_iter().collect()),
            data: Some(base64::engine::general_purpose::STANDARD.encode(data)),
            ..Default::default()
        };
        let r = self.client.create_secret(spec).await.map_err(map_err)?;
        Ok(r.id)
    }

    async fn remove_secret(&self, id: &str) -> Result<()> {
        self.client.delete_secret(id).await.map_err(map_err)
    }

    async fn create_config(
        &self,
        name: &str,
        data: &[u8],
        labels: &BTreeMap<String, String>,
    ) -> Result<String> {
        let spec = ConfigSpec {
            name: Some(name.into()),
            labels: Some(labels.clone().into_iter().collect()),
            data: Some(base64::engine::general_purpose::STANDARD.encode(data)),
            ..Default::default()
        };
        let r = self.client.create_config(spec).await.map_err(map_err)?;
        Ok(r.id)
    }

    async fn config_data(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let Some(c) = found(self.client.inspect_config(id).await.map_err(map_err))? else {
            return Ok(None);
        };
        let data = c.spec.and_then(|s| s.data).unwrap_or_default();
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map(Some)
            .map_err(|e| Error::Internal(format!("config data is not base64: {e}")))
    }

    async fn remove_config(&self, id: &str) -> Result<()> {
        self.client.delete_config(id).await.map_err(map_err)
    }

    async fn create_network(
        &self,
        n: &NewSwarmNetwork,
        labels: &BTreeMap<String, String>,
        options: &BTreeMap<String, String>,
    ) -> Result<String> {
        let mut options: HashMap<String, String> = options.clone().into_iter().collect();
        if n.encrypted {
            options.insert("encrypted".to_string(), String::new());
        }
        let req = NetworkCreateRequest {
            name: n.name.clone(),
            driver: Some("overlay".into()),
            scope: Some("swarm".into()),
            attachable: Some(n.attachable),
            internal: Some(n.internal),
            labels: (!labels.is_empty()).then(|| labels.clone().into_iter().collect()),
            options: (!options.is_empty()).then_some(options),
            ipam: n.subnet.as_ref().map(|s| Ipam {
                config: Some(vec![IpamConfig {
                    subnet: Some(s.clone()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = self.client.create_network(req).await.map_err(map_err)?;
        Ok(r.id)
    }

    async fn remove_network(&self, id: &str) -> Result<()> {
        self.client.remove_network(id).await.map_err(map_err)
    }

    async fn node(&self, id: &str) -> Result<Option<NodeSummary>> {
        match self.client.inspect_node(id).await.map_err(map_err) {
            Ok(n) => Ok(Some(summary(n))),
            Err(Error::Upstream { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What FleetOps renders reaches Docker through bollard's model and
    /// comes back through it: nothing it says may be lost on the way, or a
    /// fresh deploy would read as drifted.
    #[test]
    fn rendered_specs_survive_bollards_model() {
        use fleetops_core::compose::swarm::{canonical, sample_variables, SAMPLE_STACK};
        use fleetops_core::compose::{files::resolve, parse_mode, swarm as render, Mode};
        let vars = sample_variables();
        let mut p = parse_mode(SAMPLE_STACK, &vars, "shop", Mode::Swarm)
            .unwrap()
            .project;
        resolve(&mut p, &vars, &Default::default(), &[7; 32]).unwrap();
        for mode in ["", "global", "replicated-job", "global-job"] {
            if !mode.is_empty() {
                let d = p.services.get_mut("web").unwrap().swarm.as_mut().unwrap();
                d.mode = serde_json::from_value(json!(mode)).unwrap();
            }
            let spec = render::service_spec(&p, "web");
            let through = to_value(&service_spec(spec.clone()).unwrap()).unwrap();
            assert_eq!(canonical(&through), canonical(&spec), "mode {mode:?}");
        }
    }

    #[test]
    fn scaling_sets_replicas_and_keeps_the_rest() {
        let spec =
            json!({"Name": "web", "Labels": {"a": "b"}, "Mode": {"Replicated": {"Replicas": 3}}});
        let s = scaled(spec, 5).unwrap();
        assert_eq!(s["Mode"]["Replicated"]["Replicas"], 5);
        assert_eq!(s["Labels"], json!({"a": "b"}));
    }

    #[test]
    fn global_services_do_not_scale() {
        let e = scaled(json!({"Name": "agent", "Mode": {"Global": {}}}), 2).unwrap_err();
        assert_eq!(
            e.detail(),
            "only replicated services scale; 'agent' is global"
        );
    }

    #[test]
    fn redeploy_bumps_force_update() {
        let s = forced(json!({"TaskTemplate": {"ContainerSpec": {}}}));
        assert_eq!(s["TaskTemplate"]["ForceUpdate"], 1);
        let s = forced(json!({"TaskTemplate": {"ForceUpdate": 4}}));
        assert_eq!(s["TaskTemplate"]["ForceUpdate"], 5);
    }
}
