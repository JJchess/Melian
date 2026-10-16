#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
include!("windows_main.rs");
#[cfg(target_os = "macos")]
mod alerts;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = macos::run() {
        guard_protocol::log(&format!("启动失败：{error}"));
        eprintln!("Melian: {error}");
        std::process::exit(1);
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("Melian supports Windows and macOS.");
}
