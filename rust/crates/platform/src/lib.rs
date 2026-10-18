#[cfg(windows)]
pub mod browser;
#[cfg(windows)]
pub mod browser_pac;
pub mod codex;
#[cfg(windows)]
pub mod desktop;
#[cfg(windows)]
pub mod host;
#[cfg(windows)]
pub mod integration;
#[cfg(windows)]
pub mod ipc;
#[cfg(windows)]
pub mod launch;
#[cfg(windows)]
pub mod native_process;
#[cfg(windows)]
pub mod network;
#[cfg(windows)]
pub mod startup;
#[cfg(windows)]
pub use host::Host;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
