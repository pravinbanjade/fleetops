use crate::config::Config;
use anyhow::Context;
use axum_server::tls_rustls::RustlsConfig;
use std::{fs, io::Write, path::Path};

/// rustls needs a process-wide crypto provider; we use ring (no C toolchain
/// needed for musl cross builds). Safe to call more than once.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub async fn load_or_generate(config: &Config) -> anyhow::Result<RustlsConfig> {
    let (cert, key) = match (&config.tls_cert, &config.tls_key) {
        (Some(c), Some(k)) => (c.clone(), k.clone()),
        _ => {
            let dir = config.data.join("tls");
            let cert = dir.join("cert.pem");
            let key = dir.join("key.pem");
            if !cert.exists() || !key.exists() {
                tracing::info!(path = %dir.display(), "generating self-signed TLS certificate");
                generate_self_signed(&cert, &key)?;
            }
            (cert, key)
        }
    };
    RustlsConfig::from_pem_file(&cert, &key)
        .await
        .with_context(|| {
            format!(
                "loading TLS cert {} / key {}",
                cert.display(),
                key.display()
            )
        })
}

pub fn generate_self_signed(cert: &Path, key: &Path) -> anyhow::Result<()> {
    if let Some(dir) = cert.parent() {
        fs::create_dir_all(dir)?;
    }
    let generated =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "fleetops".to_string()])?;
    fs::write(cert, generated.cert.pem())?;
    write_private(key, generated.key_pair.serialize_pem().as_bytes())?;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(bytes)?;
    Ok(())
}
