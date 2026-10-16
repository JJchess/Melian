use anyhow::{bail, Result};
use async_trait::async_trait;
#[cfg(windows)]
use guard_core::controller::Mihomo;
use guard_core::{
    controller::{Controller, CountryProbe},
    engine::remaining_lease,
    nodes::{resolve, Snapshot},
    proxy::{sanitize_header, Proxy},
    Engine, Gate,
};
use guard_protocol::Settings;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;
fn config() -> Value {
    json!({"mode":"global","mixed-port":7897})
}
fn proxies() -> Value {
    json!({"proxies":{
        "GLOBAL":{"type":"Selector","now":"Auto","all":["Auto","US","DIRECT"]},
        "Auto":{"type":"URLTest","now":"US","all":["US"]},"US":{"type":"Shadowsocks","id":"one"},"DIRECT":{"type":"Direct"}
    }})
}
fn snapshot() -> Snapshot {
    resolve(&config(), &proxies()).unwrap()
}
#[test]
fn nested_groups() {
    let s = snapshot();
    assert_eq!(s.path, ["GLOBAL", "Auto", "US"]);
    assert_eq!(s.node, "US");
    assert_eq!(s.restorable, "US");
}
#[test]
fn unsafe_nodes_cycles_unknown() {
    for kind in ["Direct", "LoadBalance", "Relay", "FutureUnknown"] {
        let mut p = proxies();
        p["proxies"]["US"]["type"] = json!(kind);
        assert!(resolve(&config(), &p).is_err());
    }
    let mut p = proxies();
    p["proxies"]["Auto"]["now"] = json!("GLOBAL");
    p["proxies"]["Auto"]["all"] = json!(["GLOBAL"]);
    assert!(resolve(&config(), &p).is_err());
    p = proxies();
    p["proxies"]["Auto"]["now"] = json!("absent");
    assert!(resolve(&config(), &p).is_err());
}
#[test]
fn rejects_mode_and_port() {
    let mut c = config();
    c["mode"] = json!("rule");
    assert!(resolve(&c, &proxies()).is_err());
    c["mode"] = json!("global");
    c["mixed-port"] = json!(0);
    assert!(resolve(&c, &proxies()).is_err());
}
#[test]
fn config_fingerprints() {
    let a = snapshot();
    let mut c = config();
    c["ipv6"] = json!(true);
    assert_ne!(a.fingerprint, resolve(&c, &proxies()).unwrap().fingerprint);
}
#[test]
fn same_named_reload() {
    let a = snapshot();
    let mut p = proxies();
    p["proxies"]["US"]["id"] = json!("new");
    assert_ne!(a.fingerprint, resolve(&config(), &p).unwrap().fingerprint);
}
#[test]
fn rejects_dialer_chains() {
    let mut p = proxies();
    p["proxies"]["US"]["dialer-proxy"] = json!("hop");
    assert!(resolve(&config(), &p).is_err());
}
#[test]
fn verification_and_health_expiry() {
    assert_eq!(
        remaining_lease(Duration::from_secs(44), Duration::from_secs(1)),
        Duration::from_secs(1)
    );
    assert!(remaining_lease(Duration::from_secs(45), Duration::ZERO).is_zero());
    assert!(remaining_lease(Duration::ZERO, Duration::from_secs(3)).is_zero());
}
#[test]
fn monotonic_generation_lease() {
    let gate = Gate::default();
    assert!(gate.permit().is_none());
    gate.allow(7897, Duration::from_millis(20));
    let old = gate.permit().unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert!(gate.permit().is_none());
    assert!(gate.expire());
    assert!(old.revoked.is_cancelled());
    assert!(!gate.allow(7897, Duration::from_secs(2)));
    gate.revoke();
    assert!(gate.allow(7897, Duration::from_secs(2)));
    assert!(!gate.current(&old));
    let current = gate.permit().unwrap();
    gate.revoke();
    assert!(current.revoked.is_cancelled());
}
async fn proxy(gate: Arc<Gate>) -> (u16, CancellationToken) {
    let stop = CancellationToken::new();
    let p = Proxy::bind(0, gate, stop.clone()).unwrap();
    let port = p.port();
    tokio::spawn(p.run());
    (port, stop)
}
#[tokio::test]
async fn default_deny() {
    let (port, stop) = proxy(Arc::new(Gate::default())).await;
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut data = vec![];
    client.read_to_end(&mut data).await.unwrap();
    assert!(String::from_utf8_lossy(&data).starts_with("HTTP/1.1 503"));
    stop.cancel();
}
#[tokio::test]
async fn connect_revokes_existing_tunnel() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gate = Arc::new(Gate::default());
    gate.allow(
        upstream.local_addr().unwrap().port(),
        Duration::from_secs(5),
    );
    let server = tokio::spawn(async move {
        let (mut s, _) = upstream.accept().await.unwrap();
        let mut buf = [0; 1024];
        s.read(&mut buf).await.unwrap();
        s.write_all(b"HTTP/1.1 200 OK\r\n\r\nready").await.unwrap();
        let n = s.read(&mut buf).await.unwrap_or(0);
        assert_eq!(n, 0);
    });
    let (port, stop) = proxy(gate.clone()).await;
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client
        .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
        .await
        .unwrap();
    let mut bytes = vec![0; 44];
    client.read_exact(&mut bytes).await.unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("ready"));
    // Synchronous revoke closes OS sockets without relying on an async worker polling cancellation.
    gate.revoke();
    let mut b = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), client.read(&mut b))
            .await
            .unwrap()
            .unwrap_or(0),
        0
    );
    server.await.unwrap();
    stop.cancel();
}
#[tokio::test]
async fn http_strips_proxy_credentials() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gate = Arc::new(Gate::default());
    gate.allow(
        upstream.local_addr().unwrap().port(),
        Duration::from_secs(5),
    );
    let server = tokio::spawn(async move {
        let (mut s, _) = upstream.accept().await.unwrap();
        let mut b = [0; 4096];
        let n = s.read(&mut b).await.unwrap();
        let text = String::from_utf8_lossy(&b[..n]);
        assert!(!text.to_lowercase().contains("proxy-authorization"));
        assert!(text.contains("GET http://example.com/"));
        s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let (port, stop) = proxy(gate).await;
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic secret\r\nProxy-Connection: keep-alive\r\n\r\n").await.unwrap();
    let mut bytes = vec![];
    client.read_to_end(&mut bytes).await.unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("204"));
    server.await.unwrap();
    stop.cancel();
}
#[test]
fn rejects_bad_requests() {
    for input in [
        "GET / HTTP/1.1\r\n\r\n",
        "CONNECT user@example.com:443 HTTP/1.1\r\n\r\n",
        "CONNECT x:0/path HTTP/1.1\r\n\r\n",
        "GET https://example.com/ HTTP/1.1\r\n\r\n",
        "GET http://example.com/ HTTP/1.1\r\nBad Header: x\r\n\r\n",
    ] {
        assert!(sanitize_header(input.as_bytes()).is_err(), "{input}");
    }
}
#[tokio::test]
#[cfg(windows)]
async fn mihomo_named_pipe() {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = format!("ClaudeGuard-Test-{}", std::process::id());
    let path = format!(r"\\.\pipe\{name}");
    let mut server = ServerOptions::new().create(&path).unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..2 {
            server.connect().await.unwrap();
            let mut connected = server;
            server = ServerOptions::new().create(&path).unwrap();
            tokio::spawn(async move {
                let mut buf = [0; 4096];
                let n = connected.read(&mut buf).await.unwrap();
                let body = if String::from_utf8_lossy(&buf[..n]).contains("/configs") {
                    config()
                } else {
                    proxies()
                }
                .to_string();
                connected
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                // Keep the server instance alive until the client drains the response.
                let _ = connected.read(&mut buf).await;
            });
        }
    });
    assert_eq!(Mihomo::new(&name).unwrap().read().await.unwrap().node, "US");
    task.await.unwrap();
}
#[tokio::test]
#[cfg(windows)]
async fn missing_and_stalled_controller_have_distinct_reasons() {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = format!("ClaudeGuard-Errors-{}", std::process::id());
    let controller = Mihomo::new(&name).unwrap();
    let error = controller.read().await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<guard_core::nodes::CheckError>()
            .unwrap()
            .code,
        "clashUnavailable"
    );
    let _server = ServerOptions::new()
        .create(format!(r"\\.\pipe\{name}"))
        .unwrap();
    let error = controller.read().await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<guard_core::nodes::CheckError>()
            .unwrap()
            .code,
        "controllerTimeout"
    );
}
struct FakeController {
    snapshot: Mutex<Option<Snapshot>>,
    stall: AtomicBool,
    gate: Mutex<Option<Arc<Gate>>>,
    selected: AtomicBool,
}
#[async_trait]
impl Controller for FakeController {
    async fn read(&self) -> Result<Snapshot> {
        if self.stall.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        self.snapshot
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("controller down"))
    }
    async fn select(&self, _node: &str) -> Result<()> {
        if let Some(g) = &*self.gate.lock().unwrap() {
            assert!(g.permit().is_none());
        }
        self.selected.store(true, Ordering::SeqCst);
        Ok(())
    }
}
struct Probe {
    country: Mutex<Option<String>>,
    delay: Mutex<Duration>,
}
#[async_trait]
impl CountryProbe for Probe {
    async fn check(&self, _port: u16) -> Result<String> {
        let delay = *self.delay.lock().unwrap();
        let value = self.country.lock().unwrap().clone();
        tokio::time::sleep(delay).await;
        if let Some(s) = value {
            Ok(s)
        } else {
            bail!("probe down")
        }
    }
}
fn engine() -> (Arc<Engine>, Arc<FakeController>, Arc<Probe>) {
    let controller = Arc::new(FakeController {
        snapshot: Mutex::new(Some(snapshot())),
        stall: AtomicBool::new(false),
        gate: Mutex::new(None),
        selected: AtomicBool::new(false),
    });
    let probe = Arc::new(Probe {
        country: Mutex::new(Some("US".into())),
        delay: Mutex::new(Duration::ZERO),
    });
    let engine = Engine::with_services(
        Settings {
            proxy_port: 0,
            ..Settings::default()
        },
        controller.clone(),
        probe.clone(),
        false,
    );
    *controller.gate.lock().unwrap() = Some(engine.gate.clone());
    engine.start().unwrap();
    (engine, controller, probe)
}
async fn until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(6), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn us_non_us_and_selection_changes() {
    let (e, c, p) = engine();
    until(|| e.state().allowed).await;
    *p.country.lock().unwrap() = Some("CA".into());
    c.snapshot
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .fingerprint
        .push('2');
    until(|| e.state().reason_code == "country").await;
    assert!(e.gate.permit().is_none());
    assert_eq!(e.state().country, "CA");
    e.shutdown();
}
#[tokio::test]
async fn late_probe_epoch_cannot_allow() {
    let (e, c, p) = engine();
    *p.delay.lock().unwrap() = Duration::from_secs(2);
    tokio::time::sleep(Duration::from_millis(100)).await;
    e.invalidate();
    *c.snapshot.lock().unwrap() = None;
    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert!(!e.state().allowed);
    assert!(e.gate.permit().is_none());
    e.shutdown();
}
#[tokio::test]
async fn controller_stall_and_independent_watchdog() {
    let (e, c, _) = engine();
    until(|| e.state().allowed).await;
    let permit = e.gate.permit().unwrap();
    c.stall.store(true, Ordering::SeqCst);
    until(|| !e.state().allowed).await;
    until(|| permit.revoked.is_cancelled()).await;
    assert!(e.gate.permit().is_none());
    e.shutdown();
}
#[tokio::test]
async fn query_failure_controller_failure() {
    let (e, c, p) = engine();
    until(|| e.state().allowed).await;
    *p.country.lock().unwrap() = None;
    e.invalidate();
    until(|| e.state().reason_code == "probeFailed").await;
    assert!(e.gate.permit().is_none());
    *c.snapshot.lock().unwrap() = None;
    until(|| e.state().reason_code == "controllerFailed").await;
    assert!(!e.state().allowed);
    assert_eq!(e.state().suggested_action, "recheck");
    e.shutdown();
}
#[tokio::test]
async fn restore_blocks_first_and_remembers_verified_node() {
    let (e, c, _) = engine();
    until(|| e.state().allowed).await;
    assert_eq!(e.settings().preferred_node.as_deref(), Some("US"));
    e.switch_preferred(None).await.unwrap();
    assert!(c.selected.load(Ordering::SeqCst));
    assert!(!e.state().allowed);
    e.shutdown();
}
#[tokio::test]
async fn port_conflict_is_fatal() {
    let gate = Arc::new(Gate::default());
    let first = Proxy::bind(0, gate.clone(), CancellationToken::new()).unwrap();
    assert!(Proxy::bind(first.port(), gate, CancellationToken::new()).is_err());
}
