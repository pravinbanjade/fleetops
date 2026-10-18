use fleetops_ee::backup::{
    db::Retention,
    targets::{to_delete, validate_local, LocalSink, RemoteFile, S3Sink, Sink},
};
use fleetops_store::backup::backup_name;
use wiremock::{
    matchers::{method, query_param},
    Mock, MockServer, ResponseTemplate,
};

const DAY: i64 = 86_400;
const NOW: i64 = 1_800_000_000;

fn files(ages_days: &[i64]) -> Vec<RemoteFile> {
    ages_days
        .iter()
        .map(|d| RemoteFile {
            name: backup_name(NOW - d * DAY),
            size: 1,
            created_at: NOW - d * DAY,
        })
        .collect()
}

#[test]
fn retention_count_keeps_newest() {
    let f = files(&[3, 0, 1, 2]);
    let gone = to_delete(&f, Retention::Count(2), NOW);
    assert_eq!(
        gone,
        vec![backup_name(NOW - 2 * DAY), backup_name(NOW - 3 * DAY)]
    );
    assert!(to_delete(&f, Retention::Count(10), NOW).is_empty());
}

#[test]
fn retention_days() {
    let f = files(&[0, 1, 5, 9]);
    assert_eq!(
        to_delete(&f, Retention::Days(4), NOW),
        vec![backup_name(NOW - 5 * DAY), backup_name(NOW - 9 * DAY)]
    );
}

#[tokio::test]
async fn retention_ignores_foreign_and_nested_files() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join("notes.txt"), "x").unwrap();
    std::fs::write(p.join("my-backup.fob"), "x").unwrap();
    std::fs::create_dir(p.join("sub")).unwrap();
    std::fs::write(p.join("sub").join(backup_name(NOW - 50 * DAY)), "x").unwrap();
    for d in 0..5 {
        std::fs::write(p.join(backup_name(NOW - d * DAY)), "x").unwrap();
    }
    let sink = LocalSink(p.to_path_buf());
    let listed = sink.list().await.unwrap();
    assert_eq!(listed.len(), 5);
    for name in to_delete(&listed, Retention::Count(2), NOW) {
        sink.delete(&name).await.unwrap();
    }
    assert!(p.join("notes.txt").exists());
    assert!(p.join("my-backup.fob").exists());
    assert!(p.join("sub").join(backup_name(NOW - 50 * DAY)).exists());
    assert_eq!(sink.list().await.unwrap().len(), 2);
    // A foreign name can't be deleted through the sink at all.
    assert!(sink.delete("notes.txt").await.is_err());

    // S3, with a prefix holding the same mix.
    let server = MockServer::start().await;
    let key = |n: &str| {
        format!("<Contents><Key>team/{n}</Key><LastModified>2026-01-01T00:00:00.000Z</LastModified><ETag>\"e\"</ETag><Size>3</Size><StorageClass>STANDARD</StorageClass></Contents>")
    };
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>b</Name><Prefix>team/</Prefix><KeyCount>4</KeyCount><MaxKeys>1000</MaxKeys><Delimiter>/</Delimiter><IsTruncated>false</IsTruncated>{}{}{}<CommonPrefixes><Prefix>team/sub/</Prefix></CommonPrefixes></ListBucketResult>",
        key(&backup_name(NOW)),
        key("notes.txt"),
        key("my-backup.fob"),
    );
    Mock::given(method("GET"))
        .and(query_param("list-type", "2"))
        .and(query_param("delimiter", "/"))
        .and(query_param("prefix", "team/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let s3 = S3Sink::new(
        Some(&server.uri()),
        "us-east-1",
        "b",
        "/team/",
        true,
        "AK",
        "SK",
    )
    .unwrap();
    let listed = s3.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, backup_name(NOW));
    assert_eq!(listed[0].size, 3);
}

#[test]
fn local_inside_data_dir_refused() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(data.join("backups")).unwrap();
    let outside = dir.path().join("out");
    std::fs::create_dir_all(&outside).unwrap();
    let e = validate_local(&data.join("backups"), &data).unwrap_err();
    assert!(
        e.to_string().contains("can't be inside the data directory"),
        "{e}"
    );
    #[cfg(unix)]
    {
        let link = dir.path().join("sneaky");
        std::os::unix::fs::symlink(data.join("backups"), &link).unwrap();
        assert!(validate_local(&link, &data).is_err());
    }
    assert!(validate_local(std::path::Path::new("relative/dir"), &data)
        .unwrap_err()
        .to_string()
        .contains("absolute"));
    validate_local(&outside, &data).unwrap();
}

#[tokio::test]
async fn local_put_list_get_delete() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("t");
    std::fs::create_dir_all(&target).unwrap();
    let src = dir.path().join("src.fob");
    std::fs::write(&src, b"backup bytes").unwrap();
    let sink = LocalSink(target.clone());
    sink.test().await.unwrap();
    let name = backup_name(NOW);
    sink.put(&name, &src).await.unwrap();
    assert!(sink.put("../escape.fob", &src).await.is_err());
    let listed = sink.list().await.unwrap();
    assert_eq!(
        listed,
        vec![RemoteFile {
            name: name.clone(),
            size: 12,
            created_at: NOW
        }]
    );
    let back = dir.path().join("back.fob");
    sink.get(&name, &back).await.unwrap();
    assert_eq!(std::fs::read(&back).unwrap(), b"backup bytes");
    sink.delete(&name).await.unwrap();
    assert!(sink.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn s3_errors_hide_presigned_urls() {
    // Nothing listens on port 1: the request fails before any response.
    let s3 = S3Sink::new(
        Some("http://127.0.0.1:1"),
        "us-east-1",
        "b",
        "",
        true,
        "AKIAEXAMPLE",
        "SK",
    )
    .unwrap();
    let e = s3.test().await.unwrap_err().to_string();
    assert!(e.starts_with("invalid input: S3: "), "{e}");
    for leak in [
        "X-Amz-Signature",
        "X-Amz-Credential",
        "AKIAEXAMPLE",
        "127.0.0.1:1/",
    ] {
        assert!(!e.contains(leak), "{leak} in {e}");
    }
}

#[tokio::test]
async fn s3_stalled_endpoint_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(5)))
        .mount(&server)
        .await;
    let s3 = S3Sink::new(Some(&server.uri()), "us-east-1", "b", "", true, "AK", "SK")
        .unwrap()
        .with_read_timeout(std::time::Duration::from_millis(500));
    let t = std::time::Instant::now();
    assert!(s3.list().await.is_err());
    assert!(
        t.elapsed() < std::time::Duration::from_secs(3),
        "{:?}",
        t.elapsed()
    );
}
