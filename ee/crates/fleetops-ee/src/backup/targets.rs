//! Where scheduled backups go: a local directory (a mounted volume or
//! share) or S3-compatible storage. Only files named like a FleetOps
//! backup, directly under the directory or prefix, are listed or deleted.

use super::db::{Retention, S3Secrets, Target, TargetSettings};
use async_trait::async_trait;
use fleetops_core::{Error, Result};
use fleetops_store::backup::parse_name;
use futures_util::StreamExt;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RemoteFile {
    pub name: String,
    pub size: u64,
    /// When the backup was made, from its name.
    pub created_at: i64,
}

#[async_trait]
pub trait Sink: Send + Sync {
    async fn put(&self, name: &str, file: &Path) -> Result<()>;
    /// FleetOps backups only, at the top level.
    async fn list(&self) -> Result<Vec<RemoteFile>>;
    async fn get(&self, name: &str, to: &Path) -> Result<()>;
    async fn delete(&self, name: &str) -> Result<()>;
    /// Writes and deletes a small file.
    async fn test(&self) -> Result<()>;
}

const TEST_NAME: &str = ".fleetops-test";

fn io(what: &str, e: std::io::Error) -> Error {
    Error::Invalid(format!("{what}: {e}"))
}

/// The largest backup fetched for a restore, the same as an upload.
const GET_MAX: u64 = fleetops_api::backup::BACKUP_UPLOAD_MAX as u64;

fn too_large() -> Error {
    Error::Invalid(format!(
        "this backup is larger than {} MiB, the most FleetOps restores",
        GET_MAX / (1024 * 1024)
    ))
}

fn check_name(name: &str) -> Result<()> {
    if name == TEST_NAME || parse_name(name).is_some() {
        Ok(())
    } else {
        Err(Error::Invalid("not a FleetOps backup name".into()))
    }
}

/// An absolute, existing directory outside the data directory.
pub fn validate_local(path: &Path, data_dir: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::Invalid(
            "the backup directory must be an absolute path".into(),
        ));
    }
    let real = path
        .canonicalize()
        .map_err(|_| Error::Invalid("the backup directory doesn't exist".into()))?;
    if !real.is_dir() {
        return Err(Error::Invalid(
            "the backup directory is not a directory".into(),
        ));
    }
    let data = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    if real.starts_with(&data) {
        return Err(Error::Invalid(
            "the backup directory can't be inside the data directory".into(),
        ));
    }
    Ok(())
}

/// The backups to delete so that `keep` holds, newest kept first.
pub fn to_delete(files: &[RemoteFile], keep: Retention, now: i64) -> Vec<String> {
    let mut v: Vec<(i64, &str)> = files
        .iter()
        .filter_map(|f| parse_name(&f.name).map(|t| (t, f.name.as_str())))
        .collect();
    v.sort_by(|a, b| b.cmp(a));
    match keep {
        Retention::Count(n) => v
            .into_iter()
            .skip(n as usize)
            .map(|(_, n)| n.into())
            .collect(),
        Retention::Days(d) => {
            let cutoff = now - i64::from(d) * 86_400;
            v.into_iter()
                .filter(|(t, _)| *t < cutoff)
                .map(|(_, n)| n.into())
                .collect()
        }
    }
}

pub fn sink(t: &Target, secrets: Option<S3Secrets>, data_dir: &Path) -> Result<Box<dyn Sink>> {
    match &t.settings {
        TargetSettings::Local { path } => {
            validate_local(path, data_dir)?;
            Ok(Box::new(LocalSink(path.clone())))
        }
        TargetSettings::S3 {
            endpoint,
            region,
            bucket,
            prefix,
            path_style,
            access_key_id,
        } => {
            let secret = secrets
                .ok_or_else(|| Error::Invalid("save the S3 secret access key first".into()))?;
            Ok(Box::new(S3Sink::new(
                endpoint.as_deref(),
                region,
                bucket,
                prefix,
                *path_style,
                access_key_id,
                &secret.secret_access_key,
            )?))
        }
    }
}

pub struct LocalSink(pub PathBuf);

#[async_trait]
impl Sink for LocalSink {
    async fn put(&self, name: &str, file: &Path) -> Result<()> {
        check_name(name)?;
        let tmp = self.0.join(format!(".{name}.part"));
        tokio::fs::copy(file, &tmp)
            .await
            .map_err(|e| io("writing the backup", e))?;
        tokio::fs::rename(&tmp, self.0.join(name))
            .await
            .map_err(|e| io("writing the backup", e))
    }

    async fn list(&self) -> Result<Vec<RemoteFile>> {
        let mut out = Vec::new();
        let mut dir = tokio::fs::read_dir(&self.0)
            .await
            .map_err(|e| io("listing the backup directory", e))?;
        while let Some(e) = dir
            .next_entry()
            .await
            .map_err(|e| io("listing the backup directory", e))?
        {
            let Ok(name) = e.file_name().into_string() else {
                continue;
            };
            let Some(created_at) = parse_name(&name) else {
                continue;
            };
            let Ok(meta) = e.metadata().await else {
                continue;
            };
            if meta.is_file() {
                out.push(RemoteFile {
                    name,
                    size: meta.len(),
                    created_at,
                });
            }
        }
        Ok(out)
    }

    async fn get(&self, name: &str, to: &Path) -> Result<()> {
        check_name(name)?;
        let from = self.0.join(name);
        let len = tokio::fs::metadata(&from)
            .await
            .map_err(|e| io("reading the backup", e))?
            .len();
        if len > GET_MAX {
            return Err(too_large());
        }
        tokio::fs::copy(from, to)
            .await
            .map(|_| ())
            .map_err(|e| io("reading the backup", e))
    }

    async fn delete(&self, name: &str) -> Result<()> {
        check_name(name)?;
        tokio::fs::remove_file(self.0.join(name))
            .await
            .map_err(|e| io("deleting the backup", e))
    }

    async fn test(&self) -> Result<()> {
        let p = self.0.join(TEST_NAME);
        tokio::fs::write(&p, b"ok")
            .await
            .map_err(|e| io("the backup directory isn't writable", e))?;
        tokio::fs::remove_file(&p)
            .await
            .map_err(|e| io("the backup directory", e))
    }
}

pub struct S3Sink {
    bucket: Bucket,
    creds: Credentials,
    /// Empty, or ending in `/`.
    prefix: String,
    http: reqwest::Client,
}

const SIGN_FOR: Duration = Duration::from_secs(3600);
/// How long a request may sit without receiving data before it fails.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// A request error without its URL: a presigned URL is a working credential
/// for an hour, and errors end up in the run history and the audit log.
fn s3_err(e: reqwest::Error) -> Error {
    Error::Invalid(format!("S3: {}", e.without_url()))
}

impl S3Sink {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoint: Option<&str>,
        region: &str,
        bucket: &str,
        prefix: &str,
        path_style: bool,
        access_key_id: &str,
        secret_access_key: &str,
    ) -> Result<Self> {
        let endpoint = match endpoint.map(str::trim).filter(|e| !e.is_empty()) {
            Some(e) => e.to_string(),
            None => format!("https://s3.{region}.amazonaws.com"),
        };
        let url: url::Url = endpoint
            .parse()
            .map_err(|_| Error::Invalid("the S3 endpoint is not a URL".into()))?;
        let style = if path_style {
            UrlStyle::Path
        } else {
            UrlStyle::VirtualHost
        };
        let bucket = Bucket::new(url, style, bucket.to_string(), region.to_string())
            .map_err(|e| Error::Invalid(format!("S3 bucket: {e}")))?;
        let prefix = prefix.trim_matches('/');
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|e| Error::Internal(format!("S3 client: {e}")))?;
        Ok(Self {
            bucket,
            creds: Credentials::new(access_key_id, secret_access_key),
            prefix: if prefix.is_empty() {
                String::new()
            } else {
                format!("{prefix}/")
            },
            http,
        })
    }

    /// How long a request may sit without receiving data.
    pub fn with_read_timeout(mut self, t: Duration) -> Self {
        if let Ok(c) = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(t)
            .build()
        {
            self.http = c;
        }
        self
    }

    fn key(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }

    async fn check(res: reqwest::Result<reqwest::Response>) -> Result<reqwest::Response> {
        let res = res.map_err(s3_err)?;
        if res.status().is_success() {
            return Ok(res);
        }
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        let tag = |t: &str| {
            body.split(&format!("<{t}>"))
                .nth(1)
                .and_then(|r| r.split(&format!("</{t}>")).next())
                .map(str::to_string)
        };
        Err(Error::Invalid(match (tag("Code"), tag("Message")) {
            (Some(c), Some(m)) => format!("S3: {c}: {m}"),
            (Some(c), None) => format!("S3: {c}"),
            _ => format!("S3: HTTP {status}"),
        }))
    }
}

#[async_trait]
impl Sink for S3Sink {
    async fn put(&self, name: &str, file: &Path) -> Result<()> {
        check_name(name)?;
        let key = self.key(name);
        let url = self
            .bucket
            .put_object(Some(&self.creds), &key)
            .sign(SIGN_FOR);
        let f = tokio::fs::File::open(file)
            .await
            .map_err(|e| io("reading the backup", e))?;
        let len = f
            .metadata()
            .await
            .map_err(|e| io("reading the backup", e))?
            .len();
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(f));
        Self::check(
            self.http
                .put(url)
                .header(reqwest::header::CONTENT_LENGTH, len)
                .body(body)
                .send()
                .await,
        )
        .await
        .map(|_| ())
    }

    async fn list(&self) -> Result<Vec<RemoteFile>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut action = self.bucket.list_objects_v2(Some(&self.creds));
            action.with_delimiter("/");
            if !self.prefix.is_empty() {
                action.with_prefix(self.prefix.as_str());
            }
            if let Some(t) = &token {
                action.with_continuation_token(t.clone());
            }
            let url = action.sign(SIGN_FOR);
            let text = Self::check(self.http.get(url).send().await)
                .await?
                .text()
                .await
                .map_err(s3_err)?;
            let page = rusty_s3::actions::ListObjectsV2::parse_response(&text)
                .map_err(|e| Error::Invalid(format!("S3: unreadable listing: {e}")))?;
            for c in page.contents {
                let Some(name) = c.key.strip_prefix(&self.prefix) else {
                    continue;
                };
                if name.contains('/') {
                    continue;
                }
                if let Some(created_at) = parse_name(name) {
                    out.push(RemoteFile {
                        name: name.to_string(),
                        size: c.size,
                        created_at,
                    });
                }
            }
            match page.next_continuation_token {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        Ok(out)
    }

    async fn get(&self, name: &str, to: &Path) -> Result<()> {
        check_name(name)?;
        let key = self.key(name);
        let url = self
            .bucket
            .get_object(Some(&self.creds), &key)
            .sign(SIGN_FOR);
        let res = Self::check(self.http.get(url).send().await).await?;
        if res.content_length().is_some_and(|n| n > GET_MAX) {
            return Err(too_large());
        }
        let mut f = tokio::fs::File::create(to)
            .await
            .map_err(|e| io("saving the backup", e))?;
        let mut stream = res.bytes_stream();
        let mut got = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(s3_err)?;
            got += chunk.len() as u64;
            if got > GET_MAX {
                return Err(too_large());
            }
            f.write_all(&chunk)
                .await
                .map_err(|e| io("saving the backup", e))?;
        }
        f.flush().await.map_err(|e| io("saving the backup", e))
    }

    async fn delete(&self, name: &str) -> Result<()> {
        check_name(name)?;
        let key = self.key(name);
        let url = self
            .bucket
            .delete_object(Some(&self.creds), &key)
            .sign(SIGN_FOR);
        Self::check(self.http.delete(url).send().await)
            .await
            .map(|_| ())
    }

    async fn test(&self) -> Result<()> {
        let key = self.key(TEST_NAME);
        let url = self
            .bucket
            .put_object(Some(&self.creds), &key)
            .sign(SIGN_FOR);
        Self::check(self.http.put(url).body("ok").send().await).await?;
        let url = self
            .bucket
            .delete_object(Some(&self.creds), &key)
            .sign(SIGN_FOR);
        Self::check(self.http.delete(url).send().await)
            .await
            .map(|_| ())
    }
}
