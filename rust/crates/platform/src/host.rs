use crate::{ipc, launch};
use anyhow::{bail, Result};
use chrono::Utc;
use guard_core::Engine;
use guard_protocol::{pipe_path, BrowserState, Request, Response, State};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Copy)]
pub enum UiEvent {
    Show,
    Exit,
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
                    reason: "尚未连接扩展".into(),
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
    pub ui: UnboundedSender<UiEvent>,
}
impl Host {
    pub fn new(engine: Arc<Engine>, ui: UnboundedSender<UiEvent>) -> Arc<Self> {
        Arc::new(Self {
            engine,
            sessions: Mutex::new(Sessions::default()),
            ui,
        })
    }
    pub fn state(&self) -> State {
        let mut state = self.engine.state();
        self.sessions.lock().unwrap().merge(&mut state);
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
        self.sessions.lock().unwrap().queue_web(true)
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
                            let response=tokio::time::timeout(Duration::from_secs(15),host.dispatch(request)).await?.unwrap_or_else(|e|Response{ok:false,error:Some(e.to_string()),state:Some(host.state()),..Response::default()});
                            tokio::time::timeout(Duration::from_secs(3),ipc::write(&mut connected,&response)).await??;
                        } Ok(())
                    }.await;
                });
            }
        });
        Ok(())
    }
}
