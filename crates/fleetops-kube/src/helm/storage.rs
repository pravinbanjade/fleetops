//! Helm 3's Secrets storage driver (spec §2.2): one Secret per revision,
//! `data.release` holding base64 of gzip of the release JSON.

use base64::Engine;
use fleetops_core::helm::{HelmRelease, HelmRevision, UNREADABLE};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};

pub const SECRET_TYPE: &str = "helm.sh/release.v1";
pub const OWNER_SELECTOR: &str = "owner=helm";
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// A release record. Fields FleetOps does not read are kept in `rest`, so
/// a record written back loses nothing Helm stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub name: String,
    pub namespace: String,
    pub version: u32,
    pub info: Info,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub chart: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub config: Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manifest: String,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Info {
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_deployed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_deployed: Option<String>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// A record from a Secret's `data.release` bytes.
pub fn decode(raw: &[u8]) -> Result<Release, String> {
    let text: Vec<u8> = raw
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let bytes = B64.decode(text).map_err(|e| format!("not base64: {e}"))?;
    let json = if bytes.starts_with(&GZIP_MAGIC) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_end(&mut out)
            .map_err(|e| format!("not gzip: {e}"))?;
        out
    } else {
        bytes
    };
    serde_json::from_slice(&json).map_err(|e| format!("not a Helm release: {e}"))
}

/// The bytes for `data.release`, as Helm writes them.
pub fn encode(r: &Release) -> Vec<u8> {
    let json = serde_json::to_vec(r).expect("a release always serialises");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&json).expect("writing to memory");
    B64.encode(gz.finish().expect("writing to memory"))
        .into_bytes()
}

pub fn secret_name(name: &str, version: u32) -> String {
    format!("sh.helm.release.v1.{name}.v{version}")
}

/// The labels Helm puts on a release Secret; `modifiedAt` as Helm adds on update.
pub fn labels(r: &Release) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("name".to_string(), r.name.clone()),
        ("owner".to_string(), "helm".to_string()),
        ("status".to_string(), r.info.status.clone()),
        ("version".to_string(), r.version.to_string()),
        (
            "modifiedAt".to_string(),
            jiff::Timestamp::now().as_second().to_string(),
        ),
    ])
}

fn chart_field(r: &Release, key: &str) -> String {
    r.chart
        .pointer(&format!("/metadata/{key}"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn deployed_at(r: &Release) -> i64 {
    r.info.last_deployed.as_deref().map(unix).unwrap_or(0)
}

pub fn summary(r: &Release) -> HelmRelease {
    HelmRelease {
        name: r.name.clone(),
        namespace: r.namespace.clone(),
        revision: r.version,
        status: r.info.status.clone(),
        chart: chart_field(r, "name"),
        chart_version: chart_field(r, "version"),
        app_version: chart_field(r, "appVersion"),
        updated: deployed_at(r),
        error: None,
    }
}

pub fn revision(r: &Release) -> HelmRevision {
    HelmRevision {
        revision: r.version,
        status: r.info.status.clone(),
        chart_version: chart_field(r, "version"),
        app_version: chart_field(r, "appVersion"),
        description: r.info.description.clone(),
        updated: deployed_at(r),
    }
}

/// The row for a record that cannot be decoded.
pub fn unreadable(name: &str, ns: &str, version: u32, error: String) -> HelmRelease {
    HelmRelease {
        name: name.into(),
        namespace: ns.into(),
        revision: version,
        status: UNREADABLE.into(),
        chart: String::new(),
        chart_version: String::new(),
        app_version: String::new(),
        updated: 0,
        error: Some(error),
    }
}

/// The highest version of each (namespace, name), sorted.
pub fn latest_versions(
    metas: impl IntoIterator<Item = (String, String, u32)>,
) -> Vec<(String, String, u32)> {
    let mut best: BTreeMap<(String, String), u32> = BTreeMap::new();
    for (ns, name, v) in metas {
        let e = best.entry((ns, name)).or_insert(v);
        *e = (*e).max(v);
    }
    best.into_iter()
        .map(|((ns, name), v)| (ns, name, v))
        .collect()
}

/// RFC 3339 to unix seconds; 0 when it does not parse.
pub fn unix(ts: &str) -> i64 {
    ts.parse::<jiff::Timestamp>()
        .map(|t| t.as_second())
        .unwrap_or(0)
}

/// Now, as Helm writes timestamps.
pub fn now() -> String {
    jiff::Timestamp::now().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const FIXTURE: &str = include_str!("../../testdata/helm/release-v2.json");

    /// What Helm writes into `data.release`: base64 of gzip of the JSON.
    fn stored(json: &str) -> Vec<u8> {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(json.as_bytes()).unwrap();
        base64::engine::general_purpose::STANDARD
            .encode(gz.finish().unwrap())
            .into_bytes()
    }

    #[test]
    fn a_helm_record_decodes() {
        let r = decode(&stored(FIXTURE)).unwrap();
        assert_eq!(
            (r.name.as_str(), r.namespace.as_str(), r.version),
            ("demo", "apps", 2)
        );
        assert_eq!(r.info.status, "deployed");
        assert_eq!(r.config["db"]["password"], "hunter2");
        assert!(r.manifest.contains("kind: Deployment"));
    }

    #[test]
    fn re_encoding_loses_nothing() {
        let r = decode(&stored(FIXTURE)).unwrap();
        let back: Value = {
            use std::io::Read;
            let gz = base64::engine::general_purpose::STANDARD
                .decode(encode(&r))
                .unwrap();
            let mut s = String::new();
            flate2::read::GzDecoder::new(&gz[..])
                .read_to_string(&mut s)
                .unwrap();
            serde_json::from_str(&s).unwrap()
        };
        assert_eq!(back, serde_json::from_str::<Value>(FIXTURE).unwrap());
    }

    #[test]
    fn plain_json_without_gzip_decodes() {
        let raw = base64::engine::general_purpose::STANDARD
            .encode(FIXTURE)
            .into_bytes();
        assert_eq!(decode(&raw).unwrap().version, 2);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode(b"not base64 !!").is_err());
        let raw = base64::engine::general_purpose::STANDARD
            .encode("{\"name\": 1}")
            .into_bytes();
        assert!(decode(&raw).is_err());
    }

    #[test]
    fn labels_and_names_follow_helm() {
        let r = decode(&stored(FIXTURE)).unwrap();
        assert_eq!(secret_name("demo", 3), "sh.helm.release.v1.demo.v3");
        let l = labels(&r);
        assert_eq!(l["owner"], "helm");
        assert_eq!(l["name"], "demo");
        assert_eq!(l["status"], "deployed");
        assert_eq!(l["version"], "2");
    }

    #[test]
    fn a_summary_reads_the_chart_metadata() {
        let s = summary(&decode(&stored(FIXTURE)).unwrap());
        assert_eq!(
            (
                s.chart.as_str(),
                s.chart_version.as_str(),
                s.app_version.as_str()
            ),
            ("demo", "0.2.0", "1.1.0")
        );
        assert_eq!(s.revision, 2);
        assert_eq!(s.updated, unix("2026-10-07T09:05:00Z"));
        assert!(s.error.is_none());
    }

    #[test]
    fn timestamps_parse_and_bad_ones_are_zero() {
        assert_eq!(unix("1970-01-01T00:01:00Z"), 60);
        assert_eq!(unix("1970-01-01T01:00:00.999+01:00"), 0);
        assert_eq!(unix(""), 0);
        assert_eq!(unix("yesterday"), 0);
        assert!(unix(&now()) > 1_700_000_000);
    }

    #[test]
    fn only_the_highest_version_of_each_release_is_kept() {
        let got = latest_versions([
            ("b".into(), "x".into(), 1),
            ("a".into(), "y".into(), 3),
            ("a".into(), "y".into(), 10),
            ("a".into(), "y".into(), 9),
        ]);
        assert_eq!(
            got,
            vec![
                ("a".to_string(), "y".to_string(), 10),
                ("b".to_string(), "x".to_string(), 1)
            ]
        );
    }
}
