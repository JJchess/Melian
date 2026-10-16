//! Persistent browser routing without changing profiles or login data.
//! Only our newly-created ProxySettings child key is writable by this user.
//! Registry state is configuration evidence, never evidence of a live browser route.
use anyhow::{bail, Context, Result};
use guard_protocol::{settings_dir, wide, SystemProxy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    io::Write,
    mem::size_of,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, LocalFree, ERROR_NO_MORE_FILES, FILETIME, HANDLE, HLOCAL,
            WAIT_OBJECT_0,
        },
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            GroupPolicy::RefreshPolicyEx,
            Registry as registry,
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                GetCurrentProcessId, GetExitCodeProcess, GetProcessTimes, OpenProcess,
                WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Shell::{
                ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
            },
            WindowsAndMessaging::SW_HIDE,
        },
    },
};
use winreg::{enums::*, types::ToRegValue, RegKey, RegValue};

const BROWSERS: [&str; 2] = ["edge", "chrome"];
// Disabled after the campus-login incident: a persistent PAC cannot follow
// later system proxy changes when the guard is no longer running.
pub const AUTOMATIC_ENROLLMENT_ENABLED: bool = false;
pub const PAUSED_REASON: &str = "网页自动保护暂时停用，正在修复网络兼容问题";
const CHILD: &str = "ProxySettings";
static CHANGE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default)]
pub struct BrowserStatus {
    pub installed: bool,
    pub configured: bool,
    pub restart_required: bool,
    pub reason: String,
    pub changed_at_ms: u64,
    pub enabled_at_ms: u64,
    /// Changes only after a routing configuration change. A browser verification
    /// receipt must be bound to this revision, not merely to its browser name.
    pub fingerprint: String,
    /// Hash of the expected data URL, for extension/native-route agreement.
    pub pac_fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Policy {
    pac_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    policy: Policy,
    /// New installs are prepared here and atomically renamed into place. Empty
    /// for updates, which replace only the single ProxyPacUrl registry value.
    staging: String,
    changed_at_ms: u64,
    fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    browser: String,
    port: u16,
    active: Option<Policy>,
    pending: Option<Pending>,
    changed_at_ms: u64,
    #[serde(default)]
    enabled_at_ms: u64,
    fingerprint: String,
}

#[derive(Default)]
struct ChildSnapshot {
    values: Vec<(String, RegValue)>,
    subkeys: Vec<String>,
}
#[derive(Default)]
struct Snapshot {
    root_values: Vec<String>,
    root_keys: Vec<String>,
    child: Option<ChildSnapshot>,
}

fn canonical(browser: &str) -> Result<&'static str> {
    BROWSERS
        .into_iter()
        .find(|b| b.eq_ignore_ascii_case(browser))
        .context("不支持此浏览器")
}
fn policy_path(browser: &str) -> Result<&'static str> {
    Ok(match canonical(browser)? {
        "edge" => r"SOFTWARE\Policies\Microsoft\Edge",
        _ => r"SOFTWARE\Policies\Google\Chrome",
    })
}
fn label(browser: &str) -> &'static str {
    if browser == "edge" {
        "Edge"
    } else {
        "Chrome"
    }
}

pub fn resolve(browser: &str) -> Result<PathBuf> {
    let browser = canonical(browser)?;
    let file = if browser == "edge" {
        "msedge.exe"
    } else {
        "chrome.exe"
    };
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            if let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(
                format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{file}"),
                KEY_READ | view,
            ) {
                if let Ok(path) = key.get_value::<String, _>("") {
                    let path = PathBuf::from(path.trim_matches('"'));
                    if path.is_file()
                        && path
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(file))
                    {
                        return Ok(path);
                    }
                }
            }
        }
    }
    let relative = if browser == "edge" {
        "Microsoft/Edge/Application/msedge.exe"
    } else {
        "Google/Chrome/Application/chrome.exe"
    };
    for env in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(env) {
            let path = PathBuf::from(base).join(relative);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    bail!("未安装 {}", label(browser))
}

fn record_path(browser: &str) -> PathBuf {
    settings_dir().join(format!("browser-proxy-{browser}.json"))
}
fn read_record(browser: &str) -> Result<Option<Registration>> {
    match fs::read(record_path(browser)) {
        Ok(bytes) => {
            let record: Registration =
                serde_json::from_slice(&bytes).context("浏览器接入记录损坏")?;
            if record.browser != browser || record.port < 1024 {
                bail!("浏览器接入记录无效");
            }
            if let Some(pending) = &record.pending {
                if !pending.staging.is_empty() && !valid_staging(&pending.staging) {
                    bail!("浏览器接入记录无效");
                }
            }
            Ok(Some(record))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn write_record(record: &Registration) -> Result<()> {
    fs::create_dir_all(settings_dir())?;
    let path = record_path(&record.browser);
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = fs::File::create(&temp)?;
    file.write_all(&serde_json::to_vec_pretty(record)?)?;
    file.sync_all()?;
    drop(file);
    guard_protocol::atomic_replace(&temp, &path)
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn valid_staging(name: &str) -> bool {
    name.strip_prefix("ClaudeGuard.ProxySettings.")
        .is_some_and(|id| {
            id.len() == 36
                && uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id)
        })
}
fn expected_values(policy: &Policy) -> Vec<(String, RegValue)> {
    vec![
        ("ProxyMode".into(), "pac_script".to_reg_value()),
        ("ProxyPacUrl".into(), policy.pac_url.to_reg_value()),
        ("ProxyPacMandatory".into(), 1u32.to_reg_value()),
    ]
}
fn same_value(a: &RegValue, b: &RegValue) -> bool {
    a.vtype == b.vtype && a.bytes == b.bytes
}
fn matches_policy(child: &ChildSnapshot, policy: &Policy, partial: bool) -> bool {
    if !child.subkeys.is_empty() || (!partial && child.values.len() != 3) {
        return false;
    }
    let values = expected_values(policy);
    child.values.iter().all(|(name, value)| {
        values
            .iter()
            .any(|(expected, v)| expected.eq_ignore_ascii_case(name) && same_value(value, v))
    })
}
fn read_child(parent: &RegKey, name: &str) -> Result<Option<ChildSnapshot>> {
    match parent.open_subkey_with_flags(name, KEY_READ | KEY_WOW64_64KEY) {
        Ok(key) => Ok(Some(ChildSnapshot {
            values: key.enum_values().collect::<io::Result<_>>()?,
            subkeys: key.enum_keys().collect::<io::Result<_>>()?,
        })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn snapshot(hive: winreg::HKEY, browser: &str) -> Result<Snapshot> {
    let key = match RegKey::predef(hive)
        .open_subkey_with_flags(policy_path(browser)?, KEY_READ | KEY_WOW64_64KEY)
    {
        Ok(key) => key,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Snapshot::default()),
        Err(e) => return Err(e.into()),
    };
    Ok(Snapshot {
        root_values: key
            .enum_values()
            .map(|v| v.map(|(name, _)| name))
            .collect::<io::Result<_>>()?,
        root_keys: key.enum_keys().collect::<io::Result<_>>()?,
        child: read_child(&key, CHILD)?,
    })
}
fn proxy_name(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with("proxy")
}
fn owned(child: &ChildSnapshot, record: &Registration) -> bool {
    record
        .active
        .as_ref()
        .is_some_and(|p| matches_policy(child, p, false))
        || record
            .pending
            .as_ref()
            .is_some_and(|p| matches_policy(child, &p.policy, false))
}
fn validate(machine: &Snapshot, user: &Snapshot, record: Option<&Registration>) -> Result<()> {
    if machine
        .root_values
        .iter()
        .chain(&machine.root_keys)
        .any(|n| proxy_name(n))
        || machine.child.is_some()
    {
        bail!("已有设备代理策略，无法自动接入");
    }
    if user.root_values.iter().any(|n| proxy_name(n))
        || user
            .root_keys
            .iter()
            .any(|n| proxy_name(n) && !n.eq_ignore_ascii_case(CHILD))
    {
        bail!("已有浏览器代理配置，无法自动接入");
    }
    if let Some(child) = &user.child {
        if !record.is_some_and(|r| owned(child, r)) {
            bail!("浏览器代理配置已被其他程序修改");
        }
    } else if record.is_some_and(|r| r.active.is_some()) {
        // Removing our configuration is a deliberate opt-out; never reinstall
        // it silently from the background synchronization loop.
        bail!("浏览器保护配置已移除，请重新接入");
    }
    Ok(())
}

fn prepare_registration(
    machine: &Snapshot,
    user: &Snapshot,
    mut record: Option<Registration>,
    explicit_enrollment: bool,
) -> Result<Option<Registration>> {
    let reenroll_deleted = explicit_enrollment
        && user.child.is_none()
        && record.as_ref().is_some_and(|r| r.active.is_some());
    if !reenroll_deleted {
        validate(machine, user, record.as_ref())?;
        return Ok(record);
    }
    // A user's explicit retry may replace a deleted policy, but it cannot
    // override a newly-installed user or machine proxy. Validate with no claim
    // of ownership before resetting the old journal. Background sync still
    // rejects this state, so deleting a policy remains an effective opt-out.
    validate(machine, user, None)?;
    let registration = record.as_mut().expect("reenrollment requires a record");
    if registration
        .pending
        .as_ref()
        .is_some_and(|pending| !pending.staging.is_empty())
    {
        // Valid updates to an active policy never use a staging key. Do not
        // orphan an unexpected old staging transaction by discarding its name.
        bail!("浏览器接入记录不一致，请先移除浏览器保护后重试");
    }
    registration.active = None;
    registration.pending = None;
    registration.changed_at_ms = 0;
    registration.enabled_at_ms = 0;
    registration.fingerprint.clear();
    Ok(record)
}

pub fn configured_status(browser: &str, port: u16) -> BrowserStatus {
    let mut status = BrowserStatus::default();
    let result = (|| -> Result<()> {
        let browser = canonical(browser)?;
        status.installed = resolve(browser).is_ok();
        if !status.installed {
            bail!("未安装 {}", label(browser));
        }
        let record = read_record(browser)?;
        let machine = snapshot(HKEY_LOCAL_MACHINE, browser)?;
        let user = snapshot(HKEY_CURRENT_USER, browser)?;
        validate(&machine, &user, record.as_ref())?;
        let record = record.context("尚未接入浏览器保护")?;
        status.changed_at_ms = record.changed_at_ms;
        status.enabled_at_ms = record.enabled_at_ms;
        status.fingerprint = record.fingerprint.clone();
        if record.port != port || record.pending.is_some() {
            bail!("浏览器保护配置正在更新");
        }
        let expected = Policy {
            pac_url: crate::browser_pac::data_url(&crate::browser_pac::build(
                &crate::integration::system_proxy(),
                port,
            )?),
        };
        status.pac_fingerprint = hex::encode(Sha256::digest(expected.pac_url.as_bytes()));
        if record.active.as_ref() != Some(&expected) {
            bail!("正在同步系统代理，等待浏览器验证");
        }
        status.configured = user
            .child
            .as_ref()
            .is_some_and(|c| matches_policy(c, &expected, false));
        if !status.configured {
            bail!("浏览器保护配置未完成");
        }
        status.restart_required = needs_restart(browser, record.enabled_at_ms).unwrap_or(true);
        status.reason = if status.restart_required {
            "请重开浏览器以应用保护，登录状态会保留"
        } else {
            "已配置，等待浏览器验证"
        }
        .into();
        Ok(())
    })();
    if let Err(e) = result {
        status.reason = e.to_string();
    }
    status
}

pub(crate) fn needs_restart(browser: &str, enabled_at_ms: u64) -> Result<bool> {
    if enabled_at_ms == 0 {
        return Ok(true);
    }
    let name = if browser == "edge" {
        "msedge.exe"
    } else {
        "chrome.exe"
    };
    unsafe {
        let mut session = 0;
        ProcessIdToSessionId(GetCurrentProcessId(), &mut session)?;
        let snapshot = ProcessHandle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        Process32FirstW(snapshot.0, &mut entry)?;
        loop {
            let length = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..length]).eq_ignore_ascii_case(name) {
                let mut process_session = 0;
                ProcessIdToSessionId(entry.th32ProcessID, &mut process_session)?;
                if process_session == session {
                    let process = ProcessHandle(OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION,
                        false,
                        entry.th32ProcessID,
                    )?);
                    let mut created = FILETIME::default();
                    let mut exited = FILETIME::default();
                    let mut kernel = FILETIME::default();
                    let mut user = FILETIME::default();
                    GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user)?;
                    let ticks =
                        ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
                    if (ticks / 10_000).saturating_sub(11_644_473_600_000) <= enabled_at_ms {
                        return Ok(true);
                    }
                }
            }
            if Process32NextW(snapshot.0, &mut entry).is_err() {
                if GetLastError() == ERROR_NO_MORE_FILES {
                    break;
                }
                return Err(windows::core::Error::from_win32().into());
            }
        }
    }
    Ok(false)
}

fn win32_result(error: windows::Win32::Foundation::WIN32_ERROR) -> io::Result<()> {
    if error.0 == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(error.0 as i32))
    }
}
fn flush(key: &RegKey) -> io::Result<()> {
    unsafe { win32_result(registry::RegFlushKey(registry::HKEY(key.raw_handle()))) }
}
struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(HLOCAL(self.0 .0));
        }
    }
}
fn create_owned_staging(parent: &RegKey, name: &str, sid: &str) -> Result<RegKey> {
    if !valid_staging(name) {
        bail!("浏览器事务名称无效");
    }
    // Rights are confined to the new child: query/read, set values and delete.
    // No change is made to Chrome/Edge's parent policy ACL or any existing key.
    let sddl = wide(format!(
        "D:P(A;;KA;;;SY)(A;;KA;;;BA)(A;;0x0003001B;;;{sid})"
    ));
    let mut descriptor = Descriptor(PSECURITY_DESCRIPTOR::default());
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor.0,
            None,
        )?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0 .0,
            bInheritHandle: false.into(),
        };
        let mut raw = registry::HKEY::default();
        let mut disposition = registry::REG_CREATE_KEY_DISPOSITION::default();
        win32_result(registry::RegCreateKeyExW(
            registry::HKEY(parent.raw_handle()),
            PCWSTR(wide(name).as_ptr()),
            0,
            PCWSTR::null(),
            registry::REG_OPTION_NON_VOLATILE,
            registry::KEY_READ | registry::KEY_SET_VALUE | registry::KEY_WOW64_64KEY,
            Some(&attributes),
            &mut raw,
            Some(&mut disposition),
        ))?;
        let key = RegKey::predef(raw.0);
        if disposition != registry::REG_CREATED_NEW_KEY {
            bail!("浏览器事务目录已存在，请重试");
        }
        Ok(key)
    }
}
fn refresh() -> Result<()> {
    // Requests asynchronous user-policy processing; callers must verify the
    // browser's actual route and never equate this return with policy delivery.
    unsafe {
        RefreshPolicyEx(false, 0).context("已保存浏览器配置，请重启浏览器后验证")
    }
}
fn checked_pending(record: &Registration, desired: &Policy) -> Result<Option<Pending>> {
    if record
        .pending
        .as_ref()
        .is_some_and(|p| &p.policy != desired)
    {
        bail!("系统代理已变更，旧浏览器事务不能回放");
    }
    Ok(record.pending.clone())
}

// A persisted journal is not an authority to inject an arbitrary PAC into the
// elevated helper. If the system changed during enrollment, discard only our
// exact old staging data, retain any already-committed route as read evidence,
// and prepare a new transaction from the current internally-generated policy.
fn reconcile_stale_pending(
    browser: &str,
    record: &mut Registration,
    desired: &Policy,
) -> Result<()> {
    let Some(pending) = record.pending.clone().filter(|p| &p.policy != desired) else {
        return Ok(());
    };
    let machine = snapshot(HKEY_LOCAL_MACHINE, browser)?;
    let user = snapshot(HKEY_CURRENT_USER, browser)?;
    validate(&machine, &user, Some(record))?;
    if !pending.staging.is_empty() {
        match RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(policy_path(browser)?, KEY_READ | KEY_WOW64_64KEY)
        {
            Ok(parent) => {
                if let Some(child) = read_child(&parent, &pending.staging)? {
                    if !matches_policy(&child, &pending.policy, true) {
                        bail!("旧浏览器事务已被修改，未覆盖");
                    }
                    parent.delete_subkey_with_flags(&pending.staging, KEY_WOW64_64KEY)?;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if user
        .child
        .as_ref()
        .is_some_and(|c| matches_policy(c, &pending.policy, false))
    {
        record.active = Some(pending.policy);
        if record.enabled_at_ms == 0 {
            record.enabled_at_ms = now_ms();
        }
        record.changed_at_ms = pending.changed_at_ms;
        record.fingerprint = pending.fingerprint;
    }
    record.pending = None;
    write_record(record)
}

fn finish_pending(browser: &str, record: &mut Registration, desired: &Policy) -> Result<bool> {
    // This check precedes registry access, including during elevation.
    let Some(pending) = checked_pending(record, desired)? else {
        return Ok(false);
    };
    let machine = snapshot(HKEY_LOCAL_MACHINE, browser)?;
    let user = snapshot(HKEY_CURRENT_USER, browser)?;
    validate(&machine, &user, Some(record))?;
    if user
        .child
        .as_ref()
        .is_some_and(|c| matches_policy(c, &pending.policy, false))
    {
        // The registry commit completed before a crash, but the journal did not.
    } else if pending.staging.is_empty() {
        let current = user.child.as_ref().context("浏览器保护配置已移除")?;
        if !record
            .active
            .as_ref()
            .is_some_and(|p| matches_policy(current, p, false))
        {
            bail!("浏览器配置已变更，未覆盖");
        }
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
            format!(r"{}\{CHILD}", policy_path(browser)?),
            KEY_READ | KEY_SET_VALUE | KEY_WOW64_64KEY,
        )?;
        key.set_value("ProxyPacUrl", &desired.pac_url)?;
        flush(&key)?;
    } else {
        let parent = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey_with_flags(
                policy_path(browser)?,
                KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            )?
            .0;
        let staging = match read_child(&parent, &pending.staging)? {
            Some(child) => {
                if !matches_policy(&child, &pending.policy, true) {
                    bail!("浏览器事务配置已变更，未覆盖");
                }
                parent.open_subkey_with_flags(
                    &pending.staging,
                    KEY_READ | KEY_SET_VALUE | KEY_WOW64_64KEY,
                )?
            }
            None => create_owned_staging(&parent, &pending.staging, &crate::ipc::current_sid()?)?,
        };
        for (name, value) in expected_values(desired) {
            staging.set_raw_value(name, &value)?;
        }
        flush(&staging)?;
        drop(staging);
        // RegRenameKey fails if ProxySettings already exists. A competing app's
        // configuration is therefore not overwritten between check and commit.
        unsafe {
            win32_result(registry::RegRenameKey(
                registry::HKEY(parent.raw_handle()),
                PCWSTR(wide(&pending.staging).as_ptr()),
                PCWSTR(wide(CHILD).as_ptr()),
            ))?;
        }
        flush(&parent)?;
    }
    record.active = Some(pending.policy);
    if record.enabled_at_ms == 0 {
        record.enabled_at_ms = now_ms();
    }
    record.changed_at_ms = pending.changed_at_ms;
    record.fingerprint = pending.fingerprint;
    record.pending = None;
    write_record(record)?;
    Ok(true)
}

fn apply(browser: &str, port: u16, policy: &Policy, allow_new: bool) -> Result<bool> {
    let original = read_record(browser)?;
    if !allow_new && original.is_none() {
        return Ok(false);
    }
    resolve(browser)?;
    let machine = snapshot(HKEY_LOCAL_MACHINE, browser)?;
    let user = snapshot(HKEY_CURRENT_USER, browser)?;
    let original = prepare_registration(&machine, &user, original, allow_new)?;
    let mut record = original.unwrap_or(Registration {
        browser: browser.into(),
        port,
        active: None,
        pending: None,
        changed_at_ms: 0,
        enabled_at_ms: 0,
        fingerprint: String::new(),
    });
    reconcile_stale_pending(browser, &mut record, policy)?;
    let mut changed = finish_pending(browser, &mut record, policy)?;
    if record.active.as_ref() == Some(policy) && record.port == port {
        return Ok(changed);
    }
    if record.active.is_none() && !allow_new {
        return Ok(changed);
    }
    record.port = port;
    record.pending = Some(Pending {
        policy: policy.clone(),
        staging: if record.active.is_some() {
            String::new()
        } else {
            format!("ClaudeGuard.ProxySettings.{}", uuid::Uuid::new_v4())
        },
        changed_at_ms: now_ms(),
        fingerprint: uuid::Uuid::new_v4().to_string(),
    });
    // Durable intent precedes any registry writes. No cookies/session data or
    // pre-existing browser settings are read or backed up by this mechanism.
    write_record(&record)?;
    changed |= finish_pending(browser, &mut record, policy)?;
    Ok(changed)
}
fn permission_denied(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        e.downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::PermissionDenied)
    })
}
fn apply_all(port: u16, system: &SystemProxy, allow_new: bool) -> (bool, bool, Vec<String>) {
    let policy = match crate::browser_pac::build(system, port) {
        Ok(pac) => Policy {
            pac_url: crate::browser_pac::data_url(&pac),
        },
        Err(e) => return (false, false, vec![e.to_string()]),
    };
    let mut changed = false;
    let mut denied = false;
    let mut errors = vec![];
    for browser in BROWSERS {
        if resolve(browser).is_err() && read_record(browser).ok().flatten().is_none() {
            continue;
        }
        match apply(browser, port, &policy, allow_new) {
            Ok(value) => changed |= value,
            Err(e) => {
                denied |= permission_denied(&e);
                errors.push(format!("{}：{e}", label(browser)));
            }
        }
    }
    if changed {
        if let Err(e) = refresh() {
            errors.push(e.to_string());
        }
    }
    (changed, denied, errors)
}

/// Blocking API: invoke through spawn_blocking, since first-time enrollment may
/// wait for the Windows elevation dialog. Each browser commits independently.
pub fn enable(port: u16) -> Result<()> {
    if !AUTOMATIC_ENROLLMENT_ENABLED {
        bail!(PAUSED_REASON);
    }
    let _lock = CHANGE_LOCK.lock().unwrap();
    if port < 1024 {
        bail!("守卫代理端口无效");
    }
    let (_, denied, mut errors) = apply_all(port, &crate::integration::system_proxy(), true);
    if denied {
        if let Err(e) = run_elevated(true, port) {
            errors.push(e.to_string());
        }
    }
    if BROWSERS
        .into_iter()
        .any(|b| configured_status(b, port).configured)
    {
        return Ok(());
    }
    if errors.is_empty() {
        bail!("未找到可以接入的 Edge 或 Chrome");
    }
    bail!("{}", errors.join("；"))
}
pub fn sync(port: u16, system: &SystemProxy) -> Result<()> {
    if !AUTOMATIC_ENROLLMENT_ENABLED {
        return Ok(());
    }
    let _lock = CHANGE_LOCK.lock().unwrap();
    let (_, _, errors) = apply_all(port, system, false);
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("{}", errors.join("；"))
    }
}

fn remove_browser(browser: &str) -> Result<()> {
    let Some(record) = read_record(browser)? else {
        return Ok(());
    };
    let parent = match RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(policy_path(browser)?, KEY_READ | KEY_WOW64_64KEY)
    {
        Ok(key) => Some(key),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(parent) = parent {
        if read_child(&parent, CHILD)?.is_some_and(|c| owned(&c, &record)) {
            // No recursive deletion: any unexpected child/value keeps the key.
            parent.delete_subkey_with_flags(CHILD, KEY_WOW64_64KEY)?;
        }
        if let Some(pending) = &record.pending {
            if !pending.staging.is_empty()
                && read_child(&parent, &pending.staging)?
                    .is_some_and(|c| matches_policy(&c, &pending.policy, true))
            {
                parent.delete_subkey_with_flags(&pending.staging, KEY_WOW64_64KEY)?;
            }
        }
    }
    // If an external change replaced the policy, preserve it and stop claiming
    // ownership. Other browser policies and the parent keys are never removed.
    fs::remove_file(record_path(browser))?;
    Ok(())
}
fn remove_all() -> Result<()> {
    let mut errors = vec![];
    for browser in BROWSERS {
        if let Err(e) = remove_browser(browser) {
            errors.push(e);
        }
    }
    if let Some(error) = errors.into_iter().next() {
        return Err(error);
    }
    let _ = refresh();
    Ok(())
}
pub fn unregister() -> Result<()> {
    let _lock = CHANGE_LOCK.lock().unwrap();
    match remove_all() {
        Err(e) if permission_denied(&e) => run_elevated(false, 17890),
        result => result,
    }
}
pub fn elevated_policy(enable: bool, port: u16, expected_sid: &str) -> Result<()> {
    if port < 1024 || crate::ipc::current_sid()? != expected_sid {
        bail!("请使用同一 Windows 账户完成浏览器代理授权");
    }
    let _lock = CHANGE_LOCK.lock().unwrap();
    if !enable {
        return remove_all();
    }
    if !AUTOMATIC_ENROLLMENT_ENABLED {
        bail!(PAUSED_REASON);
    }
    let (_, _, errors) = apply_all(port, &crate::integration::system_proxy(), true);
    if BROWSERS
        .into_iter()
        .any(|b| configured_status(b, port).configured)
    {
        Ok(())
    } else {
        bail!(
            "{}",
            if errors.is_empty() {
                "未找到可以接入的浏览器".into()
            } else {
                errors.join("；")
            }
        )
    }
}
struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn run_elevated(enable: bool, port: u16) -> Result<()> {
    let exe = wide(guard_protocol::install_dir().join("ClaudeGuard.Setup.exe"));
    let args = wide(format!(
        "--browser-proxy-{} --port {} --sid {} --quiet",
        if enable { "enable" } else { "remove" },
        port,
        crate::ipc::current_sid()?
    ));
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(exe.as_ptr()),
            lpParameters: PCWSTR(args.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).context("浏览器接入需要 Windows 授权，请允许系统提示")?;
        let process = ProcessHandle(info.hProcess);
        if process.0.is_invalid() || WaitForSingleObject(process.0, 120_000) != WAIT_OBJECT_0 {
            bail!("浏览器代理授权尚未完成");
        }
        let mut code = 1;
        GetExitCodeProcess(process.0, &mut code)?;
        if code != 0 {
            bail!("浏览器代理授权未完成，请使用同一 Windows 账户重试");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> Policy {
        Policy {
            pac_url: "data:application/x-ns-proxy-autoconfig,test".into(),
        }
    }
    fn child() -> ChildSnapshot {
        ChildSnapshot {
            values: expected_values(&policy()),
            subkeys: vec![],
        }
    }
    fn record() -> Registration {
        Registration {
            browser: "chrome".into(),
            port: 17890,
            active: Some(policy()),
            pending: None,
            changed_at_ms: 1,
            enabled_at_ms: 1,
            fingerprint: "revision".into(),
        }
    }
    #[test]
    fn existing_machine_or_user_proxy_is_not_overwritten() {
        for name in [
            "ProxySettings",
            "ProxyMode",
            "ProxyPacUrl",
            "ProxyOverrideRules",
            "pRoXyServer",
        ] {
            let machine = Snapshot {
                root_values: vec![name.into()],
                ..Default::default()
            };
            assert!(validate(&machine, &Snapshot::default(), None).is_err());
            assert!(validate(&Snapshot::default(), &machine, None).is_err());
        }
        let user = Snapshot {
            child: Some(child()),
            ..Default::default()
        };
        assert!(validate(&Snapshot::default(), &user, None).is_err());
        assert!(validate(&Snapshot::default(), &user, Some(&record())).is_ok());
    }
    #[test]
    fn uninstall_ownership_requires_exact_values_and_no_foreign_children() {
        let record = record();
        let mut child = child();
        assert!(owned(&child, &record));
        child.values[1].1 = "data:new-user-configuration".to_reg_value();
        assert!(!owned(&child, &record));
        child = self::child();
        child
            .values
            .push(("OtherApplication".into(), 1u32.to_reg_value()));
        assert!(!owned(&child, &record));
        child = self::child();
        child.subkeys.push("NewConfiguration".into());
        assert!(!owned(&child, &record));
    }
    #[test]
    fn pending_commit_is_recoverable_but_partial_live_policy_is_not_claimed() {
        let mut record = record();
        let next = Policy {
            pac_url: "data:next".into(),
        };
        record.pending = Some(Pending {
            policy: next.clone(),
            staging: String::new(),
            changed_at_ms: 2,
            fingerprint: "new".into(),
        });
        assert!(owned(&child(), &record));
        let mut next_child = ChildSnapshot {
            values: expected_values(&next),
            subkeys: vec![],
        };
        assert!(owned(&next_child, &record));
        next_child.values.pop();
        assert!(!owned(&next_child, &record));
        assert!(matches_policy(&next_child, &next, true));
    }
    #[test]
    fn deleted_policy_is_not_silently_reenrolled() {
        assert!(validate(&Snapshot::default(), &Snapshot::default(), Some(&record())).is_err());
        assert!(validate(&Snapshot::default(), &Snapshot::default(), None).is_ok());
        assert!(!valid_staging("ProxySettings"));
        assert!(!valid_staging(r"ClaudeGuard.ProxySettings...\Other"));
        assert!(!valid_staging(
            "ClaudeGuard.ProxySettings.urn:uuid:550e8400-e29b-41d4-a716-446655440000"
        ));
        assert!(valid_staging(
            "ClaudeGuard.ProxySettings.550e8400-e29b-41d4-a716-446655440000"
        ));
    }
    #[test]
    fn explicit_reenrollment_recovers_deleted_policy_and_invalidates_old_receipts() {
        let mut old = record();
        old.pending = Some(Pending {
            policy: Policy {
                pac_url: "data:interrupted-update".into(),
            },
            staging: String::new(),
            changed_at_ms: 2,
            fingerprint: "interrupted".into(),
        });
        assert!(prepare_registration(
            &Snapshot::default(),
            &Snapshot::default(),
            Some(old.clone()),
            false
        )
        .is_err());
        let recovered =
            prepare_registration(&Snapshot::default(), &Snapshot::default(), Some(old), true)
                .unwrap()
                .unwrap();
        assert!(recovered.active.is_none());
        assert!(recovered.pending.is_none());
        assert_eq!(recovered.enabled_at_ms, 0);
        assert_eq!(recovered.changed_at_ms, 0);
        assert!(recovered.fingerprint.is_empty());
    }
    #[test]
    fn explicit_reenrollment_never_claims_replacement_or_machine_policy() {
        let foreign_machine = Snapshot {
            root_values: vec!["ProxyMode".into()],
            ..Default::default()
        };
        let foreign_user = Snapshot {
            root_values: vec!["ProxySettings".into()],
            ..Default::default()
        };
        assert!(
            prepare_registration(&foreign_machine, &Snapshot::default(), Some(record()), true)
                .is_err()
        );
        assert!(
            prepare_registration(&Snapshot::default(), &foreign_user, Some(record()), true)
                .is_err()
        );
        let mut replacement = child();
        replacement.values[1].1 = "data:replacement-owner".to_reg_value();
        let replacement_user = Snapshot {
            child: Some(replacement),
            ..Default::default()
        };
        assert!(prepare_registration(
            &Snapshot::default(),
            &replacement_user,
            Some(record()),
            true
        )
        .is_err());
    }
    #[test]
    fn pending_journal_cannot_authorize_arbitrary_proxy_content() {
        let mut record = record();
        record.pending = Some(Pending {
            policy: Policy {
                pac_url: "data:foreign-script".into(),
            },
            staging: String::new(),
            changed_at_ms: 2,
            fingerprint: "new".into(),
        });
        assert!(checked_pending(&record, &policy()).is_err());
        record.pending.as_mut().unwrap().policy = policy();
        assert!(checked_pending(&record, &policy()).unwrap().is_some());
    }
    #[test]
    fn elevated_helper_rejects_another_identity_before_any_write() {
        assert!(elevated_policy(true, 17890, "S-1-5-invalid-test").is_err());
        assert!(elevated_policy(false, 80, "S-1-5-invalid-test").is_err());
    }
}
