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
    // Masked by what the apiserver returned too, not only by what was asked
    // for: a Secret must never render unmasked.
    let secret =
        kind == KubeKind::Secrets || obj.types.as_ref().is_some_and(|t| t.kind == "Secret");
    let mut v = typed_json(kind, obj);
    if let Some(meta) = v.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
    }
    if secret {
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

fn is_secret(v: &Value) -> bool {
    v.get("kind").and_then(Value::as_str) == Some("Secret")
}

fn mask_values(v: &mut Value) {
    for field in ["data", "stringData"] {
        if let Some(map) = v.get_mut(field).and_then(Value::as_object_mut) {
            for val in map.values_mut() {
                *val = MASK.into();
            }
        }
    }
}

/// The object as the editor shows it: no `managedFields`, `status` or
/// last-applied annotation, and a Secret's values masked.
pub fn clean(v: &mut Value) {
    strip(v);
    if is_secret(v) {
        mask_values(v);
    }
}

/// `clean` without the masking: for comparing objects, where a changed
/// Secret value must still count as a change. Never shown.
pub fn strip(v: &mut Value) {
    if let Some(meta) = v.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
        if let Some(ann) = meta.get_mut("annotations").and_then(Value::as_object_mut) {
            ann.remove(LAST_APPLIED);
            if ann.is_empty() {
                meta.remove("annotations");
            }
        }
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("status");
    }
}

/// YAML for the editor. Unlike the page view it is never cut: a cut
/// document saved back would destroy the object.
pub fn to_yaml(v: &Value) -> fleetops_core::Result<String> {
    let text = serde_saphyr::to_string(v)
        .map_err(|e| fleetops_core::Error::Internal(format!("cannot render YAML: {e}")))?;
    if text.len() > YAML_CAP {
        return Err(fleetops_core::Error::Invalid(
            "this object is larger than 1 MiB and cannot be edited here".into(),
        ));
    }
    Ok(text)
}

/// Puts a Secret's real values back wherever the edit left the mask
/// (spec §2.3). A masked `stringData` value moves to `data`, since the
/// apiserver only returns `data`. Changed values pass through.
pub fn unmask(edited: &mut Value, live: Option<&Value>) -> Result<(), String> {
    if !is_secret(edited) {
        return Ok(());
    }
    let live_data = live
        .and_then(|l| l.get("data"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut restored = serde_json::Map::new();
    for field in ["data", "stringData"] {
        let Some(map) = edited.get_mut(field).and_then(Value::as_object_mut) else {
            continue;
        };
        let masked: Vec<String> = map
            .iter()
            .filter(|(_, v)| v.as_str() == Some(MASK))
            .map(|(k, _)| k.clone())
            .collect();
        for k in masked {
            let real = live_data
                .get(&k)
                .ok_or_else(|| format!("masked value for key {k} has no current value"))?;
            map.remove(&k);
            restored.insert(k, real.clone());
        }
    }
    if !restored.is_empty() {
        let obj = edited.as_object_mut().expect("a Secret is an object");
        let data = obj
            .entry("data")
            .or_insert_with(|| Value::Object(Default::default()));
        if !data.is_object() {
            *data = Value::Object(Default::default());
        }
        data.as_object_mut().unwrap().extend(restored);
    }
    Ok(())
}

/// Puts back what `clean` hid and a replace must keep: kubectl's
/// last-applied annotation (a PUT without it would drop it).
pub fn restore_hidden(edited: &mut Value, live: &Value) {
    let Some(applied) = live["metadata"]["annotations"].get(LAST_APPLIED).cloned() else {
        return;
    };
    let Some(meta) = edited.get_mut("metadata").and_then(Value::as_object_mut) else {
        return;
    };
    let ann = meta
        .entry("annotations")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(ann) = ann.as_object_mut() {
        ann.entry(LAST_APPLIED).or_insert(applied);
    }
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
    fn a_secret_is_masked_whatever_kind_was_asked_for() {
        let o = obj(
            json!({"kind": "Secret", "apiVersion": "v1", "metadata": {"name": "s"},
            "data": {"password": "aHVudGVyMg=="}}),
        );
        let y = yaml(KubeKind::ConfigMaps, &o);
        assert!(!y.contains("aHVudGVyMg=="), "{y}");
    }

    #[test]
    fn a_huge_document_is_cut_with_a_note() {
        let big = "x".repeat(YAML_CAP + 10);
        let o = obj(json!({"metadata": {"name": "c"}, "data": {"big": big}}));
        let y = yaml(KubeKind::ConfigMaps, &o);
        assert!(y.len() <= YAML_CAP + 64);
        assert!(y.ends_with("# … cut at 1 MiB\n"), "{}", &y[y.len() - 40..]);
    }

    #[test]
    fn clean_strips_server_fields_and_masks_secrets() {
        let mut v = json!({"apiVersion": "v1", "kind": "Secret",
            "metadata": {"name": "s", "managedFields": [{}], "annotations": {
                "kubectl.kubernetes.io/last-applied-configuration": "{\"data\":{\"p\":\"x\"}}", "team": "a"}},
            "status": {"x": 1}, "data": {"p": "aHVudGVyMg=="}});
        clean(&mut v);
        assert!(v["metadata"].get("managedFields").is_none());
        assert!(v.get("status").is_none());
        assert!(v["metadata"]["annotations"].get(LAST_APPLIED).is_none());
        assert_eq!(v["metadata"]["annotations"]["team"], "a");
        assert_eq!(v["data"]["p"], MASK);
    }

    #[test]
    fn clean_leaves_non_secret_data_alone() {
        let mut v =
            json!({"kind": "ConfigMap", "metadata": {"name": "c"}, "data": {"mode": "prod"}});
        clean(&mut v);
        assert_eq!(v["data"]["mode"], "prod");
    }

    #[test]
    fn unmask_restores_untouched_values_and_keeps_changed_ones() {
        let live = json!({"kind": "Secret", "data": {"a": "QQ==", "b": "Qg=="}});
        let mut edited = json!({"kind": "Secret", "data": {"a": MASK, "b": "bmV3"}});
        unmask(&mut edited, Some(&live)).unwrap();
        assert_eq!(edited["data"], json!({"a": "QQ==", "b": "bmV3"}));
    }

    #[test]
    fn a_masked_string_data_value_is_restored_into_data() {
        let live = json!({"kind": "Secret", "data": {"token": "dA=="}});
        let mut edited = json!({"kind": "Secret", "stringData": {"token": MASK, "new": "plain"}});
        unmask(&mut edited, Some(&live)).unwrap();
        assert_eq!(edited["stringData"], json!({"new": "plain"}));
        assert_eq!(edited["data"], json!({"token": "dA=="}));
    }

    #[test]
    fn a_masked_value_without_a_live_counterpart_is_refused() {
        let mut edited = json!({"kind": "Secret", "data": {"ghost": MASK}});
        let e = unmask(&mut edited, None).unwrap_err();
        assert!(e.contains("ghost"), "{e}");
    }

    #[test]
    fn unmask_ignores_other_kinds() {
        let mut edited = json!({"kind": "ConfigMap", "data": {"x": MASK}});
        unmask(&mut edited, None).unwrap();
        assert_eq!(edited["data"]["x"], MASK);
    }

    #[test]
    fn restore_hidden_puts_back_the_last_applied_annotation() {
        let live = json!({"metadata": {"annotations": {LAST_APPLIED: "{}"}}});
        let mut edited = json!({"metadata": {"name": "x"}});
        restore_hidden(&mut edited, &live);
        assert_eq!(edited["metadata"]["annotations"][LAST_APPLIED], "{}");
    }

    #[test]
    fn an_object_too_large_to_edit_is_refused() {
        let v = json!({"data": {"big": "x".repeat(YAML_CAP + 1)}});
        assert!(matches!(to_yaml(&v), Err(fleetops_core::Error::Invalid(_))));
    }
}
