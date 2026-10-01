//! A scriptable agent for connect-handler tests: real WebSocket, real
//! yamux, control messages sent by hand, data streams echoed.

use fleetops_tunnel::{
    control::{read_msg, write_msg, HostInfo, Msg},
    ws::tungstenite_io,
    Role, Session, StreamHandle,
};
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Error as WsError};

pub struct TestAgent {
    session: Session,
    rx: ReadHalf<StreamHandle>,
    tx: WriteHalf<StreamHandle>,
}

impl TestAgent {
    /// `base` is `http://127.0.0.1:port`. Err carries the HTTP status of a refused upgrade.
    pub async fn connect(base: &str, bearer: &str) -> Result<TestAgent, u16> {
        let url = format!("{}/api/v1/agent/connect", base.replacen("http", "ws", 1));
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {bearer}").parse().unwrap());
        let ws = match tokio_tungstenite::connect_async(req).await {
            Ok((ws, _)) => ws,
            Err(WsError::Http(resp)) => return Err(resp.status().as_u16()),
            Err(e) => panic!("connect: {e}"),
        };
        let session = Session::spawn(tungstenite_io(ws), Role::Client, |s| {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(s);
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        });
        let control = session.open_stream().await.unwrap();
        let (rx, tx) = tokio::io::split(control);
        Ok(TestAgent { session, rx, tx })
    }

    pub async fn hello(&mut self, proto: u16) {
        let host = HostInfo {
            hostname: "test-host".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            docker: "unix:///var/run/docker.sock".into(),
        };
        write_msg(
            &mut self.tx,
            &Msg::Hello {
                proto,
                agent_version: "0.2.0-test".into(),
                host,
            },
        )
        .await
        .unwrap();
    }

    pub async fn next(&mut self) -> Option<Msg> {
        tokio::time::timeout(std::time::Duration::from_secs(5), read_msg(&mut self.rx))
            .await
            .expect("no control message within 5 s")
            .ok()
            .flatten()
    }

    pub async fn welcome(&mut self) -> Msg {
        let m = self.next().await.expect("welcome");
        assert!(matches!(m, Msg::Welcome { .. }), "{m:?}");
        m
    }

    pub async fn pong(&mut self, n: u64) {
        write_msg(&mut self.tx, &Msg::Pong { n }).await.unwrap();
    }

    pub async fn closed(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(5), self.session.closed())
            .await
            .expect("session still open after 5 s");
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
}
