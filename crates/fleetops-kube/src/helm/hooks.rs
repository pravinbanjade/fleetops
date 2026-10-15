//! Chart hooks for install and upgrade (spec 3j §3): which run, in what
//! order, and when a hook object has finished.

use fleetops_core::helm::{HookStep, RenderedHook};
use serde_json::Value;

/// Hook kinds in Helm's install order, so equal weights run as Helm does.
fn kind_rank(kind: &str) -> usize {
    const ORDER: [&str; 12] = [
        "Namespace",
        "ServiceAccount",
        "Secret",
        "ConfigMap",
        "PersistentVolumeClaim",
        "Role",
        "RoleBinding",
        "Service",
        "Pod",
        "Deployment",
        "StatefulSet",
        "Job",
    ];
    ORDER.iter().position(|k| *k == kind).unwrap_or(ORDER.len())
}

pub fn for_phase<'a>(hooks: &'a [RenderedHook], phase: &str) -> Vec<&'a RenderedHook> {
    let mut v: Vec<_> = hooks
        .iter()
        .filter(|h| h.events.iter().any(|e| e == phase))
        .collect();
    v.sort_by(|a, b| {
        (a.weight, kind_rank(&a.kind), &a.name).cmp(&(b.weight, kind_rank(&b.kind), &b.name))
    });
    v
}

pub fn steps(hooks: &[RenderedHook], upgrade: bool) -> Vec<HookStep> {
    let act = if upgrade { "upgrade" } else { "install" };
    ["pre", "post"]
        .iter()
        .flat_map(|when| for_phase(hooks, &format!("{when}-{act}")))
        .map(|h| HookStep {
            events: h.events.clone(),
            kind: h.kind.clone(),
            name: h.name.clone(),
            weight: h.weight,
            delete_policies: policies(h).iter().map(|p| p.to_string()).collect(),
        })
        .collect()
}

pub fn policies(h: &RenderedHook) -> Vec<&str> {
    if h.delete_policies.is_empty() {
        vec!["before-hook-creation"]
    } else {
        h.delete_policies.iter().map(String::as_str).collect()
    }
}

#[derive(Debug, PartialEq)]
pub enum HookState {
    Running,
    Succeeded,
    Failed(String),
}

pub fn state(kind: &str, live: &Value) -> HookState {
    match kind {
        "Job" => {
            let conds = live
                .pointer("/status/conditions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let on = |t: &str| {
                conds
                    .iter()
                    .find(|c| c["type"] == t && c["status"] == "True")
            };
            if on("Complete").is_some() {
                HookState::Succeeded
            } else if let Some(c) = on("Failed") {
                let r = c["reason"].as_str().unwrap_or("Failed");
                HookState::Failed(match c["message"].as_str() {
                    Some(m) => format!("{r}: {m}"),
                    None => r.to_string(),
                })
            } else {
                HookState::Running
            }
        }
        "Pod" => match live.pointer("/status/phase").and_then(Value::as_str) {
            Some("Succeeded") => HookState::Succeeded,
            Some("Failed") => HookState::Failed(
                live.pointer("/status/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("Failed")
                    .to_string(),
            ),
            _ => HookState::Running,
        },
        _ => HookState::Succeeded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hook(name: &str, kind: &str, events: &[&str], weight: i32) -> RenderedHook {
        RenderedHook {
            name: name.into(),
            kind: kind.into(),
            path: "c/templates/h.yaml".into(),
            manifest: String::new(),
            events: events.iter().map(|e| e.to_string()).collect(),
            weight,
            delete_policies: vec![],
        }
    }

    #[test]
    fn a_phase_runs_by_weight_then_kind_then_name() {
        let hs = [
            hook("b", "Job", &["pre-install"], 0),
            hook("a", "Job", &["pre-install"], 0),
            hook("cm", "ConfigMap", &["pre-install"], 0),
            hook("first", "Job", &["pre-install", "pre-upgrade"], -5),
            hook("post", "Job", &["post-install"], -10),
            hook("t", "Pod", &["test"], -100),
        ];
        let names: Vec<_> = for_phase(&hs, "pre-install")
            .iter()
            .map(|h| h.name.as_str())
            .collect();
        assert_eq!(names, ["first", "cm", "a", "b"]);
        let plan: Vec<_> = steps(&hs, false).iter().map(|s| s.name.clone()).collect();
        assert_eq!(plan, ["first", "cm", "a", "b", "post"]);
        let up: Vec<_> = steps(&hs, true).iter().map(|s| s.name.clone()).collect();
        assert_eq!(up, ["first"]);
    }

    #[test]
    fn no_policy_means_before_hook_creation() {
        let mut h = hook("a", "Job", &["pre-install"], 0);
        assert_eq!(policies(&h), ["before-hook-creation"]);
        h.delete_policies = vec!["hook-succeeded".into()];
        assert_eq!(policies(&h), ["hook-succeeded"]);
    }

    #[test]
    fn job_and_pod_states() {
        assert_eq!(state("Job", &json!({"status": {}})), HookState::Running);
        assert_eq!(
            state(
                "Job",
                &json!({"status": {"conditions": [{"type": "Complete", "status": "True"}]}})
            ),
            HookState::Succeeded
        );
        assert_eq!(
            state(
                "Job",
                &json!({"status": {"conditions": [{"type": "Failed", "status": "True", "reason": "BackoffLimitExceeded", "message": "Job has reached the specified backoff limit"}]}})
            ),
            HookState::Failed(
                "BackoffLimitExceeded: Job has reached the specified backoff limit".into()
            )
        );
        assert_eq!(
            state("Pod", &json!({"status": {"phase": "Running"}})),
            HookState::Running
        );
        assert_eq!(
            state("Pod", &json!({"status": {"phase": "Succeeded"}})),
            HookState::Succeeded
        );
        assert_eq!(
            state(
                "Pod",
                &json!({"status": {"phase": "Failed", "reason": "Error"}})
            ),
            HookState::Failed("Error".into())
        );
        assert_eq!(state("ConfigMap", &json!({})), HookState::Succeeded);
    }
}
