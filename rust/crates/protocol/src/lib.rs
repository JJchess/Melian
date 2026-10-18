use anyhow::{Context, Result};
pub mod codex;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf, sync::Mutex};

pub const EXTENSION_ID: &str = "lgfobljoiaojjljlgpfhdaahppgdafkd";
pub const HOST_NAME: &str = "com.claudeguard.bridge";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_WIRE_BYTES: usize = 65_536;
static FILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub mihomo_pipe_name: String,
    pub proxy_port: u16,
    pub preferred_node: Option<String>,
    pub claude_path: Option<String>,
    pub sound_enabled: bool,
    pub auto_start: bool,
    pub island_left: Option<f64>,
    pub island_top: Option<f64>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            mihomo_pipe_name: "verge-mihomo".into(),
            proxy_port: 17890,
            preferred_node: None,
            claude_path: None,
            sound_enabled: true,
            auto_start: true,
            island_left: None,
            island_top: None,
        }
    }
}
impl Settings {
    pub fn load() -> Self {
        let _lock = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut value: Self = fs::read(settings_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if value.proxy_port < 1024 {
            value.proxy_port = 17890;
        }
        if value.mihomo_pipe_name.is_empty()
            || (cfg!(windows) && value.mihomo_pipe_name.contains('/'))
        {
            value.mihomo_pipe_name = "verge-mihomo".into();
        }
        value
    }
    pub fn save(&self) -> Result<()> {
        let _lock = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        fs::create_dir_all(settings_dir())?;
        let temp = settings_file().with_extension(format!("{}.tmp", std::process::id()));
        let mut file = fs::File::create(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        drop(file);
        atomic_replace(&temp, &settings_file()).context("无法保存设置")
    }
}
pub fn atomic_replace(source: &std::path::Path, destination: &std::path::Path) -> Result<()> {
    // Resolve redirected profile directories before calling rename APIs. A logical C:
    // profile can physically live on D:, including when the destination is new.
    let same_parent = source.parent() == destination.parent();
    let source = fs::canonicalize(source)?;
    // A packaged parent can overlay an existing real profile directory. The
    // newly written file reveals the writable layer; directory handles can
    // still resolve to the lower layer on another volume.
    let parent = if same_parent {
        source.parent().context("文件缺少父目录")?.to_path_buf()
    } else {
        fs::canonicalize(destination.parent().context("文件缺少父目录")?)?
    };
    let destination = parent.join(destination.file_name().context("文件缺少名称")?);
    fs::rename(source, destination)?;
    Ok(())
}
pub fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        value.as_ref().encode_wide().chain(Some(0)).collect()
    }
    #[cfg(not(windows))]
    value
        .as_ref()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect()
}
#[cfg(windows)]
pub fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
        .join("Programs")
        .join("ClaudeGuard")
}
#[cfg(not(windows))]
pub fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Applications/Melian.app/Contents")
}
#[cfg(windows)]
pub fn settings_dir() -> PathBuf {
    if let Ok(root) = fs::read_to_string(install_dir().join("data-root.txt")) {
        let root = PathBuf::from(root.trim());
        if root.is_absolute() {
            return root;
        }
    }
    let logical =
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("ClaudeGuard");
    fs::canonicalize(logical.join("settings.json"))
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or(logical)
}
#[cfg(not(windows))]
pub fn settings_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/Melian")
}
/// Pins the existing writable profile layer across installer, scheduled login,
/// and browser launches. No package identity or machine-specific path is assumed.
pub fn physical_settings_dir() -> Result<PathBuf> {
    let logical = settings_dir();
    fs::create_dir_all(&logical)?;
    let probe = logical.join(format!(
        ".guard-location-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    let resolved = fs::canonicalize(&probe);
    let _ = fs::remove_file(&probe);
    Ok(resolved?
        .parent()
        .context("配置目录无法解析")?
        .to_path_buf())
}
pub fn settings_file() -> PathBuf {
    settings_dir().join("settings.json")
}
pub fn desktop_exe() -> PathBuf {
    #[cfg(windows)]
    {
        install_dir().join("ClaudeGuard.exe")
    }
    #[cfg(not(windows))]
    install_dir().join("MacOS/ClaudeGuard")
}
pub fn cli_exe() -> PathBuf {
    #[cfg(windows)]
    {
        install_dir().join("cli").join("claude-guard.exe")
    }
    #[cfg(not(windows))]
    install_dir().join("MacOS/claude-guard")
}
pub fn pipe_name() -> String {
    let identity = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    format!(
        "ClaudeGuard-{}",
        &hex::encode_upper(Sha256::digest(identity.as_bytes()))[..20]
    )
}
pub fn pipe_path() -> String {
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\{}", pipe_name())
    }
    #[cfg(unix)]
    {
        std::env::temp_dir()
            .join("melian")
            .join("control.sock")
            .to_string_lossy()
            .into_owned()
    }
}
pub fn log(message: &str) {
    let _lock = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = fs::create_dir_all(settings_dir());
    let path = settings_dir().join("guard.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > 1_048_576) {
        let _ = atomic_replace(&path, &path.with_extension("log.1"));
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            file,
            "{} {}",
            chrono::Local::now().to_rfc3339(),
            message.replace(['\r', '\n'], " ")
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserState {
    pub browser: String,
    pub installed: bool,
    pub configured: bool,
    pub restart_required: bool,
    pub connection_kind: String,
    pub proxy_fingerprint: String,
    pub policy_revision: String,
    pub connected: bool,
    pub protected: bool,
    pub reason: String,
    pub last_seen: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct State {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<codex::CodexState>,
    pub status: String,
    pub reason: String,
    pub mode: String,
    pub node: String,
    pub node_path: Vec<String>,
    pub country: String,
    pub verified_at: Option<DateTime<Utc>>,
    pub last_check_at: Option<DateTime<Utc>>,
    pub upstream_port: u16,
    pub proxy_port: u16,
    pub browsers: BTreeMap<String, BrowserState>,
    pub desktop: DesktopState,
    pub allowed: bool,
    pub reason_code: String,
    pub suggested_action: String,
}
impl Default for State {
    fn default() -> Self {
        Self {
            codex: None,
            status: "checking".into(),
            reason: "正在检查连接".into(),
            mode: String::new(),
            node: String::new(),
            node_path: vec![],
            country: String::new(),
            verified_at: None,
            last_check_at: None,
            upstream_port: 0,
            proxy_port: 17890,
            browsers: BTreeMap::new(),
            desktop: DesktopState::default(),
            allowed: false,
            reason_code: "checking".into(),
            suggested_action: "recheck".into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct DesktopState {
    pub installed: bool,
    pub configured: bool,
    pub connected: bool,
    pub running: bool,
    pub restart_required: bool,
    pub reason: String,
}
impl State {
    pub fn fresh_allowed(&self) -> bool {
        let now = Utc::now();
        self.allowed
            && self.status == "allowed"
            && self.verified_at.is_some_and(|t| {
                (now - t).num_milliseconds() >= 0 && (now - t).num_milliseconds() < 45_000
            })
            && self.last_check_at.is_some_and(|t| {
                (now - t).num_milliseconds() >= 0 && (now - t).num_milliseconds() < 3_000
            })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Request {
    pub action: String,
    pub node: Option<String>,
    pub cwd: Option<String>,
    pub args: Option<Vec<String>>,
    pub browser: Option<String>,
    pub client_id: Option<String>,
    pub connected: bool,
    pub protected: bool,
    pub reason: Option<String>,
}
impl Request {
    pub fn action(action: &str) -> Self {
        Self {
            action: action.into(),
            ..Self::default()
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    pub error: Option<String>,
    pub state: Option<State>,
    pub claude_path: Option<String>,
    pub browser_command: Option<String>,
}
impl Default for Response {
    fn default() -> Self {
        Self {
            ok: true,
            error: None,
            state: None,
            claude_path: None,
            browser_command: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemProxy {
    pub supported: bool,
    pub enabled: bool,
    pub server: String,
    pub bypass: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_directory_atomic_replace() {
        fs::create_dir_all(settings_dir()).unwrap();
        let source = settings_dir().join(format!("qa-{}.new", std::process::id()));
        let dest = settings_dir().join(format!("qa-{}.json", std::process::id()));
        fs::write(&source, b"first").unwrap();
        atomic_replace(&source, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"first");
        fs::write(&source, b"second").unwrap();
        atomic_replace(&source, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"second");
        fs::remove_file(dest).unwrap();
    }
    #[test]
    fn legacy_settings() {
        let s: Settings = serde_json::from_str(
            r#"{"proxyPort":17890,"autoStart":false,"soundEnabled":true,"preferredNode":"美国"}"#,
        )
        .unwrap();
        assert!(!s.auto_start);
        assert!(s.sound_enabled);
        assert_eq!(s.preferred_node.as_deref(), Some("美国"));
    }
    #[test]
    fn wire_case_and_defaults() {
        let value = serde_json::to_value(State::default()).unwrap();
        assert!(value.get("verifiedAt").is_some());
        assert_eq!(value["allowed"], false);
        let r: Request =
            serde_json::from_str(r#"{"action":"launch","cwd":"D:\\项目","args":["--","a b"]}"#)
                .unwrap();
        assert_eq!(r.args.unwrap(), ["--", "a b"]);
    }
    #[test]
    fn fresh_gate_does_not_trust_flag_alone() {
        let mut s = State {
            allowed: true,
            status: "allowed".into(),
            ..State::default()
        };
        assert!(!s.fresh_allowed());
        s.last_check_at = Some(Utc::now());
        s.verified_at = Some(Utc::now());
        assert!(s.fresh_allowed());
        s.last_check_at = Some(Utc::now() - chrono::Duration::seconds(4));
        assert!(!s.fresh_allowed());
    }
}
