use fleetops_api::auth::SESSION_IDLE_SECS;
use fleetops_core::{now_unix, Result};
use fleetops_store::Store;
use std::time::Duration;

/// Deletes expired sessions and audit rows past retention.
/// Returns `(sessions_deleted, audit_rows_deleted)`.
pub async fn prune_once(store: &Store, now: i64) -> Result<(u64, u64)> {
    let sessions = store
        .delete_expired_sessions(now, SESSION_IDLE_SECS)
        .await?;
    let days = store.audit_retention_days().await?;
    let audit = store.prune_audit(now - days * 86_400).await?;
    Ok((sessions, audit))
}

pub async fn run(store: Store) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tick.tick().await;
        match prune_once(&store, now_unix()).await {
            Ok((s, a)) => tracing::debug!(sessions = s, audit = a, "maintenance prune"),
            Err(e) => tracing::warn!(error = %e, "maintenance prune failed"),
        }
    }
}
