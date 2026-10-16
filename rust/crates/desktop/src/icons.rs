use anyhow::Result;
use windows::{
    core::PCWSTR,
    Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{LoadImageW, HICON, IMAGE_ICON, LR_SHARED},
    },
};

/// ID 1 is the first icon group embedded by windows_resources.rs.
/// Windows owns LR_SHARED handles; callers must not destroy them.
pub fn app_icon(size: i32) -> Result<HICON> {
    unsafe {
        let handle = LoadImageW(
            GetModuleHandleW(None)?,
            PCWSTR(1usize as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_SHARED,
        )?;
        Ok(HICON(handle.0))
    }
}
