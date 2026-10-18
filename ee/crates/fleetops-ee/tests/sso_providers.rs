mod common;

use axum::http::StatusCode;
use common::{idp::*, *};
use fleetops_api::ApiConfig;
use fleetops_core::now_unix;
use fleetops_ee::sso::flow::Endpoints;
use serde_json::{json, Value};
use std::collections::HashMap;
use wiremock::{
    matchers::{method, path},
    Mock, ResponseTemplate,
};

const TENANT: &str = "72f988bf-86f1-41af-91ab-2d7cd011db47";

fn body(v: Value) -> Option<(&'static str, String)> {
    Some(("application/json", v.to_string()))
}

struct P {
    ee: Ee,
    admin: Session,
    idp: Idp,
    conn: i64,
}

/// A licensed server whose providers all live on one mock, with a
/// connection of `settings` (client id and secret filled in).
async fn provider(issuer_path: &str, mut settings: Value, secrets: Value) -> P {
    let idp = Idp::start(issuer_path).await;
    let uri = idp.uri();
    let endpoints = Endpoints {
        microsoft_login: uri.clone(),
        graph: uri.clone(),
        google_issuer: uri.clone(),
        google_token: format!("{uri}/sa-token"),
        google_directory: uri.clone(),
        github_web: uri.clone(),
        github_api: format!("{uri}/api"),
    };
    let k = key();
    let ee = ee_app_full(
        &k,
        None,
        ApiConfig {
            public_url: Some(PUBLIC_URL.into()),
            ..ApiConfig::default()
        },
        endpoints,
    )
    .await;
    let admin = ee.admin().await;
    ee.install(&admin, &license(&k, now_unix() + 86_400, None, &[]))
        .await;
    settings["client_id"] = json!(CLIENT_ID);
    let mut secrets = secrets;
    secrets["client_secret"] = json!(CLIENT_SECRET);
    let r = ee
        .send(
            "POST",
            "/api/v1/ee/sso/connections",
            body(json!({"name": "P", "settings": settings, "secrets": secrets, "enabled": true})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let conn = r.json()["id"].as_i64().unwrap();
    P {
        ee,
        admin,
        idp,
        conn,
    }
}

impl P {
    /// Starts a sign-in: (state, nonce, binding cookie).
    async fn start(&self) -> (String, String, String) {
        let r = self
            .ee
            .send(
                "GET",
                &format!("/api/v1/ee/sso/{}/start", self.conn),
                None,
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::FOUND, "{}", r.text);
        let loc = r.headers["location"].to_str().unwrap();
        let q: HashMap<String, String> = url::Url::parse(loc)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let cookie = r.headers["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        (
            q["state"].clone(),
            q.get("nonce").cloned().unwrap_or_default(),
            cookie,
        )
    }

    async fn finish(&self, state: &str, cookie: &str) -> Resp {
        let s = Session {
            cookie: cookie.into(),
            csrf: String::new(),
        };
        self.ee
            .send(
                "GET",
                &format!(
                    "/api/v1/ee/sso/{}/callback?code=c1&state={state}",
                    self.conn
                ),
                None,
                Some(&s),
            )
            .await
    }

    /// An OIDC sign-in with `claims` in the ID token.
    async fn oidc_sign_in(&self, issuer: &str, claims: Value) -> Resp {
        let (state, nonce, cookie) = self.start().await;
        self.idp
            .answer_token(Some(self.idp.id_token(issuer, &nonce, claims)))
            .await;
        self.finish(&state, &cookie).await
    }

    async fn groups_of(&self, subject: &str) -> Vec<String> {
        let g: String =
            sqlx::query_scalar("SELECT groups FROM ee_sso_identities WHERE subject = ?")
                .bind(subject)
                .fetch_one(self.ee.store.pool())
                .await
                .unwrap();
        serde_json::from_str(&g).unwrap()
    }
}

fn location(r: &Resp) -> &str {
    r.headers["location"].to_str().unwrap()
}

// ---- Microsoft ----------------------------------------------------------------

async fn microsoft() -> P {
    provider(
        &format!("/{TENANT}/v2.0"),
        json!({"kind": "microsoft", "tenant": TENANT}),
        json!({}),
    )
    .await
}

fn ms_issuer(p: &P) -> String {
    format!("{}/{TENANT}/v2.0", p.idp.uri())
}

#[tokio::test]
async fn microsoft_groups_from_token() {
    let p = microsoft().await;
    let r = p
        .oidc_sign_in(
            &ms_issuer(&p),
            json!({"oid": "o1", "tid": TENANT, "groups": ["g1", "g2"]}),
        )
        .await;
    assert_eq!(location(&r), "/", "{}", location(&r));
    assert_eq!(p.groups_of(&format!("o1:{TENANT}")).await, vec!["g1", "g2"]);
}

#[tokio::test]
async fn microsoft_overage_reads_graph() {
    let p = microsoft().await;
    p.idp
        .json(
            "/v1.0/me/transitiveMemberOf/microsoft.graph.group",
            json!({"value": [{"id": "g-graph"}]}),
        )
        .await;
    let r = p
        .oidc_sign_in(
            &ms_issuer(&p),
            json!({"oid": "o1", "tid": TENANT, "_claim_names": {"groups": "src1"}}),
        )
        .await;
    assert_eq!(location(&r), "/");
    assert_eq!(p.groups_of(&format!("o1:{TENANT}")).await, vec!["g-graph"]);
}

#[tokio::test]
async fn microsoft_graph_failure_logs_in_without_groups() {
    let p = microsoft().await;
    p.idp
        .status("/v1.0/me/transitiveMemberOf/microsoft.graph.group", 403)
        .await;
    let r = p
        .oidc_sign_in(
            &ms_issuer(&p),
            json!({"oid": "o1", "tid": TENANT, "_claim_names": {"groups": "src1"}}),
        )
        .await;
    assert_eq!(location(&r), "/");
    assert!(p.groups_of(&format!("o1:{TENANT}")).await.is_empty());
}

#[tokio::test]
async fn microsoft_wrong_tenant_refused() {
    let p = microsoft().await;
    let r = p
        .oidc_sign_in(
            &ms_issuer(&p),
            json!({"oid": "o1", "tid": "00000000-0000-0000-0000-000000000000"}),
        )
        .await;
    assert_eq!(location(&r), "/login?sso_error=not_allowed");
}

// ---- Google -----------------------------------------------------------------------

async fn google(admin_email: Option<&str>) -> P {
    let mut settings = json!({"kind": "google", "hosted_domains": ["acme.io"]});
    let mut secrets = json!({});
    if let Some(a) = admin_email {
        settings["admin_email"] = json!(a);
        secrets["service_account_json"] =
            json!(json!({"client_email": "sa@acme.iam", "private_key": PRIVATE_KEY}).to_string());
    }
    provider("", settings, secrets).await
}

#[tokio::test]
async fn google_hd_not_allowed() {
    let p = google(None).await;
    let r = p
        .oidc_sign_in(
            &p.idp.uri(),
            json!({"email_verified": true, "hd": "other.io"}),
        )
        .await;
    assert_eq!(location(&r), "/login?sso_error=not_allowed");
}

#[tokio::test]
async fn google_unverified_email_refused() {
    let p = google(None).await;
    let r = p
        .oidc_sign_in(
            &p.idp.uri(),
            json!({"email_verified": false, "hd": "acme.io"}),
        )
        .await;
    assert_eq!(location(&r), "/login?sso_error=not_allowed");
}

#[tokio::test]
async fn google_service_account_groups_mapped() {
    let p = google(Some("admin@acme.io")).await;
    Mock::given(method("POST"))
        .and(path("/sa-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "sa-1", "expires_in": 3600})),
        )
        .mount(&p.idp.server)
        .await;
    p.idp
        .json(
            "/admin/directory/v1/groups",
            json!({"groups": [{"email": "ops@acme.io"}]}),
        )
        .await;
    let r = p
        .oidc_sign_in(
            &p.idp.uri(),
            json!({"sub": "g-1", "email_verified": true, "hd": "acme.io"}),
        )
        .await;
    assert_eq!(location(&r), "/");
    assert_eq!(p.groups_of("g-1").await, vec!["ops@acme.io"]);
    let test =
        p.ee.send(
            "POST",
            &format!("/api/v1/ee/sso/connections/{}/test", p.conn),
            body(json!({"email": "jane@acme.io"})),
            Some(&p.admin),
        )
        .await;
    assert_eq!(
        test.json()["groups"],
        json!(["ops@acme.io"]),
        "{}",
        test.text
    );
}

// ---- GitHub -----------------------------------------------------------------------

async fn github(orgs: Value, teams: Value) -> P {
    let p = provider("", json!({"kind": "github", "orgs": ["Acme"]}), json!({})).await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "gh-1"})))
        .mount(&p.idp.server)
        .await;
    p.idp
        .json("/api/user", json!({"id": 42, "login": "octo"}))
        .await;
    p.idp.json("/api/user/orgs", orgs).await;
    p.idp
        .json(
            "/api/user/emails",
            json!([{"email": "octo@acme.io", "primary": true, "verified": true}]),
        )
        .await;
    p.idp.json("/api/user/teams", teams).await;
    p
}

#[tokio::test]
async fn github_org_required() {
    let p = github(json!([{"login": "other"}]), json!([])).await;
    let (state, _, cookie) = p.start().await;
    let r = p.finish(&state, &cookie).await;
    assert_eq!(location(&r), "/login?sso_error=not_allowed");
}

#[tokio::test]
async fn github_teams_become_org_slash_team() {
    let p = github(
        json!([{"login": "acme"}]),
        json!([
            {"slug": "Platform", "organization": {"login": "Acme"}},
            {"slug": "x", "organization": {"login": "other"}}
        ]),
    )
    .await;
    let (state, _, cookie) = p.start().await;
    let r = p.finish(&state, &cookie).await;
    assert_eq!(location(&r), "/", "{}", r.text);
    assert_eq!(p.groups_of("42").await, vec!["acme/platform"]);
    let u = p.ee.store.find_credentials("octo").await.unwrap();
    assert!(u.is_some());
}

#[tokio::test]
async fn test_route_reports_discovery_failure() {
    let p = provider(
        "",
        json!({"kind": "oidc", "issuer": "http://127.0.0.1:1"}),
        json!({}),
    )
    .await;
    let r =
        p.ee.send(
            "POST",
            &format!("/api/v1/ee/sso/connections/{}/test", p.conn),
            body(json!({})),
            Some(&p.admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json()["ok"], false);
}
