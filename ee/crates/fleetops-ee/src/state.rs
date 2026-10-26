//! The license in effect (editions spec §4.2-4.3). Status is computed from
//! the parsed license and the clock on every check, so expiry needs no
//! restart and per-request checks stay cheap.

use crate::license::{verify, License, Status};
use ed25519_dalek::VerifyingKey;
use fleetops_core::edition::{Edition, Entitlements, Feature};
use fleetops_core::{now_unix, Result};
use fleetops_store::Store;
use serde::Serialize;
use std::{path::PathBuf, sync::RwLock};
use utoipa::ToSchema;

#[derive(Debug, Clone)]
pub enum Loaded {
    None,
    Invalid(String),
    Ok(License),
}

/// Where the license in effect came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = LicenseSource)]
pub enum Source {
    /// Admin → License; removable there.
    Uploaded,
    /// `--license-file`; only the server's configuration can remove it.
    File,
}

/// `GET /ee/license`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LicenseInfo {
    pub status: Status,
    /// `null` when there is no license.
    pub source: Option<Source>,
    /// Why the license is invalid.
    pub reason: Option<String>,
    pub id: Option<String>,
    pub customer: Option<String>,
    pub expires_at: Option<i64>,
    /// `null`: unlimited.
    pub max_users: Option<u32>,
    /// The features the license grants, Teams always among them (in effect
    /// only while it is active).
    pub features: Vec<Feature>,
    pub users_in_use: i64,
    /// `null`: unlimited.
    pub max_nodes: Option<u32>,
    /// Docker or Podman hosts plus Kubernetes nodes managed now.
    pub nodes_in_use: u64,
}

pub struct LicenseState {
    keys: Vec<VerifyingKey>,
    file: Option<PathBuf>,
    loaded: RwLock<(Loaded, Option<Source>)>,
}

impl LicenseState {
    pub fn new(keys: Vec<VerifyingKey>, file: Option<PathBuf>) -> Self {
        Self {
            keys,
            file,
            loaded: RwLock::new((Loaded::None, None)),
        }
    }

    pub fn keys(&self) -> &[VerifyingKey] {
        &self.keys
    }

    /// Reads the uploaded license, or else the `--license-file`.
    pub async fn load(&self, store: &Store) -> Result<()> {
        let (loaded, source) = match crate::db::get(store.pool()).await? {
            Some(raw) => (self.parse(&raw), Some(Source::Uploaded)),
            None => match &self.file {
                None => (Loaded::None, None),
                Some(path) => match std::fs::read_to_string(path) {
                    Ok(text) => (self.parse(&text), Some(Source::File)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        tracing::warn!(path = %path.display(), "license file not found; Enterprise features are off");
                        (Loaded::None, None)
                    }
                    Err(e) => (
                        Loaded::Invalid(format!("cannot read {}: {e}", path.display())),
                        Some(Source::File),
                    ),
                },
            },
        };
        if let Loaded::Invalid(reason) = &loaded {
            tracing::error!(%reason, "license is not valid; Enterprise features are off");
        }
        self.set(loaded, source);
        Ok(())
    }

    fn parse(&self, text: &str) -> Loaded {
        match verify(text, &self.keys) {
            Ok(l) => Loaded::Ok(l),
            Err(e) => Loaded::Invalid(e.to_string()),
        }
    }

    pub fn set(&self, loaded: Loaded, source: Option<Source>) {
        *self.loaded.write().expect("license lock") = (loaded, source);
    }

    pub fn status(&self, now: i64) -> Status {
        match &self.loaded.read().expect("license lock").0 {
            Loaded::None => Status::None,
            Loaded::Invalid(_) => Status::Invalid,
            Loaded::Ok(l) => l.status_at(now),
        }
    }

    pub fn info(&self, now: i64, users_in_use: i64, nodes_in_use: u64) -> LicenseInfo {
        let guard = self.loaded.read().expect("license lock");
        let (loaded, source) = &*guard;
        let source = *source;
        let empty = |status, reason| LicenseInfo {
            status,
            source,
            reason,
            id: None,
            customer: None,
            expires_at: None,
            max_users: None,
            features: vec![],
            users_in_use,
            max_nodes: None,
            nodes_in_use,
        };
        match loaded {
            Loaded::None => empty(Status::None, None),
            Loaded::Invalid(r) => empty(Status::Invalid, Some(r.clone())),
            Loaded::Ok(l) => LicenseInfo {
                status: l.status_at(now),
                source,
                reason: None,
                id: Some(l.payload.id.clone()),
                customer: Some(l.payload.customer.clone()),
                expires_at: Some(l.payload.expires_at),
                max_users: l.payload.max_users,
                features: {
                    let mut f = l.features.clone();
                    for always in [Feature::Teams, Feature::Rbac, Feature::Sso, Feature::Backup] {
                        if !f.contains(&always) {
                            f.push(always);
                        }
                    }
                    f
                },
                users_in_use,
                max_nodes: l.payload.max_nodes,
                nodes_in_use,
            },
        }
    }

    /// Runs `f` on the license while it is active (valid or in grace).
    fn active<T>(&self, f: impl FnOnce(&License) -> T) -> Option<T> {
        match &self.loaded.read().expect("license lock").0 {
            Loaded::Ok(l) if l.status_at(now_unix()).active() => Some(f(l)),
            _ => None,
        }
    }
}

impl Entitlements for LicenseState {
    fn edition(&self) -> Edition {
        Edition::Enterprise
    }
    /// Teams comes with every license, whatever its payload lists, so
    /// licenses issued before it get it too (teams spec, "Licensing").
    fn has(&self, feature: Feature) -> bool {
        self.active(|l| {
            matches!(
                feature,
                Feature::Teams | Feature::Rbac | Feature::Sso | Feature::Backup
            ) || l.features.contains(&feature)
        })
        .unwrap_or(false)
    }
    fn max_users(&self) -> Option<u32> {
        self.active(|l| l.payload.max_users).flatten()
    }
    fn max_nodes(&self) -> Option<u32> {
        self.active(|l| l.payload.max_nodes).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::{sign, Payload, GRACE_SECS, VERSION};
    use ed25519_dalek::SigningKey;
    use fleetops_core::edition::{Edition, Entitlements, Feature};
    use fleetops_core::now_unix;

    fn lic(key: &SigningKey, expires_at: i64, id: &str) -> String {
        sign(
            &Payload {
                v: VERSION,
                id: id.into(),
                customer: "Acme".into(),
                issued_at: 0,
                expires_at,
                max_users: Some(3),
                max_nodes: Some(10),
                features: vec!["audit_export".into()],
            },
            key,
        )
    }

    async fn store() -> Store {
        let s = Store::in_memory().await.unwrap();
        crate::db::migrate(s.pool()).await.unwrap();
        s
    }

    fn key() -> SigningKey {
        SigningKey::from_bytes(&rand::random())
    }

    #[tokio::test]
    async fn nothing_configured_is_none_and_entitles_nothing() {
        let key = key();
        let st = LicenseState::new(vec![key.verifying_key()], None);
        st.load(&store().await).await.unwrap();
        assert_eq!(st.status(now_unix()), Status::None);
        assert_eq!(st.edition(), Edition::Enterprise);
        assert!(!st.has(Feature::AuditExport));
        assert_eq!(st.max_users(), None);
    }

    #[tokio::test]
    async fn stored_license_wins_over_the_file() {
        let key = key();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("l.lic");
        std::fs::write(&file, lic(&key, now_unix() + 1000, "from-file")).unwrap();
        let s = store().await;
        let st = LicenseState::new(vec![key.verifying_key()], Some(file));
        st.load(&s).await.unwrap();
        assert_eq!(st.info(now_unix(), 1, 0).id.as_deref(), Some("from-file"));
        crate::db::put(s.pool(), &lic(&key, now_unix() + 1000, "uploaded"), 0, 1)
            .await
            .unwrap();
        st.load(&s).await.unwrap();
        assert_eq!(st.info(now_unix(), 1, 0).id.as_deref(), Some("uploaded"));
        assert!(st.has(Feature::AuditExport));
        assert_eq!(st.max_users(), Some(3));
        assert_eq!(st.max_nodes(), Some(10));
    }

    #[tokio::test]
    async fn bad_or_missing_file() {
        let key = key();
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.lic");
        std::fs::write(&bad, "garbage").unwrap();
        let st = LicenseState::new(vec![key.verifying_key()], Some(bad));
        st.load(&store().await).await.unwrap();
        let info = st.info(now_unix(), 1, 0);
        assert_eq!(info.status, Status::Invalid);
        assert_eq!(info.reason.as_deref(), Some("not a FleetOps license"));
        let st = LicenseState::new(
            vec![key.verifying_key()],
            Some(dir.path().join("missing.lic")),
        );
        st.load(&store().await).await.unwrap();
        assert_eq!(st.status(now_unix()), Status::None);
    }

    #[tokio::test]
    async fn expired_license_entitles_nothing_and_lifts_the_limit() {
        let key = key();
        let st = LicenseState::new(vec![key.verifying_key()], None);
        let l = crate::license::verify(
            &lic(&key, now_unix() - GRACE_SECS - 1, "old"),
            &[key.verifying_key()],
        )
        .unwrap();
        st.set(Loaded::Ok(l), None);
        assert_eq!(st.status(now_unix()), Status::Expired);
        assert!(!st.has(Feature::AuditExport));
        assert_eq!(st.max_users(), None);
        assert_eq!(st.max_nodes(), None);
    }

    #[tokio::test]
    async fn teams_comes_with_any_active_license() {
        let key = key();
        let at = |expires_at: i64| {
            let st = LicenseState::new(vec![key.verifying_key()], None);
            let l = crate::license::verify(&lic(&key, expires_at, "l"), &[key.verifying_key()])
                .unwrap();
            assert_eq!(l.features, vec![Feature::AuditExport]);
            st.set(Loaded::Ok(l), None);
            st.has(Feature::Teams)
        };
        assert!(at(now_unix() + 1000), "valid");
        assert!(at(now_unix() - 10), "in grace");
        assert!(!at(now_unix() - GRACE_SECS - 1), "expired");
        let none = LicenseState::new(vec![key.verifying_key()], None);
        none.load(&store().await).await.unwrap();
        assert!(!none.has(Feature::Teams), "no license");
    }

    #[tokio::test]
    async fn backup_feature_always_on() {
        let key = key();
        let st = LicenseState::new(vec![key.verifying_key()], None);
        let l = crate::license::verify(&lic(&key, now_unix() + 1000, "l"), &[key.verifying_key()])
            .unwrap();
        assert_eq!(l.features, vec![Feature::AuditExport]);
        st.set(Loaded::Ok(l), None);
        assert!(st.has(Feature::Backup));
        assert!(st
            .info(now_unix(), 0, 0)
            .features
            .contains(&Feature::Backup));
    }
}
