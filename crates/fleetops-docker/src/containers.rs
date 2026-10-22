use crate::{convert, error::map_err, files, LocalDocker};
use async_trait::async_trait;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, DownloadFromContainerOptionsBuilder, InspectContainerOptions,
    KillContainerOptions, ListContainersOptionsBuilder, PruneContainersOptions,
    RemoveContainerOptionsBuilder, RenameContainerOptionsBuilder, RestartContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use fleetops_core::docker::{
    ContainerAction, ContainerOps, ContainerSpec, ContainerSummary, DirListing, PruneReport,
};
use fleetops_core::{Error, Result};
use futures_util::StreamExt;
use serde_json::Value;

impl LocalDocker {
    /// Downloads `path` as a tar stream, stopping after `limit` bytes.
    /// Returns the bytes and whether the limit was hit.
    async fn download(&self, id: &str, path: &str, limit: usize) -> Result<(Vec<u8>, bool)> {
        let opts = DownloadFromContainerOptionsBuilder::new()
            .path(path)
            .build();
        let _permit = files::archive_permits()
            .acquire()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let mut stream = self.client.download_from_container(id, Some(opts));
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_err)?;
            buf.extend_from_slice(&chunk);
            if buf.len() > limit {
                return Ok((buf, true));
            }
        }
        Ok((buf, false))
    }
}

#[async_trait]
impl ContainerOps for LocalDocker {
    async fn list_containers(&self) -> Result<Vec<ContainerSummary>> {
        let opts = ListContainersOptionsBuilder::new().all(true).build();
        let list = self
            .client
            .list_containers(Some(opts))
            .await
            .map_err(map_err)?;
        Ok(list.into_iter().map(convert::container_summary).collect())
    }

    async fn inspect_container(&self, id: &str) -> Result<Value> {
        let res = self
            .client
            .inspect_container(id, None::<InspectContainerOptions>)
            .await
            .map_err(map_err)?;
        serde_json::to_value(res).map_err(|e| Error::Internal(e.to_string()))
    }

    async fn container_spec(&self, id: &str) -> Result<ContainerSpec> {
        Ok(convert::inspect_to_spec(&self.inspect_container(id).await?))
    }

    async fn create_container(&self, spec: &ContainerSpec) -> Result<String> {
        spec.validate()?;
        let body = serde_json::from_value(convert::spec_to_body(spec))
            .map_err(|e| Error::Invalid(format!("invalid container configuration: {e}")))?;
        let mut opts = CreateContainerOptionsBuilder::new();
        if let Some(name) = &spec.name {
            opts = opts.name(name);
        }
        let res = self
            .client
            .create_container(Some(opts.build()), body)
            .await
            .map_err(map_err)?;
        Ok(res.id)
    }

    async fn create_container_raw(&self, name: &str, body: &Value) -> Result<String> {
        let body = serde_json::from_value(body.clone())
            .map_err(|e| Error::Invalid(format!("invalid container configuration: {e}")))?;
        let opts = CreateContainerOptionsBuilder::new().name(name).build();
        let res = self
            .client
            .create_container(Some(opts), body)
            .await
            .map_err(map_err)?;
        Ok(res.id)
    }

    async fn container_action(&self, id: &str, action: ContainerAction) -> Result<()> {
        let c = &self.client;
        match action {
            ContainerAction::Start => c.start_container(id, None::<StartContainerOptions>).await,
            ContainerAction::Stop => c.stop_container(id, None::<StopContainerOptions>).await,
            ContainerAction::Restart => {
                c.restart_container(id, None::<RestartContainerOptions>)
                    .await
            }
            ContainerAction::Kill => c.kill_container(id, None::<KillContainerOptions>).await,
            ContainerAction::Pause => c.pause_container(id).await,
            ContainerAction::Unpause => c.unpause_container(id).await,
        }
        .map_err(map_err)
    }

    async fn rename_container(&self, id: &str, new_name: &str) -> Result<()> {
        fleetops_core::docker::validate_container_name(new_name)?;
        let opts = RenameContainerOptionsBuilder::new().name(new_name).build();
        self.client
            .rename_container(id, opts)
            .await
            .map_err(map_err)
    }

    async fn remove_container(&self, id: &str, force: bool, volumes: bool) -> Result<()> {
        let opts = RemoveContainerOptionsBuilder::new()
            .force(force)
            .v(volumes)
            .build();
        self.client
            .remove_container(id, Some(opts))
            .await
            .map_err(map_err)
    }

    async fn prune_containers(&self) -> Result<PruneReport> {
        let res = self
            .client
            .prune_containers(None::<PruneContainersOptions>)
            .await
            .map_err(map_err)?;
        Ok(PruneReport {
            deleted: res.containers_deleted.unwrap_or_default(),
            space_reclaimed: res.space_reclaimed.unwrap_or(0),
        })
    }

    async fn list_dir(&self, id: &str, path: &str) -> Result<DirListing> {
        files::check_path(path)?;
        let (bytes, truncated) = self.download(id, path, files::LIST_MAX_BYTES).await?;
        files::parse_listing(&bytes, path, truncated)
    }

    async fn read_file(&self, id: &str, path: &str, max_bytes: u64) -> Result<Vec<u8>> {
        files::check_path(path)?;
        // Allow for tar headers and padding on top of the file itself.
        let limit = (max_bytes as usize).saturating_add(64 * 1024);
        let (bytes, over) = self.download(id, path, limit).await?;
        if over {
            return Err(Error::Invalid(format!(
                "file is larger than {max_bytes} bytes"
            )));
        }
        let data = files::extract_file(bytes, path)?;
        if data.len() as u64 > max_bytes {
            return Err(Error::Invalid(format!(
                "file is larger than {max_bytes} bytes"
            )));
        }
        Ok(data)
    }

    async fn put_archive(&self, id: &str, path: &str, tar: Vec<u8>) -> Result<()> {
        let opts = bollard::query_parameters::UploadToContainerOptionsBuilder::default()
            .path(path)
            .build();
        self.client
            .upload_to_container(id, Some(opts), bollard::body_full(tar.into()))
            .await
            .map_err(map_err)
    }
}
