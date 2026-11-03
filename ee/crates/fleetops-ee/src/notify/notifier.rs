//! The loop that sends: due batches become deliveries, deliveries are sent
//! and retried. Both live in the database, so a restart resumes them.

use super::db;
use super::model::{build, Message, RuleRef};
use super::send::{deliver, LiveTransport, Transport};
use crate::scan::model::Severity;
use fleetops_api::AppState;
use fleetops_core::edition::Feature;
use fleetops_core::{now_unix, Error, Result};
use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the loop looks for due batches and deliveries.
pub const TICK: Duration = Duration::from_secs(15);
/// Waits before the 2nd and 3rd attempts: 2 min, then 10 more (12 after the first).
pub const RETRY_AFTER: [i64; 2] = [120, 600];
/// Deliveries sent at once.
pub const PARALLEL: usize = 4;
/// Finished deliveries are forgotten after this.
pub const KEEP_SECS: i64 = 30 * 86_400;

pub struct Notifier {
    transport: Arc<dyn Transport>,
    pruned_at: Mutex<i64>,
}

impl Notifier {
    /// `transport` replaces HTTP and SMTP (tests).
    pub fn new(transport: Option<Arc<dyn Transport>>) -> Arc<Self> {
        Arc::new(Notifier {
            transport: transport.unwrap_or_else(|| Arc::new(LiveTransport::default())),
            pruned_at: Mutex::new(0),
        })
    }

    pub async fn run(self: Arc<Self>, s: AppState) {
        loop {
            self.tick(&s, now_unix()).await;
            tokio::time::sleep(TICK).await;
        }
    }

    #[doc(hidden)]
    pub async fn tick(&self, s: &AppState, now: i64) {
        if !s.entitlements.has(Feature::Notifications) {
            return;
        }
        if let Err(e) = self.flush(s, now).await {
            tracing::warn!(error = %e, "notifications: could not flush batches");
        }
        if let Err(e) = self.send_due(s, now).await {
            tracing::warn!(error = %e, "notifications: could not send");
        }
        let prune = {
            let mut p = self.pruned_at.lock().unwrap();
            let due = now - *p >= 86_400;
            if due {
                *p = now;
            }
            due
        };
        if prune {
            let _ = db::prune_deliveries(&s.store, now - KEEP_SECS).await;
        }
    }

    async fn flush(&self, s: &AppState, now: i64) -> Result<()> {
        for g in db::due_pending(&s.store, now).await? {
            let Some(rule) = db::get_rule(&s.store, g.rule_id).await? else {
                db::delete_pending(&s.store, g.rule_id, g.kind, &g.group_key).await?;
                continue;
            };
            if g.items.is_empty() {
                db::delete_pending(&s.store, g.rule_id, g.kind, &g.group_key).await?;
                continue;
            }
            let msg = build(
                &g,
                &RuleRef {
                    id: rule.id,
                    name: rule.name.clone(),
                },
                rule.fixable_only,
            );
            db::flush_group(&s.store, &g, &msg, &rule.destinations, now).await?;
        }
        Ok(())
    }

    async fn send_due(&self, s: &AppState, now: i64) -> Result<()> {
        let due = db::due_deliveries(&s.store, now, 20).await?;
        if due.is_empty() {
            return Ok(());
        }
        let smtp = db::get_smtp(&s.store).await.unwrap_or(None);
        let public = s.config.public_url.clone();
        futures_util::stream::iter(due)
            .for_each_concurrent(PARALLEL, |d| {
                let smtp = smtp.clone();
                let public = public.clone();
                async move {
                    let Some(dest_id) = d.destination_id else {
                        let _ =
                            db::delivery_failed(&s.store, d.id, "the destination was deleted", now)
                                .await;
                        return;
                    };
                    let result = match db::get_destination(&s.store, dest_id).await {
                        Ok((dest, secrets)) => {
                            deliver(
                                self.transport.as_ref(),
                                &dest,
                                &secrets,
                                smtp.as_ref(),
                                &d.message,
                                public.as_deref(),
                            )
                            .await
                        }
                        Err(e) => Err(e),
                    };
                    match result {
                        Ok(()) => {
                            let _ = db::delivery_sent(&s.store, d.id, now).await;
                            let _ = db::set_destination_error(&s.store, dest_id, None, now).await;
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            match RETRY_AFTER.get(d.attempts as usize) {
                                Some(wait) => {
                                    let _ =
                                        db::delivery_retry(&s.store, d.id, &msg, now + wait).await;
                                }
                                None => {
                                    let _ = db::delivery_failed(&s.store, d.id, &msg, now).await;
                                    let _ = db::set_destination_error(
                                        &s.store,
                                        dest_id,
                                        Some(&msg),
                                        now,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                }
            })
            .await;
        Ok(())
    }

    fn sample() -> Message {
        Message::NewCve {
            rule: RuleRef {
                id: 0,
                name: "Test from FleetOps".into(),
            },
            cve: "[Test] CVE-0000-0000".into(),
            severity: Severity::Critical,
            fixed_version: Some("1.0.1".into()),
            title: Some("A test notification: nothing is wrong.".into()),
            packages: vec![("example".into(), "1.0.0".into())],
            images: vec![],
            more: 0,
            fixable_only: true,
        }
    }

    /// Sends a clearly marked sample to `id` now; the error if it fails.
    pub async fn test_destination(&self, s: &AppState, id: i64) -> Result<()> {
        let (dest, secrets) = db::get_destination(&s.store, id).await?;
        let smtp = db::get_smtp(&s.store).await?;
        deliver(
            self.transport.as_ref(),
            &dest,
            &secrets,
            smtp.as_ref(),
            &Self::sample(),
            s.config.public_url.as_deref(),
        )
        .await
    }

    /// Sends the sample by email to `to` with the saved SMTP settings.
    pub async fn test_smtp(&self, s: &AppState, to: &str) -> Result<()> {
        let smtp = db::get_smtp(&s.store)
            .await?
            .ok_or_else(|| Error::Invalid("save the SMTP settings first".into()))?;
        let (subject, text, html) =
            super::format::email(&Self::sample(), s.config.public_url.as_deref());
        self.transport
            .send_mail(&smtp, &[to.to_string()], &subject, &text, &html)
            .await
    }
}
