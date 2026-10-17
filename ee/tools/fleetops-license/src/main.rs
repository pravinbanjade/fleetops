//! Issues FleetOps Enterprise licenses. Vendor-only: the signing key must
//! never be committed (editions spec §4.5).

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use ed25519_dalek::SigningKey;
use fleetops_core::edition::Feature;
use fleetops_ee::license::{self, Payload, VERSION};
use std::{io::Read, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "fleetops-license",
    about = "Issue and check FleetOps Enterprise licenses"
)]
enum Cmd {
    /// Write a new key pair to DIR: license.key (secret) and license.pub.
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Print a license signed with --key.
    Sign {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        customer: String,
        /// Expiry date, YYYY-MM-DD; the license expires at its start, UTC.
        #[arg(long)]
        expires: String,
        /// Most users allowed; unlimited when unset.
        #[arg(long)]
        max_users: Option<u32>,
        /// Most nodes allowed (Docker or Podman hosts plus Kubernetes nodes); unlimited when unset.
        #[arg(long)]
        max_nodes: Option<u32>,
        /// Comma-separated feature names, e.g. audit_export.
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
        /// License id; a random UUID when unset.
        #[arg(long)]
        id: Option<String>,
    },
    /// Verify a license (a file, or - for stdin) and print its payload.
    Inspect {
        file: String,
        /// Public key to check against; the keys built into fleetops-ee when unset.
        #[arg(long = "pub")]
        public: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cmd::parse() {
        Cmd::Keygen { out } => keygen(out),
        Cmd::Sign {
            key,
            customer,
            expires,
            max_users,
            max_nodes,
            features,
            id,
        } => {
            let text = std::fs::read_to_string(&key)
                .with_context(|| format!("reading {}", key.display()))?;
            let key = license::signing_key_from_hex(&text)
                .map_err(|e| anyhow!("{}: {e}", key.display()))?;
            for f in &features {
                if Feature::parse(f).is_none() {
                    let known: Vec<_> = Feature::ALL.iter().map(|f| f.as_str()).collect();
                    bail!("unknown feature '{f}'; known: {}", known.join(", "));
                }
            }
            let date: jiff::civil::Date = expires
                .parse()
                .with_context(|| format!("--expires {expires}: want YYYY-MM-DD"))?;
            let expires_at = date
                .to_zoned(jiff::tz::TimeZone::UTC)?
                .timestamp()
                .as_second();
            let payload = Payload {
                v: VERSION,
                id: id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                customer,
                issued_at: jiff::Timestamp::now().as_second(),
                expires_at,
                max_users,
                max_nodes,
                features,
            };
            println!("{}", license::sign(&payload, &key));
            Ok(())
        }
        Cmd::Inspect { file, public } => {
            let mut text = String::new();
            if file == "-" {
                std::io::stdin().read_to_string(&mut text)?;
            } else {
                text = std::fs::read_to_string(&file).with_context(|| format!("reading {file}"))?;
            }
            let keys = match public {
                Some(p) => vec![
                    license::verifying_key_from_hex(&std::fs::read_to_string(&p)?)
                        .map_err(|e| anyhow!("{}: {e}", p.display()))?,
                ],
                None => fleetops_ee::keys::trusted(),
            };
            let lic = license::verify(&text, &keys).map_err(|e| anyhow!("{e}"))?;
            println!("{}", serde_json::to_string_pretty(&lic.payload)?);
            let status = lic.status_at(jiff::Timestamp::now().as_second());
            println!(
                "status: {}",
                serde_json::to_value(status)?.as_str().unwrap_or("?")
            );
            Ok(())
        }
    }
}

fn keygen(out: PathBuf) -> Result<()> {
    std::fs::create_dir_all(&out)?;
    let key_path = out.join("license.key");
    if key_path.exists() {
        bail!(
            "{} exists; refusing to overwrite a signing key",
            key_path.display()
        );
    }
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    std::fs::write(&key_path, hex::encode(key.to_bytes()) + "\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::write(
        out.join("license.pub"),
        hex::encode(key.verifying_key().to_bytes()) + "\n",
    )?;
    eprintln!(
        "wrote {} (keep it secret) and license.pub",
        key_path.display()
    );
    Ok(())
}
