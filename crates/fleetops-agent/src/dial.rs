//! One WebSocket connection to the server's agent endpoint.

use anyhow::{anyhow, Context};
use fleetops_tunnel::ws::MAX_WS_MESSAGE;
use std::{path::Path, sync::Arc};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Error as WsError},
    Connector, MaybeTlsStream, WebSocketStream,
};

pub enum DialError {
    /// The server refused the credential (401) or the client (403).
    Refused(u16),
    Other(anyhow::Error),
}

pub fn tls_config(ca: Option<&Path>) -> anyhow::Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = ca {
        use rustls::pki_types::{pem::PemObject, CertificateDer};
        for cert in CertificateDer::pem_file_iter(path)
            .with_context(|| format!("reading {}", path.display()))?
        {
            roots.add(cert?)?;
        }
    }
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

fn endpoint(server: &str) -> anyhow::Result<String> {
    let base = server.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        Ok(format!("wss://{rest}/api/v1/agent/connect"))
    } else if let Some(rest) = base.strip_prefix("http://") {
        Ok(format!("ws://{rest}/api/v1/agent/connect"))
    } else {
        Err(anyhow!("--server must start with https:// or http://"))
    }
}

pub async fn dial(
    server: &str,
    bearer: &str,
    tls: Arc<rustls::ClientConfig>,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, DialError> {
    let url = endpoint(server).map_err(DialError::Other)?;
    let mut req = url
        .into_client_request()
        .map_err(|e| DialError::Other(e.into()))?;
    let auth = format!("Bearer {bearer}")
        .parse()
        .map_err(|_| DialError::Other(anyhow!("the agent credential is not a valid header")))?;
    req.headers_mut().insert("authorization", auth);
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MESSAGE))
        .max_frame_size(Some(MAX_WS_MESSAGE));
    match tokio_tungstenite::connect_async_tls_with_config(
        req,
        Some(ws_config),
        true,
        Some(Connector::Rustls(tls)),
    )
    .await
    {
        Ok((ws, _)) => Ok(ws),
        Err(WsError::Http(resp)) => Err(DialError::Refused(resp.status().as_u16())),
        Err(e) => Err(DialError::Other(e.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_map_to_the_connect_endpoint() {
        assert_eq!(
            endpoint("https://fleet.example.com/").unwrap(),
            "wss://fleet.example.com/api/v1/agent/connect"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:9000").unwrap(),
            "ws://127.0.0.1:9000/api/v1/agent/connect"
        );
        assert!(endpoint("fleet.example.com").is_err());
    }
}
