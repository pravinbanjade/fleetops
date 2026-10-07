use fleetops_core::env::Credential;
use fleetops_core::kube::KubeKind;
use fleetops_core::Error;

#[test]
fn every_kind_round_trips_through_its_path_name() {
    assert_eq!(KubeKind::ALL.len(), 24);
    for k in KubeKind::ALL {
        assert_eq!(k.as_str().parse::<KubeKind>().unwrap(), k);
        assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        assert_eq!(KubeKind::from_kind_name(k.kind_name()), Some(k));
    }
    assert_eq!(KubeKind::StatefulSets.as_str(), "statefulsets");
    assert_eq!(KubeKind::ConfigMaps.kind_name(), "ConfigMap");
}

#[test]
fn an_unknown_kind_is_not_found() {
    assert!(matches!(
        "widgets".parse::<KubeKind>(),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn scope_and_actions_follow_the_spec() {
    use KubeKind::*;
    let cluster: Vec<_> = KubeKind::ALL
        .into_iter()
        .filter(|k| k.cluster_scoped())
        .collect();
    assert_eq!(
        cluster,
        [
            Namespaces,
            Nodes,
            PersistentVolumes,
            StorageClasses,
            ClusterRoles,
            ClusterRoleBindings
        ]
    );
    let scalable: Vec<_> = KubeKind::ALL.into_iter().filter(|k| k.scalable()).collect();
    assert_eq!(scalable, [Deployments, StatefulSets]);
    let restartable: Vec<_> = KubeKind::ALL
        .into_iter()
        .filter(|k| k.restartable())
        .collect();
    assert_eq!(restartable, [Deployments, StatefulSets, DaemonSets]);
    let read_only: Vec<_> = KubeKind::ALL
        .into_iter()
        .filter(|k| k.read_only())
        .collect();
    assert_eq!(read_only, [Events]);
    let suspendable: Vec<_> = KubeKind::ALL
        .into_iter()
        .filter(|k| k.suspendable())
        .collect();
    assert_eq!(suspendable, [Deployments, StatefulSets, CronJobs]);
}

#[test]
fn a_credential_never_prints_its_kubeconfig() {
    let c = Credential::Kubeconfig("token: s3cret".into());
    assert!(!format!("{c:?}").contains("s3cret"));
}
