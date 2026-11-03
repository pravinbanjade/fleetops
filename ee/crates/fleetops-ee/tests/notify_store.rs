use fleetops_ee::notify::{
    db::{self, DeliveryStatus, NewDestination, RuleRow, SmtpRow},
    model::{
        DestinationKind, DestinationSecrets, Message, PendingItem, PendingKind, RuleRef,
        SmtpSecurity,
    },
};
use fleetops_ee::scan::{db::PasswordChange, model::Severity};
use fleetops_store::Store;

async fn store() -> Store {
    let store = Store::in_memory().await.unwrap();
    fleetops_ee::db::migrate(store.pool()).await.unwrap();
    store
}

fn slack(name: &str) -> NewDestination {
    NewDestination {
        name: name.into(),
        kind: DestinationKind::Slack,
        host: Some("hooks.slack.com".into()),
        recipients: vec![],
        secrets: DestinationSecrets {
            url: Some("https://hooks.slack.com/services/T/B/secret-token".into()),
            ..Default::default()
        },
    }
}

fn rule(name: &str, envs: Vec<i64>, dests: Vec<i64>) -> RuleRow {
    RuleRow {
        name: name.into(),
        enabled: true,
        envs,
        min_severity: Severity::Critical,
        fixable_only: true,
        running_only: true,
        destinations: dests,
    }
}

fn item(repo: &str, cve: &str) -> PendingItem {
    PendingItem {
        repository: repo.into(),
        image_ref: format!("{repo}:1"),
        env: "prod".into(),
        running: 1,
        cve: cve.into(),
        severity: Severity::Critical,
        pkg_name: "libssl3".into(),
        installed_version: "3.0.11".into(),
        fixed_version: Some("3.0.14".into()),
        title: None,
    }
}

fn message() -> Message {
    Message::NewCve {
        rule: RuleRef {
            id: 1,
            name: "r".into(),
        },
        cve: "CVE-1".into(),
        severity: Severity::Critical,
        fixed_version: None,
        title: None,
        packages: vec![],
        images: vec![],
        more: 0,
        fixable_only: true,
    }
}

#[tokio::test]
async fn destination_secrets_are_sealed_and_kept_on_update() {
    let s = store().await;
    let id = db::create_destination(&s, &slack("on-call"), 1)
        .await
        .unwrap();
    let raw: Option<String> =
        sqlx::query_scalar("SELECT secrets FROM ee_notify_destinations WHERE id = ?")
            .bind(id)
            .fetch_one(s.pool())
            .await
            .unwrap();
    assert!(raw.is_some_and(|r| !r.contains("secret-token")), "sealed");
    let (d, sec) = db::get_destination(&s, id).await.unwrap();
    assert_eq!(d.host.as_deref(), Some("hooks.slack.com"));
    assert!(sec.url.unwrap().ends_with("secret-token"));
    db::update_destination(&s, id, "on-call 2", Some("hooks.slack.com"), &[], None, 2)
        .await
        .unwrap();
    let (d, sec) = db::get_destination(&s, id).await.unwrap();
    assert_eq!(d.name, "on-call 2");
    assert!(sec.url.is_some(), "None keeps the secrets");
}

#[tokio::test]
async fn deleting_a_destination_detaches_it_from_rules() {
    let s = store().await;
    let a = db::create_destination(&s, &slack("a"), 1).await.unwrap();
    let b = db::create_destination(&s, &slack("b"), 1).await.unwrap();
    let r = db::create_rule(&s, &rule("prod", vec![], vec![a, b]), 1)
        .await
        .unwrap();
    db::delete_destination(&s, a).await.unwrap();
    let rules = db::list_rules(&s).await.unwrap();
    assert_eq!(
        rules.iter().find(|x| x.id == r).unwrap().destinations,
        vec![b]
    );
}

#[tokio::test]
async fn rule_envs_round_trip_empty_means_all() {
    let s = store().await;
    let d = db::create_destination(&s, &slack("a"), 1).await.unwrap();
    let all = db::create_rule(&s, &rule("all", vec![], vec![d]), 1)
        .await
        .unwrap();
    let some = db::create_rule(&s, &rule("some", vec![3, 4], vec![d]), 1)
        .await
        .unwrap();
    let rules = db::list_rules(&s).await.unwrap();
    assert!(rules.iter().find(|r| r.id == all).unwrap().envs.is_empty());
    assert_eq!(
        rules.iter().find(|r| r.id == some).unwrap().envs,
        vec![3, 4]
    );
    let mut changed = rule("some", vec![4], vec![d]);
    changed.min_severity = Severity::High;
    db::update_rule(&s, some, &changed, 2).await.unwrap();
    let r = db::get_rule(&s, some).await.unwrap().unwrap();
    assert_eq!((r.envs, r.min_severity), (vec![4], Severity::High));
}

#[tokio::test]
async fn reported_is_per_rule_repository_cve() {
    let s = store().await;
    let d = db::create_destination(&s, &slack("a"), 1).await.unwrap();
    let r1 = db::create_rule(&s, &rule("r1", vec![], vec![d]), 1)
        .await
        .unwrap();
    let r2 = db::create_rule(&s, &rule("r2", vec![], vec![d]), 1)
        .await
        .unwrap();
    db::mark_reported(&s, r1, "nginx", &["CVE-1".into()], 5)
        .await
        .unwrap();
    assert!(db::is_reported(&s, r1, "nginx", "CVE-1").await.unwrap());
    assert!(!db::is_reported(&s, r2, "nginx", "CVE-1").await.unwrap());
    assert!(!db::is_reported(&s, r1, "redis", "CVE-1").await.unwrap());
    assert_eq!(db::known(&s, r1, "nginx").await.unwrap(), None);
    db::set_known(&s, r1, "nginx", true).await.unwrap();
    assert_eq!(db::known(&s, r1, "nginx").await.unwrap(), Some(true));
    db::set_known(&s, r1, "nginx", false).await.unwrap();
    assert_eq!(db::known(&s, r1, "nginx").await.unwrap(), Some(false));
}

#[tokio::test]
async fn pending_merges_and_is_due_after_quiet_or_max() {
    let s = store().await;
    let d = db::create_destination(&s, &slack("a"), 1).await.unwrap();
    let r = db::create_rule(&s, &rule("r", vec![], vec![d]), 1)
        .await
        .unwrap();
    db::add_pending(
        &s,
        r,
        PendingKind::Cve,
        "CVE-9",
        &[item("nginx", "CVE-9")],
        0,
    )
    .await
    .unwrap();
    db::add_pending(
        &s,
        r,
        PendingKind::Cve,
        "CVE-9",
        &[item("redis", "CVE-9")],
        100,
    )
    .await
    .unwrap();
    assert!(
        db::due_pending(&s, 200).await.unwrap().is_empty(),
        "quiet for only 100 s"
    );
    let due = db::due_pending(&s, 220).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].items.len(), 2);
    assert_eq!((due[0].first_at, due[0].last_at), (0, 100));
    db::delete_pending(&s, r, PendingKind::Cve, "CVE-9")
        .await
        .unwrap();
    for t in (0..=600).step_by(60) {
        db::add_pending(
            &s,
            r,
            PendingKind::Image,
            "app",
            &[item("app", &format!("CVE-{t}"))],
            t,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        db::due_pending(&s, 600).await.unwrap().len(),
        1,
        "fed every 60 s: due at 600 s"
    );
}

#[tokio::test]
async fn deliveries_retry_then_fail() {
    let s = store().await;
    let d = db::create_destination(&s, &slack("a"), 1).await.unwrap();
    let id = db::enqueue_delivery(&s, d, None, &message(), 10)
        .await
        .unwrap();
    assert_eq!(db::due_deliveries(&s, 9, 10).await.unwrap().len(), 0);
    assert_eq!(db::due_deliveries(&s, 10, 10).await.unwrap()[0].id, id);
    db::delivery_retry(&s, id, "500", 130).await.unwrap();
    assert!(db::due_deliveries(&s, 129, 10).await.unwrap().is_empty());
    let again = db::due_deliveries(&s, 130, 10).await.unwrap();
    assert_eq!(again[0].attempts, 1);
    db::delivery_failed(&s, id, "500 again", 800).await.unwrap();
    let rows = db::recent_deliveries(&s, 10).await.unwrap();
    assert_eq!(rows[0].status, DeliveryStatus::Failed);
    assert_eq!(rows[0].error.as_deref(), Some("500 again"));
    assert!(db::due_deliveries(&s, 10_000, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn smtp_password_write_only() {
    let s = store().await;
    assert_eq!(db::get_smtp(&s).await.unwrap(), None);
    let row = SmtpRow {
        host: "email-smtp.eu-west-1.amazonaws.com".into(),
        port: 587,
        security: SmtpSecurity::Starttls,
        username: Some("AKIA".into()),
        from_address: "fleetops@example.com".into(),
    };
    db::put_smtp(&s, &row, PasswordChange::Set("pw".into()), 1)
        .await
        .unwrap();
    let raw: Option<String> = sqlx::query_scalar("SELECT password FROM ee_notify_smtp")
        .fetch_one(s.pool())
        .await
        .unwrap();
    assert!(raw.is_some_and(|r| r != "pw"));
    let cfg = db::get_smtp(&s).await.unwrap().unwrap();
    assert_eq!(cfg.password.as_deref(), Some("pw"));
    db::put_smtp(&s, &row, PasswordChange::Keep, 2)
        .await
        .unwrap();
    assert_eq!(
        db::get_smtp(&s).await.unwrap().unwrap().password.as_deref(),
        Some("pw")
    );
    db::put_smtp(&s, &row, PasswordChange::Clear, 3)
        .await
        .unwrap();
    assert_eq!(db::get_smtp(&s).await.unwrap().unwrap().password, None);
}
