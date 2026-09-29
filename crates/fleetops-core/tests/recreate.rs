use async_trait::async_trait;
use fleetops_core::docker::{
    ContainerAction, ContainerOps, ContainerSpec, ContainerSummary, DirListing, PruneReport,
};
use fleetops_core::recreate::recreate;
use fleetops_core::{Error, Result};
use serde_json::{json, Value};
use std::sync::Mutex;

/// Records every call; `fail_on` makes the first matching call fail.
#[derive(Default)]
struct Fake {
    running: bool,
    auto_remove: bool,
    calls: Mutex<Vec<String>>,
    fail_on: Option<&'static str>,
}

impl Fake {
    fn new(running: bool, fail_on: Option<&'static str>) -> Self {
        Fake {
            running,
            fail_on,
            ..Default::default()
        }
    }

    fn record(&self, call: String) -> Result<()> {
        let fail = self.fail_on.is_some_and(|f| call.starts_with(f));
        self.calls.lock().unwrap().push(call);
        if fail {
            Err(Error::Upstream {
                status: 500,
                message: "boom".into(),
            })
        } else {
            Ok(())
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ContainerOps for Fake {
    async fn list_containers(&self) -> Result<Vec<ContainerSummary>> {
        unimplemented!()
    }
    async fn inspect_container(&self, _id: &str) -> Result<Value> {
        Ok(json!({
            "Id": "old",
            "Name": "/web",
            "State": {"Running": self.running},
            "HostConfig": {"AutoRemove": self.auto_remove}
        }))
    }
    async fn container_spec(&self, _id: &str) -> Result<ContainerSpec> {
        unimplemented!()
    }
    async fn create_container(&self, spec: &ContainerSpec) -> Result<String> {
        self.record(format!("create {}", spec.name.as_deref().unwrap_or("")))?;
        Ok("new".into())
    }
    async fn container_action(&self, id: &str, action: ContainerAction) -> Result<()> {
        self.record(format!("{} {id}", action.as_str()))
    }
    async fn rename_container(&self, id: &str, new_name: &str) -> Result<()> {
        self.record(format!("rename {id} {new_name}"))
    }
    async fn remove_container(&self, id: &str, _force: bool, _volumes: bool) -> Result<()> {
        self.record(format!("remove {id}"))
    }
    async fn prune_containers(&self) -> Result<PruneReport> {
        unimplemented!()
    }
    async fn list_dir(&self, _id: &str, _path: &str) -> Result<DirListing> {
        unimplemented!()
    }
    async fn read_file(&self, _id: &str, _path: &str, _max: u64) -> Result<Vec<u8>> {
        unimplemented!()
    }
}

fn spec() -> ContainerSpec {
    ContainerSpec {
        image: "nginx:2".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn happy_path_keeps_name_and_removes_old() {
    let f = Fake::new(true, None);
    let out = recreate(&f, "old", &spec(), 42).await.unwrap();
    assert_eq!(out.new_id, "new");
    assert!(out.old_removed);
    assert_eq!(out.backup_name, "web-fleetops-old-42");
    assert_eq!(
        f.calls(),
        vec![
            "stop old",
            "rename old web-fleetops-old-42",
            "create web",
            "start new",
            "remove old"
        ]
    );
}

#[tokio::test]
async fn stopped_container_is_not_stopped_or_restarted() {
    let f = Fake::new(false, Some("create"));
    assert!(recreate(&f, "old", &spec(), 1).await.is_err());
    assert_eq!(
        f.calls(),
        vec![
            "rename old web-fleetops-old-1",
            "create web",
            "rename old web"
        ]
    );
}

#[tokio::test]
async fn start_failure_rolls_back_and_restarts_old() {
    let f = Fake::new(true, Some("start new"));
    let err = recreate(&f, "old", &spec(), 7).await.unwrap_err();
    assert!(matches!(err, Error::Upstream { .. }));
    assert_eq!(
        f.calls(),
        vec![
            "stop old",
            "rename old web-fleetops-old-7",
            "create web",
            "start new",
            "remove new",
            "rename old web",
            "start old",
        ]
    );
}

#[tokio::test]
async fn new_name_is_used_when_given() {
    let f = Fake::new(false, None);
    let s = ContainerSpec {
        name: Some("web2".into()),
        ..spec()
    };
    recreate(&f, "old", &s, 1).await.unwrap();
    assert!(f.calls().contains(&"create web2".to_string()));
}

#[tokio::test]
async fn old_removal_failure_is_reported_not_fatal() {
    let f = Fake::new(false, Some("remove old"));
    let out = recreate(&f, "old", &spec(), 3).await.unwrap();
    assert!(!out.old_removed);
}

#[tokio::test]
async fn invalid_spec_touches_nothing() {
    let f = Fake::new(true, None);
    let bad = ContainerSpec {
        image: String::new(),
        ..Default::default()
    };
    assert!(matches!(
        recreate(&f, "old", &bad, 1).await,
        Err(Error::Invalid(_))
    ));
    assert!(f.calls().is_empty());
}

#[tokio::test]
async fn rename_failure_restarts_old() {
    let f = Fake::new(true, Some("rename"));
    assert!(recreate(&f, "old", &spec(), 5).await.is_err());
    assert_eq!(
        f.calls(),
        vec!["stop old", "rename old web-fleetops-old-5", "start old"]
    );
}

#[tokio::test]
async fn name_reference_is_resolved_to_the_canonical_id() {
    // After the rename, "web" names the replacement; every step must target
    // the original by id or the replacement gets deleted.
    let f = Fake::new(true, None);
    let out = recreate(&f, "web", &spec(), 9).await.unwrap();
    assert!(out.old_removed);
    assert_eq!(
        f.calls(),
        vec![
            "stop old",
            "rename old web-fleetops-old-9",
            "create web",
            "start new",
            "remove old"
        ]
    );
}

#[tokio::test]
async fn name_reference_rollback_targets_the_original() {
    let f = Fake::new(true, Some("start new"));
    assert!(recreate(&f, "web", &spec(), 9).await.is_err());
    assert_eq!(
        f.calls()[4..],
        ["remove new", "rename old web", "start old"]
    );
}

#[tokio::test]
async fn auto_remove_container_is_refused_untouched() {
    let f = Fake {
        running: true,
        auto_remove: true,
        ..Default::default()
    };
    assert!(matches!(
        recreate(&f, "old", &spec(), 1).await,
        Err(Error::Conflict(_))
    ));
    assert!(f.calls().is_empty());
}
