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
    let mut restored = Vec::new();
    walk(
        submitted,
        Some(stored),
        Some(&masked),
        &mut Vec::new(),
        &mut restored,
    )
}

/// `unmask` on a copy: the full values, and an overlay of only what was
/// restored (whole lists where a restore is inside one), for helm's second
/// values file. The user's own YAML stays the first file, so helm parses it
/// by its own rules (`0644` is octal there).
pub fn restore(submitted: &Value, stored: &Value) -> Result<(Value, Option<Value>)> {
    let mut full = submitted.clone();
    let mut masked = stored.clone();
    mask_values(&mut masked);
    let mut restored = Vec::new();
    walk(
        &mut full,
        Some(stored),
        Some(&masked),
        &mut Vec::new(),
        &mut restored,
    )?;
    if restored.is_empty() {
        return Ok((full, None));
    }
    let mut overlay = Value::Object(Default::default());
    for path in restored {
        // Helm replaces lists whole, so a restore inside one carries the list.
        let cut = path
            .iter()
            .position(|s| matches!(s, Seg::Index(_)))
            .unwrap_or(path.len());
        let prefix = &path[..cut];
        if let Some(v) = get(&full, prefix) {
            set(&mut overlay, prefix, v.clone());
        }
    }
    Ok((full, Some(overlay)))
}

#[derive(Clone)]
enum Seg {
    Key(String),
    Index(usize),
}

fn shown(path: &[Seg]) -> String {
    let mut out = String::new();
    for s in path {
        match s {
            Seg::Key(k) if out.is_empty() => out.push_str(k),
            Seg::Key(k) => {
                out.push('.');
                out.push_str(k);
            }
            Seg::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    if out.is_empty() {
        "the top level".into()
    } else {
        out
    }
}

fn refused(path: &[Seg]) -> Error {
    Error::Invalid(format!(
        "values contain the mask at {}; type the value",
        shown(path)
    ))
}

fn contains_mask(v: &Value) -> bool {
    match v {
        Value::String(s) => s == MASK,
        Value::Object(m) => m.values().any(contains_mask),
        Value::Array(a) => a.iter().any(contains_mask),
        _ => false,
    }
}

fn walk(
    sub: &mut Value,
    stored: Option<&Value>,
    masked: Option<&Value>,
    path: &mut Vec<Seg>,
    restored: &mut Vec<Vec<Seg>>,
) -> Result<()> {
    match sub {
        Value::String(s) if s == MASK => match (stored, masked) {
            (Some(st), Some(Value::String(m))) if m == MASK => {
                *sub = st.clone();
                restored.push(path.clone());
                Ok(())
            }
            _ => Err(refused(path)),
        },
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                path.push(Seg::Key(k.clone()));
                walk(
                    v,
                    stored.and_then(|s| s.get(k)),
                    masked.and_then(|m| m.get(k)),
                    path,
                    restored,
                )?;
                path.pop();
            }
            Ok(())
        }
        // A list entry is matched by position, which a deletion shifts: a
        // masked entry is restored only when it is otherwise untouched.
        Value::Array(items) => {
            for (i, v) in items.iter_mut().enumerate() {
                if !contains_mask(v) {
                    continue;
                }
                path.push(Seg::Index(i));
                match (stored.and_then(|s| s.get(i)), masked.and_then(|m| m.get(i))) {
                    (Some(st), Some(m)) if m == v => {
                        *v = st.clone();
                        restored.push(path.clone());
                    }
                    _ => return Err(refused(path)),
                }
                path.pop();
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn get<'a>(v: &'a Value, path: &[Seg]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, s| match s {
        Seg::Key(k) => v.get(k),
        Seg::Index(i) => v.get(*i),
    })
}

/// Sets `value` at a path of keys, making objects on the way.
fn set(root: &mut Value, path: &[Seg], value: Value) {
    let mut cur = root;
    for (n, s) in path.iter().enumerate() {
        let Seg::Key(k) = s else { return };
        let map = cur.as_object_mut().expect("overlay levels are objects");
        if n + 1 == path.len() {
            map.insert(k.clone(), value);
            return;
        }
        cur = map
            .entry(k.clone())
            .or_insert_with(|| Value::Object(Default::default()));
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

    #[test]
    fn a_masked_list_entry_is_restored_only_when_it_is_untouched() {
        let stored = json!({"users": [{"name": "alice", "password": "A"}, {"name": "bob", "password": "B"}]});
        let mut same = json!({"users": [{"name": "alice", "password": MASK}, {"name": "bob", "password": MASK}]});
        unmask(&mut same, &stored).unwrap();
        assert_eq!(same, stored);
        // alice deleted: bob now sits where alice was, and must not get her password.
        let mut shifted = json!({"users": [{"name": "bob", "password": MASK}]});
        let e = unmask(&mut shifted, &stored).unwrap_err().to_string();
        assert!(
            e.contains("values contain the mask at users[0]; type the value"),
            "{e}"
        );
    }

    #[test]
    fn restore_gives_the_full_values_and_an_overlay_of_only_what_it_restored() {
        let stored = json!({"db": {"password": "s3cret", "host": "h"}, "users": [{"name": "a", "token": "T"}], "mode": 420});
        let sent = json!({"db": {"password": MASK, "host": "h2"}, "users": [{"name": "a", "token": MASK}], "mode": 644});
        let (full, overlay) = restore(&sent, &stored).unwrap();
        assert_eq!(
            full,
            json!({"db": {"password": "s3cret", "host": "h2"}, "users": [{"name": "a", "token": "T"}], "mode": 644})
        );
        assert_eq!(
            overlay,
            Some(json!({"db": {"password": "s3cret"}, "users": [{"name": "a", "token": "T"}]}))
        );
        let (_, none) = restore(&json!({"a": 1}), &stored).unwrap();
        assert_eq!(none, None);
    }
}
