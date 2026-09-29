use dockerer_core::Role;
use dockerer_store::Store;

#[tokio::test]
async fn file_store_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dockerer.db");
    {
        let s = Store::open(&path).await.unwrap();
        s.create_user("alice", "h", Role::Admin, 0).await.unwrap();
    }
    let s = Store::open(&path).await.unwrap();
    assert_eq!(s.count_users().await.unwrap(), 1);
}
