//! Logs merged across a workload's pods (spec 3i §4): its selector, a
//! watch of its pods, and one log task per pod into one stream.

use fleetops_core::kube::{KubeRow, RowHealth, WatchEvent};
use fleetops_core::streams::{LogLine, LogStream};
use fleetops_core::{Error, Result};
use kube::api::DynamicObject;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Most pods one merged log reads at once (spec 3i §2).
pub const WORKLOAD_LOG_PODS: usize = 25;

/// `spec.selector` as a label selector string, as `kubectl` writes one.
/// An empty selector would match every pod in the namespace, so it is refused.
pub fn selector(sel: &Value) -> Result<String> {
    let mut parts = Vec::new();
    for (k, v) in sel["matchLabels"].as_object().into_iter().flatten() {
        parts.push(format!("{k}={}", v.as_str().unwrap_or_default()));
    }
    for e in sel["matchExpressions"].as_array().into_iter().flatten() {
        let key = e["key"].as_str().unwrap_or_default();
        let values: Vec<&str> = e["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        parts.push(match e["operator"].as_str().unwrap_or_default() {
            "In" => format!("{key} in ({})", values.join(",")),
            "NotIn" => format!("{key} notin ({})", values.join(",")),
            "Exists" => key.to_string(),
            "DoesNotExist" => format!("!{key}"),
            op => return Err(Error::Invalid(format!("unknown selector operator '{op}'"))),
        });
    }
    if parts.is_empty() {
        return Err(Error::Invalid("workload has no selector".into()));
    }
    Ok(parts.join(","))
}

/// A pod as the merged log needs it. The list's row puts a container's
/// reason (`CrashLoopBackOff`) in `status`, which hides the phase this needs.
pub fn log_row(o: &DynamicObject) -> KubeRow {
    let m = &o.metadata;
    let status = if m.deletion_timestamp.is_some() {
        "Terminating"
    } else {
        o.data["status"]["phase"].as_str().unwrap_or("Pending")
    };
    let names = |field: &str| -> Vec<String> {
        o.data["spec"][field]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["name"].as_str().map(str::to_string))
            .collect()
    };
    let mut containers = names("containers");
    containers.extend(names("initContainers"));
    let restarts: i64 = o.data["status"]["containerStatuses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["restartCount"].as_i64())
        .sum();
    KubeRow {
        name: m.name.clone().unwrap_or_default(),
        namespace: m.namespace.clone(),
        created: m
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.as_second())
            .unwrap_or(0),
        status: status.to_string(),
        health: RowHealth::Unknown,
        columns: BTreeMap::from([
            ("containers".to_string(), containers.join(",")),
            ("restarts".to_string(), restarts.to_string()),
        ]),
    }
}

/// What the driver does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Read this pod's log; `since` (unix seconds) when it rejoins after a restart.
    Start {
        pod: String,
        since: Option<i64>,
    },
    Stop {
        pod: String,
    },
    Line(LogLine),
}

pub fn meta(pod: Option<&str>, text: String) -> LogLine {
    LogLine {
        stream: LogStream::Meta,
        text,
        pod: pod.map(str::to_string),
    }
}

fn restarts(r: &KubeRow) -> i64 {
    r.columns
        .get("restarts")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn streamable(r: &KubeRow) -> bool {
    matches!(r.status.as_str(), "Running" | "Succeeded" | "Failed")
}

fn has_container(r: &KubeRow, c: &str) -> bool {
    r.columns
        .get("containers")
        .is_some_and(|all| all.split(',').any(|n| n == c))
}

/// The merged log's pods (spec 3i §2). Pure: the driver feeds it watch
/// events and ended tasks, and carries out the steps it returns.
pub struct PodSet {
    container: String,
    cap: usize,
    known: BTreeMap<String, KubeRow>,
    /// Streaming now, with the restart count each started at.
    streaming: BTreeMap<String, i64>,
    /// Streamable, waiting for a slot.
    waiting: BTreeSet<String>,
    /// Their log ended: when, and the restart count it had started at.
    ended: BTreeMap<String, (i64, i64)>,
    /// Lacking the container; already said.
    skipped: BTreeSet<String>,
    synced: bool,
    capped_said: bool,
}

impl PodSet {
    pub fn new(container: impl Into<String>, cap: usize) -> Self {
        Self {
            container: container.into(),
            cap,
            known: BTreeMap::new(),
            streaming: BTreeMap::new(),
            waiting: BTreeSet::new(),
            ended: BTreeMap::new(),
            skipped: BTreeSet::new(),
            synced: false,
            capped_said: false,
        }
    }

    /// No pod is known: none matched, or every one has gone.
    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }

    pub fn on_event(&mut self, ev: WatchEvent) -> Vec<Step> {
        match ev {
            WatchEvent::Snapshot { mut rows } => {
                let announce = self.synced;
                self.synced = true;
                let present: BTreeSet<&str> = rows.iter().map(|r| r.name.as_str()).collect();
                let gone: Vec<String> = self
                    .known
                    .keys()
                    .filter(|n| !present.contains(n.as_str()))
                    .cloned()
                    .collect();
                let mut out = Vec::new();
                for n in gone {
                    out.extend(self.remove(&n));
                }
                rows.sort_by(|a, b| (a.created, &a.name).cmp(&(b.created, &b.name)));
                let total = rows.len();
                for r in rows {
                    out.extend(self.upsert(r, announce, total));
                }
                out
            }
            WatchEvent::Added { row } | WatchEvent::Modified { row } => {
                let total = self.known.len() + usize::from(!self.known.contains_key(&row.name));
                self.upsert(row, true, total)
            }
            WatchEvent::Deleted { name, .. } => self.remove(&name),
            WatchEvent::Resync => vec![],
        }
    }

    /// `pod`'s log ended by itself (the container exited, or the pod finished).
    pub fn on_ended(&mut self, pod: &str, now: i64) -> Vec<Step> {
        let Some(started_at) = self.streaming.remove(pod) else {
            return vec![];
        };
        self.ended.insert(pod.to_string(), (now, started_at));
        let mut out = self.fill();
        // A restart already seen while the log was still draining.
        if let Some(r) = self.known.get(pod).cloned() {
            let total = self.known.len();
            out.extend(self.upsert(r, true, total));
        }
        out
    }

    fn upsert(&mut self, row: KubeRow, announce: bool, total: usize) -> Vec<Step> {
        let name = row.name.clone();
        self.known.insert(name.clone(), row.clone());
        if self.streaming.contains_key(&name) || self.skipped.contains(&name) {
            return vec![];
        }
        if !streamable(&row) {
            self.waiting.remove(&name);
            return vec![];
        }
        if !has_container(&row, &self.container) {
            self.skipped.insert(name.clone());
            let text = format!("{name} has no container {}", self.container);
            return vec![Step::Line(meta(Some(&name), text))];
        }
        let since = match self.ended.get(&name) {
            Some(&(at, started_at)) if restarts(&row) > started_at => Some(at),
            Some(_) => return vec![],
            None => None,
        };
        if self.streaming.len() >= self.cap {
            self.waiting.insert(name);
            if self.capped_said {
                return vec![];
            }
            self.capped_said = true;
            let text = format!("showing {} of {total} pods", self.cap);
            return vec![Step::Line(meta(None, text))];
        }
        self.ended.remove(&name);
        self.start(name, since, announce)
    }

    fn start(&mut self, name: String, since: Option<i64>, announce: bool) -> Vec<Step> {
        let restarts = self.known.get(&name).map(restarts).unwrap_or(0);
        self.streaming.insert(name.clone(), restarts);
        let mut out = Vec::new();
        if announce {
            out.push(Step::Line(meta(Some(&name), format!("▸ {name} started"))));
        }
        out.push(Step::Start { pod: name, since });
        out
    }

    fn remove(&mut self, name: &str) -> Vec<Step> {
        self.known.remove(name);
        self.waiting.remove(name);
        self.ended.remove(name);
        self.skipped.remove(name);
        if self.streaming.remove(name).is_none() {
            return vec![];
        }
        let mut out = vec![
            Step::Stop {
                pod: name.to_string(),
            },
            Step::Line(meta(Some(name), format!("◂ {name} gone"))),
        ];
        out.extend(self.fill());
        out
    }

    /// Free slots go to the oldest waiting pods.
    fn fill(&mut self) -> Vec<Step> {
        let mut out = Vec::new();
        while self.streaming.len() < self.cap {
            let next = self
                .waiting
                .iter()
                .min_by_key(|n| (self.known.get(*n).map(|r| r.created), (*n).clone()))
                .cloned();
            let Some(n) = next else { break };
            self.waiting.remove(&n);
            out.extend(self.start(n, None, true));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn match_labels_and_every_operator_render_as_kubectl_does() {
        let s = selector(&json!({
            "matchLabels": {"app": "web"},
            "matchExpressions": [
                {"key": "tier", "operator": "In", "values": ["a", "b"]},
                {"key": "env", "operator": "NotIn", "values": ["dev"]},
                {"key": "canary", "operator": "Exists"},
                {"key": "legacy", "operator": "DoesNotExist"}
            ]
        }))
        .unwrap();
        assert_eq!(s, "app=web,tier in (a,b),env notin (dev),canary,!legacy");
    }

    #[test]
    fn an_empty_or_missing_selector_is_refused() {
        for sel in [json!({}), json!(null), json!({"matchLabels": {}})] {
            let e = selector(&sel).unwrap_err();
            assert!(
                matches!(e, Error::Invalid(ref m) if m == "workload has no selector"),
                "{e}"
            );
        }
    }

    #[test]
    fn an_unknown_operator_is_refused() {
        let e = selector(&json!({"matchExpressions": [{"key": "a", "operator": "Gt"}]}));
        assert!(matches!(e, Err(Error::Invalid(_))));
    }

    fn pod(v: Value) -> DynamicObject {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_log_row_keeps_phase_containers_and_restarts() {
        let r = log_row(&pod(json!({"apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": "web-1", "namespace": "apps", "creationTimestamp": "2026-10-14T10:00:00Z"},
            "spec": {"containers": [{"name": "web"}, {"name": "side"}], "initContainers": [{"name": "migrate"}]},
            "status": {"phase": "Running", "containerStatuses": [
                {"name": "web", "restartCount": 2}, {"name": "side", "restartCount": 1}]}})));
        assert_eq!(r.name, "web-1");
        assert_eq!(r.namespace.as_deref(), Some("apps"));
        assert_eq!(r.status, "Running");
        assert_eq!(r.columns["containers"], "web,side,migrate");
        assert_eq!(r.columns["restarts"], "3");
        assert!(r.created > 0);
    }

    #[test]
    fn a_deleting_pod_is_terminating_and_no_status_is_pending() {
        let r = log_row(&pod(json!({"apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": "a", "namespace": "apps", "deletionTimestamp": "2026-10-14T10:00:00Z"},
            "status": {"phase": "Running"}})));
        assert_eq!(r.status, "Terminating");
        let r = log_row(&pod(json!({"apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": "b", "namespace": "apps"}})));
        assert_eq!(r.status, "Pending");
        assert_eq!(r.columns["restarts"], "0");
    }

    use fleetops_core::kube::WatchEvent;
    use fleetops_core::streams::LogStream;

    fn row(name: &str, created: i64, status: &str, restarts: i64) -> KubeRow {
        KubeRow {
            name: name.into(),
            namespace: Some("apps".into()),
            created,
            status: status.into(),
            health: RowHealth::Unknown,
            columns: BTreeMap::from([
                ("containers".to_string(), "web,side".to_string()),
                ("restarts".to_string(), restarts.to_string()),
            ]),
        }
    }
    fn snap(rows: Vec<KubeRow>) -> WatchEvent {
        WatchEvent::Snapshot { rows }
    }
    fn start(pod: &str) -> Step {
        Step::Start {
            pod: pod.into(),
            since: None,
        }
    }
    fn texts(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Line(l) if l.stream == LogStream::Meta => Some(l.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_first_snapshot_starts_running_pods_quietly_oldest_first() {
        let mut s = PodSet::new("web", 25);
        let out = s.on_event(snap(vec![
            row("b", 2, "Running", 0),
            row("a", 1, "Running", 0),
            row("p", 3, "Pending", 0),
        ]));
        assert_eq!(out, [start("a"), start("b")]);
    }

    #[test]
    fn finished_pods_are_read() {
        let mut s = PodSet::new("web", 25);
        let out = s.on_event(snap(vec![
            row("j1", 1, "Succeeded", 0),
            row("j2", 2, "Failed", 0),
        ]));
        assert_eq!(out, [start("j1"), start("j2")]);
    }

    #[test]
    fn a_later_pod_joins_with_a_marker_and_a_deleted_one_leaves() {
        let mut s = PodSet::new("web", 25);
        s.on_event(snap(vec![row("a", 1, "Running", 0)]));
        assert!(s
            .on_event(WatchEvent::Added {
                row: row("b", 2, "Pending", 0)
            })
            .is_empty());
        let out = s.on_event(WatchEvent::Modified {
            row: row("b", 2, "Running", 0),
        });
        assert_eq!(texts(&out), ["▸ b started"]);
        assert!(out.contains(&start("b")));
        let out = s.on_event(WatchEvent::Deleted {
            namespace: Some("apps".into()),
            name: "a".into(),
        });
        assert!(out.contains(&Step::Stop { pod: "a".into() }));
        assert_eq!(texts(&out), ["◂ a gone"]);
    }

    #[test]
    fn marker_lines_name_their_pod() {
        let mut s = PodSet::new("web", 25);
        s.on_event(snap(vec![]));
        let out = s.on_event(WatchEvent::Added {
            row: row("b", 2, "Running", 0),
        });
        let Step::Line(l) = &out[0] else {
            panic!("{out:?}")
        };
        assert_eq!(l.pod.as_deref(), Some("b"));
    }

    #[test]
    fn past_the_cap_pods_wait_with_one_notice_and_fill_freed_slots() {
        let mut s = PodSet::new("web", 2);
        let out = s.on_event(snap(vec![
            row("a", 1, "Running", 0),
            row("b", 2, "Running", 0),
            row("c", 3, "Running", 0),
            row("d", 4, "Running", 0),
        ]));
        assert_eq!(texts(&out), ["showing 2 of 4 pods"]);
        assert!(out.contains(&start("a")) && out.contains(&start("b")));
        let out = s.on_event(WatchEvent::Added {
            row: row("e", 5, "Running", 0),
        });
        assert!(out.is_empty(), "the notice is said once: {out:?}");
        let out = s.on_event(WatchEvent::Deleted {
            namespace: None,
            name: "a".into(),
        });
        assert_eq!(texts(&out), ["◂ a gone", "▸ c started"]);
        assert!(out.contains(&start("c")));
    }

    #[test]
    fn a_pod_without_the_container_is_said_once_and_skipped() {
        let mut s = PodSet::new("api", 25);
        let out = s.on_event(snap(vec![row("a", 1, "Running", 0)]));
        assert_eq!(texts(&out), ["a has no container api"]);
        assert!(s
            .on_event(WatchEvent::Modified {
                row: row("a", 1, "Running", 1)
            })
            .is_empty());
    }

    #[test]
    fn a_pod_that_ended_stays_out_until_it_restarts() {
        let mut s = PodSet::new("web", 25);
        s.on_event(snap(vec![row("a", 1, "Running", 0)]));
        assert!(s.on_ended("a", 100).is_empty());
        assert!(s
            .on_event(WatchEvent::Modified {
                row: row("a", 1, "Running", 0)
            })
            .is_empty());
        let out = s.on_event(WatchEvent::Modified {
            row: row("a", 1, "Running", 1),
        });
        assert!(
            out.contains(&Step::Start {
                pod: "a".into(),
                since: Some(100)
            }),
            "{out:?}"
        );
    }

    #[test]
    fn a_pod_that_restarts_rejoins_from_when_it_ended_even_if_the_restart_was_seen_first() {
        let mut s = PodSet::new("web", 25);
        s.on_event(snap(vec![row("a", 1, "Running", 0)]));
        // The kubelet's status update can arrive before the log stream ends.
        assert!(s
            .on_event(WatchEvent::Modified {
                row: row("a", 1, "Running", 1)
            })
            .is_empty());
        let out = s.on_ended("a", 100);
        assert!(
            out.contains(&Step::Start {
                pod: "a".into(),
                since: Some(100)
            }),
            "{out:?}"
        );
    }

    #[test]
    fn an_ended_slot_goes_to_a_waiting_pod() {
        let mut s = PodSet::new("web", 1);
        s.on_event(snap(vec![
            row("a", 1, "Succeeded", 0),
            row("b", 2, "Succeeded", 0),
        ]));
        let out = s.on_ended("a", 100);
        assert!(out.contains(&start("b")), "{out:?}");
    }

    #[test]
    fn a_resync_snapshot_reopens_nothing_and_drops_the_missing() {
        let mut s = PodSet::new("web", 25);
        s.on_event(snap(vec![
            row("a", 1, "Running", 0),
            row("b", 2, "Running", 0),
        ]));
        assert!(s.on_event(WatchEvent::Resync).is_empty());
        let out = s.on_event(snap(vec![row("a", 1, "Running", 0)]));
        assert_eq!(out[0], Step::Stop { pod: "b".into() });
        assert_eq!(texts(&out), ["◂ b gone"]);
        assert!(!out.iter().any(|x| matches!(x, Step::Start { .. })));
    }

    #[test]
    fn is_empty_once_every_pod_is_gone() {
        let mut s = PodSet::new("web", 25);
        s.on_event(snap(vec![row("a", 1, "Running", 0)]));
        assert!(!s.is_empty());
        s.on_event(WatchEvent::Deleted {
            namespace: None,
            name: "a".into(),
        });
        assert!(s.is_empty());
    }
}
