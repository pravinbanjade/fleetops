//! What a rollback or uninstall will do (spec §3.1, §4), worked out from
//! the manifests alone; `ops` runs it against the cluster.

use crate::apply::{split_all, Doc};
use fleetops_core::helm::HelmChange;
use fleetops_core::Result;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const KEEP: &str = "helm.sh/resource-policy";

/// Helm's `UninstallOrder` (pkg/releaseutil/kind_sorter.go). Kinds not
/// listed go last, by name.
const UNINSTALL_ORDER: [&str; 36] = [
    "APIService",
    "Ingress",
    "IngressClass",
    "Service",
    "CronJob",
    "Job",
    "StatefulSet",
    "HorizontalPodAutoscaler",
    "Deployment",
    "ReplicaSet",
    "ReplicationController",
    "Pod",
    "DaemonSet",
    "RoleBindingList",
    "RoleBinding",
    "RoleList",
    "Role",
    "ClusterRoleBindingList",
    "ClusterRoleBinding",
    "ClusterRoleList",
    "ClusterRole",
    "CustomResourceDefinition",
    "PersistentVolumeClaim",
    "PersistentVolume",
    "StorageClass",
    "ConfigMap",
    "SecretList",
    "Secret",
    "ServiceAccount",
    "PodDisruptionBudget",
    "PodSecurityPolicy",
    "LimitRange",
    "ResourceQuota",
    "NetworkPolicy",
    "Namespace",
    "PriorityClass",
];

pub struct Step {
    pub doc: Doc,
    pub change: HelmChange,
    /// The merge patch, for `Patch` only.
    pub patch: Option<Value>,
}

/// (group, kind, namespace or the release's, name): how an object in one
/// revision is matched to the same object in another.
type Key = (String, String, String, String);

fn key(d: &Doc, release_ns: &str) -> Key {
    let group = d.api_version.split_once('/').map(|(g, _)| g).unwrap_or("");
    (
        group.to_string(),
        d.kind.clone(),
        d.namespace
            .clone()
            .unwrap_or_else(|| release_ns.to_string()),
        d.name.clone(),
    )
}

fn kept(d: &Doc) -> bool {
    d.value
        .pointer("/metadata/annotations")
        .and_then(|a| a.get(KEEP))
        .and_then(Value::as_str)
        == Some("keep")
}

fn rank(kind: &str) -> usize {
    UNINSTALL_ORDER
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(UNINSTALL_ORDER.len())
}

fn removal(doc: Doc) -> Step {
    let change = if kept(&doc) {
        HelmChange::Keep
    } else {
        HelmChange::Delete
    };
    Step {
        doc,
        change,
        patch: None,
    }
}

fn in_uninstall_order(mut docs: Vec<Doc>) -> Vec<Doc> {
    docs.sort_by(|a, b| (rank(&a.kind), &a.kind, &a.name).cmp(&(rank(&b.kind), &b.kind, &b.name)));
    docs
}

/// A JSON merge patch turning `base` into `target`: changed keys take
/// `target`'s value (lists whole), keys only in `base` become null, equal
/// keys are left out.
pub fn merge_patch(base: &Value, target: &Value) -> Value {
    match (base, target) {
        (Value::Object(b), Value::Object(t)) => {
            let mut out = Map::new();
            for (k, tv) in t {
                match b.get(k) {
                    Some(bv) if bv == tv => {}
                    Some(bv @ Value::Object(_)) if tv.is_object() => {
                        out.insert(k.clone(), merge_patch(bv, tv));
                    }
                    _ => {
                        out.insert(k.clone(), tv.clone());
                    }
                }
            }
            for k in b.keys().filter(|k| !t.contains_key(*k)) {
                out.insert(k.clone(), Value::Null);
            }
            Value::Object(out)
        }
        _ => target.clone(),
    }
}

pub fn rollback_steps(current: &str, target: &str, release_ns: &str) -> Result<Vec<Step>> {
    let mut now: BTreeMap<Key, Doc> = split_all(current)?
        .into_iter()
        .map(|d| (key(&d, release_ns), d))
        .collect();
    let mut steps = Vec::new();
    for doc in split_all(target)? {
        match now.remove(&key(&doc, release_ns)) {
            Some(old) => {
                let patch = merge_patch(&old.value, &doc.value);
                steps.push(Step {
                    doc,
                    change: HelmChange::Patch,
                    patch: Some(patch),
                });
            }
            None => steps.push(Step {
                doc,
                change: HelmChange::Create,
                patch: None,
            }),
        }
    }
    steps.extend(
        in_uninstall_order(now.into_values().collect())
            .into_iter()
            .map(removal),
    );
    Ok(steps)
}

pub fn uninstall_steps(current: &str) -> Result<Vec<Step>> {
    Ok(in_uninstall_order(split_all(current)?)
        .into_iter()
        .map(removal)
        .collect())
}

/// The ownership metadata Helm checks before it will upgrade an object.
pub fn adopt(value: &mut Value, release: &str, release_ns: &str) {
    let meta = value
        .as_object_mut()
        .expect("a document is an object")
        .entry("metadata")
        .or_insert_with(|| Value::Object(Map::new()));
    let set = |meta: &mut Value, field: &str, k: &str, v: &str| {
        let m = meta
            .as_object_mut()
            .expect("metadata is an object")
            .entry(field)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(m) = m.as_object_mut() {
            m.insert(k.into(), v.into());
        }
    };
    set(meta, "labels", "app.kubernetes.io/managed-by", "Helm");
    set(meta, "annotations", "meta.helm.sh/release-name", release);
    set(
        meta,
        "annotations",
        "meta.helm.sh/release-namespace",
        release_ns,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cm(name: &str, data: &str) -> String {
        format!("---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {name}\ndata:\n{data}")
    }

    #[test]
    fn a_merge_patch_carries_only_differences_and_nulls_removals() {
        let base = json!({"spec": {"replicas": 2, "paused": true, "template": {"a": 1}}, "metadata": {"name": "x"}});
        let target =
            json!({"spec": {"replicas": 1, "template": {"a": 1}}, "metadata": {"name": "x"}});
        assert_eq!(
            merge_patch(&base, &target),
            json!({"spec": {"replicas": 1, "paused": null}})
        );
        assert_eq!(merge_patch(&target, &target), json!({}));
    }

    #[test]
    fn a_changed_list_is_replaced_whole_and_an_unchanged_one_left_out() {
        let base = json!({"ports": [1, 2], "hosts": ["a"]});
        let target = json!({"ports": [1], "hosts": ["a"]});
        assert_eq!(merge_patch(&base, &target), json!({"ports": [1]}));
    }

    #[test]
    fn rollback_patches_shared_objects_creates_new_ones_and_deletes_the_rest() {
        let current = format!(
            "{}{}",
            cm("both", "  k: v2\n"),
            cm("added-later", "  k: x\n")
        );
        let target = format!("{}{}", cm("both", "  k: v1\n"), cm("only-old", "  k: o\n"));
        let steps = rollback_steps(&current, &target, "apps").unwrap();
        let got: Vec<_> = steps
            .iter()
            .map(|s| (s.change, s.doc.name.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (HelmChange::Patch, "both"),
                (HelmChange::Create, "only-old"),
                (HelmChange::Delete, "added-later"),
            ]
        );
        assert_eq!(steps[0].patch, Some(json!({"data": {"k": "v1"}})));
    }

    #[test]
    fn an_object_with_the_keep_policy_is_kept() {
        let current = "---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: precious\n  annotations:\n    helm.sh/resource-policy: keep\n";
        let steps = uninstall_steps(current).unwrap();
        assert_eq!(steps[0].change, HelmChange::Keep);
        let steps = rollback_steps(current, "", "apps").unwrap();
        assert_eq!(steps[0].change, HelmChange::Keep);
    }

    #[test]
    fn a_namespace_given_or_not_is_the_same_object() {
        let current = "---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\n  namespace: apps\ndata: {k: a}\n";
        let target = "---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\ndata: {k: b}\n";
        let steps = rollback_steps(current, target, "apps").unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].change, HelmChange::Patch);
    }

    #[test]
    fn uninstall_follows_helms_order_unknown_kinds_last() {
        let m = "---\napiVersion: v1\nkind: ConfigMap\nmetadata: {name: c}\n---\napiVersion: example.com/v1\nkind: Widget\nmetadata: {name: w}\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {name: d}\n---\napiVersion: v1\nkind: Service\nmetadata: {name: s}\n";
        let kinds: Vec<_> = uninstall_steps(m)
            .unwrap()
            .into_iter()
            .map(|s| s.doc.kind)
            .collect();
        assert_eq!(kinds, ["Service", "Deployment", "ConfigMap", "Widget"]);
    }

    #[test]
    fn a_large_manifest_is_not_capped() {
        let m: String = (0..150).map(|i| cm(&format!("c{i}"), "  k: v\n")).collect();
        assert_eq!(uninstall_steps(&m).unwrap().len(), 150);
        assert!(uninstall_steps("").unwrap().is_empty());
    }

    #[test]
    fn adopted_objects_carry_helms_ownership_metadata() {
        let mut v = json!({"metadata": {"name": "x", "labels": {"app": "x"}}});
        adopt(&mut v, "demo", "apps");
        assert_eq!(v["metadata"]["labels"]["app"], "x");
        assert_eq!(
            v["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "Helm"
        );
        assert_eq!(
            v["metadata"]["annotations"]["meta.helm.sh/release-name"],
            "demo"
        );
        assert_eq!(
            v["metadata"]["annotations"]["meta.helm.sh/release-namespace"],
            "apps"
        );
    }
}
