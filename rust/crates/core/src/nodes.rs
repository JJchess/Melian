use anyhow::{bail, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub mode: String,
    pub port: u16,
    pub node: String,
    pub path: Vec<String>,
    pub fingerprint: String,
    pub restorable: String,
    pub providers: Vec<String>,
}
#[derive(Debug)]
pub struct CheckError {
    pub code: &'static str,
    pub message: &'static str,
}
impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for CheckError {}
pub fn check_error(code: &'static str, message: &'static str) -> anyhow::Error {
    CheckError { code, message }.into()
}
fn invalid() -> anyhow::Error {
    check_error("nodeUnclear", "请在 Clash 中选择一个确定的代理节点")
}
fn blocked(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "direct"
            | "reject"
            | "rejectdrop"
            | "pass"
            | "compatible"
            | "dns"
            | "loadbalance"
            | "relay"
    )
}
pub fn selectable(proxies: &Value) -> Vec<String> {
    proxies["proxies"]["GLOBAL"]["all"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(Value::as_str)
                .filter(|name| {
                    !blocked(name)
                        && !blocked(proxies["proxies"][name]["type"].as_str().unwrap_or(""))
                })
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
pub fn resolve(config: &Value, response: &Value) -> Result<Snapshot> {
    let mode = config["mode"].as_str().unwrap_or("");
    if !mode.eq_ignore_ascii_case("global") {
        return Err(check_error("mode", "请在 Clash 中开启全局代理"));
    }
    let port = config["mixed-port"]
        .as_u64()
        .filter(|v| *v > 0)
        .or_else(|| config["port"].as_u64())
        .unwrap_or(0);
    if !(1..=65535).contains(&port) {
        return Err(check_error("proxyPort", "Clash 没有可用的 HTTP 代理端口"));
    }
    let map = response["proxies"].as_object().ok_or_else(invalid)?;
    let mut name = "GLOBAL".to_owned();
    let mut path = vec![];
    let mut seen = HashSet::new();
    let mut providers = vec![];
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(config)?);
    for _ in 0..32 {
        if !seen.insert(name.clone()) || blocked(&name) {
            return Err(invalid());
        }
        let proxy = map.get(&name).ok_or_else(invalid)?;
        let kind = proxy["type"].as_str().ok_or_else(invalid)?;
        if blocked(kind) {
            return Err(invalid());
        }
        if proxy
            .get("dialer-proxy")
            .is_some_and(|v| v.as_str().is_none_or(|s| !s.is_empty()))
        {
            return Err(invalid());
        }
        path.push(name.clone());
        hash.update(serde_json::to_vec(&(
            name.clone(),
            kind,
            &proxy["id"],
            &proxy["provider-name"],
        ))?);
        if let Some(provider) = proxy["provider-name"].as_str().filter(|s| !s.is_empty()) {
            if !providers.contains(&provider.to_owned()) {
                providers.push(provider.to_owned());
            }
        }
        if matches!(
            kind.to_ascii_lowercase().as_str(),
            "selector" | "urltest" | "fallback"
        ) {
            let next = proxy["now"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(invalid)?;
            if !proxy["all"]
                .as_array()
                .is_some_and(|all| all.iter().any(|v| v.as_str() == Some(next)))
            {
                return Err(invalid());
            }
            name = next.into();
            continue;
        }
        if !matches!(
            kind.to_ascii_lowercase().as_str(),
            "shadowsocks"
                | "shadowsocksr"
                | "snell"
                | "socks5"
                | "http"
                | "vmess"
                | "vless"
                | "trojan"
                | "hysteria"
                | "hysteria2"
                | "tuic"
                | "wireguard"
                | "ssh"
                | "anytls"
                | "mieru"
                | "sudoku"
                | "shadowquic"
                | "trusttunnel"
                | "masque"
                | "zerotier"
                | "openvpn"
                | "easytier"
                | "tailscale"
        ) || proxy.get("all").is_some()
            || proxy.get("now").is_some()
        {
            return Err(invalid());
        }
        let choices = selectable(response);
        let restorable = if choices.contains(&name) {
            name.clone()
        } else {
            path.get(1).cloned().ok_or_else(invalid)?
        };
        return Ok(Snapshot {
            mode: mode.into(),
            port: port as u16,
            node: name,
            path,
            fingerprint: hex::encode(hash.finalize()),
            restorable,
            providers,
        });
    }
    bail!(invalid())
}
