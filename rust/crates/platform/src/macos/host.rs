use super::{integration, ipc, launch};
use anyhow::{bail, Result};
use guard_core::Engine;
use guard_protocol::{Request, Response, State};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc::UnboundedSender;
#[derive(Clone)]
pub enum UiEvent {
    Show,
    Exit,
    Feedback(String),
}
pub struct Host {
    pub codex: Arc<crate::codex::Monitor>,
    pub engine: Arc<Engine>,
    pub ui: UnboundedSender<UiEvent>,
}
impl Host {
    pub fn new(engine: Arc<Engine>, ui: UnboundedSender<UiEvent>) -> Arc<Self> {
        Arc::new(Self {
            codex: crate::codex::Monitor::new(),
            engine,
            ui,
        })
    }
    pub fn state(&self) -> State {
        let mut state = self.engine.state();
        state.codex = Some(self.codex.state());
        state.desktop.reason =
            "macOS Desktop 接入尚待验证，当前仅保护专用入口启动的 Claude Code".into();
        state
    }
    pub fn ensure_allowed(&self) -> Result<()> {
        if !self.state().fresh_allowed() || self.engine.gate.permit().is_none() {
            bail!("连接尚未就绪，请查看 Melian 的提示");
        }
        Ok(())
    }
    pub fn start(self: &Arc<Self>, server: ipc::Server) {
        self.codex.start(self.engine.stop.clone());
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {_=host.engine.stop.cancelled()=>break,next=server.listener.accept()=>{
                    let Ok((mut stream,_))=next else{break};let host=host.clone();
                    tokio::spawn(async move {
                        let _ = host.connection(&mut stream).await;
                    });
                }}
            }
        });
    }
    async fn connection(&self, stream: &mut tokio::net::UnixStream) -> Result<()> {
        ipc::check_peer(stream)?;
        let Some(request) =
            tokio::time::timeout(Duration::from_secs(10), ipc::read::<Request>(stream)).await??
        else {
            return Ok(());
        };
        if request.action == "subscribe" {
            loop {
                let response = Response {
                    state: Some(self.state()),
                    ..Response::default()
                };
                tokio::time::timeout(Duration::from_secs(5), ipc::write(stream, &response))
                    .await??;
                tokio::select! {
                    _ = self.engine.stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        } else {
            let response =
                tokio::time::timeout(Duration::from_secs(20), self.handle(request)).await?;
            tokio::time::timeout(Duration::from_secs(5), ipc::write(stream, &response)).await??;
        }
        Ok(())
    }
    pub async fn handle(&self, request: Request) -> Response {
        let mut response = Response::default();
        let result: Result<()> = async {
            match request.action.as_str() {
                "codexRefresh" => self.codex.refresh(),
                "codexRead" => self.codex.mark_read(),
                "status" => {}
                "recheck" => self.engine.invalidate(),
                "switchPreferred" => self.engine.switch_preferred(request.node).await?,
                "show" => {
                    let _ = self.ui.send(UiEvent::Show);
                }
                "exit" => {
                    let _ = self.ui.send(UiEvent::Exit);
                }
                "authorizeLaunch" => {
                    self.ensure_allowed()?;
                    response.claude_path = Some(
                        launch::resolve_claude(self.engine.settings().claude_path.as_deref())?
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                "launch" => {
                    self.ensure_allowed()?;
                    launch::launch_terminal(
                        request.cwd.as_deref(),
                        request.args.as_deref().unwrap_or(&[]),
                    )?;
                }
                "toggleAutoStart" => {
                    let enabled = !integration::auto_start();
                    integration::set_auto_start(enabled)?;
                    if let Err(error) = self.engine.update_settings(|s| s.auto_start = enabled) {
                        let _ = integration::set_auto_start(!enabled);
                        return Err(error);
                    }
                }
                "startClash" | "openClash" => launch::open_clash()?,
                "launchDesktop" => bail!("macOS Desktop 保护尚未验证；请使用 Claude Code 专用入口"),
                "openWeb" | "enableBrowsers" | "browserStatus" => {
                    bail!("网页保护暂未提供，未修改系统代理或浏览器设置")
                }
                _ => bail!("不支持的操作"),
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            response.ok = false;
            response.error = Some(error.to_string());
        }
        response.state = Some(self.state());
        response
    }
}
