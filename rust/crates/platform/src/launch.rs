use anyhow::{bail, Result};
use guard_protocol::{cli_exe, wide};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::HWND,
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    },
};

pub fn resolve_claude(configured: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = configured.filter(|s| !s.trim().is_empty()) {
        let path = PathBuf::from(path);
        if path.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            return Ok(path);
        }
        bail!("找不到指定的 Claude 程序");
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let file = dir.join("claude.exe");
        if file.is_file() {
            return Ok(file);
        }
    }
    let fallback = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default())
        .join(".local/bin/claude.exe");
    if fallback.is_file() {
        return Ok(fallback);
    }
    bail!("未找到 Claude Code，请先安装 Windows 版 Claude Code")
}
pub fn claude_command(path: &Path, cwd: &Path, args: &[String], port: u16) -> Command {
    let mut command = Command::new(path);
    command.current_dir(cwd).args(args);
    for (key, _) in std::env::vars_os() {
        if ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]
            .iter()
            .any(|s| key.eq_ignore_ascii_case(s))
        {
            command.env_remove(key);
        }
    }
    let proxy = format!("http://127.0.0.1:{port}");
    command
        .env("HTTP_PROXY", &proxy)
        .env("HTTPS_PROXY", &proxy)
        .env("NO_PROXY", "localhost,127.0.0.1,::1");
    command
}
pub fn validate_front_args(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--bg" || a.starts_with("--bg="))
        || args
            .first()
            .is_some_and(|a| matches!(a.as_str(), "agents" | "daemon"))
    {
        bail!("守卫入口仅支持前台 Claude Code 会话");
    }
    Ok(())
}
pub fn quote_arg(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}
pub fn shell_open(file: &Path, params: &[String], cwd: Option<&Path>) -> Result<()> {
    unsafe {
        let file = wide(file);
        let params = wide(
            params
                .iter()
                .map(|s| quote_arg(s))
                .collect::<Vec<_>>()
                .join(" "),
        );
        let cwd = cwd.map(wide);
        let result = ShellExecuteW(
            HWND::default(),
            PCWSTR::null(),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            cwd.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
            SW_SHOWNORMAL,
        );
        if result.0 as isize <= 32 {
            bail!("无法打开程序");
        }
        Ok(())
    }
}
pub fn launch_terminal(cwd: Option<&str>, args: &[String]) -> Result<()> {
    validate_front_args(args)?;
    let cwd = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()));
    if !cwd.is_dir() {
        bail!("工作目录不存在");
    }
    let mut parameters = vec![
        "--cwd".into(),
        cwd.to_string_lossy().into_owned(),
        "--".into(),
    ];
    parameters.extend_from_slice(args);
    shell_open(&cli_exe(), &parameters, Some(&cwd))
}
