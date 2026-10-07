//! Starter YAML for "Create", one per kind (spec §2.3). `NAMESPACE` is
//! replaced by the web with the namespace being viewed.

use fleetops_core::kube::KubeKind;

pub fn get(kind: KubeKind) -> Option<&'static str> {
    Some(match kind {
        KubeKind::Deployments => include_str!("templates/deployments.yaml"),
        KubeKind::StatefulSets => include_str!("templates/statefulsets.yaml"),
        KubeKind::DaemonSets => include_str!("templates/daemonsets.yaml"),
        KubeKind::Jobs => include_str!("templates/jobs.yaml"),
        KubeKind::CronJobs => include_str!("templates/cronjobs.yaml"),
        KubeKind::Pods => include_str!("templates/pods.yaml"),
        KubeKind::Services => include_str!("templates/services.yaml"),
        KubeKind::Ingresses => include_str!("templates/ingresses.yaml"),
        KubeKind::ConfigMaps => include_str!("templates/configmaps.yaml"),
        KubeKind::Secrets => include_str!("templates/secrets.yaml"),
        KubeKind::Namespaces => include_str!("templates/namespaces.yaml"),
        KubeKind::Nodes | KubeKind::Events => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_parses_as_one_object_of_its_kind() {
        for kind in KubeKind::ALL {
            let Some(t) = get(kind) else { continue };
            let docs = crate::apply::split(t).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert_eq!(docs.len(), 1, "{kind:?}");
            assert_eq!(docs[0].kind, kind.kind_name());
            assert_eq!(
                kind.cluster_scoped(),
                !t.contains("namespace: NAMESPACE"),
                "{kind:?}"
            );
        }
        assert!(get(KubeKind::Nodes).is_none());
        assert!(get(KubeKind::Events).is_none());
    }
}
