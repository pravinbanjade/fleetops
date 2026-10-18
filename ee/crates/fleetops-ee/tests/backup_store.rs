use fleetops_ee::backup::{
    db::{self, Retention, S3Secrets, Schedule, TargetSettings},
    hooks,
};
use fleetops_ee::sso::{
    db as sso_db,
    model::{ConnectionSettings, Secrets},
};
use fleetops_store::{secrets::SecretBox, Store};
use std::path::PathBuf;

async fn store() -> Store {
    let store = Store::in_memory().await.unwrap();
    fleetops_ee::db::migrate(store.pool()).await.unwrap();
    store
}

fn s3() -> TargetSettings {
    TargetSettings::S3 {
        endpoint: Some("http://127.0.0.1:9100".into()),
        region: "us-east-1".into(),
        bucket: "b".into(),
        prefix: "".into(),
        path_style: true,
        access_key_id: "AK".into(),
    }
}

/// Every sealed EE value, written by its own module, opens with the
/// column's AAD rule.
#[tokio::test]
async fn hooks_list_every_sealed_ee_column() {
    let store = store().await;
    let settings: ConnectionSettings = serde_json::from_value(serde_json::json!({
        "kind": "oidc", "issuer": "https://idp", "client_id": "c"
    }))
    .unwrap();
    let secrets = Secrets {
        client_secret: Some("cs".into()),
        ..Default::default()
    };
    sso_db::create(
        &store,
        &sso_db::NewConnection {
            name: "idp",
            settings: &settings,
            secrets: &secrets,
            enabled: false,
            require_mapped_group: false,
            session_hours: 12,
            now: 0,
        },
    )
    .await
    .unwrap();
    db::set_passphrase(&store, "correct horse battery", 0)
        .await
        .unwrap();
    db::upsert_target(
        &store,
        &s3(),
        Some(&S3Secrets {
            secret_access_key: "SK".into(),
        }),
        0,
    )
    .await
    .unwrap();
    let cols = hooks::sealed();
    assert_eq!(cols.len(), 3);
    let boxed: &SecretBox = store.secrets();
    for col in cols {
        let rows: Vec<(i64, String)> = sqlx::query_as(&format!(
            "SELECT {}, {} FROM {} WHERE {} IS NOT NULL",
            col.id_column, col.column, col.table, col.column
        ))
        .fetch_all(store.pool())
        .await
        .unwrap();
        assert_eq!(rows.len(), 1, "{}", col.table);
        for (id, sealed) in rows {
            assert!(
                boxed.open(&col.aad_for(id), &sealed).is_ok(),
                "{}",
                col.table
            );
        }
    }
}

#[tokio::test]
async fn schedule_defaults() {
    let store = store().await;
    let s = db::get_schedule(store.pool()).await.unwrap();
    assert_eq!(
        s,
        Schedule {
            enabled: false,
            hour_utc: 2,
            retention: Retention::Count(7),
            passphrase_set: false
        }
    );
    db::put_schedule(
        store.pool(),
        &Schedule {
            enabled: true,
            hour_utc: 23,
            retention: Retention::Days(30),
            passphrase_set: false,
        },
        1,
    )
    .await
    .unwrap();
    db::set_passphrase(&store, "correct horse battery", 2)
        .await
        .unwrap();
    let s = db::get_schedule(store.pool()).await.unwrap();
    assert_eq!(
        (s.hour_utc, s.retention, s.passphrase_set),
        (23, Retention::Days(30), true)
    );
    assert_eq!(
        db::passphrase(&store).await.unwrap().as_deref(),
        Some("correct horse battery")
    );
}

#[tokio::test]
async fn one_target_per_kind() {
    let store = store().await;
    let local = TargetSettings::Local {
        path: PathBuf::from("/backups"),
    };
    let a = db::upsert_target(&store, &local, None, 0).await.unwrap();
    let b = db::upsert_target(
        &store,
        &TargetSettings::Local {
            path: PathBuf::from("/other"),
        },
        None,
        1,
    )
    .await
    .unwrap();
    assert_eq!(a.id, b.id);
    db::upsert_target(
        &store,
        &s3(),
        Some(&S3Secrets {
            secret_access_key: "SK".into(),
        }),
        0,
    )
    .await
    .unwrap();
    // Saving without secrets keeps the saved ones.
    let t = db::upsert_target(&store, &s3(), None, 2).await.unwrap();
    assert!(t.secret_set);
    assert_eq!(
        db::target_secrets(&store, t.id)
            .await
            .unwrap()
            .unwrap()
            .secret_access_key,
        "SK"
    );
    assert_eq!(db::list_targets(store.pool()).await.unwrap().len(), 2);
    assert!(db::delete_target(store.pool(), db::TargetKind::Local)
        .await
        .unwrap());
    assert_eq!(db::list_targets(store.pool()).await.unwrap().len(), 1);
}
