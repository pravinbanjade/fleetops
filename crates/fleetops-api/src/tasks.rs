//! Background tasks (image pull and build): started by a REST call that
//! returns `202 {task_id}` at once, followed over the websocket `task` topic
//! or polled with `GET /tasks/{id}`.

use fleetops_core::streams::BoxStream;
use fleetops_core::{now_unix, Error, Result};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::broadcast;

/// Progress messages kept per task for late subscribers.
pub const TASK_LOG_MAX: usize = 2000;
/// How long a finished task stays queryable.
pub const TASK_RETENTION: Duration = Duration::from_secs(600);
/// Running tasks allowed at once across the server.
pub const MAX_RUNNING_TASKS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskInfo {
    pub id: String,
    /// `image.pull` or `image.build`.
    pub kind: String,
    pub env: String,
    /// What the task works on, e.g. the image reference.
    pub target: String,
    pub status: TaskStatus,
    pub error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum TaskEvent {
    Progress(Value),
    Done(TaskInfo),
}

pub struct Task {
    pub owner: i64,
    state: Mutex<TaskState>,
    tx: broadcast::Sender<TaskEvent>,
}

struct TaskState {
    info: TaskInfo,
    log: VecDeque<Value>,
}

/// What a subscriber gets: everything so far, then live events.
pub struct TaskFeed {
    pub backlog: Vec<Value>,
    /// Set when the task had already finished.
    pub done: Option<TaskInfo>,
    pub live: broadcast::Receiver<TaskEvent>,
}

impl Task {
    pub fn info(&self) -> TaskInfo {
        self.state.lock().unwrap().info.clone()
    }

    /// Backlog and live receiver taken under one lock, so nothing is missed
    /// or seen twice.
    pub fn feed(&self) -> TaskFeed {
        let st = self.state.lock().unwrap();
        TaskFeed {
            backlog: st.log.iter().cloned().collect(),
            done: (st.info.status != TaskStatus::Running).then(|| st.info.clone()),
            live: self.tx.subscribe(),
        }
    }

    fn progress(&self, v: Value) {
        let mut st = self.state.lock().unwrap();
        if st.log.len() >= TASK_LOG_MAX {
            st.log.pop_front();
        }
        st.log.push_back(v.clone());
        let _ = self.tx.send(TaskEvent::Progress(v));
    }

    fn finish(&self, outcome: &Result<()>) -> TaskInfo {
        let mut st = self.state.lock().unwrap();
        st.info.status = if outcome.is_ok() {
            TaskStatus::Succeeded
        } else {
            TaskStatus::Failed
        };
        st.info.error = outcome.as_ref().err().map(|e| e.to_string());
        st.info.finished_at = Some(now_unix());
        let info = st.info.clone();
        let _ = self.tx.send(TaskEvent::Done(info.clone()));
        info
    }
}

#[derive(Default)]
pub struct TaskManager {
    tasks: Mutex<HashMap<String, Arc<Task>>>,
}

pub struct NewTask {
    pub kind: &'static str,
    pub env: String,
    pub target: String,
    pub owner: i64,
}

impl TaskManager {
    /// Runs `stream` to completion in the background. `on_done` runs with
    /// the outcome afterwards (for the audit log). A stream error, or a
    /// progress message carrying Docker's `error` field, fails the task.
    pub fn spawn<F, Fut>(
        &self,
        new: NewTask,
        mut stream: BoxStream<Value>,
        on_done: F,
    ) -> Result<Arc<Task>>
    where
        F: FnOnce(TaskInfo) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let mut tasks = self.tasks.lock().unwrap();
        let now = now_unix();
        tasks.retain(|_, t| {
            t.info()
                .finished_at
                .is_none_or(|f| now - f < TASK_RETENTION.as_secs() as i64)
        });
        let running = tasks
            .values()
            .filter(|t| t.info().status == TaskStatus::Running)
            .count();
        if running >= MAX_RUNNING_TASKS {
            return Err(Error::Conflict(format!(
                "{MAX_RUNNING_TASKS} tasks are already running; try again when one finishes"
            )));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let task = Arc::new(Task {
            owner: new.owner,
            state: Mutex::new(TaskState {
                info: TaskInfo {
                    id: id.clone(),
                    kind: new.kind.to_string(),
                    env: new.env,
                    target: new.target,
                    status: TaskStatus::Running,
                    error: None,
                    created_at: now,
                    finished_at: None,
                },
                log: VecDeque::new(),
            }),
            tx: broadcast::channel(256).0,
        });
        tasks.insert(id, task.clone());
        let t = task.clone();
        tokio::spawn(async move {
            let mut outcome = Ok(());
            while let Some(item) = stream.next().await {
                match item {
                    Ok(v) => {
                        if let Some(msg) =
                            v["error"].as_str().or(v["errorDetail"]["message"].as_str())
                        {
                            outcome = Err(Error::Upstream {
                                status: 500,
                                message: msg.to_string(),
                            });
                            t.progress(v);
                            break;
                        }
                        t.progress(v);
                    }
                    Err(e) => {
                        outcome = Err(e);
                        break;
                    }
                }
            }
            let info = t.finish(&outcome);
            on_done(info).await;
        });
        Ok(task)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Task>> {
        self.tasks.lock().unwrap().get(id).cloned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use serde_json::json;

    fn new_task() -> NewTask {
        NewTask {
            kind: "image.pull",
            env: "local".into(),
            target: "alpine:3.20".into(),
            owner: 1,
        }
    }

    fn items(v: Vec<Result<Value>>) -> BoxStream<Value> {
        Box::pin(stream::iter(v))
    }

    async fn wait_done(t: &Task) -> TaskInfo {
        for _ in 0..100 {
            let i = t.info();
            if i.status != TaskStatus::Running {
                return i;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("task did not finish");
    }

    #[tokio::test]
    async fn successful_stream_succeeds_and_keeps_progress() {
        let m = TaskManager::default();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let t = m
            .spawn(
                new_task(),
                items(vec![Ok(json!({"status": "a"})), Ok(json!({"status": "b"}))]),
                |info| async move {
                    let _ = done_tx.send(info);
                },
            )
            .unwrap();
        let info = wait_done(&t).await;
        assert_eq!(info.status, TaskStatus::Succeeded);
        assert!(info.finished_at.is_some());
        assert_eq!(done_rx.await.unwrap().status, TaskStatus::Succeeded);
        let feed = t.feed();
        assert_eq!(
            feed.backlog,
            [json!({"status": "a"}), json!({"status": "b"})]
        );
        assert_eq!(feed.done.unwrap().status, TaskStatus::Succeeded);
        assert!(m.get(&info.id).is_some());
    }

    #[tokio::test]
    async fn stream_error_or_error_message_fails_the_task() {
        let m = TaskManager::default();
        let t = m
            .spawn(
                new_task(),
                items(vec![Err(Error::Upstream {
                    status: 404,
                    message: "manifest unknown".into(),
                })]),
                |_| async {},
            )
            .unwrap();
        let info = wait_done(&t).await;
        assert_eq!(info.status, TaskStatus::Failed);
        assert!(info.error.unwrap().contains("manifest unknown"));
        let t = m
            .spawn(
                new_task(),
                items(vec![
                    Ok(json!({"stream": "Step 1"})),
                    Ok(json!({"errorDetail": {"message": "returned a non-zero code: 7"}})),
                    Ok(json!({"stream": "never read"})),
                ]),
                |_| async {},
            )
            .unwrap();
        let info = wait_done(&t).await;
        assert_eq!(info.status, TaskStatus::Failed);
        assert!(info.error.unwrap().contains("non-zero code: 7"));
        assert_eq!(t.feed().backlog.len(), 2);
    }

    #[tokio::test]
    async fn backlog_is_capped() {
        let m = TaskManager::default();
        let many = (0..TASK_LOG_MAX + 5).map(|i| Ok(json!(i))).collect();
        let t = m.spawn(new_task(), items(many), |_| async {}).unwrap();
        wait_done(&t).await;
        let backlog = t.feed().backlog;
        assert_eq!(backlog.len(), TASK_LOG_MAX);
        assert_eq!(backlog[0], json!(5));
    }

    #[tokio::test]
    async fn running_tasks_are_limited() {
        let m = TaskManager::default();
        for _ in 0..MAX_RUNNING_TASKS {
            m.spawn(new_task(), Box::pin(stream::pending()), |_| async {})
                .unwrap();
        }
        assert!(matches!(
            m.spawn(new_task(), Box::pin(stream::pending()), |_| async {}),
            Err(Error::Conflict(_))
        ));
    }
}
