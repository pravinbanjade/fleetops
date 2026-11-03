//! Trivy as a Kubernetes Job (Task 7 fills this in).

use super::config::Effective;
use super::model::ScanTarget;
use fleetops_core::env::Environment;
use fleetops_core::{Error, Result};
use std::sync::Arc;

fn todo() -> Error {
    Error::Invalid("Kubernetes scanning is not available yet".into())
}

pub async fn prepare(_env: &Arc<dyn Environment>, _cfg: &Effective) -> Result<()> {
    Err(todo())
}

pub async fn scan(
    _env: &Arc<dyn Environment>,
    _t: &ScanTarget,
    _cfg: &Effective,
    _scan_id: i64,
) -> Result<Vec<u8>> {
    Err(todo())
}

pub async fn cleanup_orphans(_env: &Arc<dyn Environment>) -> Result<()> {
    Ok(())
}
