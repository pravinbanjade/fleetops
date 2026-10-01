//! Kubeconfig handling for `kube-config` environments (spec §4.1): pick a
//! context, refuse users that need a helper binary or local files (the
//! server is distroless), and reduce the file to that one context.

use base64::{engine::general_purpose::STANDARD, Engine};
use fleetops_core::env::{KubeconfigSummary, UserKind};
use fleetops_core::{Error, Result};
use kube::config::{AuthInfo, Kubeconfig, NamedAuthInfo, NamedCluster, NamedContext};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::time::Duration;

pub const UNSUPPORTED_USER: &str =
    "use a ServiceAccount token or the FleetOps agent: exec and auth-provider users are not supported";
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

fn check_user(context: &str, user: &AuthInfo) -> Result<UserKind> {
    if user.exec.is_some() || user.auth_provider.is_some() {
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
    if user.token.is_some() {
        Ok(UserKind::Token)
    } else if user.client_certificate_data.is_some() && user.client_key_data.is_some() {
        Ok(UserKind::Cert)
    } else {
        Err(invalid(format!(
            "the user in context '{context}' has no token or client certificate"
        )))
    }
}

pub fn prepare(text: &str, context: Option<&str>) -> Result<Prepared> {
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
    let user_kind = check_user(&name, &auth)?;

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

/// `GET /version` with a short limit: used before saving a kubeconfig.
pub async fn check(yaml: &str) -> Result<()> {
    let client = kube::Client::try_from(config(yaml)?)
        .map_err(|e| invalid(format!("cannot reach cluster: {e}")))?;
    match tokio::time::timeout(CHECK_TIMEOUT, client.apiserver_version()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(kube::Error::Api(s))) if s.code == 401 => Err(invalid(
            "cannot reach cluster: credentials rejected (401): check the token or client certificate",
        )),
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
        let p = prepare(&kc(TOKENS, None), Some("ctx-a")).unwrap();
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
            prepare(&kc(TOKENS, Some("ctx-b")), None)
                .unwrap()
                .summary
                .context,
            "ctx-b"
        );
        let e = prepare(&kc(TOKENS, None), None).unwrap_err().to_string();
        assert!(e.contains("choose a context: ctx-a, ctx-b"), "{e}");
        let e = prepare(&kc(TOKENS, None), Some("nope"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown context 'nope'"), "{e}");
    }

    #[test]
    fn exec_and_auth_provider_users_are_refused() {
        let exec = "- name: ua\n  user:\n    exec: {apiVersion: client.authentication.k8s.io/v1, command: aws}\n- name: ub\n  user: {token: x}\n";
        let e = prepare(&kc(exec, None), Some("ctx-a"))
            .unwrap_err()
            .to_string();
        assert!(e.contains(UNSUPPORTED_USER), "{e}");
        let ap =
            "- name: ua\n  user:\n    auth-provider: {name: gcp}\n- name: ub\n  user: {token: x}\n";
        assert!(prepare(&kc(ap, None), Some("ctx-a"))
            .unwrap_err()
            .to_string()
            .contains(UNSUPPORTED_USER));
    }

    #[test]
    fn file_references_are_refused() {
        let files = "- name: ua\n  user: {client-certificate: /home/me/c.pem, client-key-data: eA==}\n- name: ub\n  user: {token: x}\n";
        let e = prepare(&kc(files, None), Some("ctx-a"))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("kubeconfig must embed its files: 'client-certificate' points to a file"),
            "{e}"
        );
        let tf = "- name: ua\n  user: {tokenFile: /t}\n- name: ub\n  user: {token: x}\n";
        assert!(prepare(&kc(tf, None), Some("ctx-a"))
            .unwrap_err()
            .to_string()
            .contains("'tokenFile'"));
    }

    #[test]
    fn a_client_certificate_user_is_cert_kind() {
        let certs = "- name: ua\n  user: {client-certificate-data: Y2VydA==, client-key-data: a2V5}\n- name: ub\n  user: {token: x}\n";
        assert_eq!(
            prepare(&kc(certs, None), Some("ctx-a"))
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
        let e = prepare(&kc(none, None), Some("ctx-a"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("has no token or client certificate"), "{e}");
    }

    #[test]
    fn config_reads_the_server_namespace_and_token_without_io() {
        let p = prepare(&kc(TOKENS, None), Some("ctx-a")).unwrap();
        let c = config(&p.yaml).unwrap();
        assert_eq!(c.cluster_url.to_string(), "https://a.example:6443/");
        assert_eq!(c.default_namespace, "apps");
    }
}
