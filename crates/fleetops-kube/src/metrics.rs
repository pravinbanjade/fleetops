//! CPU and memory now (spec 3e §3), from the `metrics.k8s.io` API that
//! metrics-server serves. Many clusters don't run it: that is a state to
//! show, not an error.

use crate::client::{timed, KubeClient, REQUEST_TIMEOUT};
use fleetops_core::kube::{KubeKind, KubeUsage, KubeUsageRow};
use fleetops_core::{Error, Result};
use k8s_openapi::api::core::v1::Node;
use kube::api::{Api, DynamicObject, ListParams};
use kube::discovery::ApiResource;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// metrics-server samples every 15s by default; asking more often than
/// this only repeats the same numbers.
pub const USAGE_TTL: Duration = Duration::from_secs(10);

fn metrics_resource(kind: &str, plural: &str) -> ApiResource {
    ApiResource {
        group: "metrics.k8s.io".into(),
        version: "v1beta1".into(),
        api_version: "metrics.k8s.io/v1beta1".into(),
        kind: kind.into(),
        plural: plural.into(),
    }
}

/// A Kubernetes quantity in its base unit (cores, bytes), e.g. `250m`,
/// `1500000n`, `128Mi`, `1e3`. `None` when it doesn't parse.
pub fn quantity(q: &str) -> Option<f64> {
    let q = q.trim();
    let split = q
        .find(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
        .unwrap_or(q.len());
    let (num, suffix) = q.split_at(split);
    let mul = match suffix {
        "" => 1.0,
        "n" => 1e-9,
        "u" => 1e-6,
        "m" => 1e-3,
        "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        "P" => 1e15,
        "E" => 1e18,
        "Ki" => 1024.0,
        "Mi" => 1024f64.powi(2),
        "Gi" => 1024f64.powi(3),
        "Ti" => 1024f64.powi(4),
        "Pi" => 1024f64.powi(5),
        "Ei" => 1024f64.powi(6),
        _ => return None,
    };
    let n: f64 = num.parse().ok()?;
    (n.is_finite() && n >= 0.0).then_some(n * mul)
}

fn millicores(q: Option<&str>) -> u64 {
    q.and_then(quantity)
        .map(|c| (c * 1000.0).round() as u64)
        .unwrap_or(0)
}

fn bytes(q: Option<&str>) -> u64 {
    q.and_then(quantity).map(|b| b.round() as u64).unwrap_or(0)
}

fn usage_of(v: &Value) -> (u64, u64) {
    (millicores(v["cpu"].as_str()), bytes(v["memory"].as_str()))
}

/// One NodeMetrics or PodMetrics item as a row; a pod sums its containers.
pub fn usage_row(kind: KubeKind, obj: &DynamicObject) -> KubeUsageRow {
    let (cpu, memory) = if kind == KubeKind::Pods {
        obj.data["containers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| usage_of(&c["usage"]))
            .fold((0, 0), |(a, b), (c, m)| (a + c, b + m))
    } else {
        usage_of(&obj.data["usage"])
    };
    KubeUsageRow {
        namespace: obj.metadata.namespace.clone(),
        name: obj.metadata.name.clone().unwrap_or_default(),
        cpu,
        memory,
        cpu_allocatable: None,
        memory_allocatable: None,
    }
}

/// The apiserver answers 404 when no metrics API is registered and 503
/// when it is registered but metrics-server is down.
fn unavailable(e: &kube::Error) -> Option<&'static str> {
    match e {
        kube::Error::Api(s) if s.code == 404 => {
            Some("metrics-server is not installed in this cluster")
        }
        kube::Error::Api(s) if s.code == 503 => Some("metrics-server is not answering"),
        _ => None,
    }
}

impl KubeClient {
    pub(crate) async fn usage_impl(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeUsage> {
        let key = (kind, ns.map(str::to_string));
        if let Some((at, u)) = self.usage_cache.lock().unwrap().get(&key) {
            if at.elapsed() < USAGE_TTL {
                return Ok(u.clone());
            }
        }
        let u = self.fetch_usage(kind, ns).await?;
        self.usage_cache
            .lock()
            .unwrap()
            .insert(key, (std::time::Instant::now(), u.clone()));
        Ok(u)
    }

    async fn fetch_usage(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeUsage> {
        let ar = match kind {
            KubeKind::Nodes => metrics_resource("NodeMetrics", "nodes"),
            KubeKind::Pods => metrics_resource("PodMetrics", "pods"),
            _ => {
                return Err(Error::Invalid(format!(
                    "there are no metrics for {}",
                    kind.as_str()
                )))
            }
        };
        let api: Api<DynamicObject> = match ns.filter(|_| kind == KubeKind::Pods) {
            Some(ns) => Api::namespaced_with(self.client.clone(), ns, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        };
        let list =
            match tokio::time::timeout(REQUEST_TIMEOUT, api.list(&ListParams::default())).await {
                Err(_) => {
                    return Ok(KubeUsage::unavailable(
                        "metrics-server did not answer in time",
                    ))
                }
                Ok(Err(e)) => match unavailable(&e) {
                    Some(why) => return Ok(KubeUsage::unavailable(why)),
                    None => return Err(crate::errors::map(e)),
                },
                Ok(Ok(l)) => l,
            };
        let mut rows: Vec<KubeUsageRow> = list.items.iter().map(|o| usage_row(kind, o)).collect();
        if kind == KubeKind::Nodes {
            // Credentials that can read metrics but not nodes still get usage.
            if let Ok(nodes) =
                timed(Api::<Node>::all(self.client.clone()).list(&ListParams::default())).await
            {
                let alloc: HashMap<String, (u64, u64)> = nodes
                    .items
                    .into_iter()
                    .filter_map(|n| {
                        let a = n.status?.allocatable?;
                        let cpu = millicores(a.get("cpu").map(|q| q.0.as_str()));
                        let mem = bytes(a.get("memory").map(|q| q.0.as_str()));
                        Some((n.metadata.name?, (cpu, mem)))
                    })
                    .collect();
                for r in &mut rows {
                    if let Some(&(c, m)) = alloc.get(&r.name) {
                        r.cpu_allocatable = Some(c);
                        r.memory_allocatable = Some(m);
                    }
                }
            }
        }
        rows.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
        Ok(KubeUsage {
            available: true,
            reason: None,
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quantities_parse_in_their_base_unit() {
        assert_eq!(quantity("250m"), Some(0.25));
        assert_eq!(quantity("2"), Some(2.0));
        assert_eq!(
            quantity("1500000n").map(|c| (c * 1000.0).round()),
            Some(2.0)
        );
        assert_eq!(quantity("128Mi"), Some(134_217_728.0));
        assert_eq!(quantity("1Gi"), Some(1_073_741_824.0));
        assert_eq!(quantity("16384Ki"), Some(16_777_216.0));
        assert_eq!(quantity("1e3"), Some(1000.0));
        assert_eq!(quantity("1.5k"), Some(1500.0));
        assert_eq!(quantity("12Q"), None);
        assert_eq!(quantity(""), None);
        assert_eq!(quantity("-1"), None);
    }

    #[test]
    fn a_pod_row_sums_its_containers() {
        let o: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "metrics.k8s.io/v1beta1", "kind": "PodMetrics",
            "metadata": {"name": "web-1", "namespace": "apps"},
            "containers": [
                {"name": "a", "usage": {"cpu": "120m", "memory": "64Mi"}},
                {"name": "b", "usage": {"cpu": "5000000n", "memory": "1024Ki"}}]}))
        .unwrap();
        let r = usage_row(KubeKind::Pods, &o);
        assert_eq!(
            (r.namespace.as_deref(), r.name.as_str()),
            (Some("apps"), "web-1")
        );
        assert_eq!(r.cpu, 125);
        assert_eq!(r.memory, 64 * 1024 * 1024 + 1024 * 1024);
    }

    #[test]
    fn a_node_row_reads_its_usage() {
        let o: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "metrics.k8s.io/v1beta1", "kind": "NodeMetrics",
            "metadata": {"name": "n1"}, "usage": {"cpu": "1", "memory": "2Gi"}}))
        .unwrap();
        let r = usage_row(KubeKind::Nodes, &o);
        assert_eq!((r.cpu, r.memory), (1000, 2 * 1024 * 1024 * 1024));
        assert_eq!(r.cpu_allocatable, None);
    }
}
