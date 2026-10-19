//! One backup run: write the file once, send it to every target, apply
//! retention there, and record what happened.

use super::{
    db::{self, NewRun, Target, Trigger},
    targets::{sink, to_delete},
};
use fleetops_api::{audit_detail, backup::data_dir, AppState};
use fleetops_core::{edition::Feature, now_unix, Error, Result};
use fleetops_store::backup::backup_name;
use serde_json::json;
use tokio::sync::Mutex;

/// One backup at a time.
#[derive(Default)]
pub struct Runner {
    lock: Mutex<()>,
}

impl Runner {
    /// Runs now, waiting for a run in progress to end.
    pub async fn run(&self, s: &AppState, trigger: Trigger, by: Option<i64>) {
        let _g = self.lock.lock().await;
        run_once(s, trigger, by).await;
    }

    /// Starts a run in the background, unless one is running.
    pub fn start(self: &std::sync::Arc<Self>, s: &AppState, by: i64) -> Result<()> {
        let guard = self
            .lock
            .try_lock()
            .map_err(|_| Error::Conflict("a backup is already running".into()))?;
        drop(guard);
        let (me, s) = (self.clone(), s.clone());
        tokio::spawn(async move { me.run(&s, Trigger::Manual, Some(by)).await });
        Ok(())
    }
}

async fn record(s: &AppState, by: Option<i64>, run: NewRun<'_>) {
    if let Err(e) = db::record_run(s.store.pool(), &run).await {
        tracing::warn!(error = %e, "could not record a backup run");
    }
    audit_detail(
        s,
        by,
        "backup.scheduled",
        "backup",
        run.file_name.unwrap_or(""),
        run.result,
        Some(json!({
            "trigger": run.trigger,
            "target": run.target.map(|t| t.settings.kind()),
            "error": run.error,
            "reason": (run.result == "skipped").then_some("license_required"),
        })),
    )
    .await;
}

/// Never fails: every outcome becomes a run row and an audit entry.
pub async fn run_once(s: &AppState, trigger: Trigger, by: Option<i64>) {
    let started_at = now_unix();
    let base = NewRun {
        started_at,
        finished_at: started_at,
        trigger,
        target: None,
        result: "skipped",
        file_name: None,
        size: None,
        error: None,
    };
    if !s.entitlements.has(Feature::Backup) {
        record(s, by, base).await;
        return;
    }
    let prepared = async {
        let pw = db::passphrase(&s.store)
            .await?
            .ok_or_else(|| Error::Invalid("set a backup passphrase first".into()))?;
        let targets = db::list_targets(s.store.pool()).await?;
        if targets.is_empty() {
            return Err(Error::Invalid("add a backup target first".into()));
        }
        let file = fleetops_api::write_backup_to_tmp(s, &pw, started_at).await?;
        Ok::<_, Error>((targets, file))
    }
    .await;
    let (targets, file) = match prepared {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            record(
                s,
                by,
                NewRun {
                    result: "failure",
                    error: Some(&msg),
                    finished_at: now_unix(),
                    ..base
                },
            )
            .await;
            return;
        }
    };
    let name = backup_name(started_at);
    let size = std::fs::metadata(&file).map(|m| m.len() as i64).ok();
    for t in &targets {
        // A failed clean-up still leaves a good backup, so it's a note on
        // a successful run rather than a failure.
        let (result, err) = match send(s, t, &name, &file, started_at).await {
            Ok(None) => ("success", None),
            Ok(Some(e)) => (
                "success",
                Some(format!("uploaded, but removing old backups failed: {e}")),
            ),
            Err(e) => ("failure", Some(e.to_string())),
        };
        record(
            s,
            by,
            NewRun {
                target: Some(t),
                result,
                file_name: Some(&name),
                size,
                error: err.as_deref(),
                finished_at: now_unix(),
                ..base
            },
        )
        .await;
    }
    let _ = std::fs::remove_file(&file);
}

/// Uploads the backup, then applies retention. `Ok(Some(_))` means the
/// upload worked but retention didn't.
async fn send(
    s: &AppState,
    t: &Target,
    name: &str,
    file: &std::path::Path,
    now: i64,
) -> Result<Option<Error>> {
    let secrets = db::target_secrets(&s.store, t.id).await?;
    let sink = sink(t, secrets, data_dir(s)?)?;
    sink.put(name, file).await?;
    let retention = async {
        let keep = db::get_schedule(s.store.pool()).await?.retention;
        let files = sink.list().await?;
        for old in to_delete(&files, keep, now) {
            if old != name {
                sink.delete(&old).await?;
            }
        }
        Ok(())
    };
    Ok(retention.await.err())
}
