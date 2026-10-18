use anyhow::{bail, ensure, Context, Result};
use chrono::{TimeZone, Utc};
use guard_protocol::codex::QuotaWindow;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
};

pub struct AccountClient {
    child: Child,
    input: ChildStdin,
    pub messages: mpsc::Receiver<Value>,
    reader: tokio::task::JoinHandle<()>,
    sequence: u64,
    pub auth_epoch: u64,
}
impl Drop for AccountClient {
    fn drop(&mut self) {
        self.reader.abort();
        let _ = self.child.start_kill();
    }
}
impl AccountClient {
    pub async fn start(path: PathBuf) -> Result<Self> {
        let mut cmd = Command::new(path);
        cmd.args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // Never inherit a project directory (or its project configuration).
        if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
            cmd.current_dir(home);
        }
        Self::connect(cmd).await
    }
    async fn connect(mut cmd: Command) -> Result<Self> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let mut child = cmd.spawn().context("Codex 启动失败")?;
        let input = child.stdin.take().context("Codex 输入不可用")?;
        let output = child.stdout.take().context("Codex 输出不可用")?;
        let (tx, messages) = mpsc::channel(32);
        let reader = tokio::spawn(async move {
            let mut output = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                // A broken/hostile executable must not allocate an unlimited line.
                use tokio::io::AsyncReadExt;
                let mut limited = (&mut output).take(1_048_577);
                let Ok(n) = limited.read_until(b'\n', &mut bytes).await else {
                    break;
                };
                if n == 0 || n > 1_048_576 {
                    break;
                }
                if let Ok(value) = serde_json::from_slice(&bytes) {
                    if tx.send(value).await.is_err() {
                        break;
                    }
                }
            }
        });
        let mut client = Self {
            child,
            input,
            messages,
            reader,
            sequence: 0,
            auth_epoch: 0,
        };
        client.request("initialize", Some(json!({"clientInfo": {"name":"melian","title":"Melian","version":guard_protocol::VERSION}}))).await?;
        client
            .send(json!({"method":"initialized","params":{}}))
            .await?;
        Ok(client)
    }
    async fn send(&mut self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        Ok(())
    }
    pub async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        // This allowlist prevents accidental login, thread creation or reset redemption.
        ensure!(
            matches!(
                method,
                "initialize" | "account/read" | "account/rateLimits/read"
            ),
            "仅支持读取"
        );
        self.sequence += 1;
        let id = self.sequence;
        let mut request = json!({"id": id, "method":method});
        if let Some(params) = params {
            request["params"] = params;
        }
        self.send(request).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(value) = self.messages.recv().await {
                if value["method"] == "account/updated" {
                    self.auth_epoch = self.auth_epoch.wrapping_add(1);
                }
                if value["id"].as_u64() == Some(id) {
                    // Do not propagate upstream errors that might contain account data.
                    ensure!(value.get("error").is_none(), "Codex 接口暂不可用");
                    return value.get("result").cloned().context("Codex 响应不完整");
                }
            }
            bail!("Codex 连接已结束")
        })
        .await
        .context("Codex 查询超时")?
    }
}

pub fn identity(account: &Value) -> Option<String> {
    (!account.is_null()).then(|| hex::encode(Sha256::digest(account.to_string().as_bytes())))
}
pub fn parse_windows(value: &Value) -> Result<Vec<QuotaWindow>> {
    let buckets: Vec<(String, &Value)> = if let Some(map) = value["rateLimitsByLimitId"]
        .as_object()
        .filter(|m| !m.is_empty())
    {
        map.iter().map(|(k, v)| (k.clone(), v)).collect()
    } else if value["rateLimits"].is_object() {
        vec![("codex".into(), &value["rateLimits"])]
    } else {
        bail!("额度数据暂不可用")
    };
    let mut windows = Vec::new();
    for (id, bucket) in buckets {
        for key in ["primary", "secondary"] {
            let w = &bucket[key];
            if w.is_null() {
                continue;
            }
            let used = w["usedPercent"]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0. && *v <= 100.)
                .context("额度数据不完整")?;
            let duration = w["windowDurationMins"].as_i64().filter(|v| *v > 0);
            let period = match duration {
                Some(10080) => "每周".into(),
                Some(m) if m % 1440 == 0 => format!("{} 天", m / 1440),
                Some(m) if m % 60 == 0 => format!("{} 小时", m / 60),
                Some(m) => format!("{m} 分钟"),
                None => if key == "primary" {
                    "主要额度"
                } else {
                    "次要额度"
                }
                .into(),
            };
            let name = bucket["limitName"].as_str().unwrap_or(&id);
            windows.push(QuotaWindow {
                id: format!("{id}:{key}"),
                label: if id == "codex" {
                    period
                } else {
                    format!("{name} · {period}")
                },
                used_percent: used,
                duration_minutes: duration,
                resets_at: w["resetsAt"]
                    .as_i64()
                    .and_then(|s| Utc.timestamp_opt(s, 0).single()),
            });
        }
    }
    ensure!(!windows.is_empty(), "此账号没有可读取的额度窗口");
    ensure!(windows.len() <= 32, "额度窗口过多");
    Ok(windows)
}

pub fn discover() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let root = PathBuf::from(local).join("OpenAI/Codex/bin");
        if let Ok(entries) = std::fs::read_dir(root) {
            let mut versions: Vec<_> = entries
                .flatten()
                .map(|e| e.path().join("codex.exe"))
                .filter(|p| p.is_file())
                .collect();
            versions.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
            candidates.extend(versions.into_iter().rev());
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            for suffix in [
                "Applications/Codex.app/Contents/Resources/codex",
                ".local/bin/codex",
                ".cargo/bin/codex",
            ] {
                candidates.push(PathBuf::from(&home).join(suffix));
            }
        }
        for path in [
            "/Applications/Codex.app/Contents/Resources/codex",
            "/opt/homebrew/bin/codex",
            "/usr/local/bin/codex",
        ] {
            candidates.push(path.into());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join(if cfg!(windows) { "codex.exe" } else { "codex" }));
            #[cfg(windows)]
            for suffix in [
                "node_modules/@openai/codex/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
                "node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
                "node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
            ] { candidates.push(dir.join(suffix)); }
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "subprocess fixture, invoked only by transport test"]
    fn stub_server() {
        if std::env::var_os("MELIAN_TEST_APP_SERVER").is_none() {
            return;
        }
        use std::io::{BufRead, Write};
        let mut initialized = false;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["method"] == "initialized" {
                initialized = true;
                continue;
            }
            let result = match v["method"].as_str().unwrap() {
                "initialize" => json!({}),
                "account/read" => {
                    assert!(initialized);
                    json!({"account":{"type":"chatgpt"}})
                }
                "account/rateLimits/read" => {
                    println!(
                        "{}",
                        json!({"method":"account/updated","params":{"authMode":"chatgpt"}})
                    );
                    println!("{}", json!({"id":999,"result":{"wrong":true}}));
                    json!({"rateLimits":{"primary":{"usedPercent":42}}})
                }
                _ => panic!("unexpected write operation"),
            };
            println!("{}", json!({"id":v["id"],"result":result}));
            std::io::stdout().flush().unwrap();
        }
    }
    #[tokio::test]
    async fn rpc_handshake_ids_notifications_and_read_only_allowlist() {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "codex::account::tests::stub_server",
            "--ignored",
            "--nocapture",
        ])
        .env("MELIAN_TEST_APP_SERVER", "1");
        let mut c = AccountClient::connect(cmd).await.unwrap();
        let a = c
            .request("account/read", Some(json!({"refreshToken":false})))
            .await
            .unwrap();
        assert_eq!(a["account"]["type"], "chatgpt");
        let r = c.request("account/rateLimits/read", None).await.unwrap();
        assert_eq!(parse_windows(&r).unwrap()[0].used_percent, 42.);
        assert_eq!(c.auth_epoch, 1);
        assert!(c
            .request("account/rateLimitResetCredit/consume", None)
            .await
            .is_err());
        assert!(c.request("thread/start", None).await.is_err());
        c.child.start_kill().unwrap();
        tokio::time::timeout(Duration::from_secs(3), c.child.wait())
            .await
            .unwrap()
            .unwrap();
    }
    #[test]
    fn multiple_windows_and_legacy_response() {
        let w = json!({"usedPercent":97,"windowDurationMins":10080,"resetsAt":1792649658});
        let r=parse_windows(&json!({"rateLimitsByLimitId":{"codex":{"primary":w,"secondary":null},"review":{"primary":{"usedPercent":0}}}})).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].label, "每周");
        assert_eq!(r[0].resets_at.unwrap().timestamp(), 1792649658);
        assert_eq!(
            parse_windows(&json!({"rateLimits":{"primary":w}}))
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn missing_and_bad_data_are_not_zero_usage() {
        assert!(parse_windows(&json!({})).is_err());
        assert!(parse_windows(&json!({"rateLimits":{"primary":{"resetsAt":123}}})).is_err());
        assert!(parse_windows(&json!({"rateLimits":{"primary":{"usedPercent":-1}}})).is_err());
    }
}
