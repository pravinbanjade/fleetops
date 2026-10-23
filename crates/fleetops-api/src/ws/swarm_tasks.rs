//! The shared task feed of a swarm (swarm spec §7). Swarm emits no task
//! events, and tasks that never get a container (rejected, pending) emit
//! nothing at all, so while anyone watches, the serving manager's task list
//! is read every `ApiConfig::swarm_task_poll` and only changes are pushed.
//! The browser never polls. One feed per swarm, stopped when nobody listens.

use fleetops_core::env::Environment;
use fleetops_core::swarm::TaskSummary;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::broadcast;

/// What a subscriber receives.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FeedMsg {
    /// Every task; first, and again after a pause.
    Snapshot { tasks: Vec<TaskSummary> },
    /// What changed since the last message.
    Changes {
        upsert: Vec<TaskSummary>,
        remove: Vec<String>,
    },
    /// The tasks cannot be read right now; a snapshot follows recovery.
    Paused { reason: String },
}

struct Feed {
    tx: broadcast::Sender<FeedMsg>,
    /// The last tasks read, `None` while paused or before the first read.
    current: Mutex<Option<Vec<TaskSummary>>>,
}

/// Feeds by environment instance.
#[derive(Default)]
pub struct SwarmTasks {
    feeds: Mutex<HashMap<super::hub::EnvKey, Arc<Feed>>>,
}

const CAPACITY: usize = 64;

impl SwarmTasks {
    /// The current snapshot, when there is one, and the messages after it.
    pub fn subscribe(
        &self,
        env: Arc<dyn Environment>,
        poll: Duration,
    ) -> (Option<FeedMsg>, broadcast::Receiver<FeedMsg>) {
        let key = super::hub::key(&env);
        let mut feeds = self.feeds.lock().unwrap();
        feeds.retain(|_, f| f.tx.receiver_count() > 0);
        if let Some(feed) = feeds.get(&key) {
            // Under the lock the poller updates `current` with, so nothing
            // is missed or seen twice between the snapshot and the receiver.
            let current = feed.current.lock().unwrap();
            let rx = feed.tx.subscribe();
            return (current.clone().map(|tasks| FeedMsg::Snapshot { tasks }), rx);
        }
        let (tx, rx) = broadcast::channel(CAPACITY);
        let feed = Arc::new(Feed {
            tx,
            current: Mutex::new(None),
        });
        feeds.insert(key, feed.clone());
        // Spawned with its first receiver already in place.
        tokio::spawn(run(env, feed, poll));
        (None, rx)
    }
}

/// `next` against `prev`: upserts by id and version, removals by id.
fn diff(prev: &[TaskSummary], next: &[TaskSummary]) -> Option<FeedMsg> {
    let before: HashMap<&str, u64> = prev.iter().map(|t| (t.id.as_str(), t.version)).collect();
    let upsert: Vec<TaskSummary> = next
        .iter()
        .filter(|t| before.get(t.id.as_str()) != Some(&t.version))
        .cloned()
        .collect();
    let now: std::collections::HashSet<&str> = next.iter().map(|t| t.id.as_str()).collect();
    let remove: Vec<String> = prev
        .iter()
        .filter(|t| !now.contains(t.id.as_str()))
        .map(|t| t.id.clone())
        .collect();
    (!upsert.is_empty() || !remove.is_empty()).then_some(FeedMsg::Changes { upsert, remove })
}

async fn run(env: Arc<dyn Environment>, feed: Arc<Feed>, poll: Duration) {
    let mut tick = tokio::time::interval(poll);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut paused = false;
    loop {
        tick.tick().await;
        if feed.tx.receiver_count() == 0 {
            return;
        }
        // Whichever manager serves now: failover needs nothing more.
        let read = match env.swarm() {
            Some(ops) => tokio::time::timeout(poll.max(Duration::from_secs(5)), ops.tasks())
                .await
                .map_err(|_| "the manager did not answer in time".to_string())
                .and_then(|r| r.map_err(|e| e.to_string())),
            None => Err("no manager agent is connected".to_string()),
        };
        let mut current = feed.current.lock().unwrap();
        match read {
            Ok(tasks) => {
                let msg = match current.as_deref() {
                    Some(prev) if !paused => diff(prev, &tasks),
                    _ => Some(FeedMsg::Snapshot {
                        tasks: tasks.clone(),
                    }),
                };
                paused = false;
                *current = Some(tasks);
                if let Some(msg) = msg {
                    let _ = feed.tx.send(msg);
                }
            }
            Err(reason) => {
                if !paused {
                    paused = true;
                    *current = None;
                    let _ = feed.tx.send(FeedMsg::Paused { reason });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, version: u64) -> TaskSummary {
        TaskSummary {
            id: id.into(),
            service_id: "s".into(),
            service_name: "s".into(),
            slot: None,
            node_id: None,
            desired_state: "running".into(),
            state: "running".into(),
            message: String::new(),
            error: None,
            container_id: None,
            updated_at: 0,
            version,
        }
    }

    #[test]
    fn diff_reports_new_changed_and_gone_tasks_only() {
        let prev = vec![task("a", 1), task("b", 1)];
        assert!(diff(&prev, &prev).is_none());
        match diff(&prev, &[task("a", 2), task("c", 1)]).unwrap() {
            FeedMsg::Changes { upsert, remove } => {
                let ids: Vec<_> = upsert.iter().map(|t| t.id.as_str()).collect();
                assert_eq!(ids, ["a", "c"]);
                assert_eq!(remove, ["b"]);
            }
            other => panic!("{other:?}"),
        }
    }
}
