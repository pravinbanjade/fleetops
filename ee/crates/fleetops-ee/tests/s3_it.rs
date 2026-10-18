//! Against a real S3-compatible server (make test-s3): FLEETOPS_S3_IT is its URL.

use fleetops_ee::backup::targets::{S3Sink, Sink};
use fleetops_store::backup::backup_name;

fn sink(prefix: &str) -> S3Sink {
    let url = std::env::var("FLEETOPS_S3_IT").expect("FLEETOPS_S3_IT");
    S3Sink::new(
        Some(&url),
        "us-east-1",
        "fleetops-it",
        prefix,
        true,
        "minioadmin",
        "minioadmin",
    )
    .unwrap()
}

#[tokio::test]
async fn minio_put_list_get_delete_test() {
    let prefix = format!("it-{}", std::process::id());
    let s = sink(&prefix);
    s.test().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("b.fob");
    std::fs::write(&src, vec![7u8; 300_000]).unwrap();
    let (a, b) = (backup_name(1_800_000_000), backup_name(1_800_086_400));
    s.put(&a, &src).await.unwrap();
    s.put(&b, &src).await.unwrap();
    let mut names: Vec<_> = s
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    names.sort();
    assert_eq!(names, vec![a.clone(), b.clone()]);
    let back = dir.path().join("back.fob");
    s.get(&a, &back).await.unwrap();
    assert_eq!(std::fs::read(&back).unwrap().len(), 300_000);
    s.delete(&a).await.unwrap();
    s.delete(&b).await.unwrap();
    assert!(s.list().await.unwrap().is_empty());
    // Wrong credentials surface S3's own error.
    let url = std::env::var("FLEETOPS_S3_IT").unwrap();
    let bad = S3Sink::new(
        Some(&url),
        "us-east-1",
        "fleetops-it",
        "",
        true,
        "minioadmin",
        "wrong",
    )
    .unwrap();
    let e = bad.test().await.unwrap_err().to_string();
    assert!(e.contains("S3: "), "{e}");
    assert!(!e.contains("wrong"), "the secret never shows: {e}");
}
