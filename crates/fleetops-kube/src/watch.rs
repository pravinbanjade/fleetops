//! Live lists (spec §2.3): kube-rs `watcher` events folded into the rows
//! FleetOps shows, with the failure policy on top.

use crate::{errors, summaries};
use fleetops_core::kube::{KubeKind, KubeRow, WatchEvent};
use fleetops_core::streams::BoxStream;
use fleetops_core::Error;
use futures_util::{Stream, StreamExt};
use kube::api::DynamicObject;
use kube::runtime::watcher::{self, Event};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

/// How one object becomes a row: a curated kind's summary, or a generic one.
pub type RowFn = Arc<dyn Fn(&DynamicObject) -> KubeRow + Send + Sync>;

pub fn curated(kind: KubeKind) -> RowFn {
    Arc::new(move |o| summaries::row(kind, o))
}

pub fn generic(k: fleetops_core::kube::ApiKind) -> RowFn {
    Arc::new(move |o| summaries::generic_row(&k, o))
}

pub const MAX_FAILURES: u32 = 5;
pub const BACKOFF_START: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

type Key = (Option<String>, String);

fn key(r: &KubeRow) -> Key {
    (r.namespace.clone(), r.name.clone())
}

/// The rows of one watched list, kept to tell `Added` from `Modified` and
/// to build snapshots.
pub struct Fold {
    row: RowFn,
    rows: BTreeMap<Key, KubeRow>,
    /// Rows of a list in progress (between `Init` and `InitDone`).
    listing: Option<BTreeMap<Key, KubeRow>>,
    /// A snapshot has gone out, so a new list is a resync.
    synced: bool,
}

impl Fold {
    pub fn new(row: RowFn) -> Self {
        Self {
            row,
            rows: BTreeMap::new(),
            listing: None,
            synced: false,
        }
    }

    pub fn step(&mut self, ev: Event<DynamicObject>) -> Vec<WatchEvent> {
        match ev {
            Event::Init => {
                self.listing = Some(BTreeMap::new());
                if self.synced {
                    vec![WatchEvent::Resync]
                } else {
                    vec![]
                }
            }
            Event::InitApply(o) => {
                let r = (self.row)(&o);
                if let Some(l) = self.listing.as_mut() {
                    l.insert(key(&r), r);
                }
                vec![]
            }
            Event::InitDone => {
                self.rows = self.listing.take().unwrap_or_default();
                self.synced = true;
                vec![WatchEvent::Snapshot {
                    rows: self.rows.values().cloned().collect(),
                }]
            }
            Event::Apply(o) => {
                let r = (self.row)(&o);
                match self.rows.insert(key(&r), r.clone()) {
                    Some(old) if old == r => vec![],
                    Some(_) => vec![WatchEvent::Modified { row: r }],
                    None => vec![WatchEvent::Added { row: r }],
                }
            }
            Event::Delete(o) => {
                let r = (self.row)(&o);
                self.rows.remove(&key(&r));
                vec![WatchEvent::Deleted {
                    namespace: r.namespace,
                    name: r.name,
                }]
            }
        }
    }
}

/// The apiserver status code behind a watcher error, when there is one.
fn api_code(e: &watcher::Error) -> Option<u16> {
    match e {
        watcher::Error::InitialListFailed(kube::Error::Api(s))
        | watcher::Error::WatchStartFailed(kube::Error::Api(s))
        | watcher::Error::WatchFailed(kube::Error::Api(s)) => Some(s.code),
        watcher::Error::WatchError(s) => Some(s.code),
        _ => None,
    }
}

fn to_error(e: watcher::Error) -> Error {
    match e {
        watcher::Error::InitialListFailed(k)
        | watcher::Error::WatchStartFailed(k)
        | watcher::Error::WatchFailed(k) => errors::map(k),
        other => Error::Upstream {
            status: 502,
            message: other.to_string(),
        },
    }
}

struct State<S> {
    upstream: S,
    fold: Fold,
    queue: VecDeque<WatchEvent>,
    failures: u32,
    backoff: Duration,
    done: bool,
}

/// `upstream` (a kube-rs watcher) as FleetOps watch events. Transient
/// errors back off and resume; 401/403, or `MAX_FAILURES` in a row, end
/// the stream with the error.
pub fn stream<S>(upstream: S, row: RowFn, backoff_start: Duration) -> BoxStream<WatchEvent>
where
    S: Stream<Item = Result<Event<DynamicObject>, watcher::Error>> + Send + Unpin + 'static,
{
    let st = State {
        upstream,
        fold: Fold::new(row),
        queue: VecDeque::new(),
        failures: 0,
        backoff: backoff_start,
        done: false,
    };
    Box::pin(futures_util::stream::unfold(st, move |mut st| async move {
        loop {
            if let Some(ev) = st.queue.pop_front() {
                return Some((Ok(ev), st));
            }
            if st.done {
                return None;
            }
            match st.upstream.next().await {
                None => return None,
                Some(Ok(ev)) => {
                    st.failures = 0;
                    st.backoff = backoff_start;
                    let out = st.fold.step(ev);
                    st.queue.extend(out);
                }
                Some(Err(e)) => {
                    st.failures += 1;
                    let fatal = matches!(api_code(&e), Some(401 | 403));
                    if fatal || st.failures >= MAX_FAILURES {
                        st.done = true;
                        return Some((Err(to_error(e)), st));
                    }
                    tracing::debug!(error = %e, failures = st.failures, "watch failed; retrying");
                    tokio::time::sleep(st.backoff).await;
                    st.backoff = (st.backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }))
}

/// A live list through `api`, each object made a row by `row`.
pub fn watch(api: kube::Api<DynamicObject>, row: RowFn) -> BoxStream<WatchEvent> {
    watch_with(api, row, watcher::Config::default())
}

/// `watch` with the apiserver narrowing the objects, e.g. by field selector.
pub fn watch_with(
    api: kube::Api<DynamicObject>,
    row: RowFn,
    config: watcher::Config,
) -> BoxStream<WatchEvent> {
    stream(watcher::watcher(api, config).boxed(), row, BACKOFF_START)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use serde_json::json;

    fn pod(name: &str, phase: &str) -> DynamicObject {
        serde_json::from_value(json!({"apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": name, "namespace": "apps"},
            "status": {"phase": phase}}))
        .unwrap()
    }

    fn names(ev: &WatchEvent) -> Vec<String> {
        match ev {
            WatchEvent::Snapshot { rows } => rows.iter().map(|r| r.name.clone()).collect(),
            _ => vec![],
        }
    }

    #[test]
    fn the_initial_list_becomes_one_snapshot() {
        let mut f = Fold::new(curated(KubeKind::Pods));
        assert!(f.step(Event::Init).is_empty());
        assert!(f.step(Event::InitApply(pod("b", "Running"))).is_empty());
        assert!(f.step(Event::InitApply(pod("a", "Running"))).is_empty());
        let out = f.step(Event::InitDone);
        assert_eq!(out.len(), 1);
        assert_eq!(names(&out[0]), ["a", "b"]);
    }

    #[test]
    fn later_changes_are_added_modified_or_deleted() {
        let mut f = Fold::new(curated(KubeKind::Pods));
        f.step(Event::Init);
        f.step(Event::InitApply(pod("a", "Running")));
        f.step(Event::InitDone);
        assert!(
            matches!(&f.step(Event::Apply(pod("b", "Pending")))[..], [WatchEvent::Added { row }] if row.name == "b")
        );
        assert!(
            matches!(&f.step(Event::Apply(pod("b", "Running")))[..], [WatchEvent::Modified { row }] if row.status == "Running")
        );
        // The same row again is not news.
        assert!(f.step(Event::Apply(pod("b", "Running"))).is_empty());
        assert!(matches!(&f.step(Event::Delete(pod("a", "Running")))[..],
            [WatchEvent::Deleted { namespace: Some(ns), name }] if ns == "apps" && name == "a"));
    }

    #[test]
    fn a_relist_is_a_resync_then_a_fresh_snapshot() {
        let mut f = Fold::new(curated(KubeKind::Pods));
        f.step(Event::Init);
        f.step(Event::InitApply(pod("a", "Running")));
        f.step(Event::InitDone);
        assert_eq!(f.step(Event::Init), [WatchEvent::Resync]);
        f.step(Event::InitApply(pod("c", "Running")));
        assert_eq!(names(&f.step(Event::InitDone)[0]), ["c"]);
    }

    fn status(code: u16) -> watcher::Error {
        let s: kube::core::Status = serde_json::from_value(json!({
            "status": "Failure", "code": code, "message": "pods is forbidden", "reason": "Forbidden"}))
        .unwrap();
        watcher::Error::InitialListFailed(kube::Error::Api(Box::new(s)))
    }

    #[tokio::test]
    async fn a_forbidden_watch_ends_at_once_with_the_reason() {
        let up = futures_util::stream::iter(vec![Err(status(403))]);
        let mut s = stream(up, curated(KubeKind::Pods), Duration::from_millis(1));
        match s.next().await {
            Some(Err(Error::Forbidden(m))) => assert!(m.contains("forbidden")),
            other => panic!("{other:?}"),
        }
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn transient_failures_are_retried_until_the_fifth() {
        let errs: Vec<Result<Event<DynamicObject>, watcher::Error>> = (0..MAX_FAILURES)
            .map(|_| Err(watcher::Error::NoResourceVersion))
            .collect();
        let mut s = stream(
            futures_util::stream::iter(errs),
            curated(KubeKind::Pods),
            Duration::from_millis(1),
        );
        assert!(matches!(s.next().await, Some(Err(Error::Upstream { .. }))));
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn a_success_resets_the_failure_count() {
        let mut items: Vec<Result<Event<DynamicObject>, watcher::Error>> = vec![];
        for _ in 0..MAX_FAILURES - 1 {
            items.push(Err(watcher::Error::NoResourceVersion));
        }
        items.push(Ok(Event::Init));
        items.push(Ok(Event::InitDone));
        for _ in 0..MAX_FAILURES - 1 {
            items.push(Err(watcher::Error::NoResourceVersion));
        }
        let s = stream(
            futures_util::stream::iter(items),
            curated(KubeKind::Pods),
            Duration::from_millis(1),
        );
        let out: Vec<_> = s.collect().await;
        assert_eq!(
            out.len(),
            1,
            "only the snapshot; the upstream ended before a 5th failure"
        );
        assert!(matches!(out[0], Ok(WatchEvent::Snapshot { .. })));
    }
}
