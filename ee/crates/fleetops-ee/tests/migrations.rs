use fleetops_store::Store;

#[tokio::test]
async fn community_store_opens_a_database_enterprise_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fleetops.db");
    let store = Store::open(&path).await.unwrap();
    fleetops_ee::db::migrate(store.pool()).await.unwrap();
    fleetops_ee::db::put(store.pool(), "x.y", 1, 1)
        .await
        .unwrap();
    drop(store);
    // A Community binary: its migrator meets version 1000000 it does not know.
    let again = Store::open(&path).await.unwrap();
    // And Enterprise again, meeting the Community versions.
    fleetops_ee::db::migrate(again.pool()).await.unwrap();
    assert_eq!(
        fleetops_ee::db::get(again.pool()).await.unwrap().as_deref(),
        Some("x.y")
    );
}
