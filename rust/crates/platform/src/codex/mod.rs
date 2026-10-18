//! Codex monitoring is read-only and never participates in the Claude connection gate.
pub mod account;
pub mod announcements;
pub mod presentation;
use chrono::{DateTime, Utc};
use guard_protocol::codex::{CodexState, ResetAnnouncement};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub fn open_link(url: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        url == announcements::SOURCE || announcements::safe_post(url),
        "公告链接不可用"
    );
    #[cfg(windows)]
    crate::launch::shell_open(std::path::Path::new(url), &[], None)?;
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .spawn()?;
    }
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    seeded: bool,
    seen: Vec<String>,
    announcement: Option<ResetAnnouncement>,
    updated_at: Option<DateTime<Utc>>,
    unread: bool,
    notification_id: u64,
    notification: String,
}
struct Inner {
    state: CodexState,
    cache: Cache,
    identity: Option<String>,
}
pub struct Monitor {
    inner: Mutex<Inner>,
    refresh_account: Notify,
    refresh_feed: Notify,
    started: AtomicBool,
    cache_path: PathBuf,
}
impl Monitor {
    pub fn new() -> Arc<Self> {
        Self::at(guard_protocol::settings_dir().join("codex-monitor.json"))
    }
    fn at(path: PathBuf) -> Arc<Self> {
        let cache: Cache = std::fs::read(&path)
            .ok()
            .filter(|b| b.len() < 65_536)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let state = CodexState {
            availability: "checking".into(),
            message: "正在读取 Codex".into(),
            announcement: cache
                .announcement
                .clone()
                .filter(|a| announcements::safe_post(&a.url)),
            announcement_updated_at: cache.updated_at,
            unread: cache.unread,
            notification_id: cache.notification_id,
            notification: cache.notification.clone(),
            ..Default::default()
        };
        Arc::new(Self {
            inner: Mutex::new(Inner {
                state,
                cache,
                identity: None,
            }),
            refresh_account: Notify::new(),
            refresh_feed: Notify::new(),
            started: AtomicBool::new(false),
            cache_path: path,
        })
    }
    pub fn state(&self) -> CodexState {
        self.inner
            .lock()
            .unwrap()
            .state
            .clone()
            .freshened(Utc::now())
    }
    pub fn refresh(&self) {
        self.refresh_account.notify_one();
        self.refresh_feed.notify_one();
    }
    pub fn mark_read(&self) {
        let mut i = self.inner.lock().unwrap();
        if i.state.unread {
            i.state.unread = false;
            i.cache.unread = false;
            self.save(&i.cache);
        }
    }
    fn save(&self, cache: &Cache) {
        // No credential, email, account fingerprint or chat content is persisted.
        let _ = (|| -> anyhow::Result<()> {
            if let Some(parent) = self.cache_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = self
                .cache_path
                .with_extension(format!("{}.tmp", std::process::id()));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            let mut file = options.open(&tmp)?;
            file.write_all(&serde_json::to_vec(cache)?)?;
            file.sync_all()?;
            drop(file);
            guard_protocol::atomic_replace(&tmp, &self.cache_path)?;
            Ok(())
        })();
    }
    fn notify(i: &mut Inner, text: &str) {
        i.state.unread = true;
        i.state.notification_id = i.state.notification_id.saturating_add(1);
        i.state.notification = text.into();
        i.cache.unread = true;
        i.cache.notification_id = i.state.notification_id;
        i.cache.notification = text.into();
    }
    fn auth(&self, account: &Value) -> bool {
        let mut i = self.inner.lock().unwrap();
        let identity = account::identity(account);
        if identity != i.identity {
            i.state.windows.clear();
            i.state.account_updated_at = None;
            i.identity = identity;
        }
        if account.is_null() {
            i.state.availability = "signedOut".into();
            i.state.message = "请先在 Codex 中登录".into();
            return false;
        }
        if !matches!(
            account["type"].as_str(),
            Some("chatgpt" | "chatgptAuthTokens")
        ) {
            i.state.windows.clear();
            i.state.availability = "unsupported".into();
            i.state.message = "此登录方式不提供订阅额度".into();
            return false;
        }
        true
    }
    fn quota(&self, value: &Value) -> anyhow::Result<()> {
        let windows = account::parse_windows(value)?;
        let now = Utc::now();
        let mut i = self.inner.lock().unwrap();
        let exhausted: Vec<_> = i
            .state
            .windows
            .iter()
            .filter(|w| w.used_percent >= 100.)
            .collect();
        let recovered = i.state.availability == "ready"
            && i.state
                .account_updated_at
                .is_some_and(|t| (now - t).num_seconds() <= 180)
            && !exhausted.is_empty()
            && exhausted.iter().all(|old| {
                windows
                    .iter()
                    .any(|w| w.id == old.id && w.used_percent < 100.)
            });
        i.state.windows = windows;
        i.state.account_updated_at = Some(now);
        i.state.availability = "ready".into();
        i.state.message = String::new();
        if recovered {
            Self::notify(&mut i, "Codex 额度已恢复");
            self.save(&i.cache);
        }
        Ok(())
    }
    fn account_error(&self, status: &str, message: &str) {
        let mut i = self.inner.lock().unwrap();
        i.state.availability = status.into();
        i.state.message = message.into();
    }
    fn feed(&self, value: Option<ResetAnnouncement>) {
        let now = Utc::now();
        let mut i = self.inner.lock().unwrap();
        if let Some(a) = &value {
            let key = format!(
                "{}|{}|{}|{}",
                a.id,
                a.phase,
                a.expected_by.map(|t| t.timestamp()).unwrap_or(0),
                a.summary
            );
            if !i.cache.seen.contains(&key) {
                let new = i.cache.seeded
                    && a.phase == "announced"
                    && a.published_at.is_some_and(|t| (now - t).num_hours() < 24);
                if new {
                    Self::notify(&mut i, "Tibo 发布了重置预告");
                }
                i.cache.seen.push(key);
                if i.cache.seen.len() > 64 {
                    i.cache.seen.remove(0);
                }
            }
        }
        i.cache.seeded = true;
        i.cache.announcement = value.clone();
        i.cache.updated_at = Some(now);
        i.state.announcement = value;
        i.state.announcement_updated_at = Some(now);
        i.state.announcement_error = None;
        self.save(&i.cache);
    }
    pub fn start(self: &Arc<Self>, stop: CancellationToken) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let monitor = self.clone();
        let cancel = stop.clone();
        tokio::spawn(async move {
            monitor.account_loop(cancel).await;
        });
        let monitor = self.clone();
        tokio::spawn(async move {
            monitor.feed_loop(stop).await;
        });
    }
    async fn account_loop(self: Arc<Self>, stop: CancellationToken) {
        let mut client: Option<account::AccountClient> = None;
        let mut due = Instant::now();
        let mut last = Instant::now() - Duration::from_secs(60);
        let mut reset_seen = std::collections::BTreeSet::new();
        let mut last_wall = Utc::now();
        loop {
            if stop.is_cancelled() {
                break;
            }
            let now = Utc::now();
            if (now - last_wall).num_seconds() > 10 || now < last_wall {
                due = Instant::now();
            }
            last_wall = now;
            for w in &self.state().windows {
                if let Some(at) = w.resets_at {
                    if at <= now && reset_seen.insert((w.id.clone(), at.timestamp())) {
                        due = Instant::now();
                    }
                }
            }
            if let Some(a) = self.state().announcement {
                if let Some(at) = a.expected_by {
                    if at <= now
                        && reset_seen.insert((format!("announcement:{}", a.id), at.timestamp()))
                    {
                        due = Instant::now();
                    }
                }
            }
            if reset_seen.len() > 128 {
                reset_seen.clear();
            }
            if Instant::now() >= due && last.elapsed() >= Duration::from_secs(15) {
                last = Instant::now();
                let result = tokio::select! {
                    _=stop.cancelled()=>break,
                    result=async {
                        if client.is_none() {
                            let Some(path)=account::discover() else {
                                self.account_error("notInstalled","未找到 Codex，请先安装并登录");
                                return Ok(true);
                            };
                            client=Some(account::AccountClient::start(path).await?);
                        }
                        let c=client.as_mut().unwrap();
                        let result=c.request("account/read",Some(json!({"refreshToken":false}))).await?;
                        let account=result.get("account").ok_or_else(||anyhow::anyhow!("账号响应不完整"))?;
                        if self.auth(account) {
                            let epoch=c.auth_epoch;
                            let data=c.request("account/rateLimits/read",None).await?;
                            let confirmed=c.request("account/read",Some(json!({"refreshToken":false}))).await?;
                            let confirmed=confirmed.get("account").ok_or_else(||anyhow::anyhow!("账号响应不完整"))?;
                            if epoch!=c.auth_epoch || account::identity(confirmed)!=account::identity(account) {
                                self.auth(confirmed);
                                let mut i=self.inner.lock().unwrap();
                                i.state.windows.clear();i.state.account_updated_at=None;
                                return Ok(false);
                            }
                            self.quota(&data)?;
                        }
                        Ok::<_,anyhow::Error>(true)
                    }=>result
                };
                due = match result {
                    Ok(true) => Instant::now() + Duration::from_secs(60),
                    Ok(false) => {
                        // Startup can emit account/updated between reads. Keep this child:
                        // restarting it would recreate the same initialization race.
                        self.account_error("checking", "正在同步 Codex 账号");
                        Instant::now() + Duration::from_secs(15)
                    }
                    Err(_) => {
                        self.account_error("unavailable", "暂时无法读取 Codex 额度");
                        client = None;
                        Instant::now() + Duration::from_secs(60)
                    }
                };
            }
            tokio::select! {
                _=stop.cancelled()=>break,
                _=self.refresh_account.notified()=>{due=Instant::now();},
                _=tokio::time::sleep(Duration::from_secs(1))=>{},
                value=async {
                    if let Some(c)=&mut client {c.messages.recv().await} else {std::future::pending().await}
                }=>{
                    match value {
                        Some(v) if v["method"]=="account/updated"=>{
                            let mut i=self.inner.lock().unwrap();
                            i.state.windows.clear();i.state.account_updated_at=None;i.identity=None;
                            i.state.availability="checking".into();i.state.message="正在更新 Codex 账号".into();
                            due=Instant::now();
                        },
                        // Verify account identity on every change; never apply a late event to another account.
                        Some(v) if v["method"]=="account/rateLimits/updated"=>{due=Instant::now();},
                        Some(_)=>{},
                        None=>{client=None;self.account_error("unavailable","Codex 连接已结束");due=Instant::now()+Duration::from_secs(60);}
                    }
                }
            }
        }
        drop(client);
    }
    async fn feed_loop(self: Arc<Self>, stop: CancellationToken) {
        let mut due = Instant::now();
        let mut last = Instant::now() - Duration::from_secs(120);
        let mut failures = 0u32;
        loop {
            tokio::select! {
                _=stop.cancelled()=>break,
                _=tokio::time::sleep_until(tokio::time::Instant::from_std(due))=>{},
                _=self.refresh_feed.notified()=>{
                    // Also respect server Retry-After / failure backoff.
                    if failures>0 || Instant::now()<due || last.elapsed()<Duration::from_secs(120){continue;}
                }
            }
            last = Instant::now();
            let result = tokio::select! {_=stop.cancelled()=>break, r=fetch_feed()=>r};
            let delay = match result {
                Ok((feed, timeline, cache_seconds)) => {
                    match announcements::parse(&feed, &timeline, Utc::now()) {
                        Ok(a) => {
                            self.feed(a);
                            failures = 0;
                        }
                        Err(_) => {
                            self.inner.lock().unwrap().state.announcement_error =
                                Some("公告来源暂未更新".into());
                            failures += 1;
                        }
                    }
                    cache_seconds.max(120)
                }
                Err(retry) => {
                    failures += 1;
                    self.inner.lock().unwrap().state.announcement_error =
                        Some("公告暂时无法更新".into());
                    retry.max((120u64.saturating_mul(1u64 << failures.min(4))).min(1800))
                }
            };
            due = Instant::now() + Duration::from_secs(delay);
        }
    }
}

async fn fetch_feed() -> Result<(Value, Value, u64), u64> {
    let client = http_client().map_err(|_| 120u64)?;
    let (feed, delay) = get_json(&client, "https://codex-reset.com/api/feed").await?;
    let (timeline, timeline_delay) =
        get_json(&client, "https://codex-reset.com/api/timeline").await?;
    Ok((feed, timeline, delay.max(timeline_delay)))
}
fn http_client() -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!(
            "Melian/{} (+https://github.com/JJchess/Melian)",
            guard_protocol::VERSION
        ));
    // reqwest uses environment proxies. Read the OS static setting when no override exists.
    if !["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|k| std::env::var_os(k).is_some())
    {
        if let Some(proxy) = system_https_proxy() {
            builder = builder.proxy(reqwest::Proxy::https(proxy)?);
        }
    }
    Ok(builder.build()?)
}
fn system_https_proxy() -> Option<String> {
    #[cfg(windows)]
    {
        let p = crate::integration::system_proxy();
        if p.enabled && p.supported {
            let selected = if p.server.contains('=') {
                p.server
                    .split(';')
                    .find_map(|s| s.trim().strip_prefix("https=").map(str::to_owned))?
            } else {
                p.server
            };
            return Some(if selected.contains("://") {
                selected
            } else {
                format!("http://{selected}")
            });
        }
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/sbin/scutil")
            .arg("--proxy")
            .output()
            .ok()?;
        let output = String::from_utf8_lossy(&output.stdout);
        let values: std::collections::BTreeMap<_, _> = output
            .lines()
            .filter_map(|l| l.trim().split_once(" : "))
            .collect();
        if values.get("HTTPSEnable") == Some(&"1") {
            return Some(format!(
                "http://{}:{}",
                values.get("HTTPSProxy")?,
                values.get("HTTPSPort")?
            ));
        }
    }
    None
}
async fn get_json(client: &reqwest::Client, url: &str) -> Result<(Value, u64), u64> {
    let mut r = client.get(url).send().await.map_err(|_| 120u64)?;
    let retry = r
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.parse::<u64>().ok().or_else(|| {
                DateTime::parse_from_rfc2822(s)
                    .ok()
                    .map(|t| (t.with_timezone(&Utc) - Utc::now()).num_seconds().max(1) as u64)
            })
        })
        .unwrap_or(120)
        .min(86400);
    if !r.status().is_success() {
        return Err(retry);
    }
    let max_age = r
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(',')
                .find_map(|x| x.trim().strip_prefix("max-age=")?.parse::<u64>().ok())
        })
        .unwrap_or(120)
        .min(86400);
    let mut body = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|_| retry)? {
        if body.len() + chunk.len() > 2_097_152 {
            return Err(retry);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map(|v| (v, max_age))
        .map_err(|_| retry)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn http_cache_and_retry_after_are_preserved() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, headers, expected) in [
            ("200 OK", "Cache-Control: max-age=600", 600u64),
            ("429 Too Many Requests", "Retry-After: 720", 720u64),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/feed", listener.local_addr().unwrap());
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut b = [0u8; 4096];
                let _ = stream.read(&mut b).await;
                stream.write_all(format!("HTTP/1.1 {status}\r\n{headers}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes()).await.unwrap();
            });
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let response = get_json(&client, &url).await;
            assert_eq!(
                match response {
                    Ok((_, seconds)) => seconds,
                    Err(seconds) => seconds,
                },
                expected
            );
            task.await.unwrap();
        }
    }
    fn monitor() -> Arc<Monitor> {
        Monitor::at(
            std::env::temp_dir().join(format!("melian-codex-test-{}.json", uuid::Uuid::new_v4())),
        )
    }
    fn quota(used: u8) -> Value {
        json!({"rateLimits":{"primary":{"usedPercent":used,"windowDurationMins":10080}}})
    }
    #[test]
    fn account_switch_clears_old_windows_and_does_not_notify() {
        let m = monitor();
        m.auth(&json!({"type":"chatgpt","email":"one"}));
        m.quota(&quota(100)).unwrap();
        m.auth(&json!({"type":"chatgpt","email":"two"}));
        assert!(m.state().windows.is_empty());
        m.quota(&quota(0)).unwrap();
        assert!(!m.state().unread);
        assert!(!m.auth(&Value::Null));
        assert!(m.state().windows.is_empty());
    }
    #[test]
    fn recovery_is_not_assumed_from_errors_or_startup() {
        let m = monitor();
        m.quota(&quota(100)).unwrap();
        assert!(!m.state().unread);
        m.account_error("unavailable", "offline");
        m.quota(&quota(0)).unwrap();
        assert!(!m.state().unread);
        m.quota(&quota(100)).unwrap();
        m.quota(&quota(1)).unwrap();
        assert!(m.state().unread);
        let id = m.state().notification_id;
        m.quota(&quota(0)).unwrap();
        assert_eq!(m.state().notification_id, id);
        m.mark_read();
        assert!(!m.state().unread);
        let _ = std::fs::remove_file(&m.cache_path);
    }
    #[test]
    fn first_feed_is_quiet_and_new_announcements_deduplicate() {
        let m = monitor();
        let a = ResetAnnouncement {
            id: "1".into(),
            phase: "announced".into(),
            published_at: Some(Utc::now()),
            url: "https://x.com/thsottiaux/status/1".into(),
            ..Default::default()
        };
        m.feed(Some(a.clone()));
        assert!(!m.state().unread);
        let b = ResetAnnouncement {
            id: "2".into(),
            ..a
        };
        m.feed(Some(b.clone()));
        assert!(m.state().unread);
        let id = m.state().notification_id;
        m.mark_read();
        m.feed(Some(b));
        assert_eq!(m.state().notification_id, id);
        assert!(!m.state().unread);
        let restored = Monitor::at(m.cache_path.clone());
        assert_eq!(restored.state().notification_id, id);
        assert!(restored.state().windows.is_empty());
        let _ = std::fs::remove_file(&m.cache_path);
    }
}
