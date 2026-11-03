//! `/ee/security` (vuln-scanning spec, Enterprise): the patch queue, image
//! results, Scan now, and the scanning settings.

use super::config::{effective, EnvOverride, GlobalConfig, Mirror, DEFAULT_TRIVY_IMAGE};
use super::db::{self, PasswordChange};
use super::engine::{target_of, Engine};
use super::model::{ImageKey, ImageUse, Trigger};
use super::view::*;
use axum::{extract::State, Extension, Json};
use fleetops_api::{
    audit_detail,
    auth::extract::{AdminUser, CurrentUser},
    authz::{effective_grant, effective_grants, env_access, require_feature, Need},
    extract::{ApiJson, ApiPath, ApiQuery},
    ApiResult, AppState,
};
use fleetops_core::access::{perm_denied, Grants};
use fleetops_core::docker::split_image_reference;
use fleetops_core::edition::Feature;
use fleetops_core::env::EnvKind;
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::{now_unix, Error, Result};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use utoipa::IntoParams;

type Eng = Extension<Arc<Engine>>;

const IMAGES_VIEW: Permission = Permission::new(Row::Images, Verb::View);
const IMAGES_CREATE: Permission = Permission::new(Row::Images, Verb::Create);
const PODS_VIEW: Permission = Permission::new(Row::Pods, Verb::View);
const WORKLOADS_OPERATE: Permission = Permission::new(Row::Workloads, Verb::Operate);

/// Whether `g` may see `u`: Docker kinds and swarms need image view on the
/// whole environment; a cluster, pod view in the use's namespace.
fn sees(kind: EnvKind, g: &Grants, u: &ImageUse) -> bool {
    if kind.is_kube() {
        g.allows(PODS_VIEW, Some(&u.namespace))
    } else {
        g.allows(IMAGES_VIEW, None)
    }
}

struct Visible {
    uses: Vec<ImageUse>,
    names: HashMap<i64, String>,
}

/// Every use the caller may see, in `only` environments when given.
async fn visible(
    s: &AppState,
    cu: &CurrentUser,
    only: Option<&HashSet<String>>,
) -> Result<Visible> {
    let grants = effective_grants(s, cu.user.id).await?;
    let mut names = HashMap::new();
    let mut kinds = HashMap::new();
    for r in s.store.list_environments().await? {
        if !grants.contains_key(&r.id) || only.is_some_and(|o| !o.contains(&r.name)) {
            continue;
        }
        let Ok(kind) = r.kind.parse::<EnvKind>() else {
            continue;
        };
        names.insert(r.id, r.name);
        kinds.insert(r.id, kind);
    }
    let ids: Vec<i64> = names.keys().copied().collect();
    let mut uses = db::uses_for_envs(&s.store, &ids).await?;
    uses.retain(|u| sees(kinds[&u.env_id], &grants[&u.env_id], u));
    Ok(Visible { uses, names })
}

async fn states(s: &AppState, uses: &[ImageUse]) -> Result<HashMap<ImageKey, db::KeyState>> {
    let mut keys: Vec<ImageKey> = uses.iter().map(|u| u.key.clone()).collect();
    keys.sort();
    keys.dedup();
    db::latest(&s.store, &keys).await
}

fn unavailable(eng: &Engine, ids: impl Iterator<Item = i64>) -> HashSet<i64> {
    ids.filter(|id| {
        matches!(
            eng.status(*id),
            super::engine::ScannerStatus::Unavailable { .. }
        )
    })
    .collect()
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImagesQuery {
    /// Comma-separated environment names; all when omitted.
    pub envs: Option<String>,
    /// Only images with a running container (default true).
    pub running: Option<bool>,
    /// Only images with a fixable critical or high vulnerability, plus any
    /// without a clean result (default true).
    pub fixable: Option<bool>,
}

#[utoipa::path(get, path = "/ee/security/images", operation_id = "listSecurityImages", tag = "enterprise",
    summary = "The patch queue: images ranked by fixable vulnerabilities on running containers",
    params(ImagesQuery),
    responses((status = 200, description = "Summary and rows", body = SecuritySummary)))]
pub async fn list_images(
    State(s): State<AppState>,
    Extension(eng): Eng,
    cu: CurrentUser,
    ApiQuery(q): ApiQuery<ImagesQuery>,
) -> ApiResult<Json<SecuritySummary>> {
    require_feature(&s, Feature::VulnScan)?;
    let only: Option<HashSet<String>> = q
        .envs
        .filter(|e| !e.trim().is_empty())
        .map(|e| e.split(',').map(|n| n.trim().to_string()).collect());
    let v = visible(&s, &cu, only.as_ref()).await?;
    let st = states(&s, &v.uses).await?;
    let un = unavailable(&eng, v.names.keys().copied());
    let c = Ctx {
        states: &st,
        unavailable: &un,
        env_names: &v.names,
        now: now_unix(),
    };
    Ok(Json(summarize(
        &v.uses,
        &c,
        q.running.unwrap_or(true),
        q.fixable.unwrap_or(true),
    )))
}

async fn detail(
    s: &AppState,
    eng: &Engine,
    image: String,
    uses: Vec<ImageUse>,
    names: &HashMap<i64, String>,
) -> Result<ImageDetail> {
    if uses.is_empty() {
        return Err(Error::NotFound(format!("image '{image}'")));
    }
    let st = states(s, &uses).await?;
    let un = unavailable(eng, uses.iter().map(|u| u.env_id));
    let mut keys: Vec<&ImageKey> = uses.iter().map(|u| &u.key).collect();
    keys.sort();
    keys.dedup();
    let mut digests = Vec::new();
    for key in keys {
        let mine: Vec<&ImageUse> = uses.iter().filter(|u| &u.key == key).collect();
        let v = key_view(
            st.get(key),
            mine.iter().any(|u| un.contains(&u.env_id)),
            now_unix(),
        );
        let findings = match v.done_scan {
            Some(id) => finding_views(db::findings(&s.store, id).await?),
            None => vec![],
        };
        digests.push(DigestDetail {
            key: key.as_str().to_string(),
            state: v.state,
            last_scan_at: v.last_scan_at,
            trivy_image: v.trivy_image,
            error: v.error,
            counts: v.counts,
            findings,
            used_by: mine.iter().map(|u| use_view(u, names)).collect(),
        });
    }
    Ok(ImageDetail { image, digests })
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImageQuery {
    /// repo:tag, as the patch queue lists it.
    pub image: String,
}

#[utoipa::path(get, path = "/ee/security/image", operation_id = "getSecurityImage", tag = "enterprise",
    summary = "One image's vulnerabilities and where it is used",
    params(ImageQuery),
    responses((status = 200, description = "Per digest", body = ImageDetail)))]
pub async fn get_image(
    State(s): State<AppState>,
    Extension(eng): Eng,
    cu: CurrentUser,
    ApiQuery(q): ApiQuery<ImageQuery>,
) -> ApiResult<Json<ImageDetail>> {
    require_feature(&s, Feature::VulnScan)?;
    let v = visible(&s, &cu, None).await?;
    let uses: Vec<ImageUse> = v
        .uses
        .into_iter()
        .filter(|u| u.image_ref == q.image)
        .collect();
    Ok(Json(detail(&s, &eng, q.image, uses, &v.names).await?))
}

#[utoipa::path(get, path = "/ee/security/envs/{env}/images/{id}", operation_id = "getEnvImageSecurity", tag = "enterprise",
    summary = "A Docker image's vulnerabilities, for its image page",
    params(("env" = String, Path), ("id" = String, Path, description = "Docker image id")),
    responses((status = 200, description = "Per digest", body = ImageDetail)))]
pub async fn get_env_image(
    State(s): State<AppState>,
    Extension(eng): Eng,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<ImageDetail>> {
    require_feature(&s, Feature::VulnScan)?;
    let (e, g) = env_access(&s, &cu, &env, Need::Perm(IMAGES_VIEW)).await?;
    g.check_env(IMAGES_VIEW)?;
    let names = HashMap::from([(e.id(), e.name().to_string())]);
    let uses: Vec<ImageUse> = db::uses_for_envs(&s.store, &[e.id()])
        .await?
        .into_iter()
        .filter(|u| u.image_id.as_deref() == Some(id.as_str()))
        .collect();
    let image = uses.first().map(|u| u.image_ref.clone()).unwrap_or(id);
    Ok(Json(detail(&s, &eng, image, uses, &names).await?))
}

#[utoipa::path(get, path = "/ee/security/envs/{env}/badges", operation_id = "listImageBadges", tag = "enterprise",
    summary = "A severity badge per Docker image, for the image list",
    params(("env" = String, Path)),
    responses((status = 200, description = "Badges", body = Vec<ImageBadge>)))]
pub async fn list_badges(
    State(s): State<AppState>,
    Extension(eng): Eng,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ImageBadge>>> {
    require_feature(&s, Feature::VulnScan)?;
    let (e, g) = env_access(&s, &cu, &env, Need::Perm(IMAGES_VIEW)).await?;
    g.check_env(IMAGES_VIEW)?;
    let uses = db::uses_for_envs(&s.store, &[e.id()]).await?;
    let st = states(&s, &uses).await?;
    let un = unavailable(&eng, std::iter::once(e.id()));
    let names = HashMap::from([(e.id(), e.name().to_string())]);
    let c = Ctx {
        states: &st,
        unavailable: &un,
        env_names: &names,
        now: now_unix(),
    };
    Ok(Json(badges(&uses, &c)))
}

#[utoipa::path(post, path = "/ee/security/scan", operation_id = "scanNow", tag = "enterprise",
    summary = "Scan images now",
    request_body = ScanNowBody,
    responses((status = 200, description = "Queued", body = ScanQueued)))]
pub async fn scan_now(
    State(s): State<AppState>,
    Extension(eng): Eng,
    cu: CurrentUser,
    ApiJson(b): ApiJson<ScanNowBody>,
) -> ApiResult<Json<ScanQueued>> {
    require_feature(&s, Feature::VulnScan)?;
    let env = s.envs.get_stored(&b.env)?;
    let g = effective_grant(&s, cu.user.id, env.id()).await?;
    if g.is_empty() {
        return Err(Error::NotFound(format!("environment '{}'", b.env)).into());
    }
    let kind = env.kind();
    let mut uses: Vec<ImageUse> = db::uses_for_envs(&s.store, &[env.id()])
        .await?
        .into_iter()
        .filter(|u| sees(kind, &g, u))
        .collect();
    if let Some(want) = &b.images {
        let want: HashSet<&str> = want.iter().map(String::as_str).collect();
        uses.retain(|u| {
            want.contains(u.image_ref.as_str())
                || u.image_id.as_deref().is_some_and(|i| want.contains(i))
                || want.contains(u.key.as_str())
        });
    }
    if uses.is_empty() {
        return Err(Error::NotFound("no such image in this environment".into()).into());
    }
    let cfg = {
        let (gc, gpw) = db::get_global(&s.store).await?;
        let o = db::get_env(&s.store, env.id()).await?;
        effective(&gc, gpw.as_deref(), o.as_ref().map(|(o, _)| o), None)
    };
    if !cfg.enabled {
        return Err(Error::Invalid("security scanning is off for this environment".into()).into());
    }
    let mut keys: Vec<ImageKey> = uses.iter().map(|u| u.key.clone()).collect();
    keys.sort();
    keys.dedup();
    let mut queued = 0;
    for key in &keys {
        let mine: Vec<&ImageUse> = uses.iter().filter(|u| &u.key == key).collect();
        if kind.is_kube() {
            if !mine
                .iter()
                .any(|u| g.allows(WORKLOADS_OPERATE, Some(&u.namespace)))
            {
                return Err(perm_denied(WORKLOADS_OPERATE).into());
            }
        } else {
            g.check_env(IMAGES_CREATE)?;
        }
        let u = mine.iter().find(|u| u.running).copied().unwrap_or(mine[0]);
        eng.enqueue(&s, target_of(u), Trigger::Manual).await?;
        queued += 1;
    }
    audit_detail(
        &s,
        Some(cu.user.id),
        "security.scan",
        "environment",
        &b.env,
        "success",
        Some(json!({"images": b.images, "queued": queued})),
    )
    .await;
    Ok(Json(ScanQueued { queued }))
}

fn check_ref(what: &str, r: &Option<String>) -> Result<()> {
    if let Some(r) = r {
        split_image_reference(r)
            .map_err(|_| Error::Invalid(format!("{what}: '{r}' is not an image reference")))?;
    }
    Ok(())
}

fn mirror_of(b: &Option<MirrorBody>) -> Result<(Option<Mirror>, PasswordChange)> {
    let Some(b) = b else {
        return Ok((None, PasswordChange::Clear));
    };
    check_ref("trivy_image", &b.trivy_image)?;
    check_ref("db_repository", &b.db_repository)?;
    check_ref("java_db_repository", &b.java_db_repository)?;
    let clean = |v: &Option<String>| {
        v.as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let pw = if b.clear_password {
        PasswordChange::Clear
    } else {
        match b.password.as_deref() {
            Some(p) if !p.is_empty() => PasswordChange::Set(p.to_string()),
            _ => PasswordChange::Keep,
        }
    };
    Ok((
        Some(Mirror {
            trivy_image: clean(&b.trivy_image),
            db_repository: clean(&b.db_repository),
            java_db_repository: clean(&b.java_db_repository),
            username: clean(&b.username),
        }),
        pw,
    ))
}

async fn settings(s: &AppState, eng: &Engine) -> Result<SecuritySettings> {
    let (g, gpw) = db::get_global(&s.store).await?;
    let overrides: HashMap<i64, (EnvOverride, Option<String>)> = db::list_envs(&s.store)
        .await?
        .into_iter()
        .map(|(id, o, p)| (id, (o, p)))
        .collect();
    let mut envs = Vec::new();
    for r in s.store.list_environments().await? {
        let Ok(kind) = r.kind.parse::<EnvKind>() else {
            continue;
        };
        let o = overrides.get(&r.id);
        let eff = effective(
            &g,
            gpw.as_deref(),
            o.map(|(o, _)| o),
            o.and_then(|(_, p)| p.as_deref()),
        );
        envs.push(EnvSettingsView {
            env: r.name,
            kind,
            enabled: o.and_then(|(o, _)| o.enabled),
            effective_enabled: eff.enabled,
            mirror: o.and_then(|(o, p)| o.mirror.as_ref().map(|m| MirrorView::of(m, p.is_some()))),
            socket_path: o.and_then(|(o, _)| o.socket_path.clone()),
            status: eng.status(r.id),
        });
    }
    Ok(SecuritySettings {
        global: GlobalView {
            enabled: g.enabled,
            mirror: g.mirror.as_ref().map(|m| MirrorView::of(m, gpw.is_some())),
            default_trivy_image: DEFAULT_TRIVY_IMAGE.to_string(),
        },
        envs,
    })
}

#[utoipa::path(get, path = "/ee/security/settings", operation_id = "getSecuritySettings", tag = "enterprise",
    summary = "Scanning settings and each environment's scanner",
    responses((status = 200, description = "Settings", body = SecuritySettings)))]
pub async fn get_settings(
    State(s): State<AppState>,
    Extension(eng): Eng,
    _a: AdminUser,
) -> ApiResult<Json<SecuritySettings>> {
    require_feature(&s, Feature::VulnScan)?;
    Ok(Json(settings(&s, &eng).await?))
}

#[utoipa::path(put, path = "/ee/security/settings", operation_id = "putSecuritySettings", tag = "enterprise",
    summary = "Turn scanning on or off for every environment that inherits, and set the mirror",
    request_body = GlobalBody,
    responses((status = 200, description = "Settings", body = SecuritySettings)))]
pub async fn put_settings(
    State(s): State<AppState>,
    Extension(eng): Eng,
    AdminUser(cu): AdminUser,
    ApiJson(b): ApiJson<GlobalBody>,
) -> ApiResult<Json<SecuritySettings>> {
    require_feature(&s, Feature::VulnScan)?;
    let (mirror, pw) = mirror_of(&b.mirror)?;
    let g = GlobalConfig {
        enabled: b.enabled,
        mirror,
    };
    db::put_global(&s.store, &g, pw, now_unix()).await?;
    audit_detail(
        &s,
        Some(cu.user.id),
        "security.settings.update",
        "security",
        "global",
        "success",
        Some(json!({"enabled": g.enabled, "mirror": g.mirror})),
    )
    .await;
    for r in s.store.list_environments().await? {
        let _ = eng.env_changed(&s, r.id).await;
    }
    Ok(Json(settings(&s, &eng).await?))
}

fn check_socket(p: &Option<String>) -> Result<Option<String>> {
    let Some(p) = p.as_ref().map(|p| p.trim()).filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    if !p.starts_with('/')
        || p.len() > 256
        || p.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Error::Invalid(
            "socket_path must be an absolute path of at most 256 characters".into(),
        ));
    }
    Ok(Some(p.to_string()))
}

#[utoipa::path(put, path = "/ee/security/envs/{env}/settings", operation_id = "putEnvSecuritySettings", tag = "enterprise",
    summary = "One environment's scanning: inherit, on or off; its own mirror and socket",
    params(("env" = String, Path)),
    request_body = EnvBody,
    responses((status = 200, description = "Settings", body = SecuritySettings)))]
pub async fn put_env_settings(
    State(s): State<AppState>,
    Extension(eng): Eng,
    AdminUser(cu): AdminUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(b): ApiJson<EnvBody>,
) -> ApiResult<Json<SecuritySettings>> {
    require_feature(&s, Feature::VulnScan)?;
    let e = s.envs.get_stored(&env)?;
    let socket_path = check_socket(&b.socket_path)?;
    let (mirror, pw) = mirror_of(&b.mirror)?;
    let o = EnvOverride {
        enabled: b.enabled,
        mirror,
        socket_path,
    };
    db::put_env(&s.store, e.id(), &o, pw, now_unix()).await?;
    audit_detail(
        &s,
        Some(cu.user.id),
        "security.env.update",
        "environment",
        &env,
        "success",
        Some(json!({"enabled": o.enabled, "mirror": o.mirror, "socket_path": o.socket_path})),
    )
    .await;
    eng.env_changed(&s, e.id()).await?;
    Ok(Json(settings(&s, &eng).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_socket_path_rejected() {
        assert!(check_socket(&Some("relative.sock".into())).is_err());
        assert!(check_socket(&Some(format!("/{}", "a".repeat(300)))).is_err());
        assert_eq!(check_socket(&Some("  ".into())).unwrap(), None);
        assert_eq!(
            check_socket(&Some("/run/user/1000/docker.sock".into()))
                .unwrap()
                .as_deref(),
            Some("/run/user/1000/docker.sock")
        );
    }

    #[test]
    fn mirror_password_change_follows_the_body() {
        let set = MirrorBody {
            password: Some("pw".into()),
            ..Default::default()
        };
        assert_eq!(
            mirror_of(&Some(set)).unwrap().1,
            PasswordChange::Set("pw".into())
        );
        let keep = MirrorBody {
            password: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(mirror_of(&Some(keep)).unwrap().1, PasswordChange::Keep);
        let clear = MirrorBody {
            clear_password: true,
            ..Default::default()
        };
        assert_eq!(mirror_of(&Some(clear)).unwrap().1, PasswordChange::Clear);
        assert_eq!(mirror_of(&None).unwrap(), (None, PasswordChange::Clear));
        let bad = MirrorBody {
            trivy_image: Some("bad ref".into()),
            ..Default::default()
        };
        assert!(mirror_of(&Some(bad)).is_err());
    }
}
