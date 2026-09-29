//! System, image, volume and network operations.

use crate::{convert, error::map_err, LocalDocker};
use async_trait::async_trait;
use bollard::models::{
    Ipam, IpamConfig, NetworkConnectRequest, NetworkCreateRequest, NetworkDisconnectRequest,
    VolumeCreateRequest,
};
use bollard::query_parameters::{
    DataUsageOptions, InspectNetworkOptions, ListImagesOptionsBuilder, ListNetworksOptions,
    ListVolumesOptions, PruneImagesOptionsBuilder, PruneNetworksOptions, PruneVolumesOptions,
    RemoveImageOptionsBuilder, RemoveVolumeOptionsBuilder, TagImageOptionsBuilder,
};
use dockerer_core::docker::{
    ContainerOps, EngineVersion, ImageOps, ImageSummary, NetworkOps, NetworkSpec, NetworkSummary,
    PruneReport, SystemOps, VolumeOps, VolumeSpec, VolumeSummary,
};
use dockerer_core::{Error, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

fn to_json<T: serde::Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Error::Internal(e.to_string()))
}

#[async_trait]
impl SystemOps for LocalDocker {
    async fn version(&self) -> Result<EngineVersion> {
        let v = self.client.version().await.map_err(map_err)?;
        Ok(EngineVersion {
            version: v.version.unwrap_or_default(),
            api_version: v.api_version.unwrap_or_default(),
            os: v.os.unwrap_or_default(),
            arch: v.arch.unwrap_or_default(),
        })
    }

    async fn info(&self) -> Result<Value> {
        to_json(self.client.info().await.map_err(map_err)?)
    }

    async fn disk_usage(&self) -> Result<Value> {
        to_json(
            self.client
                .df(None::<DataUsageOptions>)
                .await
                .map_err(map_err)?,
        )
    }
}

#[async_trait]
impl ImageOps for LocalDocker {
    async fn list_images(&self) -> Result<Vec<ImageSummary>> {
        let opts = ListImagesOptionsBuilder::new().all(false).build();
        let images = self.client.list_images(Some(opts)).await.map_err(map_err)?;
        let used: HashSet<String> = self
            .list_containers()
            .await?
            .into_iter()
            .map(|c| c.image_id)
            .collect();
        Ok(images
            .into_iter()
            .map(|i| ImageSummary {
                in_use: used.contains(&i.id),
                id: i.id,
                repo_tags: i.repo_tags,
                size: i.size,
                created: i.created,
            })
            .collect())
    }

    async fn inspect_image(&self, id: &str) -> Result<Value> {
        to_json(self.client.inspect_image(id).await.map_err(map_err)?)
    }

    async fn image_history(&self, id: &str) -> Result<Value> {
        to_json(self.client.image_history(id).await.map_err(map_err)?)
    }

    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> Result<()> {
        let opts = TagImageOptionsBuilder::new().repo(repo).tag(tag).build();
        self.client.tag_image(id, Some(opts)).await.map_err(map_err)
    }

    async fn remove_image(&self, id: &str, force: bool) -> Result<Vec<String>> {
        let opts = RemoveImageOptionsBuilder::new().force(force).build();
        let items = self
            .client
            .remove_image(id, Some(opts), None)
            .await
            .map_err(map_err)?;
        Ok(items
            .into_iter()
            .filter_map(|i| i.untagged.or(i.deleted))
            .collect())
    }

    async fn prune_images(&self, all: bool) -> Result<PruneReport> {
        let mut opts = PruneImagesOptionsBuilder::new();
        if all {
            opts = opts.filters(&HashMap::from([("dangling", vec!["false"])]));
        }
        let res = self
            .client
            .prune_images(Some(opts.build()))
            .await
            .map_err(map_err)?;
        Ok(PruneReport {
            deleted: res
                .images_deleted
                .unwrap_or_default()
                .into_iter()
                .filter_map(|i| i.deleted.or(i.untagged))
                .collect(),
            space_reclaimed: res.space_reclaimed.unwrap_or(0),
        })
    }
}

#[async_trait]
impl VolumeOps for LocalDocker {
    async fn list_volumes(&self) -> Result<Vec<VolumeSummary>> {
        let res = self
            .client
            .list_volumes(None::<ListVolumesOptions>)
            .await
            .map_err(map_err)?;
        let used: HashSet<String> = self
            .list_containers()
            .await?
            .into_iter()
            .flat_map(|c| c.volumes)
            .collect();
        Ok(res
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| convert::volume_summary(v, &used))
            .collect())
    }

    async fn inspect_volume(&self, name: &str) -> Result<Value> {
        to_json(self.client.inspect_volume(name).await.map_err(map_err)?)
    }

    async fn create_volume(&self, spec: &VolumeSpec) -> Result<VolumeSummary> {
        let req = VolumeCreateRequest {
            name: Some(spec.name.clone()).filter(|n| !n.is_empty()),
            driver: spec.driver.clone(),
            driver_opts: Some(spec.driver_opts.clone().into_iter().collect()),
            labels: Some(spec.labels.clone().into_iter().collect()),
            ..Default::default()
        };
        let v = self.client.create_volume(req).await.map_err(map_err)?;
        Ok(convert::volume_summary(v, &HashSet::new()))
    }

    async fn remove_volume(&self, name: &str, force: bool) -> Result<()> {
        let opts = RemoveVolumeOptionsBuilder::new().force(force).build();
        self.client
            .remove_volume(name, Some(opts))
            .await
            .map_err(map_err)
    }

    async fn prune_volumes(&self) -> Result<PruneReport> {
        let res = self
            .client
            .prune_volumes(None::<PruneVolumesOptions>)
            .await
            .map_err(map_err)?;
        Ok(PruneReport {
            deleted: res.volumes_deleted.unwrap_or_default(),
            space_reclaimed: res.space_reclaimed.unwrap_or(0),
        })
    }
}

#[async_trait]
impl NetworkOps for LocalDocker {
    async fn list_networks(&self) -> Result<Vec<NetworkSummary>> {
        let list = self
            .client
            .list_networks(None::<ListNetworksOptions>)
            .await
            .map_err(map_err)?;
        Ok(list.into_iter().map(convert::network_summary).collect())
    }

    async fn inspect_network(&self, id: &str) -> Result<Value> {
        to_json(
            self.client
                .inspect_network(id, None::<InspectNetworkOptions>)
                .await
                .map_err(map_err)?,
        )
    }

    async fn create_network(&self, spec: &NetworkSpec) -> Result<String> {
        if spec.name.trim().is_empty() {
            return Err(Error::Invalid("network name is required".into()));
        }
        let ipam = (spec.subnet.is_some() || spec.gateway.is_some()).then(|| Ipam {
            config: Some(vec![IpamConfig {
                subnet: spec.subnet.clone(),
                gateway: spec.gateway.clone(),
                ..Default::default()
            }]),
            ..Default::default()
        });
        let req = NetworkCreateRequest {
            name: spec.name.clone(),
            driver: spec.driver.clone(),
            internal: Some(spec.internal),
            attachable: Some(spec.attachable),
            ipam,
            labels: Some(spec.labels.clone().into_iter().collect()),
            ..Default::default()
        };
        Ok(self.client.create_network(req).await.map_err(map_err)?.id)
    }

    async fn remove_network(&self, id: &str) -> Result<()> {
        self.client.remove_network(id).await.map_err(map_err)
    }

    async fn connect_network(&self, network: &str, container: &str) -> Result<()> {
        let req = NetworkConnectRequest {
            container: container.to_string(),
            endpoint_config: None,
        };
        self.client
            .connect_network(network, req)
            .await
            .map_err(map_err)
    }

    async fn disconnect_network(&self, network: &str, container: &str, force: bool) -> Result<()> {
        let req = NetworkDisconnectRequest {
            container: container.to_string(),
            force: Some(force),
        };
        self.client
            .disconnect_network(network, req)
            .await
            .map_err(map_err)
    }

    async fn prune_networks(&self) -> Result<PruneReport> {
        let res = self
            .client
            .prune_networks(None::<PruneNetworksOptions>)
            .await
            .map_err(map_err)?;
        Ok(PruneReport {
            deleted: res.networks_deleted.unwrap_or_default(),
            space_reclaimed: 0,
        })
    }
}
