use crate::{browser, desktop, integration, ipc, launch};
use anyhow::{bail, Result};
use chrono::Utc;
use guard_core::Engine;
use guard_protocol::{pipe_path, BrowserState, DesktopState, Request, Response, State};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone)]
pub enum UiEvent {
    Show,
    Exit,
    Feedback(String),
    BrowserSetupDone(Option<String>),
    WebOpened(Option<String>),
    BrowserRouteFailed(String),
}
struct Session {
    state: BrowserState,
    seen: Instant,
}
#[derive(Default)]
pub struct Sessions {
    browsers: HashMap<String, Session>,
    commands: HashMap<String, Instant>,
}
impl Sessions {
    pub fn report(&mut self, r: &Request) -> Result<()> {
        let name = r.browser.as_deref().unwrap_or("");
        if !matches!(name, "edge" | "chrome") {
            bail!("未知浏览器");
        }
        let id = r.client_id.as_deref().unwrap_or("");
        uuid::Uuid::parse_str(id)?;
        self.browsers
            .retain(|_, s| s.seen.elapsed() < Duration::from_secs(60));
        self.commands
            .retain(|_, t| t.elapsed() < Duration::from_secs(5));
        self.browsers.insert(
            id.into(),
            Session {
                state: BrowserState {
                    browser: name.into(),
                    installed: true,
                    configured: r.connected && r.protected,
                    restart_required: false,
                    connection_kind: "extension".into(),
                    proxy_fingerprint: String::new(),
                    policy_revision: String::new(),
                    connected: r.connected,
                    protected: r.connected && r.protected,
                    reason: r
                        .reason
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(300)
                        .collect(),
                    last_seen: Some(Utc::now()),
                },
                seen: Instant::now(),
            },
        );
        Ok(())
    }
    pub fn merge(&self, state: &mut State) {
        for name in ["edge", "chrome"] {
            let session = self
                .browsers
                .values()
                .filter(|s| s.state.browser == name && s.seen.elapsed() < Duration::from_secs(5))
                .max_by_key(|s| (s.state.connected && s.state.protected, s.seen));
            state.browsers.insert(
                name.into(),
                session.map(|s| s.state.clone()).unwrap_or(BrowserState {
                    browser: name.into(),
                    reason: "尚未启用网页保护".into(),
                    ..BrowserState::default()
                }),
            );
        }
    }
    pub fn queue_web(&mut self, allowed: bool) -> Result<()> {
        if !allowed {
            bail!("连接尚未就绪");
        }
        let selected = self
            .browsers
            .iter()
            .filter(|(_, s)| {
                s.state.connected && s.state.protected && s.seen.elapsed() < Duration::from_secs(5)
            })
            .max_by_key(|(_, s)| s.seen)
            .map(|(id, _)| id.clone());
        let Some(id) = selected else {
            bail!("请先为 Edge 或 Chrome 接入扩展");
        };
        self.commands.insert(id, Instant::now());
        Ok(())
    }
    pub fn take_command(&mut self, id: &str, allowed: bool) -> Option<String> {
        let time = self.commands.remove(id)?;
        let browser = self.browsers.get(id)?;
        (allowed
            && time.elapsed() < Duration::from_secs(5)
            && browser.seen.elapsed() < Duration::from_secs(5)
            && browser.state.connected
            && browser.state.protected)
            .then(|| "openClaude".into())
    }
}
pub struct Host {
    pub engine: Arc<Engine>,
    sessions: Mutex<Sessions>,
    desktop: Mutex<DesktopState>,
    native_browsers: Mutex<BTreeMap<String, BrowserState>>,
    browser_setup: tokio::sync::Mutex<()>,
    web_pending: Arc<Mutex<Option<(String, Instant)>>>,
    pub ui: UnboundedSender<UiEvent>,
}
impl Host {
    pub fn new(engine: Arc<Engine>, ui: UnboundedSender<UiEvent>) -> Arc<Self> {
        Arc::new(Self {
            engine,
            sessions: Mutex::new(Sessions::default()),
            desktop: Mutex::new(DesktopState::default()),
            native_browsers: Mutex::new(BTreeMap::new()),
            browser_setup: tokio::sync::Mutex::new(()),
            web_pending: Arc::new(Mutex::new(None)),
            ui,
        })
    }
    pub fn state(&self) -> State {
        let mut state = self.engine.state();
        self.sessions.lock().unwrap().merge(&mut state);
        for (name, native) in self.native_browsers.lock().unwrap().iter() {
            if native.configured || !state.browsers.get(name).is_some_and(|s| s.connected) {
                let mut browser = native.clone();
                let verified = self.engine.web_probe.verified(name).is_some();
                browser.connected = browser.configured && !browser.restart_required && verified;
                browser.protected = browser.connected;
                if browser.protected {
                    browser.reason = "网页连接已通过守卫验证".into();
                }
                state.browsers.insert(name.clone(), browser);
            }
        }
        state.desktop = self.desktop.lock().unwrap().clone();
        state
    }
    fn ensure_allowed(&self) -> Result<()> {
        if !self.state().fresh_allowed() || self.engine.gate.permit().is_none() {
            bail!("连接尚未就绪");
        }
        Ok(())
    }
    pub fn open_web(&self) -> Result<()> {
        self.ensure_allowed()?;
        if self.sessions.lock().unwrap().queue_web(true).is_ok() {
            return Ok(());
        }
        self.refresh_browsers();
        let state = self.state();
        let preference = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey(
                r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
            )
            .and_then(|k| k.get_value::<String, _>("ProgId"))
            .unwrap_or_default();
        let names = if preference.to_ascii_lowercase().contains("chrome") {
            ["chrome", "edge"]
        } else {
            ["edge", "chrome"]
        };
        let selected = names.into_iter().find(|name| {
            state
                .browsers
                .get(*name)
                .is_some_and(|b| b.configured && !b.restart_required)
        });
        let Some(name) = selected else {
            bail!("请在胶囊中完成网页保护，登录状态会保留");
        };
        self.open_web_in(name)
    }
    fn open_web_in(&self, name: &str) -> Result<()> {
        self.ensure_allowed()?;
        let mut pending = self.web_pending.lock().unwrap();
        if pending
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() < Duration::from_secs(16))
        {
            return Ok(());
        }
        let status = browser::configured_status(name, self.engine.settings().proxy_port);
        if !status.configured || status.restart_required {
            bail!("浏览器保护尚未应用，请完成面板中的一步操作");
        }
        let executable = browser::resolve(name)?;
        let issued = Instant::now();
        let url = self.engine.web_probe.issue(name);
        self.ensure_allowed()?;
        launch::shell_open(&executable, &[url], None)?;
        *pending = Some((name.into(), issued));
        drop(pending);
        let pending = self.web_pending.clone();
        let engine = self.engine.clone();
        let ui = self.ui.clone();
        let name = name.to_owned();
        tokio::spawn(async move {
            let mut verified = false;
            for _ in 0..30 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if engine
                    .web_probe
                    .verified(&name)
                    .is_some_and(|t| t >= issued)
                {
                    verified = true;
                    break;
                }
                if engine.stop.is_cancelled() || engine.gate.permit().is_none() {
                    break;
                }
            }
            let mut flight = pending.lock().unwrap();
            if flight.as_ref().is_some_and(|(_, t)| *t == issued) {
                *flight = None;
                if !verified && !engine.stop.is_cancelled() && engine.gate.permit().is_some() {
                    let _ = ui.send(UiEvent::BrowserRouteFailed(name));
                }
            }
        });
        Ok(())
    }
    pub fn refresh_browsers(&self) {
        let port = self.engine.settings().proxy_port;
        let mut states = BTreeMap::new();
        for name in ["edge", "chrome"] {
            let status = browser::configured_status(name, port);
            let changed = self
                .native_browsers
                .lock()
                .unwrap()
                .get(name)
                .is_none_or(|s| s.policy_revision != status.fingerprint);
            if !status.configured || status.restart_required || changed {
                self.engine.web_probe.clear(name);
            }
            states.insert(
                name.into(),
                BrowserState {
                    browser: name.into(),
                    installed: status.installed,
                    configured: status.configured,
                    restart_required: status.restart_required,
                    proxy_fingerprint: status.pac_fingerprint,
                    policy_revision: status.fingerprint,
                    connection_kind: "native".into(),
                    reason: status.reason,
                    ..BrowserState::default()
                },
            );
        }
        *self.native_browsers.lock().unwrap() = states;
    }
    pub async fn enable_browsers(&self) -> Result<()> {
        let _lock = self.browser_setup.lock().await;
        let port = self.engine.settings().proxy_port;
        tokio::task::spawn_blocking(move || browser::enable(port)).await??;
        self.refresh_browsers();
        Ok(())
    }
    pub async fn restart_browser(&self, name: &str) -> Result<()> {
        let _lock = self.browser_setup.lock().await;
        let port = self.engine.settings().proxy_port;
        if !browser::configured_status(name, port).configured {
            bail!("请先启用网页保护");
        }
        let url = match name {
            "edge" => "edge://restart",
            "chrome" => "chrome://restart",
            _ => bail!("未知浏览器"),
        };
        let before_restart = Utc::now().timestamp_millis().max(0) as u64;
        launch::shell_open(&browser::resolve(name)?, &[url.into()], None)?;
        for _ in 0..45 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !browser::needs_restart(name, before_restart).unwrap_or(true) {
                self.refresh_browsers();
                return self.open_web_in(name);
            }
        }
        bail!("浏览器尚未完成重开；请保存内容后关闭再打开，登录状态会保留")
    }
    pub async fn launch_desktop(&self) -> Result<()> {
        self.ensure_allowed()?;
        let port = self.engine.settings().proxy_port;
        let engine = self.engine.clone();
        tokio::task::spawn_blocking(move || {
            desktop::launch(port, || engine.gate.permit().is_some())
        })
        .await??;
        Ok(())
    }
    pub async fn dispatch(&self, r: Request) -> Result<Response> {
        match r.action.as_str() {
            "status" => {
                let state = self.state();
                let command = r.client_id.as_ref().and_then(|id| {
                    self.sessions
                        .lock()
                        .unwrap()
                        .take_command(id, state.fresh_allowed())
                });
                return Ok(Response {
                    state: Some(state),
                    browser_command: command,
                    ..Response::default()
                });
            }
            "browserStatus" => self.sessions.lock().unwrap().report(&r)?,
            "recheck" => self.engine.invalidate(),
            "switchPreferred" => self.engine.switch_preferred(r.node).await?,
            "authorizeLaunch" => {
                self.ensure_allowed()?;
                return Ok(Response {
                    state: Some(self.state()),
                    claude_path: Some(
                        launch::resolve_claude(self.engine.settings().claude_path.as_deref())?
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    ..Response::default()
                });
            }
            "launch" => {
                self.ensure_allowed()?;
                launch::launch_terminal(r.cwd.as_deref(), r.args.as_deref().unwrap_or(&[]))?;
            }
            "openWeb" => self.open_web()?,
            "enableBrowsers" => self.enable_browsers().await?,
            "launchDesktop" => self.launch_desktop().await?,
            "show" => {
                let _ = self.ui.send(UiEvent::Show);
            }
            "exit" => {
                let ui = self.ui.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let _ = ui.send(UiEvent::Exit);
                });
            }
            _ => bail!("未知操作"),
        }
        Ok(Response {
            state: Some(self.state()),
            ..Response::default()
        })
    }
    pub fn start(self: &Arc<Self>) -> Result<()> {
        let first = ipc::create_server(&pipe_path(), true)?;
        let monitor = self.clone();
        std::thread::spawn(move || {
            let mut watcher = desktop::StartWatch::new();
            let mut checked = Instant::now() - Duration::from_secs(3);
            let mut upstream = 0;
            while !monitor.engine.stop.is_cancelled() {
                let port = monitor.engine.settings().proxy_port;
                if watcher
                    .check(monitor.engine.gate.permit().is_some())
                    .unwrap_or(false)
                {
                    guard_protocol::log("已阻止 Claude Desktop 的新启动");
                    let _ = monitor.ui.send(UiEvent::Feedback(
                        "连接未就绪，已阻止 Claude 桌面启动".into(),
                    ));
                }
                if checked.elapsed() >= Duration::from_secs(3) {
                    let latest = monitor.engine.state().upstream_port;
                    if latest != 0 {
                        upstream = latest;
                    }
                    *monitor.desktop.lock().unwrap() = desktop::status(port, upstream);
                    checked = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let browsers = self.clone();
        std::thread::spawn(move || {
            while !browsers.engine.stop.is_cancelled() {
                let _ = browser::sync(
                    browsers.engine.settings().proxy_port,
                    &integration::system_proxy(),
                );
                browsers.refresh_browsers();
                std::thread::sleep(Duration::from_secs(3));
            }
        });
        let host = self.clone();
        tokio::spawn(async move {
            let mut next = first;
            loop {
                tokio::select! {_=host.engine.stop.cancelled()=>break,v=next.connect()=>if v.is_err(){break;}}
                let connected = next;
                next = match ipc::create_server(&pipe_path(), false) {
                    Ok(v) => v,
                    Err(_) => {
                        host.engine.shutdown();
                        break;
                    }
                };
                let host = host.clone();
                tokio::spawn(async move {
                    let _=async {
                        let mut connected=connected;
                        let Some(request)=tokio::time::timeout(Duration::from_secs(20),ipc::read::<Request>(&mut connected)).await?? else {return Ok::<_,anyhow::Error>(());};
                        if request.action=="subscribe" {
                            loop {tokio::select!{_=host.engine.stop.cancelled()=>break,result=async {
                                tokio::time::timeout(Duration::from_secs(3),ipc::write(&mut connected,&Response{state:Some(host.state()),..Response::default()})).await??;
                                tokio::time::sleep(Duration::from_secs(1)).await;Ok::<_,anyhow::Error>(())
                            }=>{result?;}}}
                        } else {
                            let timeout=if matches!(request.action.as_str(), "launchDesktop" | "enableBrowsers") {180} else {15};
                            let response=tokio::time::timeout(Duration::from_secs(timeout),host.dispatch(request)).await?.unwrap_or_else(|e|Response{ok:false,error:Some(e.to_string()),state:Some(host.state()),..Response::default()});
                            tokio::time::timeout(Duration::from_secs(3),ipc::write(&mut connected,&response)).await??;
                        } Ok(())
                    }.await;
                });
            }
        });
        Ok(())
    }
}
