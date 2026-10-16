use anyhow::{bail, Context, Result};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
pub fn resolve_claude(configured: Option<&str>) -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut candidates = Vec::new();
    if let Some(path) = configured.filter(|p| !p.trim().is_empty()) {
        candidates.push(PathBuf::from(path));
    } else {
        candidates.push(home.join(".local/bin/claude"));
        candidates.extend(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|p| p.join("claude")),
        );
        candidates.push("/opt/homebrew/bin/claude".into());
        candidates.push("/usr/local/bin/claude".into());
    }
    candidates
        .into_iter()
        .find(|p| {
            p.is_file()
                && p.metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
        })
        .context("未找到 Claude Code，请先安装官方 Claude Code")
}
pub fn claude_command(path: &Path, cwd: &Path, args: &[String], port: u16) -> Command {
    let mut command = Command::new(path);
    command.current_dir(cwd).args(args);
    for (key, _) in std::env::vars_os() {
        if ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
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
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub fn launch_terminal(cwd: Option<&str>, args: &[String]) -> Result<()> {
    validate_front_args(args)?;
    let cwd = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()));
    if !cwd.is_dir() {
        bail!("工作目录不存在");
    }
    let mut command = format!(
        "{} --cwd {} --",
        quote_arg(&guard_protocol::cli_exe().to_string_lossy()),
        quote_arg(&cwd.to_string_lossy())
    );
    for arg in args {
        command.push(' ');
        command.push_str(&quote_arg(arg));
    }
    // Pass the shell command as argv, never interpolate it into AppleScript source.
    let script="on run argv\ntell application \"Terminal\"\nactivate\ndo script (item 1 of argv)\nend tell\nend run";
    let status = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .arg(command)
        .status()?;
    if !status.success() {
        bail!("无法打开终端，请在 Terminal 中运行 claude-guard");
    }
    Ok(())
}
pub fn open_clash() -> Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    for root in [PathBuf::from("/Applications"), home.join("Applications")] {
        for name in [
            "Clash Verge.app",
            "Clash Party.app",
            "Mihomo Party.app",
            "ClashX Meta.app",
            "ClashX.app",
        ] {
            let path = root.join(name);
            if path.is_dir() {
                if Command::new("/usr/bin/open").arg(path).status()?.success() {
                    return Ok(());
                }
            }
        }
    }
    bail!("请打开已安装的 Clash / Mihomo 客户端，并启用本机控制接口")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoting_keeps_arguments_literal() {
        assert_eq!(quote_arg("a'b;$(x)"), "'a'\\''b;$(x)'");
    }
    #[test]
    fn bypass_variables_are_removed() {
        let cmd = claude_command(
            Path::new("/bin/echo"),
            Path::new("/tmp"),
            &["a b".into()],
            17890,
        );
        assert_eq!(cmd.get_args().next().unwrap(), "a b");
        let env: Vec<_> = cmd.get_envs().collect();
        assert!(env.iter().any(|(k, v)| *k == "HTTPS_PROXY"
            && *v == Some(std::ffi::OsStr::new("http://127.0.0.1:17890"))));
    }
}
