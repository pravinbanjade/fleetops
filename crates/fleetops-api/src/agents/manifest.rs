//! The `kubectl apply` command that installs the agent in a cluster
//! (spec §4.2). One replica with the Recreate strategy: two agents with
//! one key would replace each other. The pod template carries a hash of
//! the join token, so re-applying after a rotate restarts the agent.

use super::AGENT_IMAGE;
use sha2::{Digest, Sha256};

pub const NAMESPACE: &str = "fleetops";
pub const KEY_SECRET: &str = "fleetops-agent-key";

pub fn command(server_url: &str, join_token: &str) -> String {
    format!(
        "kubectl apply -f - <<'EOF'\n{}\nEOF\n",
        manifest(server_url, join_token)
    )
}

pub fn manifest(server_url: &str, join_token: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let join_hash = &hex::encode(Sha256::digest(join_token.as_bytes()))[..12];
    format!(
        r#"apiVersion: v1
kind: Namespace
metadata:
  name: {NAMESPACE}
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: fleetops-agent
  namespace: {NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: fleetops-agent
rules:
  - apiGroups: [""]
    resources: [namespaces, nodes]
    verbs: [get, list, watch]
  - apiGroups: [""]
    resources: [pods, services, configmaps, secrets, events]
    verbs: [get, list, watch, delete]
  - apiGroups: [""]
    resources: [pods/log]
    verbs: [get]
  - apiGroups: [""]
    resources: [pods/exec]
    verbs: [get, create]
  - apiGroups: [apps]
    resources: [deployments, statefulsets, daemonsets]
    verbs: [get, list, watch, delete, patch]
  - apiGroups: [apps]
    resources: [deployments/scale, statefulsets/scale]
    verbs: [get, patch]
  - apiGroups: [apps]
    resources: [replicasets]
    verbs: [get]
  - apiGroups: [batch]
    resources: [jobs, cronjobs]
    verbs: [get, list, watch, delete]
  - apiGroups: [networking.k8s.io]
    resources: [ingresses]
    verbs: [get, list, watch, delete]
  - nonResourceURLs: [/version]
    verbs: [get]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: fleetops-agent
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: fleetops-agent
subjects:
  - kind: ServiceAccount
    name: fleetops-agent
    namespace: {NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: fleetops-agent-key
  namespace: {NAMESPACE}
rules:
  - apiGroups: [""]
    resources: [secrets]
    resourceNames: [{KEY_SECRET}]
    verbs: [get, patch]
  - apiGroups: [""]
    resources: [secrets]
    verbs: [create]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: fleetops-agent-key
  namespace: {NAMESPACE}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: fleetops-agent-key
subjects:
  - kind: ServiceAccount
    name: fleetops-agent
    namespace: {NAMESPACE}
---
apiVersion: v1
kind: Secret
metadata:
  name: fleetops-agent-join
  namespace: {NAMESPACE}
type: Opaque
stringData:
  token: {join_token}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: fleetops-agent
  namespace: {NAMESPACE}
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels: {{app: fleetops-agent}}
  template:
    metadata:
      labels: {{app: fleetops-agent}}
      annotations:
        fleetops.io/join: "{join_hash}"
    spec:
      serviceAccountName: fleetops-agent
      securityContext:
        runAsNonRoot: true
        runAsUser: 65532
        runAsGroup: 65532
        seccompProfile: {{type: RuntimeDefault}}
      containers:
        - name: agent
          image: {AGENT_IMAGE}:{version}
          args:
            - --kube
            - --server
            - {server_url}
            - --key-secret
            - {KEY_SECRET}
          env:
            - name: FLEETOPS_AGENT_JOIN_TOKEN
              valueFrom: {{secretKeyRef: {{name: fleetops-agent-join, key: token}}}}
            - name: POD_NAMESPACE
              valueFrom: {{fieldRef: {{fieldPath: metadata.namespace}}}}
          securityContext:
            readOnlyRootFilesystem: true
            allowPrivilegeEscalation: false
            capabilities: {{drop: [ALL]}}
          resources:
            requests: {{cpu: 10m, memory: 16Mi}}
            limits: {{memory: 64Mi}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_installs_a_namespaced_agent_with_its_join_token() {
        let m = manifest("https://fleet.example", "fj_abc");
        let docs: Vec<&str> = m.split("\n---\n").collect();
        let kinds: Vec<&str> = docs
            .iter()
            .map(|d| d.lines().find_map(|l| l.strip_prefix("kind: ")).unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "Namespace",
                "ServiceAccount",
                "ClusterRole",
                "ClusterRoleBinding",
                "Role",
                "RoleBinding",
                "Secret",
                "Deployment"
            ]
        );
        let with_token: Vec<&str> = docs
            .iter()
            .filter(|d| d.contains("fj_abc"))
            .copied()
            .collect();
        assert_eq!(with_token.len(), 1, "the token is only in the join Secret");
        assert!(with_token[0].contains("kind: Secret"));
        assert!(
            m.contains("- --kube")
                && m.contains("- https://fleet.example")
                && m.contains("- --key-secret")
        );
        assert!(m.contains("readOnlyRootFilesystem: true") && m.contains("type: Recreate"));
        assert!(!m.contains("docker.sock"));
    }

    #[test]
    fn a_new_token_changes_the_pod_template_so_reapplying_restarts_the_agent() {
        let a = manifest("https://f", "fj_one");
        let b = manifest("https://f", "fj_two");
        let ann = |m: &str| {
            m.lines()
                .find(|l| l.contains("fleetops.io/join:"))
                .unwrap()
                .to_string()
        };
        assert_ne!(ann(&a), ann(&b));
    }

    #[test]
    fn the_pod_names_a_numeric_non_root_user() {
        // The distroless image's USER is "nonroot", which kubelet cannot
        // check against runAsNonRoot; without a numeric uid the pod never
        // starts (CreateContainerConfigError).
        let m = manifest("https://f", "fj_x");
        assert!(
            m.contains("runAsUser: 65532") && m.contains("runAsGroup: 65532"),
            "{m}"
        );
    }

    #[test]
    fn the_command_pipes_the_manifest_to_kubectl() {
        let c = command("https://f", "fj_x");
        assert!(
            c.starts_with("kubectl apply -f - <<'EOF'\n") && c.ends_with("EOF\n"),
            "{c}"
        );
    }
}
