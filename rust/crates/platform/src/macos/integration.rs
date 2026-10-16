use anyhow::{bail, Result};
use guard_protocol::{settings_dir, Settings, SystemProxy};
use std::{fs, path::PathBuf, process::Command};

const LABEL: &str = "io.github.jjchess.melian";
fn agent_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}
fn agent(executable: &std::path::Path) -> plist::Value {
    let mut data = plist::Dictionary::new();
    data.insert("Label".into(), LABEL.into());
    data.insert(
        "ProgramArguments".into(),
        plist::Value::Array(vec![
            executable.to_string_lossy().into_owned().into(),
            "--autostart".into(),
        ]),
    );
    data.insert("RunAtLoad".into(), true.into());
    data.insert("ProcessType".into(), "Interactive".into());
    data.insert("LimitLoadToSessionType".into(), "Aqua".into());
    plist::Value::Dictionary(data)
}
pub fn auto_start() -> bool {
    agent_path().is_file()
}
pub fn set_auto_start(enabled: bool) -> Result<()> {
    let path = agent_path();
    if path.exists() {
        let value = plist::Value::from_file(&path)?;
        if value
            .as_dictionary()
            .and_then(|v| v.get("Label"))
            .and_then(|v| v.as_string())
            != Some(LABEL)
        {
            bail!("自启文件不属于 Melian");
        }
    }
    if enabled {
        let executable = guard_protocol::desktop_exe();
        if !executable.is_file() {
            bail!("请先将 Melian 安装到用户 Applications 目录");
        }
        fs::create_dir_all(path.parent().unwrap())?;
        let temp = path.with_extension("plist.new");
        agent(&executable).to_file_xml(&temp)?;
        fs::rename(temp, &path)?;
        // RunAtLoad is evaluated at the next login. Do not unload the current
        // launchd job: bootout would terminate a running guard and its tunnels.
        let _ = Command::new("/bin/launchctl")
            .args([
                "enable",
                &format!("gui/{}/{LABEL}", unsafe { libc::geteuid() }),
            ])
            .output();
    } else if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}
pub fn install_hooks() -> Result<()> {
    fs::create_dir_all(settings_dir())?;
    let mut settings = Settings::load();
    if !guard_protocol::settings_file().exists() {
        settings.sound_enabled = false;
    }
    set_auto_start(settings.auto_start)?;
    settings.save()
}
pub fn uninstall_hooks() -> Result<()> {
    set_auto_start(false)?;
    // Configuration is deliberately retained for a future reinstall.
    Ok(())
}
pub fn system_proxy() -> SystemProxy {
    SystemProxy {
        supported: false,
        reason: "macOS 网页接入尚未验证；不会修改浏览器或系统代理".into(),
        ..SystemProxy::default()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launch_agent_preserves_paths_and_login_scope() {
        let p =
            std::path::Path::new("/Users/a b/Applications/Melian.app/Contents/MacOS/ClaudeGuard");
        let value = agent(p);
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes).unwrap();
        let result = plist::Value::from_reader_xml(bytes.as_slice()).unwrap();
        let d = result.as_dictionary().unwrap();
        assert_eq!(
            d["ProgramArguments"].as_array().unwrap()[0].as_string(),
            Some(p.to_str().unwrap())
        );
        assert_eq!(d["RunAtLoad"].as_boolean(), Some(true));
        assert!(!d.contains_key("KeepAlive"));
    }
}
