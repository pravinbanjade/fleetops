use fleetops_core::compose::*;
use std::collections::BTreeMap;

fn ok(yaml: &str) -> Project {
    match parse(yaml, &[], "shop") {
        Ok(p) => p.project,
        Err(e) => panic!("unexpected issues: {e:?}"),
    }
}

fn issues(yaml: &str) -> Vec<ComposeIssue> {
    parse(yaml, &[], "shop").expect_err("expected issues")
}

fn has(issues: &[ComposeIssue], path: &str, needle: &str) -> bool {
    issues
        .iter()
        .any(|i| i.path == path && i.message.contains(needle))
}

#[test]
fn normalises_full_service() {
    let p = ok(r#"
services:
  web:
    image: nginx:1.27
    container_name: shop-front
    command: nginx -g "daemon off;"
    entrypoint: ["/docker-entrypoint.sh"]
    environment:
      A: "1"
      B: 2
    ports: ["8080:80"]
    volumes: ["data:/var/lib/data"]
    networks:
      front:
        aliases: [www]
        ipv4_address: 10.5.0.5
    depends_on:
      db:
        condition: service_healthy
    restart: unless-stopped
    labels: [team=web]
    healthcheck:
      test: curl -f http://localhost
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 1m30s
    user: "101"
    working_dir: /srv
    hostname: front
    extra_hosts: ["db.local:10.0.0.2"]
    dns: 1.1.1.1
    cap_add: [NET_ADMIN]
    cap_drop: [ALL]
    privileged: false
    read_only: true
    init: true
    tmpfs: /tmp
    devices: ["/dev/fuse:/dev/fuse"]
    shm_size: 64m
    stop_signal: SIGQUIT
    stop_grace_period: 20s
    logging:
      driver: json-file
      options: { max-size: 10m }
    pull_policy: always
    deploy:
      resources:
        limits: { memory: 512m, cpus: "0.5" }
  db:
    image: postgres:17
networks:
  front: {}
volumes:
  data: {}
"#);
    let web = &p.services["web"];
    let expected = Service {
        name: "web".into(),
        image: "nginx:1.27".into(),
        container_name: Some("shop-front".into()),
        command: Some(vec!["nginx".into(), "-g".into(), "daemon off;".into()]),
        entrypoint: Some(vec!["/docker-entrypoint.sh".into()]),
        environment: BTreeMap::from([("A".into(), "1".into()), ("B".into(), "2".into())]),
        ports: vec![Port {
            target: 80,
            published: Some(8080),
            host_ip: None,
            protocol: "tcp".into(),
        }],
        mounts: vec![Mount::Volume {
            source: "data".into(),
            target: "/var/lib/data".into(),
            read_only: false,
        }],
        networks: BTreeMap::from([(
            "front".into(),
            ServiceNetwork {
                aliases: vec!["www".into()],
                ipv4_address: Some("10.5.0.5".into()),
            },
        )]),
        network_mode: None,
        depends_on: BTreeMap::from([("db".into(), DependsCondition::Healthy)]),
        restart: Some("unless-stopped".into()),
        labels: BTreeMap::from([("team".into(), "web".into())]),
        healthcheck: Some(Healthcheck {
            test: vec!["CMD-SHELL".into(), "curl -f http://localhost".into()],
            interval_secs: 10,
            timeout_secs: 5,
            retries: 5,
            start_period_secs: 90,
            disable: false,
        }),
        user: Some("101".into()),
        working_dir: Some("/srv".into()),
        hostname: Some("front".into()),
        extra_hosts: vec!["db.local:10.0.0.2".into()],
        dns: vec!["1.1.1.1".into()],
        cap_add: vec!["NET_ADMIN".into()],
        cap_drop: vec!["ALL".into()],
        tmpfs: vec!["/tmp".into()],
        devices: vec!["/dev/fuse:/dev/fuse".into()],
        privileged: false,
        read_only: true,
        init: Some(true),
        shm_size: Some(64 * 1024 * 1024),
        stop_signal: Some("SIGQUIT".into()),
        stop_grace_period_secs: Some(20),
        logging: Some(Logging {
            driver: "json-file".into(),
            options: BTreeMap::from([("max-size".into(), "10m".into())]),
        }),
        memory_bytes: Some(512 * 1024 * 1024),
        nano_cpus: Some(500_000_000),
        pull_policy: PullPolicy::Always,
        replicas: 1,
    };
    assert_eq!(web, &expected);
}

#[test]
fn short_and_long_ports_and_volumes() {
    let p = ok(r#"
services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - "127.0.0.1:53:53/udp"
      - "80"
      - target: 443
        published: 8443
    volumes:
      - data:/var/lib
      - /srv:/srv:ro
      - type: tmpfs
        target: /cache
volumes:
  data:
"#);
    let web = &p.services["web"];
    assert_eq!(
        web.ports,
        vec![
            Port {
                target: 80,
                published: Some(8080),
                host_ip: None,
                protocol: "tcp".into()
            },
            Port {
                target: 53,
                published: Some(53),
                host_ip: Some("127.0.0.1".into()),
                protocol: "udp".into()
            },
            Port {
                target: 80,
                published: None,
                host_ip: None,
                protocol: "tcp".into()
            },
            Port {
                target: 443,
                published: Some(8443),
                host_ip: None,
                protocol: "tcp".into()
            },
        ]
    );
    assert_eq!(
        web.mounts,
        vec![
            Mount::Volume {
                source: "data".into(),
                target: "/var/lib".into(),
                read_only: false
            },
            Mount::Bind {
                source: "/srv".into(),
                target: "/srv".into(),
                read_only: true
            },
            Mount::Tmpfs {
                target: "/cache".into()
            },
        ]
    );
}

#[test]
fn rejects_unknown_and_unsupported_keys() {
    let i = issues(
        r#"
services:
  web:
    image: nginx
    build: .
    env_file: .env
    imgae: typo
    deploy:
      placement: {}
secrets:
  s: {}
"#,
    );
    assert!(has(&i, "services.web.build", "not supported"), "{i:?}");
    assert!(has(&i, "services.web.env_file", "not supported"), "{i:?}");
    assert!(has(&i, "services.web.imgae", "unknown"), "{i:?}");
    assert!(has(&i, "services.web.deploy.placement", "unknown"), "{i:?}");
    assert!(has(&i, "secrets", "not supported"), "{i:?}");
}

#[test]
fn collects_all_issues() {
    let i = issues(
        r#"
services:
  web:
    restart: sometimes
    ports: ["abc"]
"#,
    );
    assert!(has(&i, "services.web.image", "required"), "{i:?}");
    assert!(has(&i, "services.web.restart", "invalid restart"), "{i:?}");
    assert!(has(&i, "services.web.ports[0]", "invalid port"), "{i:?}");
    assert_eq!(i.len(), 3, "{i:?}");
}

#[test]
fn reference_errors() {
    let i = issues(
        r#"
services:
  a:
    image: x
    depends_on: [b]
    networks: [nope]
    volumes: ["missing:/data", "./data:/d"]
  b:
    image: x
    depends_on: [a, ghost]
"#,
    );
    assert!(has(&i, "services.a.networks.nope", "not defined"), "{i:?}");
    assert!(has(&i, "services.a.volumes[0]", "not defined"), "{i:?}");
    assert!(
        has(
            &i,
            "services.a.volumes[1]",
            "relative bind paths are not supported; use an absolute host path"
        ),
        "{i:?}"
    );
    assert!(
        has(&i, "services.b.depends_on.ghost", "does not exist"),
        "{i:?}"
    );
    assert!(
        has(&i, "services", "dependency cycle: a -> b -> a"),
        "{i:?}"
    );
}

#[test]
fn name_rules() {
    assert!(has(
        &issues("name: other\nservices:\n  web:\n    image: x\n"),
        "name",
        "must match"
    ));
    let i = issues(
        "services:\n  web:\n    image: x\n    container_name: w\n    deploy:\n      replicas: 2\n",
    );
    assert!(
        has(&i, "services.web.container_name", "more than one replica"),
        "{i:?}"
    );
    let bad = parse("services:\n  web:\n    image: x\n", &[], "Bad Name").unwrap_err();
    assert!(has(&bad, "name", "invalid stack name"), "{bad:?}");
    assert!(parse("name: shop\nservices:\n  web:\n    image: x\n", &[], "shop").is_ok());
}

#[test]
fn implicit_default_network() {
    let p = ok("services:\n  web:\n    image: x\n");
    assert_eq!(p.networks["default"].name, "shop_default");
    assert!(p.services["web"].networks.contains_key("default"));
    let host = ok("services:\n  web:\n    image: x\n    network_mode: host\n");
    assert!(host.networks.is_empty());
}

#[test]
fn x_keys_and_anchors_allowed() {
    let p = ok(r#"
x-common: &common
  image: busybox
  restart: always
services:
  a:
    <<: *common
  b:
    <<: *common
    x-note: hello
"#);
    assert_eq!(p.services["a"].image, "busybox");
    assert_eq!(p.services["b"].restart.as_deref(), Some("always"));
}

#[test]
fn unset_variable_in_image_warns_and_fails_empty_tag() {
    let err = parse("services:\n  web:\n    image: nginx:${TAG}\n", &[], "shop").unwrap_err();
    assert!(has(&err, "services.web.image", "empty tag"), "{err:?}");
    let ok = parse(
        "services:\n  web:\n    image: nginx:${TAG:-1.27}\n    environment:\n      - LEVEL=${LEVEL}\n",
        &[],
        "shop",
    )
    .unwrap();
    assert_eq!(ok.project.services["web"].image, "nginx:1.27");
    assert_eq!(ok.project.services["web"].environment["LEVEL"], "");
    assert!(ok
        .warnings
        .iter()
        .any(|w| w.message == "variable LEVEL is not set; using an empty string"));
}

#[test]
fn variables_are_interpolated_verbatim() {
    let p = parse(
        "services:\n  web:\n    image: x\n    environment:\n      P: ${P}\n      K:\n",
        &["P=a$b \"c\" ".into(), "K=x=y".into()],
        "shop",
    )
    .unwrap()
    .project;
    assert_eq!(p.services["web"].environment["P"], "a$b \"c\" ");
    assert_eq!(p.services["web"].environment["K"], "x=y");
}
