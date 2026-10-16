use anyhow::{bail, Result};
use guard_protocol::{cli_exe, desktop_exe, EXTENSION_ID};
use std::{mem::size_of, path::Path};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::*,
        System::{Diagnostics::ToolHelp::*, Threading::*},
        UI::Shell::CommandLineToArgvW,
    },
};
#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationProcess(
        handle: HANDLE,
        class: u32,
        data: *mut std::ffi::c_void,
        len: u32,
        needed: *mut u32,
    ) -> i32;
}
#[repr(C)]
struct UnicodeString {
    len: u16,
    max: u16,
    buffer: *const u16,
}
pub fn processes() -> Result<Vec<(u32, String)>> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut values = vec![];
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            if let Ok(process) = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                entry.th32ProcessID,
            ) {
                let mut path = vec![0u16; 32768];
                let mut count = path.len() as u32;
                if QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    PWSTR(path.as_mut_ptr()),
                    &mut count,
                )
                .is_ok()
                {
                    values.push((
                        entry.th32ProcessID,
                        String::from_utf16_lossy(&path[..count as usize]),
                    ));
                }
                let _ = CloseHandle(process);
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        Ok(values)
    }
}
fn command_line(process: HANDLE) -> Result<Vec<String>> {
    unsafe {
        let mut needed = 0;
        NtQueryInformationProcess(process, 60, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 || needed > 131072 {
            bail!("无法读取桥接进程参数");
        }
        let mut bytes = vec![0usize; (needed as usize + 7) / 8];
        let status = NtQueryInformationProcess(
            process,
            60,
            bytes.as_mut_ptr().cast(),
            (bytes.len() * 8) as u32,
            &mut needed,
        );
        if status < 0 {
            bail!("无法验证桥接进程");
        }
        let value = &*bytes.as_ptr().cast::<UnicodeString>();
        let low = bytes.as_ptr() as usize;
        let high = low + bytes.len() * 8;
        if (value.buffer as usize) < low || value.buffer as usize + value.len as usize > high {
            bail!("无效进程参数");
        }
        let mut text = std::slice::from_raw_parts(value.buffer, value.len as usize / 2).to_vec();
        text.push(0);
        let mut count = 0;
        let argv = CommandLineToArgvW(windows::core::PCWSTR(text.as_ptr()), &mut count);
        if argv.is_null() {
            bail!("无法解析进程参数");
        }
        let values = std::slice::from_raw_parts(argv, count as usize)
            .iter()
            .map(|s| s.to_string().unwrap_or_default())
            .collect();
        LocalFree(HLOCAL(argv.cast()));
        Ok(values)
    }
}
pub fn same_path(a: &str, b: &Path) -> bool {
    a.replace('/', "\\")
        .eq_ignore_ascii_case(&b.to_string_lossy().replace('/', "\\"))
}
pub fn stop_owned() -> Result<()> {
    unsafe {
        for (pid, path) in processes()? {
            if !same_path(&path, &desktop_exe()) && !same_path(&path, &cli_exe()) {
                continue;
            }
            let process = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )?;
            let mut actual = vec![0u16; 32768];
            let mut length = actual.len() as u32;
            if QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(actual.as_mut_ptr()),
                &mut length,
            )
            .is_err()
            {
                let _ = CloseHandle(process);
                continue;
            }
            let actual = String::from_utf16_lossy(&actual[..length as usize]);
            if !same_path(&actual, Path::new(&path)) {
                let _ = CloseHandle(process);
                continue;
            }
            let is_desktop = same_path(&actual, &desktop_exe());
            let is_bridge = !is_desktop
                && command_line(process).is_ok_and(|a| {
                    a.get(1) == Some(&format!("chrome-extension://{EXTENSION_ID}/"))
                });
            if is_desktop || is_bridge {
                let result = TerminateProcess(process, 0);
                if result.is_ok() {
                    let _ = WaitForSingleObject(process, 5000);
                }
                let _ = CloseHandle(process);
                result?;
            } else {
                let _ = CloseHandle(process);
            }
        }
        Ok(())
    }
}
pub fn wait_parent(pid: u32) {
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(handle, 15000);
            let _ = CloseHandle(handle);
        }
    }
}
