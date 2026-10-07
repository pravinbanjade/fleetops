//! `KubeOps`' generic methods (spec 3g §2.4): any discovered kind, through
//! the same list, watch, edit and delete code as curated ones.

use crate::client::{cleaned, timed, KubeClient};
use crate::{kinds, mask, summaries};
use fleetops_core::kube::{
    ApiKind, ApplyMode, ApplyResult, EditableObject, KubeEvent, KubeList, KubeObject, WatchEvent,
};
use fleetops_core::streams::BoxStream;
use fleetops_core::Result;
use kube::api::{Api, DeleteParams, DynamicObject};

impl KubeClient {
    pub(crate) fn generic_api(&self, k: &ApiKind, ns: Option<&str>) -> Api<DynamicObject> {
        let ar = kinds::generic_resource(k);
        match ns.filter(|_| k.namespaced) {
            Some(ns) => Api::namespaced_with(self.client.clone(), ns, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        }
    }

    pub(crate) async fn generic_list_impl(
        &self,
        k: &ApiKind,
        ns: Option<&str>,
    ) -> Result<KubeList> {
        self.list_rows(self.generic_api(k, ns), None, &|o| {
            summaries::generic_row(k, o)
        })
        .await
    }

    pub(crate) async fn generic_get_impl(
        &self,
        k: &ApiKind,
        ns: &str,
        name: &str,
    ) -> Result<KubeObject> {
        let obj = timed(self.generic_api(k, Some(ns)).get(name)).await?;
        Ok(KubeObject {
            row: summaries::generic_row(k, &obj),
            yaml: mask::object_yaml(kinds::generic_json(k, &obj), false),
            conditions: summaries::conditions(&obj),
            owners: summaries::owners(&obj),
            containers: vec![],
        })
    }

    pub(crate) fn generic_watch_impl(
        &self,
        k: &ApiKind,
        ns: Option<&str>,
    ) -> BoxStream<WatchEvent> {
        crate::watch::watch(self.generic_api(k, ns), crate::watch::generic(k.clone()))
    }

    pub(crate) async fn generic_events_impl(
        &self,
        k: &ApiKind,
        ns: &str,
        name: &str,
    ) -> Result<Vec<KubeEvent>> {
        self.object_events(&k.kind, k.namespaced, ns, name).await
    }

    pub(crate) async fn generic_delete_impl(
        &self,
        k: &ApiKind,
        ns: &str,
        name: &str,
    ) -> Result<()> {
        timed(
            self.generic_api(k, Some(ns))
                .delete(name, &DeleteParams::background()),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn generic_editable_impl(
        &self,
        k: &ApiKind,
        ns: &str,
        name: &str,
    ) -> Result<EditableObject> {
        let obj = timed(self.generic_api(k, Some(ns)).get(name)).await?;
        let resource_version = obj.metadata.resource_version.clone().unwrap_or_default();
        let yaml = mask::to_yaml(&cleaned(kinds::generic_json(k, &obj)))?;
        Ok(EditableObject {
            yaml,
            resource_version,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn generic_replace_impl(
        &self,
        k: &ApiKind,
        ns: &str,
        name: &str,
        yaml: &str,
        resource_version: &str,
        mode: ApplyMode,
    ) -> Result<ApplyResult> {
        self.replace_in(
            self.generic_api(k, Some(ns)),
            &k.kind,
            k.namespaced,
            ns,
            name,
            yaml,
            resource_version,
            mode,
        )
        .await
    }
}
