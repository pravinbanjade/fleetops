//! One bounded outbound queue per subscription (spec §5.2 backpressure).
//! Producers never wait on the socket, except exec, which waits for room
//! so that Docker's output is read only as fast as the client takes it.

use super::protocol::{gap, Out};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

pub const QUEUE_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// events, stats, task: the newest state matters most.
    DropOldest,
    /// logs: drop new lines and tell the client how many with `gap`.
    Gap,
    /// exec: the producer waits for room.
    Block,
}

struct Inner {
    items: VecDeque<Out>,
    /// Log lines dropped since the last `gap` went out.
    dropped: u64,
    closed: bool,
    /// The producer is done; the queue goes away once drained.
    finished: bool,
}

pub struct SubQueue {
    sub: u32,
    policy: Policy,
    capacity: usize,
    inner: Mutex<Inner>,
    /// Wakes the connection's writer.
    ready: Arc<Notify>,
    /// Wakes a blocked exec producer.
    space: Notify,
}

impl SubQueue {
    pub fn new(sub: u32, policy: Policy, capacity: usize, ready: Arc<Notify>) -> Self {
        Self {
            sub,
            policy,
            capacity,
            inner: Mutex::new(Inner {
                items: VecDeque::new(),
                dropped: 0,
                closed: false,
                finished: false,
            }),
            ready,
            space: Notify::new(),
        }
    }

    pub fn sub(&self) -> u32 {
        self.sub
    }

    /// Queues `msg`, which stands for `weight` log lines (1 otherwise).
    /// Returns false once the subscription is closed, so producers stop.
    pub fn push(&self, msg: Out, weight: u64) -> bool {
        let mut q = self.inner.lock().unwrap();
        if q.closed {
            return false;
        }
        match self.policy {
            Policy::DropOldest | Policy::Block => {
                if q.items.len() >= self.capacity {
                    q.items.pop_front();
                }
                q.items.push_back(msg);
            }
            Policy::Gap => {
                // Room for this message, plus a `gap` ahead of it if lines
                // were lost since the last one went out.
                let need = if q.dropped > 0 { 2 } else { 1 };
                if q.items.len() + need > self.capacity {
                    q.dropped += weight;
                    return true;
                }
                if q.dropped > 0 {
                    let n = std::mem::take(&mut q.dropped);
                    q.items.push_back(gap(self.sub, n));
                }
                q.items.push_back(msg);
            }
        }
        drop(q);
        self.ready.notify_one();
        true
    }

    /// Waits until there is room, then queues `msg`. Returns false once the
    /// subscription is closed.
    pub async fn push_wait(&self, msg: Out) -> bool {
        loop {
            let space = self.space.notified();
            {
                let mut q = self.inner.lock().unwrap();
                if q.closed {
                    return false;
                }
                if q.items.len() < self.capacity {
                    q.items.push_back(msg);
                    drop(q);
                    self.ready.notify_one();
                    return true;
                }
            }
            space.await;
        }
    }

    /// Queues the subscription's last message (`end` or `error`) regardless
    /// of capacity, and marks the producer finished.
    pub fn finish(&self, msg: Out) {
        let mut q = self.inner.lock().unwrap();
        if q.closed {
            return;
        }
        if q.dropped > 0 {
            let n = std::mem::take(&mut q.dropped);
            q.items.push_back(gap(self.sub, n));
        }
        q.items.push_back(msg);
        q.finished = true;
        drop(q);
        self.ready.notify_one();
    }

    pub fn pop(&self) -> Option<Out> {
        let mut q = self.inner.lock().unwrap();
        let item = q
            .items
            .pop_front()
            .or_else(|| (q.dropped > 0).then(|| gap(self.sub, std::mem::take(&mut q.dropped))));
        drop(q);
        if item.is_some() {
            self.space.notify_one();
        }
        item
    }

    /// True when the producer finished and everything has been sent.
    pub fn drained(&self) -> bool {
        let q = self.inner.lock().unwrap();
        q.finished && q.items.is_empty()
    }

    /// Unsubscribe: drop pending messages and stop producers.
    pub fn close(&self) {
        let mut q = self.inner.lock().unwrap();
        q.closed = true;
        q.items.clear();
        drop(q);
        self.space.notify_waiters();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::protocol::data;
    use serde_json::json;
    use std::time::Duration;

    fn q(policy: Policy, cap: usize) -> SubQueue {
        SubQueue::new(1, policy, cap, Arc::new(Notify::new()))
    }

    fn n(i: u64) -> Out {
        data(1, json!(i))
    }

    fn drain(q: &SubQueue) -> Vec<Out> {
        std::iter::from_fn(|| q.pop()).collect()
    }

    #[test]
    fn drop_oldest_keeps_the_newest() {
        let q = q(Policy::DropOldest, 3);
        for i in 0..5 {
            assert!(q.push(n(i), 1));
        }
        assert_eq!(drain(&q), [n(2), n(3), n(4)]);
    }

    #[test]
    fn gap_drops_new_lines_and_reports_how_many() {
        let q = q(Policy::Gap, 3);
        for i in 0..3 {
            q.push(n(i), 10);
        }
        // Full: these batches (10 lines each) are dropped.
        q.push(n(3), 10);
        q.push(n(4), 10);
        assert_eq!(q.pop(), Some(n(0)));
        assert_eq!(q.pop(), Some(n(1)));
        // Room for gap + message now.
        q.push(n(5), 10);
        assert_eq!(drain(&q), [n(2), gap(1, 20), n(5)]);
    }

    #[test]
    fn gap_is_not_lost_when_nothing_follows() {
        let q = q(Policy::Gap, 1);
        q.push(n(0), 1);
        q.push(n(1), 4);
        assert_eq!(drain(&q), [n(0), gap(1, 4)]);
    }

    #[test]
    fn finish_goes_out_last_even_when_full() {
        let q = q(Policy::Gap, 1);
        q.push(n(0), 1);
        q.push(n(1), 2);
        q.finish(n(99));
        assert!(!q.drained());
        assert_eq!(drain(&q), [n(0), gap(1, 2), n(99)]);
        assert!(q.drained());
    }

    #[test]
    fn closed_queue_rejects_pushes() {
        let q = q(Policy::DropOldest, 2);
        q.push(n(0), 1);
        q.close();
        assert!(!q.push(n(1), 1));
        assert_eq!(q.pop(), None);
    }

    #[tokio::test]
    async fn block_waits_for_room() {
        let q = Arc::new(q(Policy::Block, 2));
        assert!(q.push_wait(n(0)).await);
        assert!(q.push_wait(n(1)).await);
        let q2 = q.clone();
        let blocked = tokio::spawn(async move { q2.push_wait(n(2)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!blocked.is_finished(), "producer must wait while full");
        assert_eq!(q.pop(), Some(n(0)));
        assert!(tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap());
        assert_eq!(drain(&q), [n(1), n(2)]);
    }

    #[tokio::test]
    async fn close_releases_a_blocked_producer() {
        let q = Arc::new(q(Policy::Block, 1));
        q.push_wait(n(0)).await;
        let q2 = q.clone();
        let blocked = tokio::spawn(async move { q2.push_wait(n(1)).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        q.close();
        assert!(!tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap());
    }

    #[test]
    fn pushes_wake_the_writer() {
        let ready = Arc::new(Notify::new());
        let q = SubQueue::new(1, Policy::DropOldest, 4, ready.clone());
        q.push(n(0), 1);
        // A stored permit: the next notified() completes immediately.
        let fut = ready.notified();
        futures_util::FutureExt::now_or_never(fut).expect("writer was notified");
    }
}
