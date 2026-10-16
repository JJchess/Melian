//! Local-only discovery. Credentials are read from client configuration in memory,
//! never logged. A single endpoint is pinned for a whole controller transaction.
use super::*;
use std::{
    net::SocketAddr,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpStream, UnixStream},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Endpoint {
    address: String,
    secret: String,
}
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

fn local_address(value: &str) -> Option<String> {
    let raw = value.strip_prefix("http://").unwrap_or(value);
    let raw = raw
        .strip_prefix("localhost:")
        .map(|p| format!("127.0.0.1:{p}"))
        .unwrap_or_else(|| raw.into());
    let mut address: SocketAddr = raw.parse().ok()?;
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            "127.0.0.1".parse().unwrap()
        } else {
            "::1".parse().unwrap()
        });
    }
    (address.ip().is_loopback() && address.port() != 0).then(|| address.to_string())
}
fn socket_ok(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| {
        m.file_type().is_socket() && (m.uid() == unsafe { libc::geteuid() } || m.uid() == 0)
    })
}
fn candidates(name: &str) -> Vec<Endpoint> {
    let mut endpoints = Vec::new();
    let explicit = std::env::var("MELIAN_CONTROLLER").unwrap_or_else(|_| name.into());
    let secret = std::env::var("MELIAN_CONTROLLER_SECRET").unwrap_or_default();
    if Path::new(&explicit).is_absolute() || local_address(&explicit).is_some() {
        return vec![Endpoint {
            address: local_address(&explicit).unwrap_or(explicit),
            secret,
        }];
    }
    if explicit != "verge-mihomo" {
        // An invalid explicit endpoint must not silently select another client.
        return vec![];
    }
    let mut paths = vec![
        PathBuf::from("/tmp/verge/verge-mihomo.sock"),
        PathBuf::from("/var/tmp/verge/verge-mihomo.sock"),
        std::env::temp_dir().join("verge-mihomo.sock"),
    ];
    for dir in [
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        std::env::temp_dir(),
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten().take(1024) {
                let file = entry.file_name().to_string_lossy().into_owned();
                if (file.starts_with("mihomo-party")
                    || file.starts_with("clash-verge")
                    || file.starts_with("verge-mihomo"))
                    && file.ends_with(".sock")
                {
                    paths.push(entry.path());
                }
                if file.starts_with("clash-verge") && entry.file_type().is_ok_and(|t| t.is_dir()) {
                    if let Ok(children) = std::fs::read_dir(entry.path()) {
                        for child in children.flatten().take(32) {
                            if child.path().extension().is_some_and(|e| e == "sock") {
                                paths.push(child.path());
                            }
                        }
                    }
                }
            }
        }
    }
    for path in paths {
        if socket_ok(&path) {
            endpoints.push(Endpoint {
                address: path.to_string_lossy().into_owned(),
                secret: String::new(),
            });
        }
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let roots = [
        ".config/clash",
        ".config/mihomo",
        ".config/clash.meta",
        "Library/Application Support/io.github.clash-verge-rev.clash-verge-rev",
        "Library/Application Support/Clash Verge",
        "Library/Application Support/mihomo-party",
        "Library/Application Support/clash-party",
        "Library/Application Support/ClashX",
        "Library/Application Support/ClashX.Meta",
    ];
    for root in roots {
        for filename in [
            "config.yaml",
            "clash-verge.yaml",
            "mihomo.yaml",
            "clash.yaml",
        ] {
            let path = home.join(root).join(filename);
            if !std::fs::metadata(&path).is_ok_and(|m| m.len() < 8 * 1024 * 1024) {
                continue;
            }
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            let Ok(value) = serde_yaml::from_slice::<serde_yaml::Value>(&data) else {
                continue;
            };
            let secret = value["secret"].as_str().unwrap_or("").to_string();
            if let Some(socket) = value["external-controller-unix"].as_str() {
                let socket = PathBuf::from(socket);
                let socket = if socket.is_absolute() {
                    socket
                } else {
                    path.parent().unwrap().join(socket)
                };
                if socket_ok(&socket) {
                    endpoints.push(Endpoint {
                        address: socket.to_string_lossy().into_owned(),
                        secret: secret.clone(),
                    });
                }
            }
            if let Some(address) = value["external-controller"]
                .as_str()
                .and_then(local_address)
            {
                endpoints.push(Endpoint { address, secret });
            }
        }
    }
    for port in [9090, 9097] {
        endpoints.push(Endpoint {
            address: format!("127.0.0.1:{port}"),
            secret: String::new(),
        });
    }
    let mut unique = Vec::new();
    for endpoint in endpoints {
        if !unique.contains(&endpoint) {
            unique.push(endpoint);
        }
    }
    unique
}
async fn query(endpoint: &Endpoint, method: &str, path: &str, body: &Value) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let stream: Box<dyn Stream> = if Path::new(&endpoint.address).is_absolute() {
            if !socket_ok(Path::new(&endpoint.address)) {
                bail!("控制接口不可用");
            }
            Box::new(UnixStream::connect(&endpoint.address).await?)
        } else {
            Box::new(
                TcpStream::connect(local_address(&endpoint.address).context("仅支持本机控制接口")?)
                    .await?,
            )
        };
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        let _connection = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(conn));
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .header("Connection", "close");
        if !endpoint.secret.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", endpoint.secret));
        }
        let response = sender
            .send_request(request.body(Full::new(Bytes::from(if body.is_null() {
                vec![]
            } else {
                serde_json::to_vec(body)?
            })))?)
            .await?;
        if !response.status().is_success() {
            bail!("代理控制接口拒绝请求");
        }
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame?.into_data() {
                if bytes.len() + data.len() > 8 * 1024 * 1024 {
                    bail!("代理响应过长");
                }
                bytes.extend_from_slice(&data);
            }
        }
        if bytes.is_empty() {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_slice(&bytes)?)
        }
    })
    .await
    .map_err(|_| nodes::check_error("controllerTimeout", "代理控制接口响应超时"))?
}
pub(super) async fn request(
    controller: &Mihomo,
    method: &str,
    path: &str,
    body: Value,
) -> Result<Value> {
    let mut selected = controller.endpoint.lock().await;
    if let Some(endpoint) = selected.as_ref() {
        match query(endpoint, method, path, &body).await {
            Ok(value) => return Ok(value),
            Err(error) => {
                *selected = None;
                return Err(error);
            }
        }
    }
    // Discover with a read-only version request. Never retry a mutation on a different core.
    if method != "GET" {
        bail!("代理已变化，请重新检查后再恢复");
    }
    for endpoint in candidates(&controller.pipe) {
        if let Ok(version) = query(&endpoint, "GET", "/version", &Value::Null).await {
            if !version["version"].is_string() {
                continue;
            }
            let value = if path == "/version" {
                version
            } else {
                query(&endpoint, method, path, &body).await?
            };
            *selected = Some(endpoint);
            return Ok(value);
        }
    }
    Err(nodes::check_error(
        "clashUnavailable",
        "未发现可用的 Clash / Mihomo 控制接口",
    ))
}

pub(super) async fn read(controller: &Mihomo) -> Result<Snapshot> {
    // The mode, nodes and provider revisions must come from the same core.
    if controller.endpoint.lock().await.is_none() {
        request(controller, "GET", "/version", Value::Null).await?;
    }
    let mut selected = controller.endpoint.lock().await;
    let endpoint = selected.as_ref().context("代理控制接口已断开")?;
    let result = async {
        let (config, proxies) = tokio::try_join!(
            query(endpoint, "GET", "/configs", &Value::Null),
            query(endpoint, "GET", "/proxies", &Value::Null)
        )?;
        let mut snapshot = nodes::resolve(&config, &proxies)?;
        snapshot
            .fingerprint
            .push_str(&format!("|controller:{}", endpoint.address));
        for provider in &snapshot.providers {
            let encoded: String = provider
                .bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect();
            let value = query(
                endpoint,
                "GET",
                &format!("/providers/proxies/{encoded}"),
                &Value::Null,
            )
            .await?;
            snapshot
                .fingerprint
                .push_str(&format!("|{provider}:{}", value["updatedAt"]));
        }
        Ok(snapshot)
    }
    .await;
    if result.is_err() {
        *selected = None;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controllers_are_local_only() {
        assert_eq!(
            local_address("localhost:9090").as_deref(),
            Some("127.0.0.1:9090")
        );
        assert_eq!(
            local_address("0.0.0.0:9097").as_deref(),
            Some("127.0.0.1:9097")
        );
        for bad in [
            "https://example.com",
            "192.168.0.1:9090",
            "example.com:9090",
            "127.0.0.1:0",
        ] {
            assert!(local_address(bad).is_none());
        }
    }
}
