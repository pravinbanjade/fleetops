use fleetops_core::compose::build::*;
use fleetops_core::compose::docker::service_hash;
use fleetops_core::compose::*;

const KEY: [u8; 32] = [5; 32];

fn project(yaml: &str, vars: &[&str]) -> Project {
    let vars: Vec<String> = vars.iter().map(|v| v.to_string()).collect();
    parse(yaml, &vars, "shop").expect("valid").project
}

fn base() -> Build {
    Build {
        context: ".".into(),
        dockerfile: Some("Dockerfile".into()),
        dockerfile_inline: None,
        args: [("A".to_string(), "1".to_string())].into(),
        target: Some("prod".into()),
        hash: String::new(),
    }
}

#[test]
fn hash_is_keyed_and_covers_every_input() {
    let h = build_hash(&KEY, "shop", "api", "tree1", &base());
    assert_eq!(h.len(), 64);
    let mut variants = vec![
        build_hash(&[6; 32], "shop", "api", "tree1", &base()),
        build_hash(&KEY, "other", "api", "tree1", &base()),
        build_hash(&KEY, "shop", "web", "tree1", &base()),
        build_hash(&KEY, "shop", "api", "tree2", &base()),
    ];
    let mut b = base();
    b.dockerfile = Some("D2".into());
    variants.push(build_hash(&KEY, "shop", "api", "tree1", &b));
    let mut b = base();
    b.dockerfile_inline = Some("FROM x".into());
    variants.push(build_hash(&KEY, "shop", "api", "tree1", &b));
    let mut b = base();
    b.args.insert("A".into(), "2".into());
    variants.push(build_hash(&KEY, "shop", "api", "tree1", &b));
    let mut b = base();
    b.target = None;
    variants.push(build_hash(&KEY, "shop", "api", "tree1", &b));
    for v in variants {
        assert_ne!(v, h);
    }
}

#[test]
fn tag_uses_image_repo_or_stack_service() {
    let h = "0123456789abcdef0123";
    assert_eq!(image_tag("shop", "api", "", h), "shop-api:0123456789ab");
    assert_eq!(
        image_tag("shop", "api", "ghcr.io/acme/api:1.2", h),
        "ghcr.io/acme/api:0123456789ab"
    );
    assert_eq!(
        image_tag("shop", "api", "localhost:5000/app@sha256:abcd", h),
        "localhost:5000/app:0123456789ab"
    );
    assert_eq!(
        image_tag("shop", "api", "localhost:5000/app", h),
        "localhost:5000/app:0123456789ab"
    );
}

#[test]
fn apply_inline_stack_rules() {
    let mut p = project("services:\n  api:\n    build: ./api\n", &[]);
    assert_eq!(
        apply(&mut p, &Trees::new(), false, &KEY),
        Err(vec![ComposeIssue::new(
            "services.api.build",
            "service api: a build context needs a git stack; use dockerfile_inline for a Dockerfile without files"
        )])
    );
    let mut p = project(
        "services:\n  web:\n    build:\n      dockerfile_inline: \"FROM alpine\"\n",
        &[],
    );
    apply(&mut p, &Trees::new(), false, &KEY).unwrap();
    let svc = &p.services["web"];
    assert!(svc.image.starts_with("shop-web:"), "{}", svc.image);
    assert_eq!(svc.image.len(), "shop-web:".len() + 12);
    assert_eq!(svc.build.as_ref().unwrap().hash.len(), 64);
}

#[test]
fn apply_git_stack_requires_trees() {
    let mut p = project(
        "services:\n  api:\n    build: ./api\n  db: { image: postgres }\n",
        &[],
    );
    let issues = apply(&mut p, &Trees::new(), true, &KEY).unwrap_err();
    assert_eq!(issues[0].path, "services.api.build.context");
    let trees = Trees::from([("api".to_string(), "t1".to_string())]);
    apply(&mut p, &trees, true, &KEY).unwrap();
    assert!(p.services["api"].image.starts_with("shop-api:"));
    assert_eq!(p.services["db"].image, "postgres");
}

#[test]
fn args_not_in_canonical_json() {
    let yaml = |v: &str| {
        format!("services:\n  api:\n    build:\n      context: ./api\n      args: {{ TOKEN: \"{v}\" }}\n")
    };
    let trees = Trees::from([("api".to_string(), "t1".to_string())]);
    let mut a = project(&yaml("Zq9-secret"), &[]);
    apply(&mut a, &trees, true, &KEY).unwrap();
    assert!(!serde_json::to_string(&a.services["api"])
        .unwrap()
        .contains("Zq9-secret"));
    let mut b = project(&yaml("other"), &[]);
    apply(&mut b, &trees, true, &KEY).unwrap();
    assert_ne!(
        service_hash(&a.services["api"]),
        service_hash(&b.services["api"])
    );
}

#[test]
fn context_dir_cases() {
    assert_eq!(context_dir("deploy/compose.yaml", "."), Ok("deploy".into()));
    assert_eq!(context_dir("compose.yaml", "."), Ok("".into()));
    assert_eq!(context_dir("deploy/c.yaml", "../app"), Ok("app".into()));
    assert_eq!(
        context_dir("deploy/c.yaml", "./api/"),
        Ok("deploy/api".into())
    );
    assert!(context_dir("deploy/c.yaml", "../../x").is_err());
    assert!(context_dir("c.yaml", "/x").is_err());
    assert!(context_dir("c.yaml", "https://h/r.git").is_err());
}
