use fleetops_tunnel::control::{read_msg, write_msg, ByeReason, HostInfo, Msg, PROTO_MAJOR};
use tokio::io::{duplex, AsyncWriteExt};

#[tokio::test]
async fn messages_round_trip_in_order() {
    let (mut a, mut b) = duplex(4096);
    let sent = vec![
        Msg::Hello {
            proto: PROTO_MAJOR,
            agent_version: "0.2.0".into(),
            host: HostInfo {
                hostname: "web-01".into(),
                ..Default::default()
            },
        },
        Msg::Ping { n: 7 },
        Msg::Bye {
            reason: ByeReason::Shutdown,
        },
    ];
    for m in &sent {
        write_msg(&mut a, m).await.unwrap();
    }
    drop(a);
    for m in &sent {
        assert_eq!(&read_msg(&mut b).await.unwrap().unwrap(), m);
    }
    assert_eq!(read_msg(&mut b).await.unwrap(), None, "clean EOF");
}

#[tokio::test]
async fn unknown_fields_are_ignored_within_a_major() {
    let (mut a, mut b) = duplex(4096);
    let body = br#"{"type":"pong","n":3,"added_in_1_1":true}"#;
    a.write_all(&(body.len() as u32).to_be_bytes())
        .await
        .unwrap();
    a.write_all(body).await.unwrap();
    assert_eq!(read_msg(&mut b).await.unwrap(), Some(Msg::Pong { n: 3 }));
}

#[tokio::test]
async fn oversized_frames_are_refused() {
    let (mut a, mut b) = duplex(4096);
    a.write_all(&(1u32 << 20).to_be_bytes()).await.unwrap();
    assert!(read_msg(&mut b).await.is_err());
}

#[test]
fn wire_names_are_snake_case() {
    let v = serde_json::to_value(Msg::Bye {
        reason: ByeReason::Incompatible,
    })
    .unwrap();
    assert_eq!(
        v,
        serde_json::json!({"type": "bye", "reason": "incompatible"})
    );
}

#[test]
fn a_hello_from_an_older_agent_serves_docker() {
    use fleetops_tunnel::control::{HostInfo, Serves};
    let old =
        r#"{"hostname":"h","os":"linux","arch":"x86_64","docker":"unix:///var/run/docker.sock"}"#;
    let h: HostInfo = serde_json::from_str(old).unwrap();
    assert_eq!(h.serves, Serves::Docker);
    let kube = HostInfo {
        serves: Serves::Kube,
        ..HostInfo::default()
    };
    assert_eq!(serde_json::to_value(&kube).unwrap()["serves"], "kube");
}
