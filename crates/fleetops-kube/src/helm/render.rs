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
    /// The full values (masks restored): for scrubbing messages.
    pub values: Value,
    /// The values as the user typed them: helm's first values file, so
    /// helm parses them by its own rules (`0644` is octal there).
    pub values_yaml: String,
    /// What was restored from the stored release: helm's second values file.
    pub overlay: Option<Value>,
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
    /// Helm 3's dry-run post-renderer for charts with CRDs; see [`Discard`].
    pub discard: Option<Discard>,
}

/// A program that reads a manifest on stdin and prints nothing. Helm 3's
/// `install --dry-run=client` resolves every rendered object's kind against
/// the cluster, so a chart whose templates use a kind from its own `crds/`
/// fails before that CRD exists. The dry run is only for the chart object,
/// notes and values, so handing its objects to this post-renderer loses
/// nothing. Helm 4 needs none, and takes only plugins as post-renderers.
#[derive(Debug, Clone)]
pub struct Discard {
    pub program: String,
    pub args: Vec<String>,
}

/// The `fleetops` argument that makes the binary act as the [`Discard`].
pub const DISCARD_ARG: &str = "__helm-post-render-discard";

pub fn discard_args(d: &Discard) -> Vec<String> {
    let mut a = vec!["--post-renderer".to_string(), d.program.clone()];
    for x in &d.args {
        a.extend(["--post-renderer-args".to_string(), x.clone()]);
    }
    a
}

/// Whether a dry run failed the Helm 3 way described on [`Discard`].
fn needs_discard(e: &Error) -> bool {
    matches!(e, Error::Invalid(m) if m.contains("ensure CRDs are installed first"))
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

fn values_args(files: &[String]) -> Vec<String> {
    files
        .iter()
        .flat_map(|f| ["--values".to_string(), f.clone()])
        .collect()
}

pub fn template_args(r: &RenderInput, values_files: &[String]) -> Vec<String> {
    let mut a = vec!["template".to_string(), r.name.clone()];
    a.extend(chart_args(&r.chart));
    a.extend(["--namespace".into(), r.namespace.clone()]);
    a.extend(values_args(values_files));
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

pub fn dry_run_args(r: &RenderInput, values_files: &[String]) -> Vec<String> {
    let mut a = vec!["install".to_string(), r.name.clone()];
    a.extend(chart_args(&r.chart));
    a.extend(["--namespace".into(), r.namespace.clone()]);
    a.extend(values_args(values_files));
    a.extend(["--dry-run=client".into(), "--output".into(), "json".into()]);
    a
}

/// Whether a `# Source:` path is in a chart's (or subchart's) `crds/` folder.
fn crd_source(source: &str) -> bool {
    let parts: Vec<&str> = source.split('/').collect();
    parts
        .iter()
        .enumerate()
        .any(|(i, p)| *p == "crds" && (i == 1 || (i >= 3 && parts[i - 2] == "charts")))
}

/// `helm template`'s documents: the release manifest in Helm's stored form
/// (`---\n# Source: …\n<yaml>`), the hooks, and the `crds/` documents,
/// which Helm keeps out of the manifest.
pub fn split_template(out: &str) -> Result<(String, Vec<RenderedHook>, Vec<String>)> {
    let mut manifest = String::new();
    let mut hooks = Vec::new();
    let mut crds = Vec::new();
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
        if crd_source(&source) {
            crds.push(body.trim_start_matches('\n').to_string());
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
    Ok((manifest, hooks, crds))
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// The chart object Helm stores in a release, its notes, its metadata, and
/// the values as helm parsed them.
pub fn from_dry_run(json: &[u8]) -> Result<(Value, String, ChartMeta, Value)> {
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
    let config = match &v["config"] {
        Value::Object(_) => v["config"].clone(),
        _ => Value::Object(Default::default()),
    };
    Ok((
        chart,
        v["info"]["notes"].as_str().unwrap_or_default().to_string(),
        meta,
        config,
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

/// One helm run's private directory: the values files, an empty working
/// directory (so no local chart is ever picked up), and its own repository
/// list and index cache. helm's configuration home is left alone, so
/// `helm registry login` still applies to OCI registries.
struct Work {
    dir: tempfile::TempDir,
    env: Vec<(String, String)>,
    cwd: std::path::PathBuf,
}

fn workdir() -> Result<Work> {
    let fail =
        |e: std::io::Error| Error::Internal(format!("cannot prepare a directory for helm: {e}"));
    let dir = tempfile::tempdir().map_err(fail)?;
    let cwd = dir.path().join("work");
    std::fs::create_dir(&cwd).map_err(fail)?;
    let repos = dir.path().join("repositories.yaml");
    std::fs::write(&repos, "").map_err(fail)?;
    let env = vec![
        (
            "HELM_REPOSITORY_CONFIG".to_string(),
            repos.display().to_string(),
        ),
        (
            "HELM_REPOSITORY_CACHE".to_string(),
            dir.path().join("repository").display().to_string(),
        ),
    ];
    Ok(Work { dir, env, cwd })
}

fn write_file(dir: &std::path::Path, name: &str, text: &str) -> Result<String> {
    use std::io::Write;
    let path = dir.join(name);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(&path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| Error::Internal(format!("cannot write a values file: {e}")))?;
    Ok(path.display().to_string())
}

impl HelmCli {
    async fn helm(&self, args: &[String], w: &Work, values: &Value) -> Result<Vec<u8>> {
        let spec = Spec {
            program: &self.binary,
            args,
            env: &w.env,
            timeout: self.timeout,
            stdout_cap: HELM_STDOUT_CAP,
            cwd: Some(&w.cwd),
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

#[async_trait]
impl ChartRenderer for HelmCli {
    async fn render(&self, r: &RenderInput) -> Result<Rendered> {
        let w = workdir()?;
        let mut files = vec![write_file(w.dir.path(), "values.yaml", &r.values_yaml)?];
        if let Some(o) = &r.overlay {
            let text = serde_json::to_string(o).expect("values serialise"); // JSON is YAML
            files.push(write_file(w.dir.path(), "restored.json", &text)?);
        }
        let out = self.helm(&template_args(r, &files), &w, &r.values).await?;
        let (manifest, hooks, crds) = split_template(&String::from_utf8_lossy(&out))?;
        let mut args = dry_run_args(r, &files);
        let json = match (self.helm(&args, &w, &r.values).await, &self.discard) {
            (Err(e), Some(d)) if needs_discard(&e) => {
                args.extend(discard_args(d));
                self.helm(&args, &w, &r.values).await?
            }
            (out, _) => out?,
        };
        let (chart, notes, meta, config) = from_dry_run(&json)?;
        Ok(Rendered {
            manifest,
            hooks,
            notes,
            chart,
            meta,
            config,
            crds,
        })
    }

    async fn show(&self, chart: &ChartRef) -> Result<ChartShow> {
        let w = workdir()?;
        let none = Value::Null;
        let mut a = vec!["show".to_string(), "chart".to_string()];
        a.extend(chart_args(chart));
        let meta_yaml = self.helm(&a, &w, &none).await?;
        let m: Value = serde_saphyr::from_slice(&meta_yaml).map_err(|e| {
            Error::Invalid(format!("helm show chart printed something unexpected: {e}"))
        })?;
        let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
        a[1] = "values".into();
        let values = String::from_utf8_lossy(&self.helm(&a, &w, &none).await?).into_owned();
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
            values_yaml: String::new(),
            overlay: None,
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
        let a = template_args(&input(repo(), true), &["/tmp/v.yaml".into()]);
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
        assert!(!template_args(&input(repo(), false), &["/v".into()])
            .contains(&"--is-upgrade".to_string()));
    }

    #[test]
    fn dry_run_args_never_reach_a_cluster() {
        let a = dry_run_args(&input(repo(), false), &["/tmp/v.yaml".into()]);
        assert_eq!(&a[..2], ["install", "rel"]);
        assert!(a.contains(&"--dry-run=client".to_string()));
        assert!(a.windows(2).any(|w| w == ["--output", "json"]));
    }

    #[test]
    fn discard_args_name_the_program_and_each_argument() {
        let d = Discard {
            program: "/usr/bin/fleetops".into(),
            args: vec![DISCARD_ARG.into(), "x y".into()],
        };
        assert_eq!(
            discard_args(&d),
            [
                "--post-renderer",
                "/usr/bin/fleetops",
                "--post-renderer-args",
                DISCARD_ARG,
                "--post-renderer-args",
                "x y"
            ]
        );
    }

    #[test]
    fn only_helm_3s_missing_crd_failure_retries_with_the_discard() {
        let helm3 = Error::Invalid("helm: Error: INSTALLATION FAILED: unable to build kubernetes objects from release manifest: resource mapping not found for name: \"a\" namespace: \"\" from \"\": no matches for kind \"Gadget\" in version \"fo.example/v1\"\nensure CRDs are installed first".into());
        assert!(needs_discard(&helm3));
        assert!(!needs_discard(&Error::Invalid(
            "helm: Error: INSTALLATION FAILED: chart \"x\" not found".into()
        )));
        assert!(!needs_discard(&Error::Internal(
            "ensure CRDs are installed first".into()
        )));
    }

    #[test]
    fn template_output_splits_into_manifest_and_hooks() {
        let (manifest, hooks, _) =
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
        let (m, _, _) = split_template(out).unwrap();
        assert_eq!(m.matches("# Source: c/templates/two.yaml").count(), 2);
    }

    #[test]
    fn the_dry_run_gives_the_chart_and_notes() {
        let (chart, notes, meta, _) =
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

    #[test]
    fn chart_crds_are_kept_apart_from_the_manifest() {
        let out = "---\n# Source: op/crds/gadgets.yaml\napiVersion: apiextensions.k8s.io/v1\nkind: CustomResourceDefinition\nmetadata: {name: gadgets.fo.example}\n---\n# Source: op/charts/sub/crds/more.yaml\napiVersion: apiextensions.k8s.io/v1\nkind: CustomResourceDefinition\nmetadata: {name: more.fo.example}\n---\n# Source: op/templates/crds/not-a-crd-folder.yaml\napiVersion: v1\nkind: ConfigMap\nmetadata: {name: c}\n";
        let (manifest, hooks, crds) = split_template(out).unwrap();
        assert!(hooks.is_empty());
        assert_eq!(crds.len(), 2);
        assert!(crds[0].contains("gadgets.fo.example") && crds[1].contains("more.fo.example"));
        assert!(
            !manifest.contains("CustomResourceDefinition") && manifest.contains("kind: ConfigMap")
        );
    }

    #[test]
    fn a_second_values_file_carries_the_restored_secrets() {
        let a = template_args(
            &input(repo(), false),
            &["/t/values.yaml".into(), "/t/restored.json".into()],
        );
        assert!(a.windows(2).any(|w| w == ["--values", "/t/values.yaml"]));
        assert!(a.windows(2).any(|w| w == ["--values", "/t/restored.json"]));
    }

    #[test]
    fn helm_config_comes_from_helms_own_parse() {
        let json = br#"{"chart": {"metadata": {"name": "demo"}}, "info": {"notes": ""}, "config": {"mode": 420}}"#;
        let (_, _, _, config) = from_dry_run(json).unwrap();
        assert_eq!(config, json!({"mode": 420}));
        let (_, _, _, none) = from_dry_run(br#"{"chart": {}, "info": {}}"#).unwrap();
        assert_eq!(none, json!({}));
    }

    #[test]
    fn each_run_has_its_own_repositories_and_an_empty_working_directory() {
        let w = workdir().unwrap();
        let get = |k: &str| {
            w.env
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        let repos = std::path::PathBuf::from(get("HELM_REPOSITORY_CONFIG"));
        assert_eq!(std::fs::read_to_string(&repos).unwrap(), "");
        assert!(get("HELM_REPOSITORY_CACHE").starts_with(&*w.dir.path().to_string_lossy()));
        assert_eq!(std::fs::read_dir(&w.cwd).unwrap().count(), 0);
    }
}
