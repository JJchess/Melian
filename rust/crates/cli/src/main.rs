mod bridge;
use anyhow::{bail, Result};
use guard_platform::{ipc, launch};
use guard_protocol::{Request, EXTENSION_ID};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = run(args).await;
    let code = match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Claude 网络守卫：{e}");
            2
        }
    };
    std::process::exit(code);
}
async fn run(mut args: Vec<String>) -> Result<i32> {
    if args
        .first()
        .is_some_and(|a| a.starts_with("chrome-extension://"))
    {
        if args[0] != format!("chrome-extension://{EXTENSION_ID}/") {
            bail!("不受信任的扩展来源");
        }
        return bridge::run().await;
    }
    if args.first().is_some_and(|s| s == "--guard-help") {
        println!("claude-guard [--cwd 目录] [--] Claude Code 参数\n--desktop 打开 Claude 桌面\n--status  --recheck  --show  --open-web\n桌面接入需要新版官方 MSIX 安装；Code 命令仅支持 Windows 前台会话。");
        return Ok(0);
    }
    if let Some(action) = args.first().and_then(|s| match s.as_str() {
        "--status" => Some("status"),
        "--recheck" => Some("recheck"),
        "--show" => Some("show"),
        "--open-web" => Some("openWeb"),
        "--desktop" => Some("launchDesktop"),
        _ => None,
    }) {
        let response = ipc::send(Request::action(action)).await?;
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(if response.ok { 0 } else { 2 });
    }
    let mut cwd = std::env::current_dir()?;
    if args.first().is_some_and(|s| s == "--cwd") {
        if args.len() < 2 {
            bail!("--cwd 需要工作目录");
        }
        cwd = args[1].clone().into();
        args.drain(..2);
    }
    if args.first().is_some_and(|s| s == "--") {
        args.remove(0);
    }
    launch::validate_front_args(&args)?;
    if !cwd.is_dir() {
        bail!("工作目录不存在");
    }
    let response = ipc::send(Request::action("authorizeLaunch")).await?;
    if !response.ok {
        bail!(
            "{}",
            response.error.unwrap_or_else(|| "连接尚未就绪".into())
        );
    }
    let state = response
        .state
        .filter(|s| s.fresh_allowed())
        .ok_or_else(|| anyhow::anyhow!("连接检查已失效"))?;
    let path = response
        .claude_path
        .ok_or_else(|| anyhow::anyhow!("未找到 Claude Code"))?;
    let mut command =
        launch::claude_command(std::path::Path::new(&path), &cwd, &args, state.proxy_port);
    let mut child = command.spawn()?;
    unsafe {
        let _ = windows::Win32::System::Console::SetConsoleCtrlHandler(Some(console_signal), true);
    }
    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}
unsafe extern "system" fn console_signal(signal: u32) -> windows::Win32::Foundation::BOOL {
    // Claude receives the same console signal. Keep the launcher alive to return its exit code.
    (signal == 0 || signal == 1).into()
}
