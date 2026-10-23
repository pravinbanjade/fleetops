//! Live agent sessions by environment id and, for Swarm, node id. The
//! connect handler adds and removes them; agent environments read them on
//! every request.

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
    /// Which key the session authenticated with, so a finished rotation
    /// can drop the sessions still on the old one. Empty for join-token
    /// agents.
    key_tag: String,
    bye: mpsc::Sender<ByeReason>,
}

/// An environment and a Swarm node id; Docker and Kube agents use `""`.
type Key = (i64, String);

fn key(env_id: i64, node: &str) -> Key {
    (env_id, node.to_string())
}

pub struct Registry {
    live: Mutex<HashMap<Key, Entry>>,
    offline: Mutex<HashMap<Key, String>>,
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
        self.insert_node(env_id, "", "", session, info, bye)
    }

    /// Makes `session` the live session of Swarm node `node`. Only that
    /// node's previous session is replaced.
    pub fn insert_node(
        &self,
        env_id: i64,
        node: &str,
        key_tag: &str,
        session: Session,
        info: AgentInfo,
        bye: mpsc::Sender<ByeReason>,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let old = self.live.lock().unwrap().insert(
            key(env_id, node),
            Entry {
                live: Live { id, session, info },
                key_tag: key_tag.to_string(),
                bye,
            },
        );
        if let Some(old) = old {
            let _ = old.bye.try_send(ByeReason::Replaced);
        }
        self.offline.lock().unwrap().remove(&key(env_id, node));
        self.changed();
        id
    }

    /// Removes session `id` if it is still the live one, remembering why.
    pub fn remove(&self, env_id: i64, id: u64, reason: &str) -> bool {
        self.remove_node(env_id, "", id, reason)
    }

    pub fn remove_node(&self, env_id: i64, node: &str, id: u64, reason: &str) -> bool {
        let k = key(env_id, node);
        let mut live = self.live.lock().unwrap();
        if live.get(&k).is_some_and(|e| e.live.id == id) {
            live.remove(&k);
            drop(live);
            self.set_node_offline_reason(env_id, node, reason);
            return true;
        }
        false
    }

    /// Tells every session of the environment, on any node, `Revoked`.
    pub fn revoke(&self, env_id: i64) {
        for (k, e) in self.live.lock().unwrap().iter() {
            if k.0 == env_id {
                let _ = e.bye.try_send(ByeReason::Revoked);
            }
        }
    }

    /// Revokes only the sessions that authenticated with `key_tag`.
    pub fn revoke_tag(&self, env_id: i64, key_tag: &str) {
        for (k, e) in self.live.lock().unwrap().iter() {
            if k.0 == env_id && e.key_tag == key_tag {
                let _ = e.bye.try_send(ByeReason::Revoked);
            }
        }
    }

    pub fn set_offline_reason(&self, env_id: i64, reason: &str) {
        self.set_node_offline_reason(env_id, "", reason)
    }

    pub fn set_node_offline_reason(&self, env_id: i64, node: &str, reason: &str) {
        self.offline
            .lock()
            .unwrap()
            .insert(key(env_id, node), reason.to_string());
        self.changed();
    }

    pub fn get(&self, env_id: i64) -> Option<Live> {
        self.get_node(env_id, "")
    }

    pub fn get_node(&self, env_id: i64, node: &str) -> Option<Live> {
        self.live
            .lock()
            .unwrap()
            .get(&key(env_id, node))
            .map(|e| e.live.clone())
    }

    /// The environment's live Swarm node sessions, by node id.
    pub fn nodes(&self, env_id: i64) -> Vec<(String, Live)> {
        let mut v: Vec<(String, Live)> = self
            .live
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.0 == env_id && !k.1.is_empty())
            .map(|(k, e)| (k.1.clone(), e.live.clone()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    pub fn offline_reason(&self, env_id: i64) -> Option<String> {
        self.node_offline_reason(env_id, "")
    }

    pub fn node_offline_reason(&self, env_id: i64, node: &str) -> Option<String> {
        self.offline
            .lock()
            .unwrap()
            .get(&key(env_id, node))
            .cloned()
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
