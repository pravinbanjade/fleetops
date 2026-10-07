//! Generic kinds (spec 3g §2.1): each group's preferred version, read one
//! group at a time so one failing aggregated API costs only its own kinds.

use crate::client::{timed, KubeClient};
use crate::kinds::api_resource;
use fleetops_core::kube::{ApiKind, Discovery, KindRef, KubeKind, PrinterColumn};
use fleetops_core::Result;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
pub use k8s_openapi::apimachinery::pkg::apis::meta::v1::APIResource;
use std::time::{Duration, Instant};

pub const DISCOVERY_CACHE: Duration = Duration::from_secs(60);

/// One group's preferred version as the apiserver answered it.
pub struct GroupResult {
    pub group: String,
    pub version: String,
    pub resources: std::result::Result<Vec<APIResource>, String>,
}

fn curated(group: &str, resource: &str) -> bool {
    KubeKind::ALL.into_iter().any(|k| {
        let ar = api_resource(k);
        ar.group == group && ar.plural == resource
    }) || (group == "events.k8s.io" && resource == "events")
}

fn col(name: &str, path: &str) -> PrinterColumn {
    PrinterColumn {
        name: name.into(),
        json_path: path.into(),
        type_: "string".into(),
    }
}

/// The printer columns of `crd` at `version`, as `kubectl get` shows them.
fn crd_columns(crd: &CustomResourceDefinition, version: &str) -> Vec<PrinterColumn> {
    crd.spec
        .versions
        .iter()
        .find(|v| v.name == version)
        .and_then(|v| v.additional_printer_columns.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.priority.unwrap_or(0) == 0 && c.json_path != ".metadata.creationTimestamp")
        .map(|c| PrinterColumn {
            name: c.name,
            json_path: c.json_path,
            type_: c.type_,
        })
        .collect()
}

pub fn assemble(groups: Vec<GroupResult>, crds: &[CustomResourceDefinition]) -> Discovery {
    let mut out = Discovery::default();
    for g in groups {
        let resources = match g.resources {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(group = %g.group, error = %e, "discovery skipped a group");
                out.failed_groups.push(if g.group.is_empty() {
                    "core".into()
                } else {
                    g.group
                });
                continue;
            }
        };
        for r in resources {
            let can = |v: &str| r.verbs.iter().any(|x| x == v);
            if r.name.contains('/') || !can("list") || !can("watch") || curated(&g.group, &r.name) {
                continue;
            }
            let crd = crds
                .iter()
                .find(|c| c.spec.group == g.group && c.spec.names.plural == r.name);
            let columns = match crd {
                Some(c) => crd_columns(c, &g.version),
                None if g.group == "apiextensions.k8s.io"
                    && r.name == "customresourcedefinitions" =>
                {
                    vec![
                        col("Group", ".spec.group"),
                        col("Kind", ".spec.names.kind"),
                        col("Scope", ".spec.scope"),
                    ]
                }
                None => vec![],
            };
            out.kinds.push(ApiKind {
                api: KindRef {
                    group: g.group.clone(),
                    version: g.version.clone(),
                    resource: r.name.clone(),
                },
                kind: r.kind.clone(),
                namespaced: r.namespaced,
                verbs: r.verbs.clone(),
                columns,
                crd: crd.and_then(|c| c.metadata.name.clone()),
            });
        }
    }
    out.kinds
        .sort_by(|a, b| (&a.api.group, &a.kind).cmp(&(&b.api.group, &b.kind)));
    out.failed_groups.sort();
    out
}

impl KubeClient {
    pub(crate) async fn discover_impl(&self, refresh: bool) -> Result<Discovery> {
        if !refresh {
            if let Some((at, d)) = self.generic.lock().unwrap().as_ref() {
                if at.elapsed() < DISCOVERY_CACHE {
                    return Ok(d.clone());
                }
            }
        }
        let client = &self.client;
        let core = timed(client.list_core_api_versions()).await?;
        let groups = timed(client.list_api_groups()).await?;
        let mut asks: Vec<(String, String)> = core
            .versions
            .first()
            .map(|v| vec![(String::new(), v.clone())])
            .unwrap_or_default();
        for g in groups.groups {
            let v = g
                .preferred_version
                .or_else(|| g.versions.into_iter().next())
                .map(|v| v.version);
            if let Some(v) = v {
                asks.push((g.name, v));
            }
        }
        let results =
            futures_util::future::join_all(asks.into_iter().map(|(group, version)| async move {
                let r = if group.is_empty() {
                    timed(client.list_core_api_resources(&version)).await
                } else {
                    timed(client.list_api_group_resources(&format!("{group}/{version}"))).await
                };
                GroupResult {
                    group,
                    version,
                    resources: r.map(|l| l.resources).map_err(|e| e.detail()),
                }
            }))
            .await;
        // Without permission to read CRDs, kinds still list; only their
        // columns are missing.
        let crds: Vec<CustomResourceDefinition> = timed(
            kube::Api::<CustomResourceDefinition>::all(client.clone()).list(&Default::default()),
        )
        .await
        .map(|l| l.items)
        .unwrap_or_default();
        let d = assemble(results, &crds);
        *self.generic.lock().unwrap() = Some((Instant::now(), d.clone()));
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn res(name: &str, kind: &str, namespaced: bool, verbs: &[&str]) -> APIResource {
        serde_json::from_value(json!({"name": name, "kind": kind, "namespaced": namespaced,
            "singularName": "", "verbs": verbs}))
        .unwrap()
    }
    const LW: &[&str] = &["get", "list", "watch", "update", "delete"];

    fn crd(versions: serde_json::Value) -> CustomResourceDefinition {
        serde_json::from_value(
            json!({"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
            "metadata": {"name": "widgets.example.fleetops.io"},
            "spec": {"group": "example.fleetops.io", "scope": "Namespaced",
                     "names": {"plural": "widgets", "kind": "Widget"}, "versions": versions}}),
        )
        .unwrap()
    }

    fn groups() -> Vec<GroupResult> {
        vec![
            GroupResult {
                group: "".into(),
                version: "v1".into(),
                resources: Ok(vec![
                    res("pods", "Pod", true, LW),
                    res("pods/log", "Pod", true, &["get"]),
                    res("bindings", "Binding", true, &["create"]),
                ]),
            },
            GroupResult {
                group: "coordination.k8s.io".into(),
                version: "v1".into(),
                resources: Ok(vec![res("leases", "Lease", true, LW)]),
            },
            GroupResult {
                group: "events.k8s.io".into(),
                version: "v1".into(),
                resources: Ok(vec![res("events", "Event", true, LW)]),
            },
            GroupResult {
                group: "metrics.k8s.io".into(),
                version: "v1beta1".into(),
                resources: Err("the server is currently unable to handle the request".into()),
            },
            GroupResult {
                group: "example.fleetops.io".into(),
                version: "v2".into(),
                resources: Ok(vec![res("widgets", "Widget", true, LW)]),
            },
            GroupResult {
                group: "apiextensions.k8s.io".into(),
                version: "v1".into(),
                resources: Ok(vec![res(
                    "customresourcedefinitions",
                    "CustomResourceDefinition",
                    false,
                    LW,
                )]),
            },
        ]
    }

    #[test]
    fn curated_subresources_unlistable_and_duplicate_events_are_left_out() {
        let d = assemble(groups(), &[]);
        let names: Vec<String> = d.kinds.iter().map(|k| k.api.path()).collect();
        assert!(
            names.contains(&"coordination.k8s.io/v1/leases".to_string()),
            "{names:?}"
        );
        for gone in [
            "core/v1/pods",
            "core/v1/pods/log",
            "core/v1/bindings",
            "events.k8s.io/v1/events",
        ] {
            assert!(!names.iter().any(|n| n == gone), "{gone} in {names:?}");
        }
    }

    #[test]
    fn a_failing_group_is_named_and_the_rest_still_load() {
        let d = assemble(groups(), &[]);
        assert_eq!(d.failed_groups, ["metrics.k8s.io"]);
        assert!(d.kinds.iter().any(|k| k.kind == "Widget"));
    }

    #[test]
    fn crd_columns_come_from_the_served_preferred_version() {
        let c = crd(json!([
            {"name": "v1", "served": true, "storage": true,
             "additionalPrinterColumns": [{"name": "Old", "type": "string", "jsonPath": ".spec.old"}]},
            {"name": "v2", "served": true, "storage": false,
             "additionalPrinterColumns": [
                {"name": "Size", "type": "integer", "jsonPath": ".spec.size"},
                {"name": "Wide", "type": "string", "jsonPath": ".spec.wide", "priority": 1},
                {"name": "Age", "type": "date", "jsonPath": ".metadata.creationTimestamp"}]}
        ]));
        let d = assemble(groups(), &[c]);
        let w = d.kinds.iter().find(|k| k.kind == "Widget").unwrap();
        assert_eq!(w.api.version, "v2");
        assert_eq!(w.crd.as_deref(), Some("widgets.example.fleetops.io"));
        assert_eq!(
            w.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["Size"]
        );
    }

    #[test]
    fn crd_definitions_get_group_kind_and_scope_columns() {
        let d = assemble(groups(), &[]);
        let defs = d
            .kinds
            .iter()
            .find(|k| k.api.resource == "customresourcedefinitions")
            .unwrap();
        assert!(!defs.namespaced);
        assert_eq!(
            defs.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["Group", "Kind", "Scope"]
        );
    }

    #[test]
    fn kinds_are_sorted_by_group_then_kind() {
        let d = assemble(groups(), &[]);
        let order: Vec<String> = d
            .kinds
            .iter()
            .map(|k| format!("{}/{}", k.api.group, k.kind))
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
    }
}
