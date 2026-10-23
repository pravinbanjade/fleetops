use fleetops_store::{KeyMatch, Store, SwarmAgentRecord};

const T0: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

async fn swarm(s: &Store) -> i64 {
    s.create_swarm_environment("prod-swarm", "key-1", T0)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn a_swarm_environment_is_enrolled_from_creation() {
    let s = Store::in_memory().await.unwrap();
    let id = swarm(&s).await;
    let env = s.get_environment(id).await.unwrap();
    assert_eq!(env.kind, "swarm-agent");
    assert_eq!(env.url, format!("agent://{id}"));
    let a = s.get_agent(id).await.unwrap().unwrap();
    assert!(a.enrolled);
    assert_eq!(a.key_generation, 1);
    assert_eq!(a.join_expires_at, None);
    assert_eq!(
        s.swarm_by_key("key-1", T0).await.unwrap(),
        Some((id, KeyMatch::Current))
    );
    // Docker-agent lookups never match a swarm key's previous generation.
    assert_eq!(s.agent_by_key("key-1").await.unwrap(), Some(id));
}

#[tokio::test]
async fn the_previous_key_works_until_it_expires_or_is_finished() {
    let s = Store::in_memory().await.unwrap();
    let id = swarm(&s).await;
    let (generation, dropped) = s.rotate_swarm_key(id, "key-2", T0 + DAY).await.unwrap();
    assert_eq!(generation, 2);
    assert_eq!(dropped, None);
    assert_eq!(
        s.swarm_by_key("key-2", T0).await.unwrap(),
        Some((id, KeyMatch::Current))
    );
    assert_eq!(
        s.swarm_by_key("key-1", T0).await.unwrap(),
        Some((id, KeyMatch::Previous))
    );
    assert_eq!(s.swarm_by_key("key-1", T0 + DAY).await.unwrap(), None);
    assert_eq!(s.agent_by_key("key-1").await.unwrap(), None);

    // Rotating again while one is pending drops key-1 for good.
    let (_, dropped) = s.rotate_swarm_key(id, "key-3", T0 + DAY).await.unwrap();
    assert_eq!(dropped.as_deref(), Some("key-1"));
    assert_eq!(s.swarm_by_key("key-1", T0).await.unwrap(), None);
    assert_eq!(
        s.finish_swarm_rotation(id).await.unwrap().as_deref(),
        Some("key-2")
    );
    assert_eq!(s.swarm_by_key("key-2", T0).await.unwrap(), None);
    assert_eq!(s.finish_swarm_rotation(id).await.unwrap(), None);
}

#[tokio::test]
async fn pinning_keeps_the_first_cluster() {
    let s = Store::in_memory().await.unwrap();
    let id = swarm(&s).await;
    assert!(s.get_swarm_cluster(id).await.unwrap().is_none());
    let first = s.pin_swarm_cluster(id, "c1", "ROOT1", T0).await.unwrap();
    assert_eq!(first.cluster_id, "c1");
    let again = s
        .pin_swarm_cluster(id, "c2", "ROOT2", T0 + 1)
        .await
        .unwrap();
    assert_eq!(again.cluster_id, "c1");
    assert_eq!(again.trust_root, "ROOT1");
    assert_eq!(again.pinned_at, T0);
}

#[tokio::test]
async fn clearing_allows_a_new_pin() {
    let s = Store::in_memory().await.unwrap();
    let id = swarm(&s).await;
    s.pin_swarm_cluster(id, "c1", "ROOT1", T0).await.unwrap();
    s.clear_swarm_cluster(id).await.unwrap();
    let pinned = s
        .pin_swarm_cluster(id, "c2", "ROOT2", T0 + 1)
        .await
        .unwrap();
    assert_eq!(pinned.cluster_id, "c2");
}

#[tokio::test]
async fn swarm_agents_upsert_per_node() {
    let s = Store::in_memory().await.unwrap();
    let id = swarm(&s).await;
    let rec = |node: &str, version: Option<&str>, err: Option<&str>| SwarmAgentRecord {
        node_id: node.into(),
        hostname: version.map(|_| format!("host-{node}")),
        manager: node == "n1",
        agent_version: version.map(str::to_string),
        last_seen_at: Some(T0),
        last_error: err.map(str::to_string),
    };
    s.record_swarm_agent(id, &rec("n1", Some("0.7.0"), None))
        .await
        .unwrap();
    s.record_swarm_agent(id, &rec("n2", Some("0.7.0"), None))
        .await
        .unwrap();
    // A disconnect: no version or host given keeps them; the error is written.
    s.record_swarm_agent(id, &rec("n1", None, Some("connection lost")))
        .await
        .unwrap();
    let all = s.list_swarm_agents(id).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].node_id, "n1");
    assert_eq!(all[0].agent_version.as_deref(), Some("0.7.0"));
    assert_eq!(all[0].hostname.as_deref(), Some("host-n1"));
    assert!(all[0].manager);
    assert_eq!(all[0].last_error.as_deref(), Some("connection lost"));
    assert_eq!(all[1].last_error, None);
}

#[tokio::test]
async fn deleting_the_environment_removes_its_rows() {
    let s = Store::in_memory().await.unwrap();
    let id = swarm(&s).await;
    s.pin_swarm_cluster(id, "c1", "ROOT1", T0).await.unwrap();
    s.record_swarm_agent(
        id,
        &SwarmAgentRecord {
            node_id: "n1".into(),
            hostname: None,
            manager: true,
            agent_version: None,
            last_seen_at: None,
            last_error: None,
        },
    )
    .await
    .unwrap();
    s.delete_environment(id).await.unwrap();
    assert!(s.get_swarm_cluster(id).await.unwrap().is_none());
    assert!(s.list_swarm_agents(id).await.unwrap().is_empty());
    assert_eq!(s.swarm_by_key("key-1", T0).await.unwrap(), None);
}
