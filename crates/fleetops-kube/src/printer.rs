//! CRD printer columns (spec 3g §2.3): kubectl's JSONPath against one
//! object, formatted by the column's type. A path that fails or finds
//! nothing is an empty cell, never an error.

use fleetops_core::kube::PrinterColumn;
use jsonpath_rust::JsonPath;
use serde_json::Value;

/// kubectl's `.a.b[?(@.x=="y")]` as RFC 9535 JSONPath: rooted at `$`, and
/// with the filter's optional parentheses dropped.
fn normalize(path: &str) -> String {
    let p = path.trim();
    let rooted = if p.starts_with('$') {
        p.to_string()
    } else {
        format!("${p}")
    };
    rooted.replace("[?(", "[?").replace(")]", "]")
}

fn format(ty: &str, v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) if ty == "date" => s
            .parse::<k8s_openapi::jiff::Timestamp>()
            .map(|t| t.as_second().to_string())
            .unwrap_or_else(|_| s.clone()),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

pub fn evaluate(col: &PrinterColumn, obj: &Value) -> String {
    match obj.query(&normalize(&col.json_path)) {
        Ok(found) => found
            .into_iter()
            .map(|v| format(&col.type_, v))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => {
            tracing::debug!(path = %col.json_path, error = %e, "printer column did not evaluate");
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::kube::PrinterColumn;
    use serde_json::json;

    fn col(path: &str, ty: &str) -> PrinterColumn {
        PrinterColumn {
            name: "c".into(),
            json_path: path.into(),
            type_: ty.into(),
        }
    }

    fn cert() -> serde_json::Value {
        json!({"spec": {"secretName": "tls", "replicas": 3, "paused": false},
               "status": {"conditions": [{"type": "Issuing", "status": "False"}, {"type": "Ready", "status": "True"}],
                          "notAfter": "2026-10-01T00:00:00Z"}})
    }

    #[test]
    fn a_plain_path_reads_the_field() {
        assert_eq!(evaluate(&col(".spec.secretName", "string"), &cert()), "tls");
        assert_eq!(evaluate(&col(".spec.replicas", "integer"), &cert()), "3");
        assert_eq!(evaluate(&col(".spec.paused", "boolean"), &cert()), "false");
    }

    #[test]
    fn kubectl_filter_syntax_finds_the_condition() {
        let c = col(r#".status.conditions[?(@.type=="Ready")].status"#, "string");
        assert_eq!(evaluate(&c, &cert()), "True");
    }

    #[test]
    fn a_date_becomes_unix_seconds() {
        assert_eq!(
            evaluate(&col(".status.notAfter", "date"), &cert()),
            "1790812800"
        );
    }

    #[test]
    fn missing_fields_and_bad_paths_are_empty() {
        assert_eq!(evaluate(&col(".spec.nope", "string"), &cert()), "");
        assert_eq!(evaluate(&col(".spec[[[", "string"), &cert()), "");
    }

    #[test]
    fn several_matches_are_joined() {
        let c = col(".status.conditions[*].type", "string");
        assert_eq!(evaluate(&c, &cert()), "Issuing,Ready");
    }
}
