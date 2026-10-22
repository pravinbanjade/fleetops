use fleetops_git::testing::{GitServer, ServerOpts, TestRepo};
use fleetops_git::{
    fetch_commit, fetch_file, has_commit, read_at, tar_dir, tar_tree, tree_has_file, tree_ids,
    Limits, Source, TarLimits,
};
use std::time::Duration;

const TOKEN: &str = "s3cret-token";
const COMPOSE: &str = "services:\n  web:\n    image: nginx:1.27\n";

fn source(server: &GitServer, branch: &str, path: &str, token: Option<&str>) -> Source {
    Source {
        url: server.url("repo.git"),
        branch: branch.into(),
        path: path.into(),
        token: token.map(Into::into),
    }
}

async fn served(opts: ServerOpts) -> (TestRepo, GitServer) {
    let repo = TestRepo::new("main");
    let server = GitServer::start(&repo.root(), opts).await;
    (repo, server)
}

fn message(e: fleetops_core::Error) -> String {
    e.detail()
}

#[tokio::test]
async fn reads_file_at_branch_head() {
    let (repo, server) = served(ServerOpts::default()).await;
    let head = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let f = fetch_file(
        &cache.path().join("1"),
        &source(&server, "main", "compose.yaml", None),
        Limits::default(),
    )
    .await
    .unwrap();
    assert_eq!(f.commit, head);
    assert_eq!(f.content, COMPOSE);
}

#[tokio::test]
async fn second_fetch_sees_new_commit_in_same_cache() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let src = source(&server, "main", "compose.yaml", None);
    fetch_file(&dir, &src, Limits::default()).await.unwrap();
    assert!(dir.exists());
    let two = repo.commit(&[("compose.yaml", "services: {}\n")], "two");
    let f = fetch_file(&dir, &src, Limits::default()).await.unwrap();
    assert_eq!(f.commit, two);
    assert_eq!(f.content, "services: {}\n");
}

#[tokio::test]
async fn custom_branch_and_path() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(&[("compose.yaml", COMPOSE)], "main");
    let id = repo.commit_on("deploy", &[("ops/stack.yml", "x: 1\n")], "deploy");
    let cache = tempfile::tempdir().unwrap();
    let f = fetch_file(
        &cache.path().join("1"),
        &source(&server, "deploy", "ops/stack.yml", None),
        Limits::default(),
    )
    .await
    .unwrap();
    assert_eq!(f.commit, id);
    assert_eq!(f.content, "x: 1\n");
}

#[tokio::test]
async fn missing_branch() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let e = fetch_file(
        &cache.path().join("1"),
        &source(&server, "nope", "compose.yaml", None),
        Limits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(message(e), "branch 'nope' not found");
}

#[tokio::test]
async fn missing_path() {
    let (repo, server) = served(ServerOpts::default()).await;
    let head = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let e = fetch_file(
        &cache.path().join("1"),
        &source(&server, "main", "other.yaml", None),
        Limits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(e),
        format!("other.yaml not found at {}", &head[..7])
    );
}

#[tokio::test]
async fn token_required_and_used() {
    let (repo, server) = served(ServerOpts {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let none = fetch_file(
        &dir,
        &source(&server, "main", "compose.yaml", None),
        Limits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(message(none), "the repository needs an access token");
    let wrong = fetch_file(
        &dir,
        &source(&server, "main", "compose.yaml", Some("wrong")),
        Limits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(message(wrong), "the repository refused the credentials");
    let f = fetch_file(
        &dir,
        &source(&server, "main", "compose.yaml", Some(TOKEN)),
        Limits::default(),
    )
    .await
    .unwrap();
    assert_eq!(f.content, COMPOSE);
}

#[tokio::test]
async fn errors_never_contain_token() {
    let (repo, server) = served(ServerOpts {
        token: Some("other".into()),
        ..Default::default()
    })
    .await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let mut errors = vec![];
    for (branch, path) in [("main", "compose.yaml"), ("nope", "compose.yaml")] {
        let e = fetch_file(
            &cache.path().join("1"),
            &source(&server, branch, path, Some(TOKEN)),
            Limits::default(),
        )
        .await
        .unwrap_err();
        errors.push(e.to_string());
    }
    // An unreachable server, too.
    let gone = Source {
        url: "http://127.0.0.1:9/repo.git".into(),
        branch: "main".into(),
        path: "compose.yaml".into(),
        token: Some(TOKEN.into()),
    };
    errors.push(
        fetch_file(&cache.path().join("2"), &gone, Limits::default())
            .await
            .unwrap_err()
            .to_string(),
    );
    for e in errors {
        assert!(!e.contains(TOKEN), "{e}");
    }
}

fn noise(len: usize) -> String {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from(b"0123456789abcdef"[(x & 15) as usize])
        })
        .collect()
}

#[tokio::test]
async fn size_cap_interrupts() {
    let (repo, server) = served(ServerOpts {
        delay: Duration::from_millis(300),
        ..Default::default()
    })
    .await;
    repo.commit(
        &[("compose.yaml", COMPOSE), ("big.txt", &noise(512 * 1024))],
        "big",
    );
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let e = fetch_file(
        &dir,
        &source(&server, "main", "compose.yaml", None),
        Limits {
            max_bytes: 4096,
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(e),
        "the repository is larger than 200 MB at depth 1"
    );
    // The partial cache is removed once the fetch has stopped.
    for _ in 0..600 {
        if !dir.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the cache dir was not removed");
}

#[tokio::test]
async fn timeout_interrupts() {
    let (repo, server) = served(ServerOpts {
        delay: Duration::from_secs(2),
        ..Default::default()
    })
    .await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let e = fetch_file(
        &cache.path().join("1"),
        &source(&server, "main", "compose.yaml", None),
        Limits {
            timeout: Duration::from_millis(200),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(e),
        "fetching the repository took longer than 2 minutes"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn file_too_large() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let e = fetch_file(
        &cache.path().join("1"),
        &source(&server, "main", "compose.yaml", None),
        Limits {
            max_file: 10,
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(message(e), "compose.yaml is larger than 10 bytes");
}

#[tokio::test]
async fn force_push_fetches_new_head() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    repo.commit(&[("compose.yaml", "a: 1\n")], "two");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let src = source(&server, "main", "compose.yaml", None);
    fetch_file(&dir, &src, Limits::default()).await.unwrap();
    let rewritten = repo.force_reset(&[("compose.yaml", "b: 2\n")], "rewritten");
    let f = fetch_file(&dir, &src, Limits::default()).await.unwrap();
    assert_eq!(f.commit, rewritten);
    assert_eq!(f.content, "b: 2\n");
}

#[tokio::test]
async fn fetch_after_an_interrupted_one_is_not_disturbed_by_its_cleanup() {
    let (repo, server) = served(ServerOpts {
        delay: Duration::from_millis(700),
        ..Default::default()
    })
    .await;
    let head = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let src = source(&server, "main", "compose.yaml", None);
    let cut = fetch_file(
        &dir,
        &src,
        Limits {
            timeout: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .await;
    assert!(cut.is_err());
    // Straight away, while the interrupted fetch is still winding down.
    let f = fetch_file(&dir, &src, Limits::default()).await.unwrap();
    assert_eq!(f.commit, head);
    // Its late cleanup must not remove the cache the second fetch made.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(dir.exists());
    let again = fetch_file(&dir, &src, Limits::default()).await.unwrap();
    assert_eq!(again.commit, head);
}

fn packs(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir.join("objects/pack"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "pack"))
                .count()
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn cache_packs_stay_bounded() {
    let (repo, server) = served(ServerOpts::default()).await;
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let src = source(&server, "main", "compose.yaml", None);
    for i in 0..(fleetops_git::MAX_PACKS + 4) {
        let head = repo.commit(&[("compose.yaml", &format!("# {i}\n{COMPOSE}"))], "next");
        let f = fetch_file(&dir, &src, Limits::default()).await.unwrap();
        assert_eq!(f.commit, head);
        assert!(
            packs(&dir) <= fleetops_git::MAX_PACKS,
            "{} packs",
            packs(&dir)
        );
    }
}

#[tokio::test]
async fn read_at_reads_paths_at_the_commit() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(
        &[
            ("deploy/compose.yaml", COMPOSE),
            ("deploy/conf/a.conf", "a=1\n"),
            ("b.txt", "b"),
        ],
        "one",
    );
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let f = fetch_file(
        &dir,
        &source(&server, "main", "deploy/compose.yaml", None),
        Limits::default(),
    )
    .await
    .unwrap();
    let got = read_at(
        &dir,
        &f.commit,
        &["deploy/conf/a.conf".into(), "b.txt".into()],
        1024,
    )
    .await
    .unwrap();
    assert_eq!(got, vec!["a=1\n".to_string(), "b".to_string()]);
    let e = read_at(&dir, &f.commit, &["b.txt".into()], 0)
        .await
        .unwrap_err();
    assert!(message(e).contains("b.txt is larger than"));
}

#[tokio::test]
async fn read_at_reports_missing_path() {
    let (repo, server) = served(ServerOpts::default()).await;
    let head = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    fetch_file(
        &dir,
        &source(&server, "main", "compose.yaml", None),
        Limits::default(),
    )
    .await
    .unwrap();
    let e = read_at(&dir, &head, &["nginx.conf".into()], 1024)
        .await
        .unwrap_err();
    assert_eq!(
        message(e),
        format!("nginx.conf not found at {}", &head[..7])
    );
}

#[tokio::test]
async fn read_at_works_right_after_the_fetch_that_hits_the_pack_cap() {
    let (repo, server) = served(ServerOpts::default()).await;
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let src = source(&server, "main", "compose.yaml", None);
    for i in 0..(fleetops_git::MAX_PACKS + 2) {
        repo.commit(
            &[("compose.yaml", COMPOSE), ("a.conf", &format!("{i}"))],
            "next",
        );
        let f = fetch_file(&dir, &src, Limits::default()).await.unwrap();
        let got = read_at(&dir, &f.commit, &["a.conf".into()], 1024)
            .await
            .unwrap();
        assert_eq!(got, vec![i.to_string()]);
    }
}

const BIG: TarLimits = TarLimits {
    max_bytes: 100 * 1024 * 1024,
    max_entries: 20_000,
};

async fn fetched(server: &GitServer, dir: &std::path::Path) -> String {
    fetch_file(
        dir,
        &source(server, "main", "compose.yaml", None),
        Limits::default(),
    )
    .await
    .unwrap()
    .commit
}

/// (path, mode, kind, content or link target) of every entry.
fn entries(tar: &[u8]) -> Vec<(String, u32, char, String)> {
    use std::io::Read;
    let mut a = tar::Archive::new(tar);
    a.entries()
        .unwrap()
        .map(|e| {
            let mut e = e.unwrap();
            let h = e.header().clone();
            let path = e.path().unwrap().to_string_lossy().into_owned();
            let kind = if h.entry_type().is_dir() {
                'd'
            } else if h.entry_type().is_symlink() {
                'l'
            } else {
                'f'
            };
            let mut body = String::new();
            if kind == 'l' {
                body = e
                    .link_name()
                    .unwrap()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
            } else {
                e.read_to_string(&mut body).unwrap();
            }
            (path, h.mode().unwrap() & 0o777, kind, body)
        })
        .collect()
}

#[tokio::test]
async fn tree_ids_change_only_with_the_directory() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(
        &[("compose.yaml", COMPOSE), ("api/a", "1"), ("web/w", "1")],
        "one",
    );
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let c1 = fetched(&server, &dir).await;
    let dirs = vec!["api".to_string(), "web".to_string(), String::new()];
    let one = tree_ids(&dir, &c1, &dirs).await.unwrap();
    repo.commit(&[("web/w", "2")], "two");
    let c2 = fetched(&server, &dir).await;
    let two = tree_ids(&dir, &c2, &dirs).await.unwrap();
    assert_eq!(one[0], two[0], "api untouched");
    assert_ne!(one[1], two[1], "web changed");
    assert_ne!(one[2], two[2], "root changed");
    assert_eq!(one[0].len(), 40);
}

#[tokio::test]
async fn tar_dir_is_deterministic_and_applies_dockerignore() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(
        &[
            ("compose.yaml", COMPOSE),
            ("app/Dockerfile", "FROM alpine\n"),
            ("app/run.sh", "#!/bin/sh\n"),
            ("app/.dockerignore", "*.log\nsecret/\n"),
            ("app/x.log", "noise"),
            ("app/secret/key", "k"),
            ("app/src/main.c", "int main;"),
            ("other/file", "o"),
        ],
        "one",
    );
    repo.set_executable("app/run.sh");
    repo.add_symlink("app/link", "src/main.c");
    repo.commit(&[], "modes");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let c = fetched(&server, &dir).await;
    let a = tar_dir(&dir, &c, "app", BIG).await.unwrap();
    let b = tar_dir(&dir, &c, "app", BIG).await.unwrap();
    assert_eq!(a.tar, b.tar, "deterministic");
    let got = entries(&a.tar);
    let files: Vec<&str> = got
        .iter()
        .filter(|e| e.2 != 'd')
        .map(|e| e.0.as_str())
        .collect();
    assert_eq!(
        files,
        vec![
            ".dockerignore",
            "Dockerfile",
            "link",
            "run.sh",
            "src/main.c"
        ],
        "{got:?}"
    );
    let find = |p: &str| got.iter().find(|e| e.0 == p).unwrap().clone();
    assert_eq!(find("run.sh").1, 0o755);
    assert_eq!(find("Dockerfile").1, 0o644);
    assert_eq!(
        (find("link").2, find("link").3.as_str()),
        ('l', "src/main.c")
    );
    assert!(a.skipped.is_empty());
}

#[tokio::test]
async fn tar_dir_limits() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(
        &[("compose.yaml", COMPOSE), ("app/a", "1"), ("app/b", "2")],
        "one",
    );
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let c = fetched(&server, &dir).await;
    let e = tar_dir(
        &dir,
        &c,
        "app",
        TarLimits {
            max_bytes: 1 << 20,
            max_entries: 1,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(message(e), "the build context app has more than 1 entries");
    let e = tar_dir(
        &dir,
        &c,
        "app",
        TarLimits {
            max_bytes: 1,
            max_entries: 100,
        },
    )
    .await
    .unwrap_err();
    assert!(message(e).starts_with("the build context app is larger than"));
}

#[tokio::test]
async fn missing_dir_is_reported() {
    let (repo, server) = served(ServerOpts::default()).await;
    let head = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    fetched(&server, &dir).await;
    let e = tree_ids(&dir, &head, &["api".into()]).await.unwrap_err();
    assert_eq!(message(e), format!("api not found at {}", &head[..7]));
    let e = tar_dir(&dir, &head, "compose.yaml", BIG).await.unwrap_err();
    assert_eq!(message(e), "compose.yaml is not a directory");
}

#[tokio::test]
async fn fetch_commit_brings_an_old_commit_back() {
    let (repo, server) = served(ServerOpts::default()).await;
    let c1 = repo.commit(&[("compose.yaml", COMPOSE), ("app/a", "1")], "one");
    repo.commit(&[("app/a", "2")], "two");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    fetched(&server, &dir).await;
    assert!(!has_commit(&dir, &c1));
    let src = source(&server, "main", "compose.yaml", None);
    fetch_commit(&dir, &src, &c1, Limits::default())
        .await
        .unwrap();
    assert!(has_commit(&dir, &c1));
    assert_eq!(tree_ids(&dir, &c1, &["app".into()]).await.unwrap().len(), 1);
}

#[tokio::test]
async fn fetch_commit_refused_names_the_commit() {
    let (repo, server) = served(ServerOpts {
        v0_only: true,
        ..Default::default()
    })
    .await;
    let c1 = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    repo.force_reset(&[("compose.yaml", COMPOSE), ("x", "y")], "rewritten");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    fetched(&server, &dir).await;
    let src = source(&server, "main", "compose.yaml", None);
    let e = fetch_commit(&dir, &src, &c1, Limits::default())
        .await
        .unwrap_err();
    assert_eq!(message(e), format!("commit {} cannot be fetched", &c1[..7]));
}

#[tokio::test]
async fn allow_list_dockerignore_keeps_the_dockerfile() {
    let (repo, server) = served(ServerOpts::default()).await;
    repo.commit(
        &[
            ("compose.yaml", COMPOSE),
            ("app/Dockerfile", "FROM alpine\n"),
            ("app/.dockerignore", "*\n!src\n"),
            ("app/src/main.c", "int main;"),
            ("app/notes.txt", "n"),
        ],
        "one",
    );
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let c = fetched(&server, &dir).await;
    let tree = tree_ids(&dir, &c, &["app".into()]).await.unwrap().remove(0);
    let out = tar_tree(&dir, &tree, "app", &["Dockerfile".into()], BIG)
        .await
        .unwrap();
    let got = entries(&out.tar);
    let files: Vec<&str> = got
        .iter()
        .filter(|e| e.2 != 'd')
        .map(|e| e.0.as_str())
        .collect();
    assert_eq!(
        files,
        vec![".dockerignore", "Dockerfile", "src/main.c"],
        "{got:?}"
    );
    assert!(tree_has_file(&dir, &tree, "Dockerfile").await.unwrap());
    assert!(!tree_has_file(&dir, &tree, "Nope").await.unwrap());
}
