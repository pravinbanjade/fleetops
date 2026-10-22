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
        files: vec![],
        build: None,
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
    build: { context: ., ssh: [default] }
    env_file: .env
    imgae: typo
    deploy:
      placement: {}
secrets:
  s: {}
"#,
    );
    assert!(has(&i, "services.web.build.ssh", "not supported"), "{i:?}");
    assert!(has(&i, "services.web.env_file", "not supported"), "{i:?}");
    assert!(has(&i, "services.web.imgae", "unknown"), "{i:?}");
    assert!(has(&i, "services.web.deploy.placement", "unknown"), "{i:?}");
    assert!(has(&i, "secrets.s", "needs one source"), "{i:?}");
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

#[test]
fn secrets_and_configs_parse_with_defaults() {
    let p = ok(r#"
services:
  db:
    image: postgres
    secrets:
      - db_password
      - source: api_key
        target: pw
        uid: "999"
        gid: "998"
        mode: 0400
    configs:
      - pg_conf
      - source: app_conf
        target: /etc/app.conf
        mode: "0440"
secrets:
  db_password: { file: ./pw.txt }
  api_key: { environment: API_KEY }
configs:
  pg_conf: { content: "x" }
  app_conf: { file: ./app.conf }
"#);
    let f = &p.services["db"].files;
    let get = |kind: FileKind, source: &str| {
        f.iter()
            .find(|m| m.kind == kind && m.source == source)
            .unwrap_or_else(|| panic!("{source} missing: {f:?}"))
            .clone()
    };
    let pw = get(FileKind::Secret, "db_password");
    assert_eq!(pw.target, "/run/secrets/db_password");
    assert_eq!(
        (pw.uid.as_str(), pw.gid.as_str(), pw.mode),
        ("0", "0", 0o444)
    );
    assert_eq!(pw.digest, "");
    let key = get(FileKind::Secret, "api_key");
    assert_eq!(key.target, "/run/secrets/pw");
    assert_eq!(
        (key.uid.as_str(), key.gid.as_str(), key.mode),
        ("999", "998", 0o400)
    );
    assert_eq!(get(FileKind::Config, "pg_conf").target, "/pg_conf");
    let app = get(FileKind::Config, "app_conf");
    assert_eq!((app.target.as_str(), app.mode), ("/etc/app.conf", 0o440));
}

#[test]
fn top_level_sources() {
    let p = parse(
        r#"
services:
  web: { image: x, secrets: [a, b], configs: [c, d] }
secrets:
  a: { file: ./a.txt }
  b: { environment: B }
configs:
  c: { content: "a=${X}" }
  d: { file: conf/d.conf }
"#,
        &["X=1".into()],
        "shop",
    )
    .expect("valid")
    .project;
    assert_eq!(
        p.secrets["a"],
        FileSource::File {
            path: "./a.txt".into()
        }
    );
    assert_eq!(p.secrets["b"], FileSource::Environment { var: "B".into() });
    assert_eq!(p.configs["c"], FileSource::Content { text: "a=1".into() });
    assert_eq!(
        p.configs["d"],
        FileSource::File {
            path: "conf/d.conf".into()
        }
    );
}

#[test]
fn refused_file_forms() {
    let i = issues(
        r#"
services:
  web:
    image: x
    secrets: [ext, missing, s1, { source: s2, target: /run/secrets/s1 }]
    configs: [{ source: c1, target: rel/path }]
  db:
    image: x
    volumes: ["data:/var/lib/data", "/srv/conf:/etc/conf"]
    tmpfs: /run/tmp
    configs:
      - { source: c1, target: /var/lib/data }
      - { source: c1, target: /etc/conf/x.conf }
    secrets: [{ source: s1, target: /run/tmp/s }]
  ro:
    image: x
    read_only: true
    secrets: [s1]
volumes:
  data:
secrets:
  ext: { external: true }
  named: { name: other, file: ./x }
  drv: { file: ./x, driver: foo }
  tpl: { file: ./x, template_driver: golang }
  two: { file: ./x, environment: X }
  none: {}
  sc: { content: "x" }
  s1: { file: ./s1 }
  s2: { file: ./s2 }
configs:
  c1: { content: "x" }
"#,
    );
    assert!(has(&i, "secrets.ext.external", "not supported"), "{i:?}");
    assert!(has(&i, "secrets.named.name", "not supported"), "{i:?}");
    assert!(has(&i, "secrets.drv.driver", "not supported"), "{i:?}");
    assert!(
        has(&i, "secrets.tpl.template_driver", "not supported"),
        "{i:?}"
    );
    assert!(has(&i, "secrets.two", "only one of"), "{i:?}");
    assert!(has(&i, "secrets.none", "needs one source"), "{i:?}");
    assert!(has(&i, "secrets.sc.content", "unknown"), "{i:?}");
    assert!(
        has(
            &i,
            "services.web.secrets[1]",
            "secret 'missing' is not defined under secrets"
        ),
        "{i:?}"
    );
    assert!(has(&i, "services.web.secrets[3]", "same target"), "{i:?}");
    assert!(
        has(&i, "services.web.configs[0].target", "absolute"),
        "{i:?}"
    );
    assert!(has(&i, "services.db.configs[0]", "mount"), "{i:?}");
    assert!(has(&i, "services.db.configs[1]", "mount"), "{i:?}");
    assert!(has(&i, "services.db.secrets[0]", "mount"), "{i:?}");
    assert!(has(&i, "services.ro.secrets", "read-only"), "{i:?}");
}

#[test]
fn x_keys_allowed_in_file_declarations() {
    let p = ok(r#"
services:
  web:
    image: x
    secrets: [{ source: a, x-note: hi }]
secrets:
  a: { file: ./a, x-owner: ops }
configs:
  b: { content: "x", x-owner: ops }
"#);
    assert_eq!(p.services["web"].files.len(), 1);
    assert!(p.configs.contains_key("b"));
}

#[test]
fn build_string_and_mapping_forms() {
    let p = parse(
        r#"
services:
  api:
    build: ./api
  web:
    build:
      context: ./web
      dockerfile: docker/Dockerfile
      args: { A: "${X}" }
      target: prod
  job:
    build:
      args: ["B=2"]
"#,
        &["X=1".into()],
        "shop",
    )
    .expect("valid")
    .project;
    let api = p.services["api"].build.clone().unwrap();
    assert_eq!(api.context, "./api");
    assert_eq!(p.services["api"].image, "");
    let web = p.services["web"].build.clone().unwrap();
    assert_eq!(
        (
            web.context.as_str(),
            web.dockerfile.as_deref(),
            web.target.as_deref()
        ),
        ("./web", Some("docker/Dockerfile"), Some("prod"))
    );
    assert_eq!(web.args["A"], "1");
    let job = p.services["job"].build.clone().unwrap();
    assert_eq!((job.context.as_str(), job.args["B"].as_str()), (".", "2"));
    assert_eq!(job.hash, "");
}

#[test]
fn dockerfile_inline_parses() {
    let p = ok("services:\n  web:\n    build:\n      dockerfile_inline: |\n        FROM alpine\n    pull_policy: build\n");
    let b = p.services["web"].build.clone().unwrap();
    assert_eq!(b.context, ".");
    assert_eq!(b.dockerfile_inline.as_deref(), Some("FROM alpine\n"));
    assert_eq!(p.services["web"].pull_policy, PullPolicy::Build);
}

#[test]
fn refused_build_forms() {
    let i = issues(
        r#"
services:
  a:
    build:
      ssh: [default]
      secrets: [x]
      cache_from: [y]
      additional_contexts: { c: ./c }
      platforms: [linux/arm64]
      network: host
      tags: [z]
  b:
    build: { dockerfile: D, dockerfile_inline: "FROM x" }
  c:
    build: /abs/path
  d:
    build: https://example.com/r.git
  e:
    build: .
    pull_policy: always
  f:
    build: .
    pull_policy: never
  g:
    image: x
    pull_policy: build
  h:
    restart: always
"#,
    );
    for k in [
        "ssh",
        "secrets",
        "cache_from",
        "additional_contexts",
        "platforms",
        "network",
        "tags",
    ] {
        assert!(
            has(&i, &format!("services.a.build.{k}"), "not supported"),
            "{k}: {i:?}"
        );
    }
    assert!(has(&i, "services.b.build", "only one of"), "{i:?}");
    assert!(has(&i, "services.c.build", "relative path"), "{i:?}");
    assert!(has(&i, "services.d.build", "relative path"), "{i:?}");
    assert!(has(&i, "services.e.pull_policy", "pull_policy"), "{i:?}");
    assert!(has(&i, "services.f.pull_policy", "pull_policy"), "{i:?}");
    assert!(has(&i, "services.g.pull_policy", "build"), "{i:?}");
    assert!(has(&i, "services.h.image", "image is required"), "{i:?}");
}
