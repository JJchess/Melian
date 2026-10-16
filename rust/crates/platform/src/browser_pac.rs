//! Deterministic browser routing. This module never changes the system proxy or registry.
use anyhow::{bail, Context, Result};
use guard_protocol::SystemProxy;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, Ipv6Addr},
};

pub const MARKER: &str = "// ClaudeGuard persistent fail-closed browser route v2";
pub const PROBE_HOST: &str = "connect.claudeguard.invalid";

#[derive(Default, Serialize)]
struct Config {
    routes: BTreeMap<String, String>,
    bypass: Vec<Bypass>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Bypass {
    Local {
        local: bool,
    },
    Cidr {
        network: String,
        prefix: u8,
    },
    Host {
        scheme: String,
        host: String,
        port: u16,
    },
}

/// Produce a persistent PAC with no direct fallback for protected hosts.
/// Unsupported input is an error, never an inferred DIRECT configuration.
pub fn build(system: &SystemProxy, port: u16) -> Result<String> {
    if port < 1024 {
        bail!("守卫代理端口无效");
    }
    let config = parse(system, port)?;
    Ok(format!(
        "{MARKER}\n{}\nfunction FindProxyForURL(url, host) {{ return claudeGuardRoute(url, host, {}, {port}); }}\n",
        include_str!("browser_pac_route.js"),
        serde_json::to_string(&config)?
    ))
}

/// Embed the PAC in the policy itself: it remains available when the guard exits.
pub fn data_url(script: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut result = String::with_capacity(script.len() * 2 + 40);
    result.push_str("data:application/x-ns-proxy-autoconfig,");
    for byte in script.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            result.push(byte as char);
        } else {
            result.push('%');
            result.push(HEX[(byte >> 4) as usize] as char);
            result.push(HEX[(byte & 15) as usize] as char);
        }
    }
    result
}

fn parse(system: &SystemProxy, guard_port: u16) -> Result<Config> {
    if !system.supported {
        bail!(
            "{}",
            if system.reason.is_empty() {
                "系统自动代理配置不受支持"
            } else {
                &system.reason
            }
        );
    }
    let mut config = Config::default();
    if !system.enabled {
        return Ok(config);
    }
    let server = system.server.trim();
    if server.is_empty() {
        bail!("系统代理已启用，但地址为空");
    }
    if server.contains('=') {
        for item in server.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (scheme, endpoint) = item.split_once('=').context("系统代理映射格式不受支持")?;
            let scheme = scheme.to_ascii_lowercase();
            if !matches!(scheme.as_str(), "http" | "https" | "ftp" | "socks")
                || config.routes.contains_key(&scheme)
            {
                bail!("系统代理映射格式不受支持");
            }
            let proxy = parse_endpoint(
                endpoint,
                if scheme == "socks" { "SOCKS" } else { "PROXY" },
                guard_port,
            )?;
            config.routes.insert(scheme, proxy);
        }
    } else {
        config
            .routes
            .insert("all".into(), parse_endpoint(server, "PROXY", guard_port)?);
    }
    if config.routes.is_empty() {
        bail!("系统代理映射为空");
    }
    for raw in system
        .bypass
        .split([';', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let raw = raw.to_ascii_lowercase();
        if raw == "<-loopback>" {
            // Chromium applies implicit local bypasses before executing any PAC.
            // It cannot represent the subtractive manual-proxy rule faithfully.
            bail!("浏览器自动代理不支持取消本地地址绕过的系统规则");
        }
        if raw == "<local>" {
            config.bypass.push(Bypass::Local { local: true });
            continue;
        }
        if !raw.contains("://") && raw.contains('/') {
            let (network, prefix) = raw.split_once('/').context("系统代理绕过网段无效")?;
            let address: Ipv4Addr = network.parse().context("系统代理包含不受支持的绕过网段")?;
            let prefix: u8 = prefix.parse().context("系统代理绕过网段无效")?;
            if prefix > 32 {
                bail!("系统代理绕过网段无效");
            }
            config.bypass.push(Bypass::Cidr {
                network: address.to_string(),
                prefix,
            });
            continue;
        }
        let (scheme, target) = if let Some((scheme, host)) = raw.split_once("://") {
            if !matches!(scheme, "http" | "https" | "ftp" | "ws" | "wss") {
                bail!("系统代理绕过协议不受支持");
            }
            (scheme, host)
        } else {
            ("", raw.as_str())
        };
        let (host, port) = split_authority(target)?;
        let host = if host.starts_with('[') {
            host[1..host.len() - 1]
                .parse::<Ipv6Addr>()
                .context("系统代理绕过地址无效")?
                .to_string()
        } else {
            validate_host(host, true)?;
            if host.starts_with('.') {
                format!("*{host}")
            } else {
                host.to_string()
            }
        };
        config.bypass.push(Bypass::Host {
            scheme: scheme.into(),
            host,
            port: port.unwrap_or(0),
        });
    }
    Ok(config)
}

fn split_authority(raw: &str) -> Result<(&str, Option<u16>)> {
    let (host, tail) = if raw.starts_with('[') {
        let end = raw.find(']').context("系统代理 IPv6 地址无效")? + 1;
        (&raw[..end], &raw[end..])
    } else if let Some(index) = raw.find(':') {
        (&raw[..index], &raw[index..])
    } else {
        (raw, "")
    };
    let port = if tail.is_empty() {
        None
    } else {
        let value = tail.strip_prefix(':').context("系统代理端口无效")?;
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            bail!("系统代理端口无效");
        }
        let value: u16 = value.parse().context("系统代理端口无效")?;
        if value == 0 {
            bail!("系统代理端口无效");
        }
        Some(value)
    };
    Ok((host, port))
}

fn validate_host(host: &str, wildcard: bool) -> Result<()> {
    if host.is_empty()
        || !host.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'.' | b'_' | b'-')
                || wildcard && matches!(b, b'*' | b'?')
        })
        || !host
            .bytes()
            .any(|b| b.is_ascii_alphanumeric() || wildcard && matches!(b, b'*' | b'?'))
    {
        bail!("系统静态代理地址或绕过规则不受支持");
    }
    Ok(())
}

fn parse_endpoint(raw: &str, default_kind: &str, guard_port: u16) -> Result<String> {
    let raw = raw.trim().to_ascii_lowercase();
    let (kind, target) = if let Some((scheme, target)) = raw.split_once("://") {
        let kind = match scheme {
            "http" => "PROXY",
            "https" => "HTTPS",
            "socks4" => "SOCKS",
            "socks" | "socks5" => "SOCKS5",
            _ => bail!("系统静态代理协议不受支持"),
        };
        (kind, target)
    } else {
        (default_kind, raw.as_str())
    };
    let (host, port) = split_authority(target)?;
    let port = port.unwrap_or(if kind == "HTTPS" {
        443
    } else if kind.starts_with("SOCKS") {
        1080
    } else {
        80
    });
    let local = if host.starts_with('[') {
        let ip: Ipv6Addr = host[1..host.len() - 1]
            .parse()
            .context("系统静态代理 IPv6 地址无效")?;
        ip.is_loopback()
            || ip.is_unspecified()
            || ip
                .to_ipv4()
                .is_some_and(|ip| ip.is_loopback() || ip.is_unspecified())
    } else {
        validate_host(host, false)?;
        let normalized = host.trim_end_matches('.');
        let numeric = normalized.bytes().all(|b| b.is_ascii_digit() || b == b'.');
        let hex_numeric =
            normalized.starts_with("0x") && normalized[2..].bytes().all(|b| b.is_ascii_hexdigit());
        if hex_numeric {
            bail!("系统代理使用了不明确的数字地址");
        }
        let ip = if numeric {
            Some(
                normalized
                    .parse::<Ipv4Addr>()
                    .context("系统代理使用了不明确的数字地址")?,
            )
        } else {
            None
        };
        normalized == "localhost"
            || normalized.ends_with(".localhost")
            || normalized == "loopback"
            || ip.is_some_and(|ip| ip.is_loopback() || ip.is_unspecified())
    };
    if local && port == guard_port {
        bail!("系统代理指向守卫自身，无法接入浏览器");
    }
    Ok(format!("{kind} {host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    fn system(server: &str, bypass: &str) -> SystemProxy {
        SystemProxy {
            supported: true,
            enabled: true,
            server: server.into(),
            bypass: bypass.into(),
            reason: String::new(),
        }
    }

    #[test]
    fn invalid_configuration_never_becomes_direct() {
        assert!(build(&SystemProxy::default(), 17890).is_err());
        for server in [
            "",
            ";;;",
            "proxy:99999",
            "proxy:0",
            "proxy:-1",
            "http=user:password@proxy:80",
            "http=a:80;HTTP=b:80",
            "quic=proxy:443",
            "proxy:80; DIRECT",
            "https://proxy:443/path",
            "[xyz]:80",
            "2130706433:17890",
            "0x7f000001:17890",
            "127.1:17890",
        ] {
            assert!(
                build(&system(server, ""), 17890).is_err(),
                "accepted {server}"
            );
        }
        for bypass in [
            "10.1.1.1/99",
            "10.999.0.0/8",
            "arbitrary<script>",
            "<-loopback>",
            "http://host:0",
            "https://host:65536",
            "[no-ipv6]",
            "[::1]/128",
        ] {
            assert!(
                build(&system("proxy:80", bypass), 17890).is_err(),
                "accepted {bypass}"
            );
        }
        assert!(build(&system("proxy:80", ""), 80).is_err());
        let disabled = SystemProxy {
            supported: true,
            enabled: false,
            server: "ignored".into(),
            ..Default::default()
        };
        assert!(build(&disabled, 17890).is_ok());
    }

    #[test]
    fn guard_self_reference_is_rejected() {
        for server in [
            "127.0.0.1:17890",
            "127.2.3.4:17890",
            "localhost:17890",
            "LOCALHOST.:17890",
            "a.localhost:17890",
            "[::1]:17890",
            "[::ffff:127.0.0.1]:17890",
            "0.0.0.0:17890",
            "http=proxy:80;socks=localhost:17890",
        ] {
            assert!(
                build(&system(server, ""), 17890).is_err(),
                "accepted {server}"
            );
        }
        assert!(build(&system("127.0.0.1:7897", ""), 17890).is_ok());
        assert!(build(&system("remote.example:17890", ""), 17890).is_ok());
    }

    #[test]
    fn data_url_encodes_utf8_without_external_dependencies() {
        assert_eq!(
            data_url("a z/中?#%"),
            "data:application/x-ns-proxy-autoconfig,a%20z%2F%E4%B8%AD%3F%23%25"
        );
    }

    // Run the exact generated JavaScript, not a Rust imitation of its routing.
    #[test]
    fn generated_pac_routes_in_javascript() {
        let proxy = "PROXY 127.0.0.1:7897";
        let guard = "PROXY 127.0.0.1:17890";
        let mut suites = Vec::new();
        let cases = [
            ("https://claude.ai/", guard),
            ("http://CLAUDE.COM./", guard),
            ("https://api.anthropic.com/", guard),
            ("wss://a.b.claude.ai/chat", guard),
            ("http://connect.claudeguard.invalid/start", guard),
            ("http://CONNECT.CLAUDEGUARD.INVALID./", guard),
            ("https://notclaude.ai/", proxy),
            ("https://claude.ai.attacker.test/", proxy),
            ("https://anthropic.com.evil/", proxy),
            ("http://fake.connect.claudeguard.invalid/", proxy),
            ("http://connect.claudeguard.invalid.evil/", proxy),
            ("https://example.com/", proxy),
            ("http://printer/", "DIRECT"),
            ("http://printer./", proxy),
            ("https://a.internal/", "DIRECT"),
            ("https://x.example:8443/", "DIRECT"),
            ("https://x.example/", proxy),
            ("http://x.example:8443/", proxy),
            ("http://10.2.99.1/", "DIRECT"),
            ("http://10.3.1.1/", proxy),
            ("http://[::1]/", "DIRECT"),
            ("http://[2001:db8::1]/", "DIRECT"),
            ("http://localhost/", "DIRECT"),
            ("http://a.localhost/", "DIRECT"),
            ("http://loopback/", "DIRECT"),
            ("http://127.2.3.4/", "DIRECT"),
            ("http://169.254.2.3/", "DIRECT"),
            ("http://[fe80::1]/", "DIRECT"),
            ("http://literal.example.test/", "DIRECT"),
            ("http://literalXexample.test/", proxy),
        ];
        let config = system("127.0.0.1:7897", "<local>;*.internal;https://x.example:8443;10.2.0.0/16;[2001:0db8::1];literal.example.test;claude.ai;*.anthropic.com");
        suites.push(serde_json::json!({"script":build(&config,17890).unwrap(),"cases":cases}));
        let config = system(
            "http=proxy.test:80;https=https://secure.test;socks=socks5://socks.test",
            "",
        );
        suites.push(serde_json::json!({"script":build(&config,17890).unwrap(),"cases":[
            ["http://example.com/","PROXY proxy.test:80"], ["https://example.com/","HTTPS secure.test:443"],
            ["ws://example.com/","SOCKS5 socks.test:1080"], ["wss://example.com/","SOCKS5 socks.test:1080"],
            ["ftp://example.com/","SOCKS5 socks.test:1080"], ["https://claude.ai/",guard]
        ]}));
        let config = system("http=proxy.test:81;https=secure.test:82", "");
        suites.push(serde_json::json!({"script":build(&config,17890).unwrap(),"cases":[
            ["ws://example.com/","PROXY secure.test:82"], ["wss://example.com/","PROXY secure.test:82"]
        ]}));
        let config = system("http=proxy.test:81", "");
        suites.push(
            serde_json::json!({"script":build(&config,17890).unwrap(),"cases":[
                ["https://example.com/","DIRECT"], ["wss://example.com/","PROXY proxy.test:81"]
            ]}),
        );
        for (server, expected) in [
            ("socks=proxy.test", "SOCKS proxy.test:1080"),
            ("socks://proxy.test", "SOCKS5 proxy.test:1080"),
        ] {
            suites.push(serde_json::json!({"script":build(&system(server,""),17890).unwrap(),"cases":[["https://example.com/",expected]]}));
        }
        let config = SystemProxy {
            supported: true,
            ..Default::default()
        };
        suites.push(
            serde_json::json!({"script":build(&config,23456).unwrap(),"cases":[
                ["https://example.com/","DIRECT"], ["https://claude.ai/","PROXY 127.0.0.1:23456"]
            ]}),
        );
        let script = r#"const fs=require('fs'),vm=require('vm'),assert=require('assert/strict');
for (const suite of JSON.parse(fs.readFileSync(0,'utf8'))) {
 const context=vm.createContext({}); vm.runInContext(suite.script,context);
 for(const [url,expected] of suite.cases) {
  const host=new URL(url).hostname.replace(/^\[|\]$/g,'');
  assert.equal(context.FindProxyForURL(url,host),expected,url);
 }
}"#;
        let mut command = Command::new("node");
        command
            .args(["-e", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = command
            .spawn()
            .expect("PAC JavaScript verification requires Node.js (also used by extension tests)");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&suites).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
