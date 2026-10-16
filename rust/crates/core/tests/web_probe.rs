use guard_core::{proxy::Proxy, web_probe::WebProbe, Gate};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

async fn start(gate: Arc<Gate>, probe: Arc<WebProbe>) -> (u16, CancellationToken) {
    let stop = CancellationToken::new();
    let proxy = Proxy::bind(0, gate, stop.clone())
        .unwrap()
        .with_web_probe(probe);
    let port = proxy.port();
    tokio::spawn(proxy.run());
    (port, stop)
}

async fn request(port: u16, method: &str, url: &str) -> String {
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let _ = client
        .write_all(
            format!("{method} {url} HTTP/1.1\r\nHost: connect.claudeguard.invalid\r\n\r\n")
                .as_bytes(),
        )
        .await;
    let mut response = vec![];
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(response).unwrap()
}

#[tokio::test]
async fn browser_probe_is_denied_until_network_is_allowed() {
    let gate = Arc::new(Gate::default());
    let probe = Arc::new(WebProbe::default());
    let url = probe.issue("edge");
    let (port, stop) = start(gate.clone(), probe.clone()).await;
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let _ = client
        .write_all(
            format!("GET {url} HTTP/1.1\r\nHost: connect.claudeguard.invalid\r\n\r\n").as_bytes(),
        )
        .await;
    let mut bytes = vec![];
    let result = tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut bytes))
        .await
        .unwrap();
    let response = String::from_utf8(bytes).unwrap();
    // Default denial closes before reading any input. Windows may reset a
    // socket that already has unread request bytes, with or without the 503.
    assert!(
        response.starts_with("HTTP/1.1 503 ")
            || result.is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionReset)
    );
    assert!(!response.contains("Location:"));
    assert!(probe.verified("edge").is_none());
    gate.allow(1, Duration::from_secs(10));
    assert!(request(port, "GET", &url)
        .await
        .starts_with("HTTP/1.1 302 "));
    assert!(probe.verified("edge").is_some());
    stop.cancel();
}

#[tokio::test]
async fn registered_probe_redirects_once_without_creating_an_upstream_connection() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gate = Arc::new(Gate::default());
    gate.allow(
        upstream.local_addr().unwrap().port(),
        Duration::from_secs(10),
    );
    let probe = Arc::new(WebProbe::default());
    let url = probe.issue("Chrome");
    let (port, stop) = start(gate, probe.clone()).await;
    let response = request(port, "GET", &url).await;
    assert!(response.starts_with("HTTP/1.1 302 Found\r\n"));
    assert!(response.contains("Location: https://claude.ai/\r\n"));
    assert!(response.contains("Cache-Control: no-store\r\n"));
    assert!(response.contains("Referrer-Policy: no-referrer\r\n"));
    assert!(response.contains("Connection: close\r\n"));
    assert!(response.ends_with("Content-Length: 0\r\n\r\n"));
    assert_eq!(response.matches("HTTP/1.1").count(), 1);
    assert!(probe.verified("chrome").is_some());
    assert!(probe.verified("edge").is_none());
    let replay = request(port, "GET", &url).await;
    assert!(replay.starts_with("HTTP/1.1 404 "));
    assert!(!replay.contains("Location:"));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), upstream.accept())
            .await
            .is_err()
    );
    stop.cancel();
}

#[tokio::test]
async fn invalid_probe_requests_never_redirect_or_reach_the_upstream() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gate = Arc::new(Gate::default());
    gate.allow(
        upstream.local_addr().unwrap().port(),
        Duration::from_secs(10),
    );
    let probe = Arc::new(WebProbe::default());
    let url = probe.issue("edge");
    let unsupported = probe.issue("unregistered");
    let (port, stop) = start(gate, probe.clone()).await;
    for (method, target) in [
        ("GET", "http://connect.claudeguard.invalid/wrong".to_owned()),
        ("GET", format!("{url}?redirect=https://example.com")),
        ("GET", url.replacen(".invalid/", ".invalid:80/", 1)),
        ("GET", format!("{url}/extra")),
        ("GET", unsupported),
        ("POST", url.clone()),
        ("CONNECT", "connect.claudeguard.invalid:443".to_owned()),
    ] {
        let response = request(port, method, &target).await;
        assert!(
            response.starts_with("HTTP/1.1 404 "),
            "{method} {target}: {response}"
        );
        assert!(!response.contains("Location:"));
    }
    assert!(probe.verified("edge").is_none());
    assert!(request(port, "GET", &url)
        .await
        .starts_with("HTTP/1.1 302 "));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), upstream.accept())
            .await
            .is_err()
    );
    stop.cancel();
}

#[tokio::test]
async fn replacing_and_clearing_registration_removes_previous_browser_receipt() {
    let gate = Arc::new(Gate::default());
    // A deliberately unused upstream port also proves the probe is local.
    gate.allow(1, Duration::from_secs(10));
    let probe = Arc::new(WebProbe::default());
    let (port, stop) = start(gate, probe.clone()).await;
    let first = probe.issue("edge");
    assert!(request(port, "GET", &first)
        .await
        .starts_with("HTTP/1.1 302 "));
    assert!(probe.verified("edge").is_some());
    let second = probe.issue("edge");
    assert!(probe.verified("edge").is_none());
    assert!(request(port, "GET", &first)
        .await
        .starts_with("HTTP/1.1 404 "));
    assert!(request(port, "GET", &second)
        .await
        .starts_with("HTTP/1.1 302 "));
    probe.clear("edge");
    assert!(probe.verified("edge").is_none());
    assert!(request(port, "GET", &second)
        .await
        .starts_with("HTTP/1.1 404 "));
    stop.cancel();
}
