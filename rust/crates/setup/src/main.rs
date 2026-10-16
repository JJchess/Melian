#![windows_subsystem = "windows"]
mod process;
use anyhow::{bail, Context, Result};
use guard_platform::{integration, ipc};
use guard_protocol::{desktop_exe, install_dir, log, settings_dir, settings_file, wide, Request};
use std::{
    fs,
    io::{Cursor, Read},
    os::windows::{fs::MetadataExt, process::CommandExt},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use windows::{
    core::PCWSTR,
    Win32::{Foundation::HWND, UI::WindowsAndMessaging::*},
};
const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let quiet = args.iter().any(|a| a == "--quiet");
    if let Err(error) = run(&args, quiet) {
        log(&format!("安装失败：{error:#}"));
        if !quiet {
            message(&format!("{error:#}"), "安装未完成", MB_OK | MB_ICONERROR);
        }
        std::process::exit(1);
    }
}
fn message(text: &str, title: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(
            HWND::default(),
            PCWSTR(wide(text).as_ptr()),
            PCWSTR(wide(title).as_ptr()),
            style,
        )
    }
}
fn nonce() -> String {
    format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        std::process::id()
    )
}
fn validate_dir(path: &Path) -> Result<()> {
    if path != install_dir() {
        bail!("安装目录验证失败");
    }
    let mut current = Some(path);
    while let Some(p) = current {
        if let Ok(meta) = fs::symlink_metadata(p) {
            if meta.file_attributes() & 0x400 != 0 {
                bail!("安装目录不能经过重解析点");
            }
        }
        current = p.parent();
    }
    Ok(())
}
fn run(args: &[String], quiet: bool) -> Result<()> {
    let current = std::env::current_exe()?;
    if args.iter().any(|a| a == "--check-payload") {
        validate_payload()?;
        return Ok(());
    }
    if let Some(index) = args.iter().position(|a| a == "--wait-parent") {
        if let Some(pid) = args.get(index + 1).and_then(|a| a.parse().ok()) {
            process::wait_parent(pid);
        }
    }
    let uninstall = args.iter().any(|a| a == "--uninstall");
    if current.starts_with(install_dir()) {
        let helper = std::env::temp_dir().join(format!("ClaudeGuard-setup-{}.exe", nonce()));
        fs::copy(&current, &helper)?;
        let mut command = Command::new(&helper);
        command
            .args(args)
            .args(["--wait-parent", &std::process::id().to_string()])
            .creation_flags(0x08000000)
            .spawn()?;
        return Ok(());
    }
    validate_dir(&install_dir())?;
    if uninstall {
        if !quiet && message("卸载 Claude 网络守卫？\n\n请先在 Edge 和 Chrome 中移除配套扩展，浏览器才会恢复原有代理设置。\n用户设置和日志将保留。","卸载 Claude 网络守卫",MB_OKCANCEL|MB_ICONINFORMATION)!=IDOK{return Ok(());}
        stop()?;
        integration::unregister()?;
        if install_dir().exists() {
            fs::remove_dir_all(install_dir())?;
        }
        if !quiet {
            let _ = integration::extension_guide("edge");
            let _ = integration::extension_guide("chrome");
            message(
                "守卫已卸载。\n请在浏览器扩展页移除「Claude 网络守卫」。",
                "卸载完成",
                MB_OK,
            );
        }
        return Ok(());
    }
    validate_payload()?;
    if !quiet && message("安装 Claude 网络守卫？\n\n界面是一个小胶囊和四项菜单。\n新安装默认开机自启；升级保留已有选择。","Claude 网络守卫",MB_OKCANCEL|MB_ICONINFORMATION)!=IDOK{return Ok(());}
    install(quiet)
}
fn archive() -> Result<zip::ZipArchive<Cursor<&'static [u8]>>> {
    if PAYLOAD.is_empty() {
        bail!("缺少安装资源，请使用 build.ps1 构建完整安装包");
    }
    Ok(zip::ZipArchive::new(Cursor::new(PAYLOAD))?)
}
fn validate_payload() -> Result<()> {
    let mut zip = archive()?;
    let mut desktop = false;
    let mut cli = false;
    let mut manifest = false;
    for index in 0..zip.len() {
        let entry = zip.by_index(index)?;
        let path = entry.enclosed_name().context("安装资源含有非法路径")?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0xf000 == 0xa000)
        {
            bail!("安装资源不能包含符号链接");
        }
        match path.to_string_lossy().replace('\\', "/").as_str() {
            "ClaudeGuard.exe" => desktop = true,
            "cli/claude-guard.exe" => cli = true,
            "extension/manifest.json" => manifest = true,
            _ => {}
        }
    }
    if !desktop || !cli || !manifest {
        bail!("安装资源不完整");
    }
    Ok(())
}
fn extract(stage: &Path) -> Result<()> {
    fs::create_dir(stage)?;
    let mut zip = archive()?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let relative = entry.enclosed_name().context("非法安装资源路径")?;
        let target = stage.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(target)?;
            continue;
        }
        fs::create_dir_all(target.parent().unwrap())?;
        let mut file = fs::File::create(target)?;
        std::io::copy(&mut entry, &mut file)?;
    }
    Ok(())
}
fn valid_pe(path: &Path) -> Result<()> {
    let mut file = fs::File::open(path)?;
    let mut bytes = [0; 4096];
    let size = file.read(&mut bytes)?;
    if size < 64 || &bytes[..2] != b"MZ" {
        bail!("无效的 Windows 程序");
    }
    let offset = u32::from_le_bytes(bytes[60..64].try_into()?) as usize;
    if offset + 6 > size
        || &bytes[offset..offset + 4] != b"PE\0\0"
        || &bytes[offset + 4..offset + 6] != [0x64, 0x86]
    {
        bail!("安装资源不是 Windows x64 程序");
    }
    Ok(())
}
fn stop() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _ = runtime.block_on(ipc::send(Request::action("exit")));
    std::thread::sleep(Duration::from_millis(500));
    process::stop_owned()
}
fn launch_installed() -> Result<std::process::Child> {
    Ok(Command::new(desktop_exe())
        .current_dir(install_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()?)
}
fn install(quiet: bool) -> Result<()> {
    let target = install_dir();
    let parent = target.parent().unwrap();
    fs::create_dir_all(parent)?;
    let stage = parent.join(format!("ClaudeGuard.stage-{}", nonce()));
    let backup = parent.join(format!("ClaudeGuard.rollback-{}", nonce()));
    extract(&stage)?;
    // Carry the original data directory into the stable installation. Packaged
    // launchers may virtualize APPDATA, while Windows logon tasks do not.
    let data_root = guard_protocol::physical_settings_dir()?;
    fs::write(
        stage.join("data-root.txt"),
        data_root.to_string_lossy().as_bytes(),
    )?;
    valid_pe(&stage.join("ClaudeGuard.exe"))?;
    valid_pe(&stage.join("cli/claude-guard.exe"))?;
    let check = Command::new(stage.join("cli/claude-guard.exe"))
        .arg("--guard-help")
        .creation_flags(0x08000000)
        .output()?;
    if !check.status.success() {
        bail!("新程序运行检查失败，现有安装未改动");
    }
    fs::copy(
        std::env::current_exe()?,
        stage.join("ClaudeGuard.Setup.exe"),
    )?;
    let config_backup = if settings_file().exists() {
        let dir = settings_dir().join("backups");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("settings-{}.json", nonce()));
        fs::copy(settings_file(), &path)?;
        Some(path)
    } else {
        None
    };
    let had_install = target.is_dir();
    let prior_version = installed_version();
    // Everything above is staged before interrupting the installed guard.
    stop()?;
    if had_install {
        if let Err(error) = fs::rename(&target, &backup) {
            let _ = launch_installed();
            return Err(error).context("无法备份当前安装；已重新启动原守卫");
        }
    }
    let change = (|| -> Result<()> {
        fs::rename(&stage, &target).context("无法将已检查的程序放入安装目录")?;
        if cfg!(feature = "fault-injection")
            && std::env::var_os("CLAUDE_GUARD_FAIL_UPGRADE").is_some()
        {
            bail!("测试：替换后模拟失败");
        }
        integration::register().context("无法写入 Windows 集成设置")?;
        let mut child = launch_installed().context("无法启动刚安装的守卫")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        std::thread::sleep(Duration::from_secs(2));
        for _ in 0..40 {
            if child.try_wait()?.is_some() {
                bail!("新守卫未能保持运行");
            }
            if runtime
                .block_on(ipc::send(Request::action("status")))
                .is_ok_and(|r| r.ok && r.state.is_some())
                && window_ready(child.id())
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        bail!("新守卫健康检查超时")
    })();
    if let Err(error) = change {
        let _ = stop();
        if target.exists() {
            validate_dir(&target)?;
            fs::remove_dir_all(&target)?;
        }
        if had_install {
            fs::rename(&backup, &target)?;
            if let Some(config) = &config_backup {
                fs::copy(config, settings_file())?;
            }
            let restored_integration = integration::register().and_then(|_| {
                if prior_version.starts_with("1.") {
                    integration::restore_legacy_startup(guard_protocol::Settings::load().auto_start)
                } else {
                    Ok(())
                }
            });
            restore_version(&prior_version);
            let restored_process = launch_installed();
            restored_integration?;
            restored_process?;
        } else {
            let _ = integration::unregister();
        }
        return Err(error.context("升级未完成，已恢复先前安装"));
    }
    log("Rust 2.0 安装完成；旧程序保存在相邻 rollback 目录");
    if !quiet {
        message(
            "安装完成。\n\n单击顶部胶囊打开菜单。\n首次打开网页时，按面板指引接入浏览器。",
            "Claude 网络守卫",
            MB_OK,
        );
    }
    Ok(())
}
fn window_ready(pid: u32) -> bool {
    unsafe {
        if let Ok(hwnd) = FindWindowW(PCWSTR(wide("ClaudeGuard.Rust.Island").as_ptr()), None) {
            let mut actual = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut actual));
            actual == pid
        } else {
            false
        }
    }
}
fn installed_version() -> String {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ClaudeGuard")
        .and_then(|key| key.get_value("DisplayVersion"))
        .unwrap_or_default()
}
fn restore_version(version: &str) {
    if let Ok((key, _)) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ClaudeGuard")
    {
        let _ = key.set_value("DisplayVersion", &version);
    }
}
