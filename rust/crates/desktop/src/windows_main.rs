
mod accessibility;
mod alerts;
mod icons;
mod render;
mod ui;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
pub const WM_ACTION: u32 = WM_APP + 1;
pub const WM_FOCUS_ITEM: u32 = WM_APP + 2;
pub const WM_COLLAPSE: u32 = WM_APP + 3;
struct Instance(windows::Win32::Foundation::HANDLE);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn main() {
    if let Err(error) = run() {
        guard_protocol::log(&format!("桌面启动失败：{error}"));
        if std::env::args().any(|a| a == "--autostart") {
            std::process::exit(1);
        }
        unsafe {
            use windows::{
                core::PCWSTR,
                Win32::{Foundation::HWND, UI::WindowsAndMessaging::*},
            };
            MessageBoxW(
                HWND::default(),
                PCWSTR(guard_protocol::wide(error.to_string()).as_ptr()),
                PCWSTR(guard_protocol::wide("Melian").as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }
}
fn run() -> anyhow::Result<()> {
    guard_platform::native_process::signal_started()?;
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    // Descendants can inherit registry virtualization without package identity.
    if !args.iter().any(|arg| arg == "--native-context") {
        args.push("--native-context".into());
        let mut child = guard_platform::native_process::spawn(&std::env::current_exe()?, &args)?;
        if args.iter().any(|a| a == "--health-check") {
            std::process::exit(child.wait()? as i32);
        }
        return Ok(());
    }
    use guard_platform::{ipc, network::NetworkMonitor, Host};
    use guard_protocol::{Request, Settings};
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let _enter = runtime.enter();
    if std::env::args().any(|a| a == "--health-check") {
        let response = runtime.block_on(ipc::send(Request::action("status")))?;
        std::process::exit(if response.ok { 0 } else { 2 });
    }
    // Probe the compatible pipe before creating a second window.
    let preview = std::env::args().any(|a| a == "--preview");
    let _instance = if !preview {
        unsafe {
            use windows::{
                core::PCWSTR,
                Win32::{
                    Foundation::{GetLastError, ERROR_ALREADY_EXISTS},
                    System::Threading::CreateMutexW,
                },
            };
            let name =
                guard_protocol::wide(format!("Local\\{}-instance", guard_protocol::pipe_name()));
            let handle = CreateMutexW(None, false, PCWSTR(name.as_ptr()))?;
            let existing = GetLastError() == ERROR_ALREADY_EXISTS;
            let instance = Instance(handle);
            if existing {
                if std::env::args().any(|a| a == "--autostart") {
                    return Ok(());
                }
                for _ in 0..20 {
                    if runtime.block_on(ipc::send(Request::action("show"))).is_ok() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                return Ok(());
            }
            Some(instance)
        }
    } else {
        None
    };
    if !preview && runtime.block_on(ipc::send(Request::action("show"))).is_ok() {
        return Ok(());
    }
    let mut settings = Settings::load();
    if guard_protocol::desktop_exe().exists() {
        settings.auto_start = guard_platform::integration::auto_start();
    }
    let engine = if preview {
        settings.proxy_port = 0;
        settings.island_left = Some(700.);
        settings.island_top = Some(120.);
        guard_core::Engine::with_services(
            settings.clone(),
            std::sync::Arc::new(guard_core::controller::Mihomo::new(
                &settings.mihomo_pipe_name,
            )?),
            std::sync::Arc::new(guard_core::controller::HttpCountryProbe),
            false,
        )
    } else {
        guard_core::Engine::new(settings)?
    };
    let (sender, events) = tokio::sync::mpsc::unbounded_channel();
    let host = Host::new(engine.clone(), sender);
    if !preview {
        host.start()?;
    }
    // A port conflict remains visible as a blocking state.
    if engine.start().is_err() {
        guard_protocol::log("守卫代理未启动：端口不可用");
    }
    let _network = NetworkMonitor::new(engine.clone())?;
    let result = ui::run(host, events);
    engine.shutdown();
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
