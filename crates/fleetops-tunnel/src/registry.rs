//! Live agent sessions by environment id. The connect handler adds and
//! removes them; agent environments read them on every request.

use crate::{
    control::{ByeReason, HostInfo},
    Session,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub agent_version: String,
    pub host: HostInfo,
    /// Unix seconds.
    pub connected_at: i64,
}

#[derive(Clone)]
pub struct Live {
    /// Distinguishes successive sessions of one environment.
    pub id: u64,
    pub session: Session,
    pub info: AgentInfo,
}

struct Entry {
    live: Live,
    bye: mpsc::Sender<ByeReason>,
}

pub struct Registry {
    live: Mutex<HashMap<i64, Entry>>,
    offline: Mutex<HashMap<i64, String>>,
    next_id: AtomicU64,
    changes: watch::Sender<u64>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            live: Mutex::default(),
            offline: Mutex::default(),
            next_id: AtomicU64::new(1),
            changes: watch::channel(0).0,
        }
    }
}

impl Registry {
    fn changed(&self) {
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Makes `session` the environment's live session. A previous one is
    /// told `Replaced`; its own cleanup then finds a different id and
    /// leaves this one alone.
    pub fn insert(
        &self,
        env_id: i64,
        session: Session,
        info: AgentInfo,
        bye: mpsc::Sender<ByeReason>,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let old = self.live.lock().unwrap().insert(
            env_id,
            Entry {
                live: Live { id, session, info },
                bye,
            },
        );
        if let Some(old) = old {
            let _ = old.bye.try_send(ByeReason::Replaced);
        }
        self.offline.lock().unwrap().remove(&env_id);
        self.changed();
        id
    }

    /// Removes session `id` if it is still the live one, remembering why.
    pub fn remove(&self, env_id: i64, id: u64, reason: &str) -> bool {
        let mut live = self.live.lock().unwrap();
        if live.get(&env_id).is_some_and(|e| e.live.id == id) {
            live.remove(&env_id);
            drop(live);
            self.set_offline_reason(env_id, reason);
            return true;
        }
        false
    }

    pub fn revoke(&self, env_id: i64) {
        if let Some(e) = self.live.lock().unwrap().get(&env_id) {
            let _ = e.bye.try_send(ByeReason::Revoked);
        }
    }

    pub fn set_offline_reason(&self, env_id: i64, reason: &str) {
        self.offline
            .lock()
            .unwrap()
            .insert(env_id, reason.to_string());
        self.changed();
    }

    pub fn get(&self, env_id: i64) -> Option<Live> {
        self.live
            .lock()
            .unwrap()
            .get(&env_id)
            .map(|e| e.live.clone())
    }

    pub fn offline_reason(&self, env_id: i64) -> Option<String> {
        self.offline.lock().unwrap().get(&env_id).cloned()
    }

    pub fn connected(&self) -> usize {
        self.live.lock().unwrap().len()
    }

    /// Resolves at the next insert, remove or reason change, for any environment.
    pub async fn wait_change(&self) {
        let mut rx = self.changes.subscribe();
        let _ = rx.changed().await;
    }
}
