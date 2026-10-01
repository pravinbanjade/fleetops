use fleetops_tunnel::{ws::tungstenite_io, Role, Session};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A yamux session carried over a real WebSocket on loopback TCP, the
/// agent (client) echoing every stream the server opens.
#[tokio::test]
async fn a_yamux_session_runs_over_a_websocket() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        Session::spawn(tungstenite_io(ws), Role::Server, |_s| {})
    });
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
        .await
        .unwrap();
    let _agent = Session::spawn(tungstenite_io(ws), Role::Client, |s| {
        tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(s);
            let _ = tokio::io::copy(&mut r, &mut w).await;
            let _ = w.shutdown().await;
        });
    });
    let server = server.await.unwrap();

    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let s = server.open_stream().await.unwrap();
    let (mut r, mut w) = tokio::io::split(s);
    let send = async {
        w.write_all(&payload).await.unwrap();
        w.shutdown().await.unwrap();
    };
    let mut back = Vec::new();
    let recv = r.read_to_end(&mut back);
    let (_, n) = tokio::join!(send, recv);
    n.unwrap();
    assert_eq!(back, payload);
}
