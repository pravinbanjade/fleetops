//! Kubeconfig handling for `kube-config` environments (spec §4.1): pick a
//! context, refuse users that need a helper binary or local files (the
//! server is distroless), and reduce the file to that one context.

use crate::exec_auth::{ExecAuthLayer, ExecPolicy, ProcessRunner, TokenSource};
use base64::{engine::general_purpose::STANDARD, Engine};
use fleetops_core::env::{KubeconfigSummary, UserKind};
use fleetops_core::{Error, Result};
use kube::config::{AuthInfo, Cluster, Kubeconfig, NamedAuthInfo, NamedCluster, NamedContext};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::sync::Arc;
use std::time::Duration;

pub const UNSUPPORTED_USER: &str =
    "use a ServiceAccount token, an exec plugin (--kube-exec-plugins) or the FleetOps agent: auth-provider users are not supported";
/// How long the create-time reachability check waits.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn parse(text: &str) -> Result<Kubeconfig> {
    Kubeconfig::from_yaml(text).map_err(|e| invalid(format!("not a kubeconfig: {e}")))
}

pub struct Contexts {
    pub contexts: Vec<String>,
    pub current: Option<String>,
}

pub fn contexts(text: &str) -> Result<Contexts> {
    let kc = parse(text)?;
    let contexts: Vec<String> = kc.contexts.iter().map(|c| c.name.clone()).collect();
    if contexts.is_empty() {
        return Err(invalid("the kubeconfig has no contexts"));
    }
    let current = kc.current_context.filter(|c| contexts.contains(c));
    Ok(Contexts { contexts, current })
}

pub struct Prepared {
    /// A self-contained kubeconfig with exactly one context.
    pub yaml: String,
    pub summary: KubeconfigSummary,
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prepared")
            .field("yaml", &"<redacted>")
            .field("summary", &self.summary)
            .finish()
    }
}

fn pick(kc: &Kubeconfig, context: Option<&str>) -> Result<String> {
    let names: Vec<&str> = kc.contexts.iter().map(|c| c.name.as_str()).collect();
    match context {
        Some(c) if names.contains(&c) => Ok(c.to_string()),
        Some(c) => Err(invalid(format!(
            "unknown context '{c}'; the kubeconfig has: {}",
            names.join(", ")
        ))),
        None => match (kc.current_context.as_deref(), names.as_slice()) {
            (Some(cur), _) if names.contains(&cur) => Ok(cur.to_string()),
            (_, [only]) => Ok(only.to_string()),
            (_, []) => Err(invalid("the kubeconfig has no contexts")),
            _ => Err(invalid(format!("choose a context: {}", names.join(", ")))),
        },
    }
}

fn file_ref(field: &str) -> Error {
    invalid(format!(
        "kubeconfig must embed its files: '{field}' points to a file"
    ))
}

/// The user's kind, and an exec user's command (spec 3k §1).
fn check_user(
    context: &str,
    user: &AuthInfo,
    cluster: &Cluster,
    policy: &ExecPolicy,
) -> Result<(UserKind, Option<String>)> {
    if user.auth_provider.is_some() {
        return Err(invalid(UNSUPPORTED_USER));
    }
    if user.token_file.is_some() {
        return Err(file_ref("tokenFile"));
    }
    if user.client_certificate.is_some() {
        return Err(file_ref("client-certificate"));
    }
    if user.client_key.is_some() {
        return Err(file_ref("client-key"));
    }
    if let Some(exec) = &user.exec {
        let plugin = policy.check(exec, cluster)?;
        return Ok((UserKind::Exec, Some(plugin.command)));
    }
    if user.token.is_some() {
        Ok((UserKind::Token, None))
    } else if user.client_certificate_data.is_some() && user.client_key_data.is_some() {
        Ok((UserKind::Cert, None))
    } else {
        Err(invalid(format!(
            "the user in context '{context}' has no token or client certificate"
        )))
    }
}

pub fn prepare(text: &str, context: Option<&str>, policy: &ExecPolicy) -> Result<Prepared> {
    let kc = parse(text)?;
    let name = pick(&kc, context)?;
    let ctx = kc.contexts.iter().find(|c| c.name == name).expect("picked");
    let inner = ctx
        .context
        .clone()
        .ok_or_else(|| invalid(format!("context '{name}' is empty")))?;
    let cluster = kc
        .clusters
        .iter()
        .find(|c| c.name == inner.cluster)
        .ok_or_else(|| {
            invalid(format!(
                "context '{name}' names a missing cluster '{}'",
                inner.cluster
            ))
        })?;
    let cl = cluster
        .cluster
        .clone()
        .ok_or_else(|| invalid(format!("cluster '{}' is empty", cluster.name)))?;
    if cl.certificate_authority.is_some() {
        return Err(file_ref("certificate-authority"));
    }
    let server = cl
        .server
        .clone()
        .ok_or_else(|| invalid(format!("the cluster in context '{name}' has no server")))?;
    let user_name = inner.user.clone().unwrap_or_default();
    let user = kc
        .auth_infos
        .iter()
        .find(|u| u.name == user_name)
        .ok_or_else(|| {
            invalid(format!(
                "context '{name}' names a missing user '{user_name}'"
            ))
        })?;
    let auth = user.auth_info.clone().unwrap_or_default();
    let (user_kind, exec_command) = check_user(&name, &auth, &cl, policy)?;

    let minified = Kubeconfig {
        api_version: Some("v1".into()),
        kind: Some("Config".into()),
        current_context: Some(name.clone()),
        clusters: vec![NamedCluster {
            name: cluster.name.clone(),
            cluster: Some(cl),
            ..cluster.clone()
        }],
        auth_infos: vec![NamedAuthInfo {
            name: user.name.clone(),
            auth_info: Some(auth),
            ..user.clone()
        }],
        contexts: vec![NamedContext {
            name: name.clone(),
            context: Some(inner),
            ..ctx.clone()
        }],
        ..Kubeconfig::default()
    };
    let yaml = serde_saphyr::to_string(&minified)
        .map_err(|e| Error::Internal(format!("cannot write kubeconfig: {e}")))?;
    Ok(Prepared {
        yaml,
        summary: KubeconfigSummary {
            server,
            context: name,
            user_kind,
            exec_command,
        },
    })
}

/// A kube-rs config for a prepared kubeconfig, built without I/O so the
/// environment factory (synchronous) can use it.
pub fn config(yaml: &str) -> Result<kube::Config> {
    let kc = parse(yaml)?;
    let ctx = kc
        .contexts
        .first()
        .and_then(|c| c.context.clone())
        .ok_or_else(|| invalid("stored kubeconfig has no context"))?;
    let cl = kc
        .clusters
        .first()
        .and_then(|c| c.cluster.clone())
        .ok_or_else(|| invalid("stored kubeconfig has no cluster"))?;
    let server = cl.server.clone().unwrap_or_default();
    let mut c = kube::Config::new(
        server
            .parse()
            .map_err(|e| invalid(format!("bad server URL '{server}': {e}")))?,
    );
    c.default_namespace = ctx.namespace.unwrap_or_else(|| "default".into());
    c.accept_invalid_certs = cl.insecure_skip_tls_verify.unwrap_or(false);
    c.tls_server_name = cl.tls_server_name.clone();
    c.connect_timeout = Some(CHECK_TIMEOUT);
    if let Some(data) = cl
        .certificate_authority_data
        .as_deref()
        .filter(|d| !d.is_empty())
    {
        let pem = STANDARD
            .decode(data)
            .map_err(|_| invalid("certificate-authority-data is not base64"))?;
        let certs: Vec<Vec<u8>> = CertificateDer::pem_slice_iter(&pem)
            .filter_map(|c| c.ok())
            .map(|c| c.to_vec())
            .collect();
        c.root_cert = Some(certs);
    }
    c.auth_info = kc
        .auth_infos
        .first()
        .and_then(|u| u.auth_info.clone())
        .unwrap_or_default();
    Ok(c)
}

/// The kube-rs client for a stored kubeconfig. An exec user's plugin is
/// checked against `policy` now and run on the first request (spec 3k §2).
pub fn client(yaml: &str, policy: &ExecPolicy) -> Result<kube::Client> {
    let mut c = config(yaml)?;
    let Some(exec) = c.auth_info.exec.take() else {
        return kube::Client::try_from(c).map_err(|e| invalid(e.to_string()));
    };
    let cluster = parse(yaml)?
        .clusters
        .first()
        .and_then(|c| c.cluster.clone())
        .unwrap_or_default();
    let plugin = policy.check(&exec, &cluster)?;
    let runner = Arc::new(ProcessRunner {
        timeout: policy.timeout(),
    });
    let source = Arc::new(TokenSource::new(plugin, runner));
    let builder = kube::client::ClientBuilder::try_from(c).map_err(|e| invalid(e.to_string()))?;
    Ok(builder.with_layer(&ExecAuthLayer(source)).build())
}

/// `GET /version` with a short limit: used before saving a kubeconfig.
pub async fn check(yaml: &str, policy: &ExecPolicy) -> Result<()> {
    let client = client(yaml, policy)?;
    match tokio::time::timeout(CHECK_TIMEOUT, client.apiserver_version()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(kube::Error::Api(s))) if s.code == 401 => Err(invalid(
            "cannot reach cluster: credentials rejected (401): check the token or client certificate",
        )),
        Ok(Err(kube::Error::Service(e)))
            if e.downcast_ref::<crate::exec_auth::ExecError>().is_some() =>
        {
            Err(invalid(e.to_string()))
        }
        Ok(Err(e)) => Err(invalid(format!("cannot reach cluster: {e}"))),
        Err(_) => Err(invalid(format!(
            "cannot reach cluster: no answer within {}s",
            CHECK_TIMEOUT.as_secs()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec_auth::ExecPolicy;
    use fleetops_core::env::UserKind;

    fn kc(users: &str, current: Option<&str>) -> String {
        format!(
            "apiVersion: v1\nkind: Config\n{}clusters:\n- name: a\n  cluster: {{server: 'https://a.example:6443', certificate-authority-data: ''}}\n- name: b\n  cluster: {{server: 'https://b.example:6443'}}\ncontexts:\n- name: ctx-a\n  context: {{cluster: a, user: ua, namespace: apps}}\n- name: ctx-b\n  context: {{cluster: b, user: ub}}\nusers:\n{users}",
            current.map(|c| format!("current-context: {c}\n")).unwrap_or_default()
        )
    }

    const TOKENS: &str =
        "- name: ua\n  user: {token: token-a}\n- name: ub\n  user: {token: token-b}\n";

    #[test]
    fn contexts_lists_names_and_the_current_one() {
        let c = contexts(&kc(TOKENS, Some("ctx-b"))).unwrap();
        assert_eq!(c.contexts, ["ctx-a", "ctx-b"]);
        assert_eq!(c.current.as_deref(), Some("ctx-b"));
        assert!(contexts("not: [yaml").is_err());
    }

    #[test]
    fn prepare_keeps_only_the_chosen_context() {
        let p = prepare(&kc(TOKENS, None), Some("ctx-a"), &ExecPolicy::default()).unwrap();
        assert_eq!(p.summary.server, "https://a.example:6443");
        assert_eq!(p.summary.context, "ctx-a");
        assert_eq!(p.summary.user_kind, UserKind::Token);
        assert!(p.yaml.contains("token-a"));
        assert!(
            !p.yaml.contains("token-b") && !p.yaml.contains("b.example"),
            "{}",
            p.yaml
        );
        // The minified file is itself a valid one-context kubeconfig.
        let again = contexts(&p.yaml).unwrap();
        assert_eq!(again.contexts, ["ctx-a"]);
        assert_eq!(again.current.as_deref(), Some("ctx-a"));
    }

    #[test]
    fn without_a_choice_the_current_context_wins_and_ambiguity_is_refused() {
        assert_eq!(
            prepare(&kc(TOKENS, Some("ctx-b")), None, &ExecPolicy::default())
                .unwrap()
                .summary
                .context,
            "ctx-b"
        );
        let e = prepare(&kc(TOKENS, None), None, &ExecPolicy::default())
            .unwrap_err()
            .to_string();
        assert!(e.contains("choose a context: ctx-a, ctx-b"), "{e}");
        let e = prepare(&kc(TOKENS, None), Some("nope"), &ExecPolicy::default())
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown context 'nope'"), "{e}");
    }

    const EXEC: &str = "- name: ua\n  user:\n    exec: {apiVersion: client.authentication.k8s.io/v1, command: aws, args: [eks, get-token, --cluster-name, c]}\n- name: ub\n  user: {token: x}\n";

    #[test]
    fn an_exec_user_needs_the_flag() {
        let off = ExecPolicy::default();
        let e = prepare(&kc(EXEC, None), Some("ctx-a"), &off)
            .unwrap_err()
            .to_string();
        assert!(e.contains(crate::exec_auth::OFF), "{e}");
        let on = ExecPolicy::parse("default").unwrap();
        let p = prepare(&kc(EXEC, None), Some("ctx-a"), &on).unwrap();
        assert_eq!(p.summary.user_kind, UserKind::Exec);
        assert_eq!(p.summary.exec_command.as_deref(), Some("aws"));
        assert!(p.yaml.contains("get-token"), "the exec block is stored");
    }

    #[test]
    fn a_disallowed_exec_user_says_which_rule() {
        let on = ExecPolicy::parse("default").unwrap();
        let bad = EXEC.replace("[eks, get-token, --cluster-name, c]", "[s3, ls]");
        let e = prepare(&kc(&bad, None), Some("ctx-a"), &on)
            .unwrap_err()
            .to_string();
        assert!(e.contains("may only run 'eks get-token'"), "{e}");
    }

    #[test]
    fn auth_provider_users_are_still_refused() {
        let ap =
            "- name: ua\n  user:\n    auth-provider: {name: gcp}\n- name: ub\n  user: {token: x}\n";
        let on = ExecPolicy::parse("default").unwrap();
        let e = prepare(&kc(ap, None), Some("ctx-a"), &on)
            .unwrap_err()
            .to_string();
        assert!(e.contains("auth-provider"), "{e}");
    }

    #[test]
    fn token_summaries_have_no_exec_command_on_the_wire() {
        let p = prepare(&kc(TOKENS, None), Some("ctx-a"), &ExecPolicy::default()).unwrap();
        assert_eq!(
            serde_json::to_value(&p.summary).unwrap(),
            serde_json::json!({"server": "https://a.example:6443", "context": "ctx-a", "user_kind": "token"})
        );
    }

    #[tokio::test]
    async fn a_client_for_an_exec_user_is_built_without_running_it() {
        // `fleetops-no-such-plugin` would fail if run: building must not run it.
        let on = ExecPolicy::parse("fleetops-no-such-plugin").unwrap();
        let u = "- name: ua\n  user:\n    exec: {apiVersion: client.authentication.k8s.io/v1, command: fleetops-no-such-plugin}\n- name: ub\n  user: {token: x}\n";
        let p = prepare(&kc(u, None), Some("ctx-a"), &on).unwrap();
        let c = client(&p.yaml, &on).unwrap();
        let e = c.apiserver_version().await.unwrap_err().to_string();
        assert!(
            e.contains("exec plugin 'fleetops-no-such-plugin' was not found"),
            "{e}"
        );
        let e = client(&p.yaml, &ExecPolicy::default())
            .err()
            .expect("refused while off")
            .to_string();
        assert!(e.contains(crate::exec_auth::OFF), "{e}");
    }

    #[test]
    fn file_references_are_refused() {
        let files = "- name: ua\n  user: {client-certificate: /home/me/c.pem, client-key-data: eA==}\n- name: ub\n  user: {token: x}\n";
        let e = prepare(&kc(files, None), Some("ctx-a"), &ExecPolicy::default())
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("kubeconfig must embed its files: 'client-certificate' points to a file"),
            "{e}"
        );
        let tf = "- name: ua\n  user: {tokenFile: /t}\n- name: ub\n  user: {token: x}\n";
        assert!(
            prepare(&kc(tf, None), Some("ctx-a"), &ExecPolicy::default())
                .unwrap_err()
                .to_string()
                .contains("'tokenFile'")
        );
    }

    #[test]
    fn a_client_certificate_user_is_cert_kind() {
        let certs = "- name: ua\n  user: {client-certificate-data: Y2VydA==, client-key-data: a2V5}\n- name: ub\n  user: {token: x}\n";
        assert_eq!(
            prepare(&kc(certs, None), Some("ctx-a"), &ExecPolicy::default())
                .unwrap()
                .summary
                .user_kind,
            UserKind::Cert
        );
    }

    #[test]
    fn a_user_with_no_token_or_certificate_is_refused() {
        let none =
            "- name: ua\n  user: {username: admin, password: pw}\n- name: ub\n  user: {token: x}\n";
        let e = prepare(&kc(none, None), Some("ctx-a"), &ExecPolicy::default())
            .unwrap_err()
            .to_string();
        assert!(e.contains("has no token or client certificate"), "{e}");
    }

    #[test]
    fn config_reads_the_server_namespace_and_token_without_io() {
        let p = prepare(&kc(TOKENS, None), Some("ctx-a"), &ExecPolicy::default()).unwrap();
        let c = config(&p.yaml).unwrap();
        assert_eq!(c.cluster_url.to_string(), "https://a.example:6443/");
        assert_eq!(c.default_namespace, "apps");
    }
}
