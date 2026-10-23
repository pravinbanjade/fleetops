//! Swarm cluster calls on a manager's engine.

use crate::{error::map_err, LocalDocker};
use async_trait::async_trait;
use bollard::models::{Node, NodeSpecRoleEnum};
use bollard::query_parameters::ListNodesOptions;
use fleetops_core::swarm::{NodeRole, NodeSummary, SwarmInfo, SwarmOps};
use fleetops_core::{Error, Result};

fn summary(n: Node) -> NodeSummary {
    let spec = n.spec.unwrap_or_default();
    NodeSummary {
        id: n.id.unwrap_or_default(),
        hostname: n
            .description
            .and_then(|d| d.hostname)
            .unwrap_or_default(),
        role: match spec.role {
            Some(NodeSpecRoleEnum::MANAGER) => NodeRole::Manager,
            _ => NodeRole::Worker,
        },
        state: n
            .status
            .and_then(|s| s.state)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".into()),
        availability: spec
            .availability
            .map(|a| a.to_string())
            .unwrap_or_default(),
        leader: n.manager_status.and_then(|m| m.leader).unwrap_or(false),
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

    async fn node(&self, id: &str) -> Result<Option<NodeSummary>> {
        match self.client.inspect_node(id).await.map_err(map_err) {
            Ok(n) => Ok(Some(summary(n))),
            Err(Error::Upstream { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
