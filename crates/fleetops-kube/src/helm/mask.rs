//! What a release shows (spec §2.4): values with credential-looking keys
//! masked, and the manifest with Secret values masked. Best effort for
//! values: a credential under an innocent key is shown.

use fleetops_core::kube::MASK;
use serde_json::Value;

const SECRETISH: [&str; 9] = [
    "password",
    "passwd",
    "secret",
    "token",
    "key",
    "credential",
    "auth",
    "cert",
    "private",
];

pub(crate) fn secretish(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    SECRETISH.iter().any(|s| k.contains(s))
}

/// Replaces every non-null value under a credential-looking key, whole
/// subtrees included.
pub fn mask_values(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if secretish(k) && !val.is_null() {
                    *val = MASK.into();
                } else {
                    mask_values(val);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_values),
        _ => {}
    }
}

/// Masked values as YAML; empty when there are none.
pub fn values_yaml(config: &Value) -> String {
    if config.is_null() || config.as_object().is_some_and(|m| m.is_empty()) {
        return String::new();
    }
    let mut v = config.clone();
    mask_values(&mut v);
    serde_saphyr::to_string(&v).unwrap_or_else(|e| format!("# cannot render: {e}\n"))
}

/// The manifest with each Secret's values masked. Other documents are
/// shown as rendered; one that does not parse is replaced, since it might
/// be a Secret.
pub fn manifest(m: &str) -> String {
    let mut out = String::new();
    for doc in crate::apply::documents(m) {
        if doc.trim().is_empty() {
            continue;
        }
        let comments: String = doc
            .lines()
            .take_while(|l| l.starts_with('#'))
            .map(|l| format!("{l}\n"))
            .collect();
        out.push_str("---\n");
        match serde_saphyr::from_str::<Value>(&doc) {
            Ok(mut v) if v.get("kind").and_then(Value::as_str) == Some("Secret") => {
                for field in ["data", "stringData"] {
                    if let Some(map) = v.get_mut(field).and_then(Value::as_object_mut) {
                        map.values_mut().for_each(|x| *x = MASK.into());
                    }
                }
                out.push_str(&comments);
                out.push_str(
                    &serde_saphyr::to_string(&v).unwrap_or_else(|_| "# (Secret hidden)\n".into()),
                );
            }
            Ok(_) => out.push_str(&doc),
            Err(_) => {
                out.push_str(&comments);
                out.push_str("# (document could not be parsed; hidden)\n");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::kube::MASK;
    use serde_json::json;

    #[test]
    fn secretish_keys_are_masked_anywhere() {
        let mut v = json!({
            "replicas": 2,
            "db": {"host": "db", "password": "hunter2"},
            "users": [{"name": "a", "apiKey": "k1"}],
            "TLS_CERT": "-----BEGIN",
            "auth": {"user": "u", "pass": "p"},
            "privateRegistry": null,
            "tokens": ["t1", "t2"]
        });
        mask_values(&mut v);
        assert_eq!(v["replicas"], 2);
        assert_eq!(v["db"]["host"], "db");
        assert_eq!(v["db"]["password"], MASK);
        assert_eq!(v["users"][0]["name"], "a");
        assert_eq!(v["users"][0]["apiKey"], MASK);
        assert_eq!(v["TLS_CERT"], MASK);
        assert_eq!(v["auth"], MASK);
        assert!(
            v["privateRegistry"].is_null(),
            "an unset value is not a secret"
        );
        assert_eq!(v["tokens"], MASK);
    }

    #[test]
    fn values_render_as_yaml_and_nothing_is_empty() {
        let y = values_yaml(&json!({"db": {"password": "hunter2"}}));
        assert!(
            y.contains("password") && y.contains(MASK) && !y.contains("hunter2"),
            "{y}"
        );
        assert_eq!(values_yaml(&serde_json::Value::Null), "");
        assert_eq!(values_yaml(&json!({})), "");
    }

    #[test]
    fn manifest_secrets_and_unparseable_documents_are_hidden() {
        let m = "---\n# Source: x/secret.yaml\napiVersion: v1\nkind: Secret\nmetadata:\n  name: s\ndata:\n  a: aHVudGVyMg==\nstringData:\n  b: plain-secret\n---\n# Source: x/cm.yaml\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\ndata:\n  k: visible\n---\napiVersion: v1\nkind: Secret\ndata: [\n";
        let out = manifest(m);
        assert!(
            !out.contains("aHVudGVyMg==") && !out.contains("plain-secret"),
            "{out}"
        );
        assert!(
            out.contains("# Source: x/secret.yaml"),
            "comments kept: {out}"
        );
        assert!(
            out.contains("k: visible"),
            "other kinds shown as rendered: {out}"
        );
        assert!(out.contains("could not be parsed"), "{out}");
        assert!(!out.contains("data: ["), "{out}");
    }
}
