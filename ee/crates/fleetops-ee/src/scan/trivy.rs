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
    /// From the registry (Kubernetes).
    Remote,
}

/// `trivy …` arguments for one image. `Remote` scans print the report to
/// stdout; the others write it to [`REPORT_PATH`].
pub fn image_args(image_ref: &str, cfg: &Effective, src: ImageSource) -> Vec<String> {
    let mut a: Vec<String> = ["image", "--scanners", "vuln", "--format", "json"]
        .map(String::from)
        .into();
    if src != ImageSource::Remote {
        a.extend(["--output".into(), REPORT_PATH.into()]);
    }
    a.extend(["--cache-dir", "/cache", "--timeout", "15m", "--quiet"].map(String::from));
    match src {
        ImageSource::Socket => a.extend(["--image-src".into(), "docker".into()]),
        ImageSource::Tar => a.extend(["--input".into(), INPUT_PATH.into()]),
        ImageSource::Remote => a.extend(["--image-src".into(), "remote".into()]),
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

/// `trivy …` that only downloads the vulnerability database into the cache:
/// what turning scanning on checks.
pub fn db_args(cfg: &Effective) -> Vec<String> {
    let mut a: Vec<String> = [
        "image",
        "--download-db-only",
        "--cache-dir",
        "/cache",
        "--timeout",
        "15m",
        "--quiet",
    ]
    .map(String::from)
    .into();
    if let Some(r) = &cfg.db_repository {
        a.extend(["--db-repository".into(), r.clone()]);
    }
    a
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
        let mut m = base.clone();
        m.db_repository = Some("mirror.local/trivy-db".into());
        m.java_db_repository = Some("mirror.local/trivy-java-db".into());
        assert_eq!(
            image_args("nginx@sha256:1", &m, ImageSource::Remote),
            [
                "image",
                "--scanners",
                "vuln",
                "--format",
                "json",
                "--cache-dir",
                "/cache",
                "--timeout",
                "15m",
                "--quiet",
                "--image-src",
                "remote",
                "--db-repository",
                "mirror.local/trivy-db",
                "--java-db-repository",
                "mirror.local/trivy-java-db",
                "nginx@sha256:1"
            ]
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
    fn auth_env_only_with_credentials() {
        let mut c = cfg();
        assert!(auth_env(&c).is_empty());
        c.auth = Some(("u".into(), "p".into()));
        assert_eq!(auth_env(&c), ["TRIVY_USERNAME=u", "TRIVY_PASSWORD=p"]);
    }
}
