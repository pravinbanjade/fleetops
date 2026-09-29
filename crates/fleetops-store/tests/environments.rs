use fleetops_core::Error;
use fleetops_store::{NewAudit, Store};

#[tokio::test]
async fn crud_roundtrip() {
    let s = Store::in_memory().await.unwrap();
    let e = s
        .create_environment("local", "docker-local", "unix:///var/run/docker.sock", 10)
        .await
        .unwrap();
    assert_eq!(e.name, "local");
    assert_eq!(e.kind, "docker-local");
    assert_eq!(s.get_environment(e.id).await.unwrap(), e);

    let u = s
        .update_environment(e.id, "home", "unix:///run/podman.sock", 20)
        .await
        .unwrap();
    assert_eq!(
        (u.name.as_str(), u.url.as_str(), u.updated_at, u.created_at),
        ("home", "unix:///run/podman.sock", 20, 10)
    );
    assert_eq!(s.list_environments().await.unwrap(), vec![u.clone()]);

    s.delete_environment(e.id).await.unwrap();
    assert!(s.list_environments().await.unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_names_conflict() {
    let s = Store::in_memory().await.unwrap();
    s.create_environment("local", "docker-local", "u", 0)
        .await
        .unwrap();
    let other = s
        .create_environment("other", "docker-local", "u", 0)
        .await
        .unwrap();
    assert!(
        matches!(s.create_environment("local", "docker-local", "u", 0).await, Err(Error::Conflict(m)) if m.contains("local"))
    );
    assert!(matches!(
        s.update_environment(other.id, "local", "u", 1).await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn missing_environment_is_not_found() {
    let s = Store::in_memory().await.unwrap();
    assert!(matches!(
        s.get_environment(9).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        s.update_environment(9, "x", "u", 0).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        s.delete_environment(9).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn deleting_environment_keeps_audit_rows() {
    let s = Store::in_memory().await.unwrap();
    let e = s
        .create_environment("local", "docker-local", "u", 0)
        .await
        .unwrap();
    s.insert_audit(&NewAudit {
        ts: 1,
        user_id: None,
        environment_id: Some(e.id),
        action: "container.start",
        target_type: Some("container"),
        target_id: Some("abc"),
        result: "success",
        detail: None,
    })
    .await
    .unwrap();
    s.delete_environment(e.id).await.unwrap();
    let rows = s.list_audit(10, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].environment_id, None);
}
