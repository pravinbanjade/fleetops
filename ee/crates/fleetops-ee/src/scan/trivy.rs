//! Trivy's command line and its JSON report.

use super::config::Effective;
use super::model::{Finding, Severity};
use fleetops_core::{Error, Result};
use serde::Deserialize;
use std::collections::HashSet;

/// Where the report is written inside a Docker scan container.
pub const REPORT_PATH: &str = "/tmp/report.json";
/// Where the exported image tar is put for an [`ImageSource::Tar`] scan.
pub const INPUT_DIR: &str = "/in";
pub const INPUT_PATH: &str = "/in/image.tar";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSource {
    /// From the engine, through the mounted socket.
    Socket,
    /// From an exported tar at [`INPUT_PATH`].
    Tar,
}

/// `trivy …` arguments for one image on a Docker host; the report goes to
/// [`REPORT_PATH`].
pub fn image_args(image_ref: &str, cfg: &Effective, src: ImageSource) -> Vec<String> {
    let mut a: Vec<String> = [
        "image",
        "--scanners",
        "vuln",
        "--format",
        "json",
        "--output",
        REPORT_PATH,
    ]
    .map(String::from)
    .into();
    a.extend(["--cache-dir", "/cache", "--timeout", "15m", "--quiet"].map(String::from));
    match src {
        ImageSource::Socket => a.extend(["--image-src".into(), "docker".into()]),
        ImageSource::Tar => a.extend(["--input".into(), INPUT_PATH.into()]),
    }
    if let Some(r) = &cfg.db_repository {
        a.extend(["--db-repository".into(), r.clone()]);
    }
    if let Some(r) = &cfg.java_db_repository {
        a.extend(["--java-db-repository".into(), r.clone()]);
    }
    if src != ImageSource::Tar {
        a.push(image_ref.to_string());
    }
    a
}

/// `trivy rootfs …` run inside the scanned image (Kubernetes): offline, on
/// databases the init containers refreshed, with every option that changes
/// findings explicit so the image's own `TRIVY_*` settings can't. The
/// report goes to stdout.
pub fn rootfs_args(_cfg: &Effective) -> Vec<String> {
    [
        "rootfs",
        "--scanners",
        "vuln",
        "--format",
        "json",
        "--cache-dir",
        "/cache",
        "--timeout",
        "15m",
        "--quiet",
        "--skip-db-update",
        "--skip-java-db-update",
        "--offline-scan",
        "--severity",
        SEVERITIES,
        "--ignore-unfixed=false",
        "--ignorefile",
        "/fleetops/none/.trivyignore",
        "--config",
        "/fleetops/none/trivy.yaml",
        "--skip-dirs",
        "/fleetops,/cache,/proc,/sys,/dev",
        "/",
    ]
    .map(String::from)
    .into()
}

/// Every severity, so none is filtered out.
pub const SEVERITIES: &str = "UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL";

/// `trivy …` that only downloads the vulnerability database into the cache:
/// what turning scanning on checks.
pub fn db_args(cfg: &Effective) -> Vec<String> {
    download_args("--download-db-only", "--db-repository", &cfg.db_repository)
}

/// `trivy …` that only downloads the Java database into the cache, so no
/// scan has to fetch it midway.
pub fn java_db_args(cfg: &Effective) -> Vec<String> {
    download_args(
        "--download-java-db-only",
        "--java-db-repository",
        &cfg.java_db_repository,
    )
}

fn download_args(only: &str, repo_flag: &str, repo: &Option<String>) -> Vec<String> {
    let mut a: Vec<String> = [
        "image",
        only,
        "--cache-dir",
        "/cache",
        "--timeout",
        "15m",
        "--quiet",
    ]
    .map(String::from)
    .into();
    if let Some(r) = repo {
        a.extend([repo_flag.into(), r.clone()]);
    }
    a
}

/// The JSON report in a scan pod's log, which also carries whatever Trivy
/// said on stderr: from the first line that opens an object to the last
/// that closes one. Trivy indents its report, so only those two lines sit
/// in column 0.
pub fn report_from_log(lines: &[String]) -> Vec<u8> {
    let start = lines.iter().position(|l| l.starts_with('{'));
    let end = lines.iter().rposition(|l| l.starts_with('}'));
    match (start, end) {
        (Some(s), Some(e)) if s <= e => lines[s..=e].join("\n").into_bytes(),
        _ => lines.join("\n").into_bytes(),
    }
}

/// Registry credentials for the mirror, as Trivy reads them.
pub fn auth_env(cfg: &Effective) -> Vec<String> {
    match &cfg.auth {
        Some((u, p)) => vec![format!("TRIVY_USERNAME={u}"), format!("TRIVY_PASSWORD={p}")],
        None => vec![],
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Report {
    #[serde(default)]
    results: Option<Vec<ReportResult>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ReportResult {
    #[serde(default)]
    vulnerabilities: Option<Vec<Vuln>>,
}

#[derive(Deserialize)]
struct Vuln {
    #[serde(rename = "VulnerabilityID")]
    id: String,
    #[serde(rename = "PkgName")]
    pkg: String,
    #[serde(rename = "InstalledVersion", default)]
    installed: String,
    #[serde(rename = "FixedVersion", default)]
    fixed: Option<String>,
    #[serde(rename = "Severity", default)]
    severity: String,
    #[serde(rename = "Title", default)]
    title: Option<String>,
    #[serde(rename = "PrimaryURL", default)]
    url: Option<String>,
}

/// The findings in a `trivy image --format json` report, one per
/// vulnerability, package and installed version.
pub fn parse_report(json: &[u8]) -> Result<Vec<Finding>> {
    let r: Report =
        serde_json::from_slice(json).map_err(|e| Error::Invalid(format!("trivy report: {e}")))?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for v in r
        .results
        .into_iter()
        .flatten()
        .flat_map(|r| r.vulnerabilities.into_iter().flatten())
    {
        if !seen.insert((v.id.clone(), v.pkg.clone(), v.installed.clone())) {
            continue;
        }
        out.push(Finding {
            vuln_id: v.id,
            pkg_name: v.pkg,
            installed_version: v.installed,
            fixed_version: v.fixed.filter(|f| !f.trim().is_empty()),
            severity: Severity::parse(&v.severity),
            title: v.title.filter(|t| !t.is_empty()),
            url: v.url.filter(|u| !u.is_empty()),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::config::{effective, GlobalConfig};

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/trivy-nginx.json");

    fn cfg() -> Effective {
        effective(&GlobalConfig::default(), None, None, None)
    }

    #[test]
    fn parse_fixture_dedups_and_maps_fixed_version() {
        let f = parse_report(FIXTURE).unwrap();
        assert_eq!(f.len(), 5, "6 entries, one duplicate: {f:#?}");
        let ssl = f.iter().find(|f| f.vuln_id == "CVE-2024-5535").unwrap();
        assert_eq!(ssl.severity, Severity::Critical);
        assert_eq!(ssl.fixed_version.as_deref(), Some("3.0.14-1~deb12u2"));
        assert_eq!(
            ssl.url.as_deref(),
            Some("https://avd.aquasec.com/nvd/cve-2024-5535")
        );
        let nofix = f.iter().find(|f| f.vuln_id == "CVE-2023-45853").unwrap();
        assert_eq!(nofix.fixed_version, None, "empty FixedVersion is no fix");
        assert!(
            f.iter().any(|f| f.pkg_name == "golang.org/x/net"),
            "both Results read"
        );
    }

    #[test]
    fn parse_rejects_non_json() {
        let e = parse_report(b"FATAL image scan error").unwrap_err();
        assert!(e.to_string().contains("trivy report"), "{e}");
    }

    #[test]
    fn parse_empty_results_is_no_findings() {
        assert!(parse_report(br#"{"SchemaVersion": 2}"#).unwrap().is_empty());
        assert!(parse_report(br#"{"Results": [{"Target": "x"}]}"#)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn args_for_each_source_and_mirror() {
        let base = cfg();
        assert_eq!(
            image_args("nginx:1.25", &base, ImageSource::Socket),
            [
                "image",
                "--scanners",
                "vuln",
                "--format",
                "json",
                "--output",
                REPORT_PATH,
                "--cache-dir",
                "/cache",
                "--timeout",
                "15m",
                "--quiet",
                "--image-src",
                "docker",
                "nginx:1.25"
            ]
        );
        assert_eq!(
            image_args("ignored", &base, ImageSource::Tar),
            [
                "image",
                "--scanners",
                "vuln",
                "--format",
                "json",
                "--output",
                REPORT_PATH,
                "--cache-dir",
                "/cache",
                "--timeout",
                "15m",
                "--quiet",
                "--input",
                INPUT_PATH
            ]
        );
    }

    #[test]
    fn rootfs_args_are_exact_and_offline() {
        let want = [
            "rootfs",
            "--scanners",
            "vuln",
            "--format",
            "json",
            "--cache-dir",
            "/cache",
            "--timeout",
            "15m",
            "--quiet",
            "--skip-db-update",
            "--skip-java-db-update",
            "--offline-scan",
            "--severity",
            "UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL",
            "--ignore-unfixed=false",
            "--ignorefile",
            "/fleetops/none/.trivyignore",
            "--config",
            "/fleetops/none/trivy.yaml",
            "--skip-dirs",
            "/fleetops,/cache,/proc,/sys,/dev",
            "/",
        ];
        let mut c = cfg();
        assert_eq!(rootfs_args(&c), want);
        c.db_repository = Some("mirror.local/trivy-db".into());
        c.java_db_repository = Some("mirror.local/trivy-java-db".into());
        assert_eq!(
            rootfs_args(&c),
            want,
            "repositories only matter to downloads"
        );
    }

    #[test]
    fn db_args_download_only_from_the_mirror() {
        let mut c = cfg();
        assert_eq!(
            db_args(&c),
            [
                "image",
                "--download-db-only",
                "--cache-dir",
                "/cache",
                "--timeout",
                "15m",
                "--quiet"
            ]
        );
        c.db_repository = Some("mirror.local/trivy-db".into());
        let a = db_args(&c);
        assert!(a
            .windows(2)
            .any(|w| w == ["--db-repository", "mirror.local/trivy-db"]));
    }

    #[test]
    fn java_db_args_download_only_from_the_mirror() {
        let mut c = cfg();
        assert_eq!(
            java_db_args(&c),
            [
                "image",
                "--download-java-db-only",
                "--cache-dir",
                "/cache",
                "--timeout",
                "15m",
                "--quiet"
            ]
        );
        c.java_db_repository = Some("mirror.local/trivy-java-db".into());
        assert!(java_db_args(&c)
            .windows(2)
            .any(|w| w == ["--java-db-repository", "mirror.local/trivy-java-db"]));
    }

    #[test]
    fn report_from_log_skips_stray_lines() {
        let lines: Vec<String> = [
            "2026-11-03T21:37:13Z\tWARN\tsomething",
            "{",
            "  \"Results\": [",
            "    {\"Target\": \"x\"}",
            "  ]",
            "}",
            "2026-11-03T21:37:14Z\tINFO\tdone",
        ]
        .map(String::from)
        .into();
        let r = report_from_log(&lines);
        assert!(parse_report(&r).unwrap().is_empty());
        assert!(String::from_utf8(r).unwrap().starts_with('{'));
    }

    #[test]
    fn auth_env_only_with_credentials() {
        let mut c = cfg();
        assert!(auth_env(&c).is_empty());
        c.auth = Some(("u".into(), "p".into()));
        assert_eq!(auth_env(&c), ["TRIVY_USERNAME=u", "TRIVY_PASSWORD=p"]);
    }
}
