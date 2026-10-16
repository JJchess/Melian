#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
include!("windows_main.rs");
#[cfg(not(windows))]
fn main() {
    eprintln!("Use install.sh to install the macOS app bundle.");
}
