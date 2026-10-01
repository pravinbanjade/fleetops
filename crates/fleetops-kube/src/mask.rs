//! The YAML shown on an object's page: `managedFields` stripped, and every
//! Secret value masked, including the copy kubectl keeps in the
//! last-applied annotation (spec §8, Review Focus 2).

use crate::kinds::typed_json;
use fleetops_core::kube::{KubeKind, MASK, YAML_CAP};
use kube::api::DynamicObject;
use serde_json::Value;

const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";
const CUT_NOTE: &str = "\n# … cut at 1 MiB\n";

pub fn yaml(kind: KubeKind, obj: &DynamicObject) -> String {
    let mut v = typed_json(kind, obj);
    if let Some(meta) = v.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
    }
    if kind == KubeKind::Secrets {
        for field in ["data", "stringData"] {
            if let Some(map) = v.get_mut(field).and_then(Value::as_object_mut) {
                for val in map.values_mut() {
                    *val = MASK.into();
                }
            }
        }
        if let Some(ann) = v["metadata"]
            .get_mut("annotations")
            .and_then(Value::as_object_mut)
        {
            if ann.contains_key(LAST_APPLIED) {
                ann.insert(LAST_APPLIED.into(), MASK.into());
            }
        }
    }
    let mut text =
        serde_saphyr::to_string(&v).unwrap_or_else(|e| format!("# cannot render: {e}\n"));
    if text.len() > YAML_CAP {
        let mut cut = YAML_CAP;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str(CUT_NOTE);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: serde_json::Value) -> DynamicObject {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn managed_fields_are_stripped() {
        let o = obj(
            json!({"metadata": {"name": "c", "managedFields": [{"manager": "kubectl"}]}, "data": {"mode": "prod"}}),
        );
        let y = yaml(KubeKind::ConfigMaps, &o);
        assert!(
            !y.contains("managedFields")
                && y.contains("mode: prod")
                && y.contains("kind: ConfigMap"),
            "{y}"
        );
    }

    #[test]
    fn secret_values_are_masked_but_keys_stay() {
        let o = obj(json!({"metadata": {"name": "s"}, "type": "Opaque",
            "data": {"password": "aHVudGVyMg=="}, "stringData": {"token": "plain"}}));
        let y = yaml(KubeKind::Secrets, &o);
        assert!(
            y.contains("password") && y.contains("token") && y.contains(MASK),
            "{y}"
        );
        assert!(!y.contains("aHVudGVyMg==") && !y.contains("plain"), "{y}");
    }

    #[test]
    fn a_secrets_last_applied_annotation_is_masked() {
        let applied = r#"{"apiVersion":"v1","kind":"Secret","stringData":{"password":"hunter2"}}"#;
        let o = obj(json!({"metadata": {"name": "s", "annotations": {
            "kubectl.kubernetes.io/last-applied-configuration": applied, "team": "a"}}, "data": {}}));
        let y = yaml(KubeKind::Secrets, &o);
        assert!(!y.contains("hunter2"), "{y}");
        assert!(y.contains("team"), "{y}");
    }

    #[test]
    fn a_huge_document_is_cut_with_a_note() {
        let big = "x".repeat(YAML_CAP + 10);
        let o = obj(json!({"metadata": {"name": "c"}, "data": {"big": big}}));
        let y = yaml(KubeKind::ConfigMaps, &o);
        assert!(y.len() <= YAML_CAP + 64);
        assert!(y.ends_with("# … cut at 1 MiB\n"), "{}", &y[y.len() - 40..]);
    }
}
