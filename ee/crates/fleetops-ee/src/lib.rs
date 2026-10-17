//! FleetOps Enterprise (see ee/LICENSE): license verification, the
//! Enterprise API and its tables. Built into the server only with the
//! `enterprise` feature.

pub mod db;
pub mod export;
pub mod keys;
pub mod license;
pub mod roles;
pub mod routes;
pub mod state;
pub mod teams;

use axum::Extension;
use ed25519_dalek::VerifyingKey;
use fleetops_api::AppState;
use fleetops_core::{
    access::{CustomRoles, ExtraGrants},
    edition::Entitlements,
    now_unix, Result,
};
use fleetops_store::Store;
use futures_util::future::BoxFuture;
use state::LicenseState;
use std::{path::PathBuf, sync::Arc, time::Duration};
use utoipa_axum::{router::OpenApiRouter, routes};

pub struct EeConfig {
    /// `--license-file` / `FLEETOPS_LICENSE_FILE`.
    pub license_file: Option<PathBuf>,
}

/// What the server wires in (editions spec §3.4).
pub struct EeParts {
    pub entitlements: Arc<dyn Entitlements>,
    pub routes: OpenApiRouter<AppState>,
    /// Team and custom-role grants, checked with direct ones on every request.
    pub grants: Arc<dyn ExtraGrants>,
    /// Custom roles granted straight to users, through the core access routes.
    pub roles: Arc<dyn CustomRoles>,
    /// Logs license status changes; spawn it.
    pub background: BoxFuture<'static, ()>,
}

/// The Enterprise routes, for the OpenAPI document (no license attached).
pub fn api_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(
            routes::get_license,
            routes::put_license,
            routes::delete_license
        ))
        .routes(routes!(routes::export_audit))
        .routes(routes!(teams::routes::list, teams::routes::create))
        .routes(routes!(
            teams::routes::get,
            teams::routes::update,
            teams::routes::delete
        ))
        .routes(routes!(
            teams::routes::add_member,
            teams::routes::remove_member
        ))
        .routes(routes!(
            teams::routes::set_access,
            teams::routes::revoke_access
        ))
        .routes(routes!(teams::routes::user_access))
        .routes(routes!(teams::routes::env_teams))
}

pub async fn init(store: &Store, config: EeConfig) -> Result<EeParts> {
    init_with_keys(store, config, keys::trusted()).await
}

pub async fn init_with_keys(
    store: &Store,
    config: EeConfig,
    keys: Vec<VerifyingKey>,
) -> Result<EeParts> {
    db::migrate(store.pool()).await?;
    roles::db::reconcile_direct_grants(store.pool()).await?;
    let state = Arc::new(LicenseState::new(keys, config.license_file));
    state.load(store).await?;
    Ok(EeParts {
        entitlements: state.clone(),
        routes: api_routes().layer(Extension(state.clone())),
        grants: Arc::new(teams::grants::TeamGrants::new(store.pool().clone())),
        roles: Arc::new(roles::grants::EeRoles::new(
            store.pool().clone(),
            state.clone(),
        )),
        background: Box::pin(watch(state)),
    })
}

async fn watch(state: Arc<LicenseState>) {
    let mut last = state.status(now_unix());
    tracing::info!(status = ?last, "Enterprise license");
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    tick.tick().await;
    loop {
        tick.tick().await;
        let now = state.status(now_unix());
        if now != last {
            tracing::warn!(from = ?last, to = ?now, "Enterprise license status changed");
            last = now;
        }
    }
}
