use fleetops_core::compose::files::resolve;
use fleetops_core::compose::swarm::*;
use fleetops_core::compose::*;
use serde_json::{json, Value};

fn project() -> Project {
    let vars = sample_variables();
    let mut p = parse_mode(SAMPLE_STACK, &vars, "shop", Mode::Swarm)
        .expect("sample parses")
        .project;
    resolve(&mut p, &vars, &Default::default(), &[7; 32]).expect("resolves");
    p
}

#[test]
fn names_follow_docker_stack_deploy() {
    let p = project();
    assert_eq!(service_name(&p, "web"), "shop_web");
    let secret = object_name(&p, FileKind::Secret, "db");
    let digest = &p.services["web"]
        .files
        .iter()
        .find(|f| f.source == "db")
        .unwrap()
        .digest;
    assert_eq!(secret, format!("shop_db_{}", &digest[..10]));
    assert!(object_name(&p, FileKind::Config, "site").starts_with("shop_site_"));
}

#[test]
fn external_objects_keep_their_name() {
    let mut p = parse_mode(
        "services:\n  web:\n    image: x\n    secrets: [tls]\nsecrets:\n  tls: { external: true, name: shared_tls }\n",
        &[],
        "shop",
        Mode::Swarm,
    )
    .unwrap()
    .project;
    resolve(&mut p, &[], &Default::default(), &[7; 32]).unwrap();
    assert_eq!(object_name(&p, FileKind::Secret, "tls"), "shared_tls");
    let spec = service_spec(&p, "web");
    assert_eq!(
        spec["TaskTemplate"]["ContainerSpec"]["Secrets"][0]["SecretName"],
        "shared_tls"
    );
}

#[test]
fn renders_a_full_service() {
    let p = project();
    let spec = service_spec(&p, "web");
    let secret = object_name(&p, FileKind::Secret, "db");
    let config = object_name(&p, FileKind::Config, "site");
    let s = 1_000_000_000i64;
    let mut expected = json!({
        "Name": "shop_web",
        "Labels": {
            "team": "web",
            "com.docker.stack.namespace": "shop",
            "com.docker.stack.image": "nginx:1.27",
        },
        "TaskTemplate": {
            "ContainerSpec": {
                "Image": "nginx:1.27",
                "Labels": {"tier": "front", "com.docker.stack.namespace": "shop"},
                "Command": ["/docker-entrypoint.sh"],
                "Args": ["nginx", "-g", "daemon off;"],
                "Hostname": "web",
                "Env": ["DB_PASSWORD=hunter2", "MODE=prod"],
                "Dir": "/srv",
                "User": "101",
                "ReadOnly": true,
                "Init": true,
                "Mounts": [
                    {"Type": "volume", "Source": "shop_data", "Target": "/data",
                     "VolumeOptions": {"Labels": {"com.docker.stack.namespace": "shop"},
                                       "DriverConfig": {"Name": "local", "Options": {"type": "none"}}}},
                    {"Type": "bind", "Source": "/srv/static", "Target": "/static", "ReadOnly": true},
                    {"Type": "volume", "Target": "/cache"},
                ],
                "StopSignal": "SIGQUIT",
                "StopGracePeriod": 20 * s,
                "HealthCheck": {"Test": ["CMD", "true"], "Interval": 10 * s, "Timeout": 2 * s,
                                "Retries": 3, "StartPeriod": 5 * s},
                "Hosts": ["10.0.0.2 db.local"],
                "DNSConfig": {"Nameservers": ["1.1.1.1"]},
                "CapabilityAdd": ["NET_ADMIN"],
                "CapabilityDrop": ["ALL"],
                "Secrets": [{"File": {"Name": "/run/secrets/db", "UID": "0", "GID": "0", "Mode": 0o444},
                             "SecretName": secret}],
                "Configs": [{"File": {"Name": "/etc/nginx/conf.d/site.conf", "UID": "0", "GID": "0", "Mode": 0o440},
                             "ConfigName": config}],
            },
            "Resources": {
                "Limits": {"NanoCPUs": 500_000_000, "MemoryBytes": 128 * 1024 * 1024, "Pids": 100},
                "Reservations": {"NanoCPUs": 250_000_000, "MemoryBytes": 64 * 1024 * 1024},
            },
            "RestartPolicy": {"Condition": "on-failure", "Delay": 5 * s, "MaxAttempts": 3, "Window": 120 * s},
            "Placement": {
                "Constraints": ["node.role == worker"],
                "Preferences": [{"Spread": {"SpreadDescriptor": "node.labels.zone"}}],
                "MaxReplicas": 2,
            },
            "Networks": [{"Target": "shop_front", "Aliases": ["web", "www"]}],
            "LogDriver": {"Name": "json-file", "Options": {"max-size": "10m"}},
        },
        "Mode": {"Replicated": {"Replicas": 3}},
        "UpdateConfig": {"Parallelism": 2, "Delay": 10 * s, "FailureAction": "rollback",
                         "Monitor": 30 * s, "MaxFailureRatio": 0.1, "Order": "start-first"},
        "RollbackConfig": {"Parallelism": 1, "FailureAction": "pause"},
        "EndpointSpec": {
            "Mode": "vip",
            "Ports": [
                {"Protocol": "tcp", "TargetPort": 80, "PublishedPort": 8080, "PublishMode": "ingress"},
                {"Protocol": "tcp", "TargetPort": 443, "PublishedPort": 8443, "PublishMode": "host"},
            ],
        },
    });
    let hash = spec_hash(&expected);
    expected["Labels"][LABEL_SPEC_HASH] = json!(hash);
    assert_eq!(spec, expected);
}

#[test]
fn modes_render_as_docker_names_them() {
    for (mode, want) in [
        ("global", json!({"Global": {}})),
        (
            "replicated-job",
            json!({"ReplicatedJob": {"TotalCompletions": 1}}),
        ),
        ("global-job", json!({"GlobalJob": {}})),
    ] {
        let mut p = parse_mode(
            &format!("services:\n  web:\n    image: x\n    deploy:\n      mode: {mode}\n"),
            &[],
            "shop",
            Mode::Swarm,
        )
        .unwrap()
        .project;
        resolve(&mut p, &[], &Default::default(), &[7; 32]).unwrap();
        let spec = service_spec(&p, "web");
        assert_eq!(spec["Mode"], want, "{mode}");
        // An empty Global still tells global from replicated.
        assert_eq!(canonical(&spec)["Mode"], want, "{mode}");
    }
}

#[test]
fn the_label_holds_the_hash_of_the_rest() {
    let p = project();
    let spec = service_spec(&p, "web");
    assert_eq!(spec["Labels"][LABEL_SPEC_HASH], json!(spec_hash(&spec)));
    let mut other = spec.clone();
    other["TaskTemplate"]["ContainerSpec"]["Env"] = json!(["MODE=dev"]);
    assert_ne!(spec_hash(&other), spec_hash(&spec));
}

#[test]
fn canonical_drops_docker_defaults() {
    let live = json!({
        "Name": "a",
        "Labels": {LABEL_SPEC_HASH: "x"},
        "TaskTemplate": {
            "ContainerSpec": {"Image": "nginx:1.27@sha256:abc", "Isolation": "default", "DNSConfig": {},
                              "Secrets": [{"SecretID": "id1", "SecretName": "s", "File": {"Name": "s"}}]},
            "Resources": {"Limits": {}, "Reservations": {}},
            "ForceUpdate": 0,
            "Runtime": "container",
        },
        "Mode": {"Replicated": {"Replicas": 0}},
        "EndpointSpec": {"Mode": "vip", "Ports": [{"Protocol": "tcp", "TargetPort": 80, "PublishMode": "ingress"}]},
    });
    assert_eq!(
        canonical(&live),
        json!({
            "Name": "a",
            "TaskTemplate": {"ContainerSpec": {"Image": "nginx:1.27",
                                               "Secrets": [{"SecretName": "s", "File": {"Name": "s"}}]}},
            "Mode": {"Replicated": {"Replicas": 0}},
            "EndpointSpec": {"Ports": [{"TargetPort": 80}]},
        })
    );
}

/// A rendered spec as Docker 29 hands it back: its defaults filled in, the
/// image pinned, networks and secrets by id.
#[test]
fn a_live_spec_with_docker_defaults_has_no_drift() {
    let p = project();
    let spec = service_spec(&p, "web");
    let fixture: Value = serde_json::from_str(include_str!("fixtures/swarm_service_inspect.json"))
        .expect("fixture is JSON");
    let mut live = fixture["Spec"].clone();
    // The fixture was written against the sample with these names.
    live["Labels"][LABEL_SPEC_HASH] = spec["Labels"][LABEL_SPEC_HASH].clone();
    let cs = &mut live["TaskTemplate"]["ContainerSpec"];
    cs["Secrets"][0]["SecretName"] =
        spec["TaskTemplate"]["ContainerSpec"]["Secrets"][0]["SecretName"].clone();
    cs["Configs"][0]["ConfigName"] =
        spec["TaskTemplate"]["ContainerSpec"]["Configs"][0]["ConfigName"].clone();
    let live = named(
        &live,
        &[("netid1".to_string(), "shop_front".to_string())].into(),
    );
    assert_eq!(
        spec_changes(&canonical(&spec), &canonical(&live)),
        Vec::<String>::new()
    );
    assert_eq!(
        spec_hash(&live),
        spec["Labels"][LABEL_SPEC_HASH].as_str().unwrap()
    );
}

#[test]
fn spec_changes_hide_values() {
    let p = project();
    let old = canonical(&service_spec(&p, "web"));
    let mut new = old.clone();
    new["TaskTemplate"]["ContainerSpec"]["Env"] =
        json!(["DB_PASSWORD=other", "MODE=prod", "NEW=1"]);
    new["TaskTemplate"]["ContainerSpec"]["Image"] = json!("nginx:1.28");
    new["Labels"]["com.docker.stack.image"] = json!("nginx:1.28");
    new["Mode"]["Replicated"]["Replicas"] = json!(7);
    new["TaskTemplate"]["Placement"]["Constraints"] = json!(["node.role == manager"]);
    new["TaskTemplate"]["ContainerSpec"]["Labels"]["tier"] = json!("back");
    let changes = spec_changes(&old, &new);
    assert_eq!(
        changes,
        vec![
            "env DB_PASSWORD changed",
            "env NEW added",
            "image: nginx:1.27 → nginx:1.28",
            "label tier changed",
            "replicas: 3 → 7",
            "TaskTemplate.Placement.Constraints changed",
        ]
    );
    assert!(!changes
        .iter()
        .any(|c| c.contains("other") || c.contains("hunter2")));
}
