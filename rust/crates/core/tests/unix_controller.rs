#![cfg(unix)]
use guard_core::controller::{Controller, Mihomo};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};
#[tokio::test]
async fn unix_controller_resolves_live_mode_and_nested_nodes() {
    let path = std::env::temp_dir().join(format!("melian-controller-{}.sock", std::process::id()));
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            connections.spawn(async move {
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    bytes.push(stream.read_u8().await.unwrap());
                    assert!(bytes.len() < 4096);
                }
                let request = String::from_utf8_lossy(&bytes);
                let body = if request.contains("/version") {
                    json!({"version":"test"})
                } else if request.contains("/configs") {
                    json!({"mode":"global","mixed-port":7890})
                } else {
                    json!({"proxies":{
                        "GLOBAL":{"type":"Selector","now":"Auto","all":["Auto","US"]},
                        "Auto":{"type":"URLTest","now":"US","all":["US"]},
                        "US":{"type":"Shadowsocks","id":"test"}
                    }})
                }.to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
                // Wait for the client to finish its HTTP write before closing
                // the socket; otherwise macOS can report EPIPE on a valid reply.
                let mut rest = Vec::new();
                stream.read_to_end(&mut rest).await.unwrap();
            });
        }
        while let Some(result) = connections.join_next().await {
            result.unwrap();
        }
    });
    let snapshot = Mihomo::new(path.to_str().unwrap())
        .unwrap()
        .read()
        .await
        .unwrap();
    assert_eq!(snapshot.node, "US");
    assert_eq!(snapshot.port, 7890);
    server.await.unwrap();
    std::fs::remove_file(path).unwrap();
}
