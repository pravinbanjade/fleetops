//! Multi-document YAML as applied (spec §2.3): split, checked, and given
//! a namespace when the kind needs one.

use fleetops_core::kube::{ApplyResult, APPLY_MAX_DOCS};
use fleetops_core::{Error, Result};
use kube::api::GroupVersionKind;
use serde_json::Value;

#[derive(Debug)]
pub struct Doc {
    pub index: usize,
    pub value: Value,
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

impl Doc {
    /// A result naming this document, with nothing done yet.
    pub fn result(&self) -> ApplyResult {
        ApplyResult {
            index: self.index,
            api_version: self.api_version.clone(),
            kind: self.kind.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
            action: None,
            before: None,
            after: None,
            error: None,
        }
    }
}

/// The text of each document. A line of `---` (optionally followed by a
/// space or comment) separates documents.
pub(crate) fn documents(yaml: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for line in yaml.lines() {
        if line == "---" || line.starts_with("--- ") || line.starts_with("---\t") {
            out.push(String::new());
            continue;
        }
        let cur = out.last_mut().expect("never empty");
        cur.push_str(line);
        cur.push('\n');
    }
    out
}

fn text(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn split(yaml: &str) -> Result<Vec<Doc>> {
    let mut docs = Vec::new();
    for raw in documents(yaml) {
        let n = docs.len() + 1;
        let value: Value = serde_saphyr::from_str(&raw)
            .map_err(|e| Error::Invalid(format!("document {n}: {e}")))?;
        if value.is_null() {
            continue;
        }
        if !value.is_object() {
            return Err(Error::Invalid(format!(
                "document {n} is not a Kubernetes object"
            )));
        }
        let need = |ptr: &str, what: &str| {
            text(&value, ptr).ok_or_else(|| Error::Invalid(format!("document {n} has no {what}")))
        };
        let doc = Doc {
            index: docs.len(),
            api_version: need("/apiVersion", "apiVersion")?,
            kind: need("/kind", "kind")?,
            name: need("/metadata/name", "metadata.name")?,
            namespace: text(&value, "/metadata/namespace"),
            value,
        };
        docs.push(doc);
        if docs.len() > APPLY_MAX_DOCS {
            return Err(Error::Invalid(format!(
                "at most {APPLY_MAX_DOCS} documents can be applied at once"
            )));
        }
    }
    if docs.is_empty() {
        return Err(Error::Invalid("there is nothing to apply".into()));
    }
    Ok(docs)
}

/// Namespaced kinds without a namespace take `default_ns`; cluster-scoped
/// kinds lose any namespace they were given.
pub fn set_namespace(doc: &mut Doc, namespaced: bool, default_ns: &str) {
    let meta = doc.value["metadata"]
        .as_object_mut()
        .expect("checked in split");
    if namespaced {
        let ns = doc.namespace.get_or_insert_with(|| default_ns.to_string());
        meta.insert("namespace".into(), Value::String(ns.clone()));
    } else {
        doc.namespace = None;
        meta.remove("namespace");
    }
}

pub fn gvk(api_version: &str, kind: &str) -> GroupVersionKind {
    let (group, version) = api_version.split_once('/').unwrap_or(("", api_version));
    GroupVersionKind::gvk(group, version, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_are_split_and_empty_ones_skipped() {
        let y = "---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n---\n# only a comment\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: b\n  namespace: x\n";
        let docs = split(y).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(
            (docs[0].index, docs[0].kind.as_str(), docs[0].name.as_str()),
            (0, "ConfigMap", "a")
        );
        assert_eq!(
            (docs[1].index, docs[1].namespace.as_deref()),
            (1, Some("x"))
        );
    }

    #[test]
    fn a_document_without_a_name_is_refused_with_its_position() {
        let e = split("apiVersion: v1\nkind: ConfigMap\nmetadata: {}\n").unwrap_err();
        assert!(
            matches!(&e, Error::Invalid(m) if m.contains("document 1") && m.contains("metadata.name")),
            "{e}"
        );
    }

    #[test]
    fn bad_yaml_names_the_document() {
        let e = split("apiVersion: v1\nkind: [\n").unwrap_err();
        assert!(
            matches!(&e, Error::Invalid(m) if m.starts_with("document 1")),
            "{e}"
        );
    }

    #[test]
    fn nothing_to_apply_is_an_error() {
        assert!(matches!(split("\n---\n"), Err(Error::Invalid(_))));
    }

    #[test]
    fn more_than_the_cap_is_refused() {
        let one = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n";
        let y = vec![one; APPLY_MAX_DOCS + 1].join("---\n");
        assert!(matches!(split(&y), Err(Error::Invalid(m)) if m.contains("100")));
    }

    #[test]
    fn namespaced_documents_take_the_default_and_cluster_ones_drop_it() {
        let mut d = split("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n")
            .unwrap()
            .remove(0);
        set_namespace(&mut d, true, "apps");
        assert_eq!(d.namespace.as_deref(), Some("apps"));
        assert_eq!(d.value["metadata"]["namespace"], "apps");
        let mut n =
            split("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: ns1\n  namespace: x\n")
                .unwrap()
                .remove(0);
        set_namespace(&mut n, false, "apps");
        assert!(n.namespace.is_none() && n.value["metadata"].get("namespace").is_none());
    }

    #[test]
    fn a_core_api_version_has_an_empty_group() {
        let g = gvk("v1", "Pod");
        assert_eq!((g.group.as_str(), g.version.as_str()), ("", "v1"));
        let g = gvk("apps/v1", "Deployment");
        assert_eq!(
            (g.group.as_str(), g.version.as_str(), g.kind.as_str()),
            ("apps", "v1", "Deployment")
        );
    }
}
