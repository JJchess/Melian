use crate::nodes::{self, Snapshot};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use http_body_util::{BodyExt, Full};
use hyper::{body::Bytes, Request};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::windows::named_pipe::ClientOptions;

#[async_trait]
pub trait Controller: Send + Sync {
    async fn read(&self) -> Result<Snapshot>;
    async fn select(&self, node: &str) -> Result<()>;
}
#[async_trait]
pub trait CountryProbe: Send + Sync {
    async fn check(&self, port: u16) -> Result<String>;
}
pub struct Mihomo {
    pub pipe: String,
}
impl Mihomo {
    pub fn new(name: &str) -> Result<Self> {
        let name = name.strip_prefix(r"\\.\pipe\").unwrap_or(name);
        if name.is_empty() || name.contains(['\\', '/']) {
            bail!("无效的 Clash 管道名称");
        }
        Ok(Self {
            pipe: format!(r"\\.\pipe\{name}"),
        })
    }
    async fn request(&self, method: &str, path: &str, body: Value) -> Result<Value> {
        tokio::time::timeout(Duration::from_secs(2), async {
            let pipe = loop {
                match ClientOptions::new().open(&self.pipe) {
                    Ok(pipe) => break pipe,
                    Err(e) if e.raw_os_error() == Some(231) => {
                        tokio::time::sleep(Duration::from_millis(20)).await
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(nodes::check_error("clashUnavailable", "等待 Clash 启动"));
                    }
                    Err(e) => return Err(e.into()),
                }
            };
            let (mut sender, conn) =
                hyper::client::conn::http1::handshake(TokioIo::new(pipe)).await?;
            let connection = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(conn));
            let payload = if body.is_null() {
                Vec::new()
            } else {
                serde_json::to_vec(&body)?
            };
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header("Host", "localhost")
                .header("Content-Type", "application/json")
                .header("Connection", "close")
                .body(Full::new(Bytes::from(payload)))?;
            let result = async {
                let response = sender.send_request(request).await?;
                if !response.status().is_success() {
                    bail!("Clash 控制请求失败");
                }
                let mut body = response.into_body();
                let mut bytes = Vec::new();
                while let Some(frame) = body.frame().await {
                    if let Ok(data) = frame?.into_data() {
                        if bytes.len() + data.len() > 8 * 1024 * 1024 {
                            bail!("Clash 响应过长");
                        }
                        bytes.extend_from_slice(&data);
                    }
                }
                if bytes.is_empty() {
                    Ok(Value::Null)
                } else {
                    Ok(serde_json::from_slice(&bytes)?)
                }
            }
            .await;
            connection.abort();
            result
        })
        .await
        .map_err(|_| nodes::check_error("controllerTimeout", "Clash 响应超时"))?
    }
}
#[async_trait]
impl Controller for Mihomo {
    async fn read(&self) -> Result<Snapshot> {
        let (config, proxies) = tokio::try_join!(
            self.request("GET", "/configs", Value::Null),
            self.request("GET", "/proxies", Value::Null)
        )?;
        let mut snapshot = nodes::resolve(&config, &proxies)?;
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
            let value = self
                .request("GET", &format!("/providers/proxies/{encoded}"), Value::Null)
                .await?;
            snapshot
                .fingerprint
                .push_str(&format!("|{}:{}", provider, value["updatedAt"]));
        }
        Ok(snapshot)
    }
    async fn select(&self, node: &str) -> Result<()> {
        let proxies = self.request("GET", "/proxies", Value::Null).await?;
        if !nodes::selectable(&proxies).contains(&node.to_owned()) {
            bail!("恢复节点已失效，请在 Clash 中选择节点");
        }
        self.request("PUT", "/proxies/GLOBAL", json!({"name":node}))
            .await?;
        self.request("PATCH", "/configs", json!({"mode":"global"}))
            .await?;
        Ok(())
    }
}
pub struct HttpCountryProbe;
#[async_trait]
impl CountryProbe for HttpCountryProbe {
    async fn check(&self, port: u16) -> Result<String> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))?)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(8))
            .pool_max_idle_per_host(0)
            .build()?;
        let mut response = client
            .get("https://api.country.is/")
            .send()
            .await?
            .error_for_status()?;
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if data.len() + chunk.len() > 16 * 1024 {
                bail!("IP 属地响应过长");
            }
            data.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&data)?;
        let country = value["country"]
            .as_str()
            .filter(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic()))
            .context("未返回 IP 属地")?;
        Ok(country.to_ascii_uppercase())
    }
}
