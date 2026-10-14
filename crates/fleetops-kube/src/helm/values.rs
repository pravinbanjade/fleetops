//! User-supplied values (spec 3j §4): parsed from the editor's YAML, with
//! masked leaves restored from the stored release on the server.

use super::mask::{mask_values, secretish};
use fleetops_core::kube::MASK;
use fleetops_core::{Error, Result};
use serde_json::Value;

/// The editor's YAML as values: empty is none; anything else must be a mapping.
pub fn parse(yaml: &str) -> Result<Value> {
    if yaml.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let v: Value = serde_saphyr::from_str(yaml)
        .map_err(|e| Error::Invalid(format!("values are not valid YAML: {e}")))?;
    match v {
        Value::Object(_) => Ok(v),
        Value::Null => Ok(Value::Object(Default::default())),
        _ => Err(Error::Invalid(
            "values must be a mapping of keys to values".into(),
        )),
    }
}

/// Puts the stored value back wherever the editor sent the mask unchanged.
/// A mask anywhere the stored values were not masked is refused.
pub fn unmask(submitted: &mut Value, stored: &Value) -> Result<()> {
    let mut masked = stored.clone();
    mask_values(&mut masked);
    walk(submitted, Some(stored), Some(&masked), String::new())
}

fn walk(
    sub: &mut Value,
    stored: Option<&Value>,
    masked: Option<&Value>,
    path: String,
) -> Result<()> {
    match sub {
        Value::String(s) if s == MASK => match (stored, masked) {
            (Some(st), Some(Value::String(m))) if m == MASK => {
                *sub = st.clone();
                Ok(())
            }
            _ => {
                let at = if path.is_empty() {
                    "the top level".to_string()
                } else {
                    path
                };
                Err(Error::Invalid(format!(
                    "values contain the mask at {at}; type the value"
                )))
            }
        },
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(
                    v,
                    stored.and_then(|s| s.get(k)),
                    masked.and_then(|m| m.get(k)),
                    p,
                )?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for (i, v) in items.iter_mut().enumerate() {
                walk(
                    v,
                    stored.and_then(|s| s.get(i)),
                    masked.and_then(|m| m.get(i)),
                    format!("{path}[{i}]"),
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Every scalar under a credential-looking key, as text, for scrubbing
/// messages. Very short ones are left: they would scrub ordinary words.
pub fn secret_strings(v: &Value) -> Vec<String> {
    fn leaves(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Number(n) => out.push(n.to_string()),
            Value::Object(m) => m.values().for_each(|x| leaves(x, out)),
            Value::Array(a) => a.iter().for_each(|x| leaves(x, out)),
            _ => {}
        }
    }
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    if secretish(k) {
                        leaves(x, out);
                    } else {
                        walk(x, out);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out.retain(|s| s.len() >= 4);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_is_no_values_and_a_mapping_is_required() {
        assert_eq!(parse("").unwrap(), json!({}));
        assert_eq!(parse("  \n").unwrap(), json!({}));
        assert_eq!(
            parse("a: 1\nb: {c: x}\n").unwrap(),
            json!({"a": 1, "b": {"c": "x"}})
        );
        for bad in ["- a\n- b\n", "just text", "a: [1,\n"] {
            let e = parse(bad).unwrap_err();
            assert!(matches!(e, Error::Invalid(_)), "{bad}: {e}");
        }
        assert!(parse("- a\n").unwrap_err().to_string().contains("mapping"));
    }

    #[test]
    fn an_untouched_mask_is_the_stored_value() {
        let stored =
            json!({"db": {"password": "s3cret", "host": "h"}, "auth": {"user": "u", "token": "t"}});
        let mut sent = json!({"db": {"password": MASK, "host": "h2"}, "auth": MASK});
        unmask(&mut sent, &stored).unwrap();
        assert_eq!(
            sent,
            json!({"db": {"password": "s3cret", "host": "h2"}, "auth": {"user": "u", "token": "t"}})
        );
    }

    #[test]
    fn a_mask_where_nothing_was_masked_is_refused() {
        let stored = json!({"db": {"host": "h"}});
        for (mut sent, path) in [
            (json!({"db": {"host": MASK}}), "db.host"),
            (json!({"new": {"password": MASK}}), "new.password"),
            (json!({"list": [1, MASK]}), "list[1]"),
        ] {
            let e = unmask(&mut sent, &stored).unwrap_err().to_string();
            assert!(
                e.contains(&format!(
                    "values contain the mask at {path}; type the value"
                )),
                "{e}"
            );
        }
        let mut install = json!({"password": MASK});
        assert!(
            unmask(&mut install, &Value::Null).is_err(),
            "an install has nothing to restore"
        );
    }

    #[test]
    fn secret_strings_are_the_leaves_under_secret_keys() {
        let v = json!({"image": "busybox", "db": {"password": "hunter2", "port": 5432}, "tls": {"privateKey": {"pem": "-----BEGIN"}}, "x": "ab"});
        let mut s = secret_strings(&v);
        s.sort();
        assert_eq!(s, ["-----BEGIN", "hunter2"]);
    }
}
