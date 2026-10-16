use anyhow::Result;
use guard_platform::{integration, ipc};
use guard_protocol::{Request, Settings, State, MAX_WIRE_BYTES};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub async fn run() -> Result<i32> {
    let id = uuid::Uuid::new_v4().to_string();
    let browser = Arc::new(Mutex::new(None::<String>));
    let input_browser = browser.clone();
    let input_id = id.clone();
    let input = async move {
        let mut stdin = tokio::io::stdin();
        loop {
            let mut size = [0; 4];
            if stdin.read_exact(&mut size).await.is_err() {
                break;
            }
            let len = u32::from_le_bytes(size) as usize;
            if len == 0 || len > MAX_WIRE_BYTES {
                break;
            }
            let mut bytes = vec![0; len];
            if stdin.read_exact(&mut bytes).await.is_err() {
                break;
            }
            let Ok(mut request) = serde_json::from_slice::<Request>(&bytes) else {
                break;
            };
            if !matches!(
                request.action.as_str(),
                "browserStatus" | "recheck" | "switchPreferred"
            ) {
                continue;
            }
            if request.action == "browserStatus" {
                *input_browser.lock().unwrap() = request.browser.clone();
            }
            request.client_id = Some(input_id.clone());
            let _ = ipc::send(request).await;
        }
    };
    let output = async {
        let mut stdout = tokio::io::stdout();
        loop {
            let response = ipc::send(Request {
                client_id: Some(id.clone()),
                ..Request::action("status")
            })
            .await;
            let (state, command) = match response {
                Ok(r) => (r.state.unwrap_or_default(), r.browser_command),
                Err(_) => (
                    State {
                        status: "blocked".into(),
                        reason: "桌面守卫未连接".into(),
                        reason_code: "guardUnavailable".into(),
                        proxy_port: Settings::load().proxy_port,
                        ..State::default()
                    },
                    None,
                ),
            };
            let data = serde_json::to_vec(
                &serde_json::json!({"type":"status","state":state,"systemProxy":integration::system_proxy(),"command":command}),
            )?;
            if data.len() > MAX_WIRE_BYTES {
                anyhow::bail!("状态消息过长");
            }
            stdout.write_all(&(data.len() as u32).to_le_bytes()).await?;
            stdout.write_all(&data).await?;
            stdout.flush().await?;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {_=input=>{},_=output=>{}}
    let last = browser.lock().unwrap().clone();
    if let Some(browser) = last {
        let _ = ipc::send(Request {
            client_id: Some(id),
            browser: Some(browser),
            connected: false,
            protected: false,
            reason: Some("浏览器扩展已断开".into()),
            ..Request::action("browserStatus")
        })
        .await;
    }
    Ok(0)
}
