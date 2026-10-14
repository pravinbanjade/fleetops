//! Rendering a chart with the server's helm (spec 3j §2). helm never
//! touches the cluster here: `helm template` gives the manifest and hooks,
//! `helm install --dry-run=client -o json` the chart object and notes.

use crate::process::{self, RunError, Spec};
use async_trait::async_trait;
use fleetops_core::helm::{ChartMeta, ChartRef, ChartShow, Rendered, RenderedHook};
use fleetops_core::{Error, Result};
use serde_json::Value;
use std::time::Duration;

pub const RENDER_TIMEOUT: Duration = Duration::from_secs(120);
pub const HELM_STDOUT_CAP: usize = 16 << 20;
const STDERR_SHOWN: usize = 2048;
const HOOK: &str = "helm.sh/hook";

pub struct RenderInput {
    pub name: String,
    pub namespace: String,
    pub chart: ChartRef,
    pub values: Value,
    pub upgrade: bool,
    pub kube_version: String,
    pub api_versions: Vec<String>,
}

#[async_trait]
pub trait ChartRenderer: Send + Sync + std::fmt::Debug {
    async fn render(&self, r: &RenderInput) -> Result<Rendered>;
    async fn show(&self, chart: &ChartRef) -> Result<ChartShow>;
    fn insecure_repos(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct HelmCli {
    pub binary: String,
    pub insecure_repos: bool,
    pub timeout: Duration,
}

pub fn chart_args(c: &ChartRef) -> Vec<String> {
    let mut a = match (&c.repo_url, &c.chart, &c.oci) {
        (_, _, Some(o)) => vec![o.clone()],
        (Some(u), Some(ch), _) => vec![ch.clone(), "--repo".into(), u.clone()],
        _ => vec![],
    };
    if let Some(v) = &c.version {
        a.extend(["--version".into(), v.clone()]);
    }
    a
}

pub fn template_args(r: &RenderInput, values_file: &str) -> Vec<String> {
    let mut a = vec!["template".to_string(), r.name.clone()];
    a.extend(chart_args(&r.chart));
    a.extend([
        "--namespace".into(),
        r.namespace.clone(),
        "--values".into(),
        values_file.into(),
    ]);
    a.extend(["--kube-version".into(), r.kube_version.clone()]);
    for v in &r.api_versions {
        a.extend(["--api-versions".into(), v.clone()]);
    }
    a.push("--include-crds".into());
    if r.upgrade {
        a.push("--is-upgrade".into());
    }
    a
}

pub fn dry_run_args(r: &RenderInput, values_file: &str) -> Vec<String> {
    let mut a = vec!["install".to_string(), r.name.clone()];
    a.extend(chart_args(&r.chart));
    a.extend([
        "--namespace".into(),
        r.namespace.clone(),
        "--values".into(),
        values_file.into(),
        "--dry-run=client".into(),
        "--output".into(),
        "json".into(),
    ]);
    a
}

/// `helm template`'s documents: the release manifest in Helm's stored form
/// (`---\n# Source: …\n<yaml>`), and the hooks.
pub fn split_template(out: &str) -> Result<(String, Vec<RenderedHook>)> {
    let mut manifest = String::new();
    let mut hooks = Vec::new();
    let mut source = String::new();
    for chunk in out
        .split("\n---")
        .flat_map(|c| c.strip_prefix("---").or(Some(c)))
    {
        let mut body = String::new();
        for line in chunk.lines() {
            match line.strip_prefix("# Source: ") {
                Some(s) if body.trim().is_empty() => source = s.trim().to_string(),
                _ => {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
        if body.trim().is_empty() {
            continue;
        }
        let v: Value = serde_saphyr::from_str(&body).map_err(|e| {
            Error::Invalid(format!(
                "helm printed a document that is not YAML ({source}): {e}"
            ))
        })?;
        if v.is_null() {
            continue;
        }
        let ann = |k: &str| {
            v.pointer("/metadata/annotations")
                .and_then(|a| a.get(k))
                .and_then(Value::as_str)
        };
        match ann(HOOK) {
            Some(events) => hooks.push(RenderedHook {
                name: v
                    .pointer("/metadata/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                kind: v["kind"].as_str().unwrap_or_default().into(),
                path: source.clone(),
                manifest: body.trim_start_matches('\n').to_string(),
                events: list(events),
                weight: ann("helm.sh/hook-weight")
                    .and_then(|w| w.trim().parse().ok())
                    .unwrap_or(0),
                delete_policies: ann("helm.sh/hook-delete-policy")
                    .map(list)
                    .unwrap_or_default(),
            }),
            None => {
                manifest.push_str(&format!(
                    "---\n# Source: {source}\n{}",
                    body.trim_start_matches('\n')
                ));
            }
        }
    }
    Ok((manifest, hooks))
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// The chart object Helm stores in a release, its notes, and its metadata.
pub fn from_dry_run(json: &[u8]) -> Result<(Value, String, ChartMeta)> {
    let v: Value = serde_json::from_slice(json)
        .map_err(|e| Error::Invalid(format!("helm's dry run printed something unexpected: {e}")))?;
    let chart = v["chart"].clone();
    let m = &chart["metadata"];
    let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
    let meta = ChartMeta {
        name: s("name"),
        version: s("version"),
        app_version: s("appVersion"),
        description: s("description"),
    };
    Ok((
        chart,
        v["info"]["notes"].as_str().unwrap_or_default().to_string(),
        meta,
    ))
}

/// helm's stderr for a message: cut, and any secret value replaced.
pub fn shown_stderr(stderr: &str, values: &Value) -> String {
    // Scrub before cutting, so a secret the cut splits is not half shown.
    let mut s = stderr.trim().to_string();
    for secret in super::values::secret_strings(values) {
        s = s.replace(&secret, fleetops_core::kube::MASK);
    }
    if s.len() > STDERR_SHOWN {
        let mut cut = STDERR_SHOWN;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

impl HelmCli {
    async fn helm(
        &self,
        args: &[String],
        env: &[(String, String)],
        values: &Value,
    ) -> Result<Vec<u8>> {
        let spec = Spec {
            program: &self.binary,
            args,
            env,
            timeout: self.timeout,
            stdout_cap: HELM_STDOUT_CAP,
        };
        let out = process::run(spec).await.map_err(|e| match e {
            RunError::NotFound => {
                Error::Invalid(format!("helm was not found at '{}'", self.binary))
            }
            RunError::TimedOut => Error::Invalid(format!(
                "helm did not finish within {}s",
                self.timeout.as_secs()
            )),
            RunError::TooLarge => Error::Invalid(format!(
                "helm printed more than {} MiB",
                HELM_STDOUT_CAP >> 20
            )),
            RunError::Io(m) => Error::Internal(format!("helm could not run: {m}")),
        })?;
        if !out.success() {
            return Err(Error::Invalid(format!(
                "helm: {}",
                shown_stderr(&out.stderr, values)
            )));
        }
        Ok(out.stdout)
    }
}

/// A private directory for one request: helm's cache and the values file.
fn workdir() -> Result<(tempfile::TempDir, Vec<(String, String)>)> {
    let dir = tempfile::tempdir()
        .map_err(|e| Error::Internal(format!("cannot make a temporary directory: {e}")))?;
    let env = vec![(
        "HELM_CACHE_HOME".to_string(),
        dir.path().join("cache").display().to_string(),
    )];
    Ok((dir, env))
}

fn write_values(dir: &std::path::Path, values: &Value) -> Result<String> {
    use std::io::Write;
    let path = dir.join("values.yaml");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let text = serde_json::to_string(values).expect("values serialise"); // JSON is YAML
    opts.open(&path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| Error::Internal(format!("cannot write the values file: {e}")))?;
    Ok(path.display().to_string())
}

#[async_trait]
impl ChartRenderer for HelmCli {
    async fn render(&self, r: &RenderInput) -> Result<Rendered> {
        let (dir, env) = workdir()?;
        let file = write_values(dir.path(), &r.values)?;
        let out = self.helm(&template_args(r, &file), &env, &r.values).await?;
        let (manifest, hooks) = split_template(&String::from_utf8_lossy(&out))?;
        let json = self.helm(&dry_run_args(r, &file), &env, &r.values).await?;
        let (chart, notes, meta) = from_dry_run(&json)?;
        Ok(Rendered {
            manifest,
            hooks,
            notes,
            chart,
            meta,
        })
    }

    async fn show(&self, chart: &ChartRef) -> Result<ChartShow> {
        let (_dir, env) = workdir()?;
        let none = Value::Null;
        let mut a = vec!["show".to_string(), "chart".to_string()];
        a.extend(chart_args(chart));
        let meta_yaml = self.helm(&a, &env, &none).await?;
        let m: Value = serde_saphyr::from_slice(&meta_yaml).map_err(|e| {
            Error::Invalid(format!("helm show chart printed something unexpected: {e}"))
        })?;
        let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
        a[1] = "values".into();
        let values = String::from_utf8_lossy(&self.helm(&a, &env, &none).await?).into_owned();
        Ok(ChartShow {
            meta: ChartMeta {
                name: s("name"),
                version: s("version"),
                app_version: s("appVersion"),
                description: s("description"),
            },
            values,
        })
    }

    fn insecure_repos(&self) -> bool {
        self.insecure_repos
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(chart: ChartRef, upgrade: bool) -> RenderInput {
        RenderInput {
            name: "rel".into(),
            namespace: "apps".into(),
            chart,
            values: json!({}),
            upgrade,
            kube_version: "v1.32.0".into(),
            api_versions: vec!["v1".into(), "apps/v1".into()],
        }
    }
    fn repo() -> ChartRef {
        ChartRef {
            repo_url: Some("https://c.example".into()),
            chart: Some("demo".into()),
            version: Some("0.2.0".into()),
            oci: None,
        }
    }

    #[test]
    fn chart_args_for_a_repo_and_an_oci_reference() {
        assert_eq!(
            chart_args(&repo()),
            ["demo", "--repo", "https://c.example", "--version", "0.2.0"]
        );
        let o = ChartRef {
            oci: Some("oci://r/c".into()),
            ..Default::default()
        };
        assert_eq!(chart_args(&o), ["oci://r/c"]);
    }

    #[test]
    fn template_args_carry_capabilities_and_upgrade() {
        let a = template_args(&input(repo(), true), "/tmp/v.yaml");
        assert_eq!(&a[..2], ["template", "rel"]);
        for want in [
            vec!["--namespace", "apps"],
            vec!["--values", "/tmp/v.yaml"],
            vec!["--kube-version", "v1.32.0"],
            vec!["--api-versions", "v1"],
            vec!["--api-versions", "apps/v1"],
        ] {
            assert!(
                a.windows(2).any(|w| w == want.as_slice()),
                "{want:?} in {a:?}"
            );
        }
        assert!(
            a.contains(&"--include-crds".to_string()) && a.contains(&"--is-upgrade".to_string())
        );
        assert!(!template_args(&input(repo(), false), "/v").contains(&"--is-upgrade".to_string()));
    }

    #[test]
    fn dry_run_args_never_reach_a_cluster() {
        let a = dry_run_args(&input(repo(), false), "/tmp/v.yaml");
        assert_eq!(&a[..2], ["install", "rel"]);
        assert!(a.contains(&"--dry-run=client".to_string()));
        assert!(a.windows(2).any(|w| w == ["--output", "json"]));
    }

    #[test]
    fn template_output_splits_into_manifest_and_hooks() {
        let (manifest, hooks) =
            split_template(include_str!("../../testdata/helm/template-out.yaml")).unwrap();
        assert!(
            manifest.contains("# Source: demo/templates/deploy.yaml")
                && manifest.contains("kind: Deployment")
        );
        assert!(
            !manifest.contains("helm.sh/hook"),
            "hooks are not part of the manifest"
        );
        assert_eq!(hooks.len(), 1);
        let h = &hooks[0];
        assert_eq!(
            (h.kind.as_str(), h.name.as_str(), h.weight),
            ("Job", "rel-migrate", -5)
        );
        assert_eq!(h.events, ["pre-install", "pre-upgrade"]);
        assert_eq!(h.delete_policies, ["hook-succeeded"]);
        assert_eq!(h.path, "demo/templates/hook.yaml");
        assert!(h.manifest.contains("kind: Job"));
    }

    #[test]
    fn a_document_without_its_own_source_keeps_the_previous_one() {
        let out = "---\n# Source: c/templates/two.yaml\napiVersion: v1\nkind: ConfigMap\nmetadata: {name: a}\n---\napiVersion: v1\nkind: ConfigMap\nmetadata: {name: b}\n";
        let (m, _) = split_template(out).unwrap();
        assert_eq!(m.matches("# Source: c/templates/two.yaml").count(), 2);
    }

    #[test]
    fn the_dry_run_gives_the_chart_and_notes() {
        let (chart, notes, meta) =
            from_dry_run(include_bytes!("../../testdata/helm/dry-run.json")).unwrap();
        assert_eq!(chart["metadata"]["name"], "demo");
        assert_eq!(notes, "Hello rel\n");
        assert_eq!(
            (
                meta.name.as_str(),
                meta.version.as_str(),
                meta.app_version.as_str()
            ),
            ("demo", "0.1.0", "1.0.0")
        );
        assert!(from_dry_run(b"not json").is_err());
    }

    #[test]
    fn stderr_is_cut_and_never_shows_a_secret_value() {
        let values = json!({"db": {"password": "hunter2"}, "image": "busybox"});
        let s = shown_stderr(
            &format!("Error: bad value hunter2 for busybox\n{}", "x".repeat(5000)),
            &values,
        );
        assert!(!s.contains("hunter2") && s.contains("busybox"));
        assert!(s.len() <= 2048 + 3);
    }
}
