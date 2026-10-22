use fleetops_core::compose::docker::*;
use fleetops_core::compose::*;

fn project(yaml: &str) -> Project {
    parse(yaml, &[], "shop").expect("valid").project
}

#[test]
fn names() {
    let p = project(
        r#"
services:
  web: { image: x }
  db: { image: y, container_name: shop-database, volumes: ["data:/d"], networks: [default, ext] }
volumes:
  data:
networks:
  ext: { external: true, name: corp-net }
"#,
    );
    assert_eq!(container_name(&p, &p.services["web"], 1), "shop-web-1");
    assert_eq!(container_name(&p, &p.services["db"], 1), "shop-database");
    assert_eq!(p.volumes["data"].name, "shop_data");
    assert_eq!(p.networks["default"].name, "shop_default");
    assert_eq!(p.networks["ext"].name, "corp-net");
}

#[test]
fn hash_stable_and_scale_insensitive() {
    let a = project("services:\n  web:\n    image: x\n    environment: {A: '1'}\n");
    let b = project("services:\n  web:\n    image: x\n    environment: {A: '1'}\n");
    let scaled = project(
        "services:\n  web:\n    image: x\n    environment: {A: '1'}\n    deploy: {replicas: 3}\n",
    );
    let changed = project("services:\n  web:\n    image: x\n    environment: {A: '2'}\n");
    let h = service_hash(&a.services["web"]);
    assert_eq!(h, service_hash(&b.services["web"]));
    assert_eq!(h, service_hash(&scaled.services["web"]));
    assert_ne!(h, service_hash(&changed.services["web"]));
    assert_eq!(h.len(), 64);
}

#[test]
fn start_order_follows_dependencies() {
    let p = project(
        r#"
services:
  web: { image: x, depends_on: [api] }
  api: { image: x, depends_on: [db] }
  db: { image: x }
  cache: { image: x }
  worker: { image: x, network_mode: "service:api" }
"#,
    );
    let order = start_order(&p);
    let pos = |n: &str| order.iter().position(|s| s == n).unwrap();
    assert!(pos("db") < pos("api") && pos("api") < pos("web"));
    assert!(pos("api") < pos("worker"));
    assert_eq!(order[0], "cache");
    assert_eq!(order.len(), 5);
}

#[test]
fn body_has_labels_and_networks() {
    let p = project(
        r#"
services:
  web:
    image: nginx:1.27
    ports: ["8080:80"]
    networks:
      default:
        aliases: [www]
"#,
    );
    let b = container_body(&p, &p.services["web"], 1, 3);
    assert_eq!(
        b.pointer("/Labels/com.docker.compose.project").unwrap(),
        "shop"
    );
    assert_eq!(
        b.pointer("/Labels/com.docker.compose.service").unwrap(),
        "web"
    );
    assert_eq!(
        b.pointer("/Labels/com.docker.compose.container-number")
            .unwrap(),
        "1"
    );
    assert_eq!(
        b.pointer("/Labels/io.fleetops.stack.revision").unwrap(),
        "3"
    );
    assert_eq!(
        b.pointer("/Labels/io.fleetops.stack.config-hash").unwrap(),
        &serde_json::json!(service_hash(&p.services["web"]))
    );
    assert_eq!(
        b.pointer("/HostConfig/PortBindings/80~1tcp/0/HostPort")
            .unwrap(),
        "8080"
    );
    assert_eq!(
        b.pointer("/HostConfig/NetworkMode").unwrap(),
        "shop_default"
    );
    assert_eq!(
        b.pointer("/NetworkingConfig/EndpointsConfig/shop_default/Aliases/0")
            .unwrap(),
        "web"
    );
    assert_eq!(
        b.pointer("/NetworkingConfig/EndpointsConfig/shop_default/Aliases/1")
            .unwrap(),
        "www"
    );
}

#[test]
fn network_and_volume_specs_carry_labels() {
    let p = project("services:\n  web: {image: x, volumes: ['data:/d']}\nvolumes:\n  data:\n");
    let n = network_create(&p, "default", &p.networks["default"], 2);
    assert_eq!(n.name, "shop_default");
    assert_eq!(n.labels[LABEL_PROJECT], "shop");
    assert_eq!(n.labels[LABEL_NETWORK], "default");
    assert_eq!(n.labels[LABEL_REVISION], "2");
    let v = volume_create(&p, "data", &p.volumes["data"], 2);
    assert_eq!(v.name, "shop_data");
    assert_eq!(v.labels[LABEL_VOLUME], "data");
}

/// The canonical JSON of a service without secrets or configs must not
/// change, or upgrading FleetOps would recreate every stack. The expected
/// hash was captured before `Service` gained `files`.
#[test]
fn hash_of_file_less_service_is_unchanged() {
    let p = project(
        r#"
services:
  web:
    image: nginx:1.27
    command: ["nginx", "-g", "daemon off;"]
    environment: {A: "1"}
    ports: ["8080:80"]
    volumes: ["data:/data"]
    labels: [team=web]
    healthcheck: { test: curl -f http://localhost, interval: 10s }
    restart: unless-stopped
volumes:
  data:
"#,
    );
    assert_eq!(
        service_hash(&p.services["web"]),
        "ef7c7a5598cc4bcf1290d2c504a4a192d5246844f79ced7a642a4cf17dcf6d03"
    );
}
