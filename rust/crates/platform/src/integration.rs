use anyhow::{bail, Context, Result};
use guard_protocol::{
    cli_exe, desktop_exe, install_dir, wide, Settings, SystemProxy, EXTENSION_ID, HOST_NAME,
    VERSION,
};
use std::{fs, path::PathBuf};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED, STGM_READ,
        },
        UI::{
            Shell::{
                FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, SHGetKnownFolderPath, ShellLink,
                KF_FLAG_DEFAULT, SLGP_RAWPATH,
            },
            WindowsAndMessaging::{SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE},
        },
    },
};
use winreg::{enums::*, RegKey};
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ClaudeGuard";
pub fn auto_start() -> bool {
    if let Ok(Some(enabled)) = crate::startup::enabled() {
        return enabled;
    }
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(RUN)
        .and_then(|k| k.get_value::<String, _>("ClaudeGuard"))
        .is_ok_and(|s| {
            s.replace('/', "\\")
                .eq_ignore_ascii_case(&format!("\"{}\" --autostart", desktop_exe().display()))
        })
}
pub fn set_auto_start(enabled: bool) -> Result<()> {
    crate::startup::set_enabled(enabled)?;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN)?;
    // Remove the legacy Run entry after the scheduled task is committed.
    if let Err(e) = key.delete_value("ClaudeGuard") {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    }
    Ok(())
}
// Used only when an installer rollback restores the archived 1.x application,
// whose menu knows the Run key but cannot control a scheduled task.
pub fn restore_legacy_startup(enabled: bool) -> Result<()> {
    crate::startup::set_enabled(false)?;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN)?;
    if enabled {
        key.set_value(
            "ClaudeGuard",
            &format!("\"{}\" --autostart", desktop_exe().display()),
        )?;
    } else if let Err(e) = key.delete_value("ClaudeGuard") {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    }
    Ok(())
}
pub fn system_proxy() -> SystemProxy {
    let current = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) =
        current.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
    else {
        return SystemProxy {
            reason: "无法读取系统代理".into(),
            ..Default::default()
        };
    };
    let flags = key
        .open_subkey("Connections")
        .and_then(|k| k.get_raw_value("DefaultConnectionSettings"))
        .ok()
        .and_then(|v| v.bytes.get(8).copied())
        .unwrap_or(0);
    if !key
        .get_value::<String, _>("AutoConfigURL")
        .unwrap_or_default()
        .trim()
        .is_empty()
        || flags & 12 != 0
    {
        return SystemProxy {
            reason: "系统使用自动代理，请改用 Clash 静态系统代理".into(),
            ..Default::default()
        };
    }
    let enabled = key.get_value::<u32, _>("ProxyEnable").unwrap_or(0) == 1;
    let server = key
        .get_value::<String, _>("ProxyServer")
        .unwrap_or_default();
    let supported = !enabled || !server.trim().is_empty();
    SystemProxy {
        supported,
        enabled,
        server,
        bypass: key.get_value("ProxyOverride").unwrap_or_default(),
        reason: if supported {
            ""
        } else {
            "系统代理信息不完整"
        }
        .into(),
    }
}
fn shortcuts_named(name: &str) -> Result<Vec<PathBuf>> {
    unsafe {
        [FOLDERID_Desktop, FOLDERID_Programs]
            .iter()
            .map(|id| {
                let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)?;
                let value = path.to_string();
                CoTaskMemFree(Some(path.0.cast()));
                Ok(PathBuf::from(value?).join(name))
            })
            .collect()
    }
}
fn shortcuts() -> Result<Vec<PathBuf>> {
    shortcuts_named("Melian.lnk")
}
fn remove_legacy_shortcuts() -> Result<()> {
    unsafe {
        for path in shortcuts_named("Claude 网络守卫.lnk")? {
            if !path.is_file() {
                continue;
            }
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let persist: IPersistFile = link.cast()?;
            if persist
                .Load(PCWSTR(wide(&path).as_ptr()), STGM_READ)
                .is_err()
            {
                continue;
            }
            let mut target = vec![0u16; 32768];
            if link
                .GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
                .is_err()
            {
                continue;
            }
            let len = target.iter().position(|c| *c == 0).unwrap_or(target.len());
            if String::from_utf16_lossy(&target[..len])
                .replace('/', "\\")
                .eq_ignore_ascii_case(&desktop_exe().to_string_lossy().replace('/', "\\"))
            {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}
fn create_shortcuts() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        for path in shortcuts()? {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(PCWSTR(wide(desktop_exe()).as_ptr()))?;
            link.SetIconLocation(PCWSTR(wide(desktop_exe()).as_ptr()), 0)?;
            link.SetWorkingDirectory(PCWSTR(wide(install_dir()).as_ptr()))?;
            link.SetDescription(PCWSTR(wide("Melian").as_ptr()))?;
            let persist: IPersistFile = link.cast()?;
            persist.Save(PCWSTR(wide(path).as_ptr()), true)?;
        }
        remove_legacy_shortcuts()
    }
}
fn set_path(add: bool) -> Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey("Environment")?;
    let path = key.get_value::<String, _>("Path").unwrap_or_default();
    let cli = install_dir().join("cli").to_string_lossy().into_owned();
    let mut entries: Vec<_> = path
        .split(';')
        .filter(|s| !s.is_empty() && !s.trim_end_matches('\\').eq_ignore_ascii_case(&cli))
        .map(str::to_owned)
        .collect();
    if add {
        entries.push(cli);
    }
    let kind = key
        .get_raw_value("Path")
        .map(|v| v.vtype)
        .unwrap_or(REG_EXPAND_SZ);
    let bytes = wide(entries.join(";"))
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    key.set_raw_value(
        "Path",
        &winreg::RegValue {
            vtype: if kind == REG_SZ {
                REG_SZ
            } else {
                REG_EXPAND_SZ
            },
            bytes,
        },
    )?;
    unsafe {
        let text = wide("Environment");
        let _ = SendMessageTimeoutW(
            HWND(0xffffusize as *mut _),
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(text.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    }
    Ok(())
}
pub fn register() -> Result<()> {
    let current = RegKey::predef(HKEY_CURRENT_USER);
    fs::create_dir_all(install_dir())?;
    let manifest = install_dir().join("native-host.json");
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name":HOST_NAME,"description":"Melian本机桥接","path":cli_exe(),
            "type":"stdio","allowed_origins":[format!("chrome-extension://{EXTENSION_ID}/")]
        }))?,
    )?;
    for browser in [r"Software\Google\Chrome", r"Software\Microsoft\Edge"] {
        current
            .create_subkey(format!(r"{browser}\NativeMessagingHosts\{HOST_NAME}"))?
            .0
            .set_value("", &manifest.to_string_lossy().as_ref())?;
    }
    let fresh = current.open_subkey(UNINSTALL).is_err();
    let mut settings = Settings::load();
    if fresh {
        settings.auto_start = true;
        settings.sound_enabled = false;
    }
    set_auto_start(settings.auto_start).context("无法注册登录启动任务")?;
    settings.save().context("无法保存安装偏好")?;
    set_path(true).context("无法注册命令入口")?;
    create_shortcuts().context("无法创建快捷方式")?;
    let (key, _) = current.create_subkey(UNINSTALL)?;
    for (name, value) in [
        ("DisplayName", "Melian".into()),
        ("DisplayVersion", VERSION.into()),
        ("Publisher", "Melian".into()),
        (
            "InstallLocation",
            install_dir().to_string_lossy().into_owned(),
        ),
        ("DisplayIcon", desktop_exe().to_string_lossy().into_owned()),
        (
            "UninstallString",
            format!(
                "\"{}\" --uninstall",
                install_dir().join("ClaudeGuard.Setup.exe").display()
            ),
        ),
    ] {
        key.set_value(name, &value)?;
    }
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    Ok(())
}
pub fn unregister() -> Result<()> {
    crate::browser::unregister()?;
    crate::desktop::unregister()?;
    set_auto_start(false)?;
    set_path(false)?;
    let current = RegKey::predef(HKEY_CURRENT_USER);
    for browser in [r"Software\Google\Chrome", r"Software\Microsoft\Edge"] {
        let _ = current.delete_subkey_all(format!(r"{browser}\NativeMessagingHosts\{HOST_NAME}"));
    }
    let _ = current.delete_subkey_all(UNINSTALL);
    for path in shortcuts()? {
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    let mut settings = Settings::load();
    settings.auto_start = false;
    settings.save()?;
    Ok(())
}
pub fn extension_guide(browser: &str) -> Result<()> {
    let (suffix, url) = match browser {
        "edge" => (
            r"Microsoft\Edge\Application\msedge.exe",
            "edge://extensions/",
        ),
        "chrome" => (
            r"Google\Chrome\Application\chrome.exe",
            "chrome://extensions/",
        ),
        _ => bail!("未知浏览器"),
    };
    let executable = ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|p| PathBuf::from(p).join(suffix))
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow::anyhow!("未安装此浏览器"))?;
    crate::launch::shell_open(&executable, &[url.into()], None)
}
pub fn open_extension_folder() -> Result<()> {
    crate::launch::shell_open(&install_dir().join("extension"), &[], None)
}
pub fn open_clash() -> Result<()> {
    let mut candidates = vec![];
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for branch in [
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ] {
            if let Ok(key) = RegKey::predef(hive).open_subkey(branch) {
                for name in key.enum_keys().flatten() {
                    if let Ok(app) = key.open_subkey(name) {
                        if app
                            .get_value::<String, _>("DisplayName")
                            .is_ok_and(|s| s.to_ascii_lowercase().contains("clash verge"))
                        {
                            if let Ok(location) = app.get_value::<String, _>("InstallLocation") {
                                candidates.push(PathBuf::from(location).join("clash-verge.exe"));
                            }
                            if let Ok(icon) = app.get_value::<String, _>("DisplayIcon") {
                                candidates.push(PathBuf::from(
                                    icon.trim_matches('"')
                                        .trim_end_matches(",0")
                                        .trim_matches('"'),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    for root in ["ProgramFiles", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(root) {
            candidates.push(PathBuf::from(&root).join("Clash Verge/clash-verge.exe"));
        }
    }
    let path = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow::anyhow!("未找到 Clash Verge，请手动打开"))?;
    crate::launch::shell_open(&path, &[], None)
}
