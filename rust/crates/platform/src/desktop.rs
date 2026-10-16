//! Claude Desktop's supported fixed-proxy policy. This is persistent routing,
//! not a firewall: updates, remote hosts and tools ignoring proxies are excluded.
use anyhow::{bail, Context, Result};
use guard_protocol::{settings_dir, DesktopState};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    mem::size_of,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::Stdio,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    core::{w, HSTRING, PCWSTR, PWSTR},
    Management::Deployment::PackageManager,
    Win32::{
        Foundation::*,
        NetworkManagement::IpHelper::*,
        System::{Com::*, Threading::*},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};
use winreg::{enums::*, RegKey, RegValue};

const POLICY: &str = r"SOFTWARE\Policies\Claude";
const PROXY: &str = "egressProxyUrl";
const PAC: &str = "egressProxyPacUrl";
static CHANGE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    proxy_url: String,
    // Windows process creation time, in 100 ns ticks since 1601.
    enabled_at: u64,
}
struct Com(bool);
impl Com {
    fn new() -> Result<Self> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_err() && result != RPC_E_CHANGED_MODE {
            result.ok()?;
        }
        Ok(Self(result.is_ok()))
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() }
        }
    }
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub fn supported_version(v: [u16; 4]) -> bool {
    v >= [1, 44121, 1, 0]
}
pub fn resolve() -> Result<PathBuf> {
    let _com = Com::new()?;
    let packages = PackageManager::new()?.FindPackagesByUserSecurityIdPackageFamilyName(
        &HSTRING::new(),
        &HSTRING::from("Claude_pzs8sxrjxfjjc"),
    )?;
    for package in packages {
        let id = package.Id()?;
        let v = id.Version()?;
        if !supported_version([v.Major, v.Minor, v.Build, v.Revision]) {
            bail!("请先更新 Claude 桌面，再接入守卫");
        }
        let exe =
            PathBuf::from(package.InstalledLocation()?.Path()?.to_string()).join("app/claude.exe");
        if exe.is_file() {
            return Ok(exe);
        }
    }
    bail!("未找到新版 Claude 桌面，请安装官方 Windows 版")
}
fn proxy_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}
fn record_path() -> PathBuf {
    settings_dir().join("desktop-proxy.json")
}
fn record() -> Result<Option<Registration>> {
    match fs::read(record_path()) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("桌面接入记录损坏")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn write_record(value: &Registration) -> Result<()> {
    fs::create_dir_all(settings_dir())?;
    let temp = record_path().with_extension(format!("{}.tmp", std::process::id()));
    let mut file = fs::File::create(&temp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    drop(file);
    guard_protocol::atomic_replace(&temp, &record_path())
}
fn now_ticks() -> u64 {
    116_444_736_000_000_000
        + SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            / 100
}
fn values(root: &RegKey, path: &str) -> Result<Vec<(String, RegValue)>> {
    match root.open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY) {
        Ok(key) => Ok(key.enum_values().collect::<std::io::Result<_>>()?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e).context("无法读取 Claude 代理策略"),
    }
}
fn readable(v: &RegValue) -> bool {
    matches!(v.vtype, REG_SZ | REG_EXPAND_SZ | REG_DWORD)
}
fn text(v: &RegValue) -> Option<String> {
    if v.vtype != REG_SZ || v.bytes.len() % 2 != 0 {
        return None;
    }
    let chars: Vec<_> = v
        .bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|c| *c != 0)
        .collect();
    String::from_utf16(&chars).ok()
}
fn validate_policy(
    machine: &[(String, RegValue)],
    user: &[(String, RegValue)],
    url: &str,
) -> Result<bool> {
    // A single readable machine value masks the entire user hive, even an
    // unrelated update setting or the unnamed default value.
    if machine.iter().any(|(_, v)| readable(v)) {
        bail!("Claude 桌面受设备策略管理，无法接入守卫");
    }
    if user.iter().any(|(k, _)| k.eq_ignore_ascii_case(PAC)) {
        bail!("Claude 桌面已有自动代理配置，请先移除该配置");
    }
    match user.iter().find(|(k, _)| k.eq_ignore_ascii_case(PROXY)) {
        Some((_, v)) if text(v).as_deref() == Some(url) => Ok(true),
        Some(_) => bail!("Claude 桌面已有其他代理配置，请先移除该配置"),
        None => Ok(false),
    }
}
fn policy_ready(port: u16) -> Result<bool> {
    validate_policy(
        &values(&RegKey::predef(HKEY_LOCAL_MACHINE), POLICY)?,
        &values(&RegKey::predef(HKEY_CURRENT_USER), POLICY)?,
        &proxy_url(port),
    )
}
fn managed_env_conflict(value: &serde_json::Value, url: &str) -> bool {
    let Some(env) = value.get("env").and_then(|v| v.as_object()) else {
        return false;
    };
    env.iter()
        .any(|(k, v)| match k.to_ascii_uppercase().as_str() {
            "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" => v.as_str() != Some(url),
            "NO_PROXY" => v.as_str().is_none_or(|s| {
                s.split(',').any(|host| {
                    !matches!(
                        host.trim(),
                        "" | "localhost" | "127.0.0.1" | "::1" | ".local"
                    )
                })
            }),
            _ => false,
        })
}
fn check_code_policy(port: u16) -> Result<()> {
    let url = proxy_url(port);
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for (name, value) in values(&RegKey::predef(hive), r"SOFTWARE\Policies\ClaudeCode")? {
            if name.eq_ignore_ascii_case("Settings") {
                let json: serde_json::Value =
                    serde_json::from_str(&text(&value).context("无法验证 Claude Code 策略")?)
                        .context("无法验证 Claude Code 策略")?;
                if managed_env_conflict(&json, &url) {
                    bail!("内置 Code 的代理策略与守卫冲突");
                }
            }
        }
    }
    // System-managed Code settings override the Desktop-supplied proxy.
    let dir =
        PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_default()).join("ClaudeCode");
    let mut files = vec![dir.join("managed-settings.json")];
    match fs::read_dir(dir.join("managed-settings.d")) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                if path.extension().is_some_and(|s| s == "json") {
                    files.push(path);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("无法验证 Claude Code 策略"),
    }
    for file in files {
        match fs::read(file) {
            Ok(bytes) => {
                let json = serde_json::from_slice(&bytes).context("无法验证 Claude Code 策略")?;
                if managed_env_conflict(&json, &url) {
                    bail!("内置 Code 的代理策略与守卫冲突");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("无法验证 Claude Code 策略"),
        }
    }
    Ok(())
}
// Do not confuse the separate Claude Code executable with Electron Desktop.
#[repr(C)]
struct ProcessInfoHeader {
    next: u32,
    threads: u32,
    reserved: [u8; 48],
    name: UNICODE_STRING,
    base_priority: i32,
    pid: HANDLE,
}
#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(
        class: u32,
        buffer: *mut std::ffi::c_void,
        length: u32,
        needed: *mut u32,
    ) -> i32;
}
// SystemProcessInformation's documented header avoids Toolhelp's repeated
// snapshot conversion on every 200ms poll. Still validate the executable path
// and creation time before considering any process to be Desktop.
fn named_process_ids(name: &str) -> Result<Vec<u32>> {
    let mut bytes = 256 * 1024usize;
    for _ in 0..8 {
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let mut needed = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                5,
                storage.as_mut_ptr().cast(),
                (storage.len() * size_of::<usize>()) as u32,
                &mut needed,
            )
        };
        if status as u32 == 0xc0000004 {
            bytes = (needed as usize + 65536).max(bytes * 2);
            if bytes > 16 * 1024 * 1024 {
                break;
            }
            continue;
        }
        if status < 0 {
            bail!("无法读取桌面进程列表");
        }
        let base = storage.as_ptr() as usize;
        let limit = (needed as usize).min(storage.len() * size_of::<usize>());
        let mut offset = 0usize;
        let mut result = vec![];
        loop {
            if offset + size_of::<ProcessInfoHeader>() > limit {
                bail!("进程列表长度无效");
            }
            let item = unsafe { &*((base + offset) as *const ProcessInfoHeader) };
            let pointer = item.name.Buffer.0 as usize;
            let length = item.name.Length as usize;
            if length == name.len() * 2
                && pointer >= base
                && pointer % 2 == 0
                && pointer
                    .checked_add(length)
                    .is_some_and(|end| end <= base + limit)
            {
                let value = unsafe { std::slice::from_raw_parts(item.name.Buffer.0, length / 2) };
                if value
                    .iter()
                    .zip(name.bytes())
                    .all(|(a, b)| *a < 128 && (*a as u8).eq_ignore_ascii_case(&b))
                {
                    result.push(item.pid.0 as usize as u32);
                }
            }
            if item.next == 0 {
                return Ok(result);
            }
            if (item.next as usize) < size_of::<ProcessInfoHeader>() || item.next % 8 != 0 {
                bail!("进程列表偏移无效");
            }
            offset += item.next as usize;
        }
    }
    bail!("进程列表变化过快，请重试")
}
fn processes() -> Result<Vec<(u32, u64)>> {
    unsafe {
        let mut found = vec![];
        for pid in named_process_ids("claude.exe")? {
            let process = Handle(
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                    .context("无法确认已运行的 Claude，请先完全退出桌面客户端")?,
            );
            let mut path = vec![0u16; 32768];
            let mut len = path.len() as u32;
            QueryFullProcessImageNameW(
                process.0,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut len,
            )?;
            let path = PathBuf::from(String::from_utf16_lossy(&path[..len as usize]));
            if path
                .parent()
                .is_some_and(|p| p.join("resources/app.asar").is_file())
            {
                let (mut created, mut exited, mut kernel, mut user) = (
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                );
                GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user)?;
                found.push((
                    pid,
                    ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
                ));
            }
        }
        Ok(found)
    }
}
fn needs_restart(record: Option<&Registration>, processes: &[(u32, u64)]) -> bool {
    !processes.is_empty() && record.is_none_or(|r| processes.iter().any(|(_, t)| *t < r.enabled_at))
}
pub fn status(port: u16, upstream: u16) -> DesktopState {
    let mut state = DesktopState::default();
    let result = (|| -> Result<()> {
        resolve()?;
        state.installed = true;
        let running = processes()?;
        state.running = !running.is_empty();
        check_code_policy(port)?;
        state.configured = policy_ready(port)?;
        let registration = record()?.filter(|r| r.proxy_url == proxy_url(port));
        state.restart_required = needs_restart(registration.as_ref(), &running);
        let (connected, bypass) = route_evidence(&running, port, upstream)?;
        state.connected = state.configured && connected && !bypass && !state.restart_required;
        state.reason = if bypass {
            state.restart_required = true;
            "检测到连接绕过守卫，请完全退出后重开"
        } else if !state.configured {
            "尚未接入"
        } else if state.restart_required {
            "请完全退出 Claude 桌面后重新打开"
        } else if state.connected {
            "已连接守卫"
        } else if state.running {
            "已配置，等待连接验证"
        } else {
            "已接入，尚未运行"
        }
        .into();
        Ok(())
    })();
    if let Err(error) = result {
        state.configured = false;
        state.connected = false;
        state.reason = error.to_string();
    }
    state
}
fn route_evidence(processes: &[(u32, u64)], port: u16, upstream: u16) -> Result<(bool, bool)> {
    unsafe {
        let mut size = 0;
        let _ = GetExtendedTcpTable(None, &mut size, false, 2, TCP_TABLE_OWNER_PID_ALL, 0);
        if size == 0 {
            bail!("无法验证桌面连接路径");
        }
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        let result = GetExtendedTcpTable(
            Some(storage.as_mut_ptr().cast()),
            &mut size,
            false,
            2,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );
        if result != 0 {
            bail!("暂时无法验证桌面连接路径");
        }
        let table = &*storage.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
        let count = table.dwNumEntries as usize;
        if 4 + count * size_of::<MIB_TCPROW_OWNER_PID>() > storage.len() * size_of::<usize>() {
            bail!("桌面连接表无效");
        }
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), count);
        let mut guard = false;
        let mut bypass = false;
        for row in rows {
            if row.dwState != 5 || !processes.iter().any(|(pid, _)| *pid == row.dwOwningPid) {
                continue;
            }
            if row.dwRemoteAddr.to_ne_bytes() != [127, 0, 0, 1] {
                continue;
            }
            let remote = u16::from_be(row.dwRemotePort as u16);
            guard |= remote == port;
            bypass |= upstream != 0 && upstream != port && remote == upstream;
        }
        Ok((guard, bypass))
    }
}
/// Preserve sessions already open when monitoring begins or the network fails.
/// Only a fresh Desktop launch while blocked is terminated; the proxy handles
/// network revocation for existing sessions independently of this monitor.
pub struct StartWatch {
    was_running: bool,
    live: Vec<Handle>,
}
impl StartWatch {
    pub fn new() -> Self {
        Self {
            was_running: processes().map(|p| !p.is_empty()).unwrap_or(true),
            live: vec![],
        }
    }
    fn observe(&mut self, running: bool, allowed: bool) -> bool {
        let block = running && !self.was_running && !allowed;
        self.was_running = running && !block;
        block
    }
    pub fn check(&mut self, allowed: bool) -> Result<bool> {
        if !self.live.is_empty() {
            self.live
                .retain(|handle| unsafe { WaitForSingleObject(handle.0, 0) == WAIT_TIMEOUT });
            if !self.live.is_empty() {
                return Ok(false);
            }
            self.was_running = false;
        }
        let running = processes()?;
        if !self.observe(!running.is_empty(), allowed) {
            self.live = running
                .iter()
                .filter_map(|(pid, _)| unsafe {
                    OpenProcess(PROCESS_SYNCHRONIZE, false, *pid)
                        .ok()
                        .map(Handle)
                })
                .collect();
            return Ok(false);
        }
        let mut stopped = false;
        for (pid, created) in running {
            unsafe {
                let Ok(handle) = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                    false,
                    pid,
                ) else {
                    continue;
                };
                let handle = Handle(handle);
                let (mut c, mut e, mut k, mut u) = (
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                );
                if GetProcessTimes(handle.0, &mut c, &mut e, &mut k, &mut u).is_ok()
                    && (((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64) == created
                {
                    stopped |= TerminateProcess(handle.0, 1).is_ok();
                }
            }
        }
        Ok(stopped)
    }
}
pub fn launch(port: u16, allowed: impl Fn() -> bool) -> Result<()> {
    let _lock = CHANGE_LOCK.lock().unwrap();
    let exe = resolve()?;
    check_code_policy(port)?;
    let ready = policy_ready(port)?;
    let running = processes()?;
    let registration = record()?.filter(|r| r.proxy_url == proxy_url(port));
    if !running.is_empty() {
        if !allowed() {
            bail!("连接未就绪，暂时无法打开 Claude 桌面");
        }
        if !ready || needs_restart(registration.as_ref(), &running) {
            bail!("请先从托盘完全退出 Claude 桌面，再从这里打开");
        }
        for (pid, _) in &running {
            unsafe {
                let _ = EnumWindows(Some(focus_window), LPARAM(*pid as isize));
            }
        }
        return Ok(());
    }
    if !ready
        || registration
            .as_ref()
            .is_none_or(|r| r.enabled_at == u64::MAX)
    {
        if ready && registration.is_none() {
            // An identical existing value belongs to the user; do not claim it
            // or remove it on uninstall. Require explicit removal for enrollment.
            bail!("Claude 桌面已有手动代理配置，请先移除后接入");
        }
        write_record(&Registration {
            proxy_url: proxy_url(port),
            enabled_at: u64::MAX,
        })?;
        if !ready {
            match set_policy(port) {
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    run_elevated(true, port)?
                }
                result => result?,
            }
        }
        write_record(&Registration {
            proxy_url: proxy_url(port),
            enabled_at: now_ticks(),
        })?;
        if !processes()?.is_empty() {
            bail!("接入期间 Claude 桌面已启动，请完全退出后重新打开");
        }
    }
    if !policy_ready(port)? {
        bail!("Claude 桌面代理配置未生效");
    }
    let cwd = exe.parent().context("Claude 桌面路径无效")?;
    if !allowed() {
        bail!("连接未就绪，已保存接入配置，请恢复网络后重试");
    }
    let mut command = crate::launch::claude_command(&exe, cwd, &[], port);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000);
    let mut child = command.spawn().context("无法启动 Claude 桌面")?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    if child.try_wait()?.is_some() && processes()?.is_empty() {
        bail!("Claude 桌面未能启动");
    }
    // No job object / kill-on-drop: network faults must keep sessions alive.
    Ok(())
}
unsafe extern "system" fn focus_window(hwnd: HWND, param: LPARAM) -> BOOL {
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == param.0 as u32 && IsWindowVisible(hwnd).as_bool() {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    true.into()
}
pub fn unregister() -> Result<()> {
    let _lock = CHANGE_LOCK.lock().unwrap();
    let Some(registration) = record()? else {
        return Ok(());
    };
    let port: u16 = registration
        .proxy_url
        .strip_prefix("http://127.0.0.1:")
        .context("桌面接入记录中的地址无效")?
        .parse()?;
    match clear_policy(port) {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            run_elevated(false, port)?
        }
        result => result?,
    }
    fs::remove_file(record_path())?;
    Ok(())
}
fn set_policy(port: u16) -> std::io::Result<()> {
    RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(POLICY)?
        .0
        .set_value(PROXY, &proxy_url(port))
}
fn clear_policy(port: u16) -> std::io::Result<()> {
    let current = RegKey::predef(HKEY_CURRENT_USER);
    let key = match current.open_subkey(POLICY) {
        Ok(key) => key,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if key
        .get_value::<String, _>(PROXY)
        .is_ok_and(|v| v == proxy_url(port))
    {
        remove_owned_proxy(
            &current.open_subkey_with_flags(POLICY, KEY_READ | KEY_SET_VALUE)?,
            &proxy_url(port),
        )?;
    }
    Ok(())
}
/// A narrowly scoped elevated installer mode. It accepts no paths or arbitrary
/// registry data, does not launch Claude, and refuses another admin's identity.
pub fn elevated_policy(enable: bool, port: u16, expected_sid: &str) -> Result<()> {
    if port < 1024 || crate::ipc::current_sid()? != expected_sid {
        bail!("请使用同一 Windows 账户完成桌面代理授权");
    }
    if enable {
        resolve()?;
        check_code_policy(port)?;
        if !processes()?.is_empty() {
            bail!("请先完全退出 Claude 桌面");
        }
        if !policy_ready(port)? {
            set_policy(port)?;
        }
    } else {
        clear_policy(port)?;
    }
    Ok(())
}
fn run_elevated(enable: bool, port: u16) -> Result<()> {
    let exe = guard_protocol::wide(guard_protocol::install_dir().join("ClaudeGuard.Setup.exe"));
    let args = guard_protocol::wide(format!(
        "--desktop-proxy-{} --port {} --sid {} --quiet",
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
        ShellExecuteExW(&mut info).context("桌面接入需要 Windows 授权，请重试并允许系统提示")?;
        let process = Handle(info.hProcess);
        if process.0.is_invalid() || WaitForSingleObject(process.0, 120_000) != WAIT_OBJECT_0 {
            bail!("桌面代理授权尚未完成，请完成系统提示后重试");
        }
        let mut code = 1;
        GetExitCodeProcess(process.0, &mut code)?;
        if code != 0 {
            bail!("桌面代理授权未完成，请确认使用同一 Windows 账户并重试");
        }
    }
    Ok(())
}
fn remove_owned_proxy(key: &RegKey, owned_url: &str) -> std::io::Result<()> {
    if key
        .get_value::<String, _>(PROXY)
        .is_ok_and(|v| v == owned_url)
    {
        key.delete_value(PROXY)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::types::ToRegValue;
    #[test]
    fn native_process_snapshot_finds_current_process() {
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap();
        assert!(named_process_ids(name)
            .unwrap()
            .contains(&std::process::id()));
        assert!(named_process_ids("ClaudeGuard-nonexistent-test.exe")
            .unwrap()
            .is_empty());
    }
    #[test]
    fn blocked_start_watch_preserves_existing_sessions_and_recovers() {
        let mut watch = StartWatch {
            was_running: true,
            live: vec![],
        };
        assert!(!watch.observe(true, false)); // existing session, network fails
        assert!(!watch.observe(false, false)); // user exits
        assert!(watch.observe(true, false)); // fresh blocked start
        assert!(watch.observe(true, false)); // late child from that start
        assert!(!watch.observe(false, false));
        assert!(!watch.observe(true, true)); // permitted start
        assert!(!watch.observe(true, false)); // preserve it on a later fault
    }
    #[test]
    fn elevated_helper_rejects_another_identity_before_changes() {
        assert!(elevated_policy(true, 17890, "S-1-5-invalid-test").is_err());
        assert!(elevated_policy(false, 80, "S-1-5-invalid-test").is_err());
    }
    #[test]
    fn uninstall_preserves_other_values_and_later_user_edits() {
        let current = RegKey::predef(HKEY_CURRENT_USER);
        let path = format!(r"Software\ClaudeGuardTests\{}", uuid::Uuid::new_v4());
        let (key, _) = current.create_subkey(&path).unwrap();
        key.set_value("disableAutoUpdates", &1u32).unwrap();
        key.set_value(PROXY, &proxy_url(17890)).unwrap();
        remove_owned_proxy(&key, &proxy_url(17890)).unwrap();
        assert!(key.get_raw_value(PROXY).is_err());
        assert_eq!(key.get_value::<u32, _>("disableAutoUpdates").unwrap(), 1);
        key.set_value(PROXY, &"http://new-user-proxy:8080").unwrap();
        remove_owned_proxy(&key, &proxy_url(17890)).unwrap();
        assert_eq!(
            key.get_value::<String, _>(PROXY).unwrap(),
            "http://new-user-proxy:8080"
        );
        drop(key);
        current.delete_subkey(&path).unwrap();
    }
    #[test]
    fn old_desktop_versions_rejected() {
        assert!(!supported_version([1, 44120, 9, 0]));
        assert!(supported_version([1, 44121, 1, 0]));
        assert!(supported_version([2, 37337, 1, 0]));
    }
    #[test]
    fn any_machine_policy_masks_user() {
        let user = vec![(PROXY.into(), proxy_url(17890).to_reg_value())];
        for name in ["disableAutoUpdates", ""] {
            assert!(validate_policy(
                &[(name.into(), 0u32.to_reg_value())],
                &user,
                &proxy_url(17890)
            )
            .is_err());
        }
    }
    #[test]
    fn pac_never_accepted_as_fixed_proxy() {
        assert!(
            validate_policy(&[], &[(PAC.into(), "".to_reg_value())], &proxy_url(17890)).is_err()
        );
    }
    #[test]
    fn other_proxy_not_overwritten() {
        assert!(validate_policy(
            &[],
            &[(PROXY.into(), "http://127.0.0.1:7897".to_reg_value())],
            &proxy_url(17890)
        )
        .is_err());
        assert!(validate_policy(
            &[],
            &[(PROXY.into(), proxy_url(17890).to_reg_value())],
            &proxy_url(17890)
        )
        .unwrap());
        assert!(!validate_policy(&[], &[], &proxy_url(17890)).unwrap());
    }
    #[test]
    fn preexisting_process_requires_restart() {
        let r = Registration {
            proxy_url: proxy_url(17890),
            enabled_at: 100,
        };
        assert!(needs_restart(Some(&r), &[(1, 90), (2, 110)]));
        assert!(!needs_restart(Some(&r), &[(2, 110)]));
        assert!(needs_restart(None, &[(2, 110)]));
        assert!(!needs_restart(None, &[]));
    }
    #[test]
    fn code_managed_bypass_rejected() {
        assert!(managed_env_conflict(
            &serde_json::json!({"env":{"NO_PROXY":"*"}}),
            &proxy_url(17890)
        ));
        assert!(managed_env_conflict(
            &serde_json::json!({"env":{"https_proxy":"http://localhost:7897"}}),
            &proxy_url(17890)
        ));
        assert!(!managed_env_conflict(
            &serde_json::json!({"env":{"HTTPS_PROXY":proxy_url(17890),"NO_PROXY":"localhost,127.0.0.1,::1,.local"}}),
            &proxy_url(17890)
        ));
    }
}
