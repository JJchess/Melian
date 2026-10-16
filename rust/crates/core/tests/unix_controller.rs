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
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut bytes = [0; 4096];
                let n = stream.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..n]);
                let body=if request.contains("/version"){json!({"version":"test"})}else if request.contains("/configs"){json!({"mode":"global","mixed-port":7890})}else{json!({"proxies":{"GLOBAL":{"type":"Selector","now":"Auto"},"Auto":{"type":"URLTest","now":"US"},"US":{"type":"Shadowsocks","server":"test","port":443}}})}.to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
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
