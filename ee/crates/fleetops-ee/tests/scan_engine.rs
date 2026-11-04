mod common;

use async_trait::async_trait;
use common::*;
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Error, Result};
use fleetops_ee::scan::{
    config::{Effective, EnvOverride, GlobalConfig},
    db::{self, PasswordChange, Status},
    engine::ScannerStatus,
    inventory::Inventory,
    model::{ImageKey, ImageUse, ScanTarget},
    runner::Runner,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const REPORT: &str = r#"{"Results": [{"Vulnerabilities": [
  {"VulnerabilityID": "CVE-1", "PkgName": "libssl3", "InstalledVersion": "3.0.11",
   "FixedVersion": "3.0.14", "Severity": "CRITICAL"}]}]}"#;

#[derive(Default)]
struct FakeRunner {
    fail_prepare: Mutex<HashSet<String>>,
    fail_scan: Mutex<bool>,
    delay: Mutex<Duration>,
    prepares: Mutex<Vec<String>>,
    scans: Mutex<Vec<(String, String)>>,
    now_by_env: Mutex<HashMap<String, usize>>,
    max_by_env: Mutex<HashMap<String, usize>>,
    now_all: Mutex<usize>,
    max_all: Mutex<usize>,
}

#[async_trait]
impl Runner for FakeRunner {
    async fn prepare(
        &self,
        env: &Arc<dyn Environment>,
        _node: &str,
        _cfg: &Effective,
    ) -> Result<()> {
        self.prepares.lock().unwrap().push(env.name().to_string());
        if self.fail_prepare.lock().unwrap().contains(env.name()) {
            return Err(Error::Invalid("pulling aquasec/trivy: pull failed".into()));
        }
        Ok(())
    }

    async fn scan(
        &self,
        env: &Arc<dyn Environment>,
        t: &ScanTarget,
        _cfg: &Effective,
        _id: i64,
    ) -> Result<Vec<u8>> {
        let name = env.name().to_string();
        {
            let mut n = self.now_by_env.lock().unwrap();
            let c = n.entry(name.clone()).or_default();
            *c += 1;
            let mut m = self.max_by_env.lock().unwrap();
            let mx = m.entry(name.clone()).or_default();
            *mx = (*mx).max(*c);
            let mut all = self.now_all.lock().unwrap();
            *all += 1;
            let mut ma = self.max_all.lock().unwrap();
            *ma = (*ma).max(*all);
        }
        self.scans
            .lock()
            .unwrap()
            .push((name.clone(), t.key.as_str().to_string()));
        let delay = *self.delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        *self.now_by_env.lock().unwrap().get_mut(&name).unwrap() -= 1;
        *self.now_all.lock().unwrap() -= 1;
        if *self.fail_scan.lock().unwrap() {
            return Err(Error::Internal("trivy exited 1: boom".into()));
        }
        Ok(REPORT.as_bytes().to_vec())
    }

    async fn cleanup_orphans(&self, _env: &Arc<dyn Environment>) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeInventory {
    uses: Mutex<HashMap<i64, Vec<ImageUse>>>,
}

#[async_trait]
impl Inventory for FakeInventory {
    async fn uses(&self, env: &Arc<dyn Environment>) -> Result<Vec<ImageUse>> {
        Ok(self
            .uses
            .lock()
            .unwrap()
            .get(&env.id())
            .cloned()
            .unwrap_or_default())
    }
}

fn running(env_id: i64, key: &str, image: &str) -> ImageUse {
    ImageUse {
        env_id,
        node: String::new(),
        key: ImageKey(key.into()),
        image_ref: image.into(),
        image_id: Some(format!("cfg-{key}")),
        namespace: String::new(),
        workload: String::new(),
        container: format!("c-{key}"),
        running: true,
        pull_secrets: vec![],
        platform: None,
    }
}

struct Fx {
    ee: Ee,
    runner: Arc<FakeRunner>,
    inv: Arc<FakeInventory>,
    envs: StubEnvs,
}

async fn fx(licensed: bool) -> Fx {
    let k = key();
    let runner = Arc::new(FakeRunner::default());
    let inv = Arc::new(FakeInventory::default());
    let envs = StubEnvs::default();
    let ee = ee_scan_app(&k, envs.clone(), (runner.clone(), inv.clone())).await;
    if licensed {
        let admin = ee.admin().await;
        let r = ee
            .install(&admin, &license(&k, now_unix() + 86_400, None, &[]))
            .await;
        assert!(r.status.is_success(), "{}", r.text);
    }
    Fx {
        ee,
        runner,
        inv,
        envs,
    }
}

impl Fx {
    async fn enable(&self) {
        let g = GlobalConfig {
            enabled: true,
            mirror: None,
        };
        db::put_global(&self.ee.store, &g, PasswordChange::Keep, 0)
            .await
            .unwrap();
    }

    fn set_uses(&self, env_id: i64, uses: Vec<ImageUse>) {
        self.inv.uses.lock().unwrap().insert(env_id, uses);
    }

    /// Ticks at `now` until preparing, inventories and scans have settled.
    async fn run(&self, now: i64) {
        for _ in 0..4 {
            self.ee.scan.tick(&self.ee.state, now).await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.ee.scan.settle().await;
        }
    }

    fn scans(&self) -> Vec<(String, String)> {
        self.runner.scans.lock().unwrap().clone()
    }

    async fn state(&self, key: &str) -> db::KeyState {
        db::latest(&self.ee.store, &[ImageKey(key.into())])
            .await
            .unwrap()[&ImageKey(key.into())]
            .clone()
    }
}

#[tokio::test]
async fn off_by_default_makes_no_runner_calls() {
    let f = fx(true).await;
    let id = f.ee.env("local", "docker-local").await;
    f.set_uses(id, vec![running(id, "sha256:a", "nginx:1")]);
    f.run(now_unix()).await;
    assert!(f.runner.prepares.lock().unwrap().is_empty());
    assert!(f.scans().is_empty());
    assert_eq!(f.ee.scan.status(id), ScannerStatus::Off);
    assert!(
        db::uses_for_envs(&f.ee.store, &[id])
            .await
            .unwrap()
            .is_empty(),
        "no inventory either"
    );
}

#[tokio::test]
async fn lapsed_license_stops_new_scans() {
    let f = fx(false).await;
    let id = f.ee.env("local", "docker-local").await;
    f.enable().await;
    f.set_uses(id, vec![running(id, "sha256:a", "nginx:1")]);
    f.run(now_unix()).await;
    assert!(f.runner.prepares.lock().unwrap().is_empty());
    assert!(f.scans().is_empty());
}

#[tokio::test]
async fn enable_prepares_then_scans_every_image() {
    let f = fx(true).await;
    let id = f.ee.env("local", "docker-local").await;
    f.set_uses(
        id,
        vec![
            running(id, "sha256:a", "nginx:1"),
            running(id, "sha256:b", "redis:7"),
        ],
    );
    f.enable().await;
    f.run(now_unix()).await;
    assert_eq!(*f.runner.prepares.lock().unwrap(), ["local"]);
    assert_eq!(f.ee.scan.status(id), ScannerStatus::Ready);
    let mut keys: Vec<_> = f.scans().into_iter().map(|s| s.1).collect();
    keys.sort();
    assert_eq!(keys, ["sha256:a", "sha256:b"]);
    let st = f.state("sha256:a").await;
    assert_eq!(st.last.status, Status::Done);
    assert_eq!(st.last_done.unwrap().counts.unwrap().critical_fixable, 1);
}

#[tokio::test]
async fn same_digest_scanned_once() {
    let f = fx(true).await;
    let a = f.ee.env("a", "docker-local").await;
    let b = f.ee.env("b", "docker-agent").await;
    f.set_uses(a, vec![running(a, "sha256:same", "nginx:1")]);
    f.set_uses(b, vec![running(b, "sha256:same", "nginx:stable")]);
    f.enable().await;
    f.run(now_unix()).await;
    assert_eq!(f.scans().len(), 1, "{:?}", f.scans());
}

#[tokio::test]
async fn moved_tag_triggers_new_scan() {
    let f = fx(true).await;
    let id = f.ee.env("local", "docker-local").await;
    f.set_uses(id, vec![running(id, "sha256:old", "app:latest")]);
    f.enable().await;
    let now = now_unix();
    f.run(now).await;
    f.set_uses(id, vec![running(id, "sha256:new", "app:latest")]);
    f.run(now + 601).await;
    let keys: Vec<_> = f.scans().into_iter().map(|s| s.1).collect();
    assert_eq!(keys, ["sha256:old", "sha256:new"]);
}

#[tokio::test]
async fn one_scan_per_host_at_a_time() {
    let f = fx(true).await;
    *f.runner.delay.lock().unwrap() = Duration::from_millis(150);
    let a = f.ee.env("a", "docker-local").await;
    let b = f.ee.env("b", "docker-agent").await;
    f.set_uses(
        a,
        vec![running(a, "sha256:1", "x:1"), running(a, "sha256:2", "x:2")],
    );
    f.set_uses(b, vec![running(b, "sha256:3", "y:1")]);
    f.enable().await;
    f.run(now_unix()).await;
    f.run(now_unix()).await;
    assert_eq!(f.scans().len(), 3, "{:?}", f.scans());
    let max = f.runner.max_by_env.lock().unwrap().clone();
    assert_eq!(max["a"], 1);
    assert_eq!(
        *f.runner.max_all.lock().unwrap(),
        2,
        "two hosts in parallel"
    );
}

#[tokio::test]
async fn offline_env_fails_scan_without_blocking_queue() {
    let f = fx(true).await;
    let a = f.ee.env("a", "docker-local").await;
    let b = f.ee.env("b", "docker-agent").await;
    f.set_uses(a, vec![running(a, "sha256:down", "x:1")]);
    f.set_uses(b, vec![running(b, "sha256:up", "y:1")]);
    f.envs.down.lock().unwrap().insert("a".into());
    f.enable().await;
    f.run(now_unix()).await;
    let down = f.state("sha256:down").await;
    assert_eq!(down.last.status, Status::Failed);
    assert!(down.last.error.unwrap().contains("environment unreachable"));
    assert_eq!(f.state("sha256:up").await.last.status, Status::Done);
    assert_eq!(f.scans(), [("b".to_string(), "sha256:up".to_string())]);
    assert!(f
        .ee
        .actions()
        .await
        .contains(&"security.scan.failed".to_string()));
}

#[tokio::test]
async fn unavailable_scanner_never_reads_clean() {
    let f = fx(true).await;
    let id = f.ee.env("airgap", "docker-local").await;
    f.runner
        .fail_prepare
        .lock()
        .unwrap()
        .insert("airgap".into());
    f.set_uses(id, vec![running(id, "sha256:a", "nginx:1")]);
    f.enable().await;
    f.run(now_unix()).await;
    match f.ee.scan.status(id) {
        ScannerStatus::Unavailable { reason } => {
            assert!(reason.contains("pull failed"), "{reason}")
        }
        s => panic!("{s:?}"),
    }
    assert!(f.scans().is_empty());
    assert_eq!(
        db::uses_for_envs(&f.ee.store, &[id]).await.unwrap().len(),
        1,
        "inventoried anyway"
    );
    assert!(
        db::latest(&f.ee.store, &[ImageKey("sha256:a".into())])
            .await
            .unwrap()
            .is_empty(),
        "nothing queued that cannot run, and never a clean result"
    );
}

#[tokio::test]
async fn stuck_rows_of_a_gone_environment_do_not_block_the_fleet() {
    let f = fx(true).await;
    for i in 0..60 {
        let t = ScanTarget {
            env_id: 9_999,
            node: String::new(),
            key: ImageKey(format!("sha256:gone{i}")),
            image_ref: "x".into(),
            namespace: String::new(),
            pull_secrets: vec![],
            platform: None,
        };
        db::insert_scan(
            &f.ee.store,
            &t,
            fleetops_ee::scan::model::Trigger::Arrival,
            now_unix(),
        )
        .await
        .unwrap();
    }
    let b = f.ee.env("b", "docker-agent").await;
    f.set_uses(b, vec![running(b, "sha256:up", "y:1")]);
    f.enable().await;
    f.run(now_unix()).await;
    assert_eq!(f.state("sha256:up").await.last.status, Status::Done);
    let gone = f.state("sha256:gone0").await;
    assert_eq!(gone.last.status, Status::Failed, "its environment is gone");
}

#[tokio::test]
async fn an_unavailable_environment_does_not_hold_a_shared_digest() {
    let f = fx(true).await;
    let a = f.ee.env("a", "docker-local").await;
    let b = f.ee.env("b", "docker-agent").await;
    f.runner.fail_prepare.lock().unwrap().insert("a".into());
    f.set_uses(a, vec![running(a, "sha256:same", "x:1")]);
    f.set_uses(b, vec![running(b, "sha256:same", "x:1")]);
    f.enable().await;
    f.run(now_unix()).await;
    assert_eq!(f.state("sha256:same").await.last.status, Status::Done);
    assert_eq!(f.scans(), [("b".to_string(), "sha256:same".to_string())]);
}

#[tokio::test]
async fn failed_scan_keeps_previous_findings() {
    let f = fx(true).await;
    let id = f.ee.env("local", "docker-local").await;
    f.set_uses(id, vec![running(id, "sha256:a", "nginx:1")]);
    f.enable().await;
    let now = now_unix();
    f.run(now).await;
    *f.runner.fail_scan.lock().unwrap() = true;
    f.run(now + 86_400 + 601).await;
    let st = f.state("sha256:a").await;
    assert_eq!(st.last.status, Status::Failed);
    assert!(st.last.error.as_deref().unwrap().contains("boom"));
    let done = st.last_done.expect("the earlier good scan");
    assert_eq!(db::findings(&f.ee.store, done.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn periodic_rescan_after_24h() {
    let f = fx(true).await;
    let id = f.ee.env("local", "docker-local").await;
    f.set_uses(id, vec![running(id, "sha256:a", "nginx:1")]);
    f.enable().await;
    let now = now_unix();
    f.run(now).await;
    f.run(now + 601).await;
    assert_eq!(f.scans().len(), 1, "not yet due");
    f.run(now + 86_400 + 601).await;
    assert_eq!(f.scans().len(), 2);
}

#[tokio::test]
async fn turning_off_clears_uses_and_queue() {
    let f = fx(true).await;
    let id = f.ee.env("local", "docker-local").await;
    f.set_uses(id, vec![running(id, "sha256:a", "nginx:1")]);
    f.enable().await;
    f.run(now_unix()).await;
    let t = ScanTarget {
        env_id: id,
        node: String::new(),
        key: ImageKey("sha256:b".into()),
        image_ref: "redis:7".into(),
        namespace: String::new(),
        pull_secrets: vec![],
        platform: None,
    };
    db::insert_scan(
        &f.ee.store,
        &t,
        fleetops_ee::scan::model::Trigger::Arrival,
        1,
    )
    .await
    .unwrap();
    let off = EnvOverride {
        enabled: Some(false),
        ..Default::default()
    };
    db::put_env(&f.ee.store, id, &off, PasswordChange::Keep, 1)
        .await
        .unwrap();
    f.ee.scan.env_changed(&f.ee.state, id).await.unwrap();
    assert_eq!(f.ee.scan.status(id), ScannerStatus::Off);
    assert!(db::uses_for_envs(&f.ee.store, &[id])
        .await
        .unwrap()
        .is_empty());
    let st = f.state("sha256:b").await;
    assert_eq!(st.last.status, Status::Failed);
    assert_eq!(st.last.error.as_deref(), Some("scanning turned off"));
}

#[tokio::test]
async fn restart_requeues_interrupted() {
    let f = fx(true).await;
    let id = f.ee.env("local", "docker-local").await;
    let t = ScanTarget {
        env_id: id,
        node: String::new(),
        key: ImageKey("sha256:a".into()),
        image_ref: "nginx:1".into(),
        namespace: String::new(),
        pull_secrets: vec![],
        platform: None,
    };
    let old = db::insert_scan(
        &f.ee.store,
        &t,
        fleetops_ee::scan::model::Trigger::Arrival,
        1,
    )
    .await
    .unwrap();
    db::mark_running(&f.ee.store, old, 2).await.unwrap();
    f.ee.scan.recover(&f.ee.state).await;
    let st = f.state("sha256:a").await;
    assert_ne!(st.last.id, old);
    assert_eq!(st.last.status, Status::Queued);
    assert_eq!(
        db::scan(&f.ee.store, old).await.unwrap().unwrap().status,
        Status::Failed
    );
}
