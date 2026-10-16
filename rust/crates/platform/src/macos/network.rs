use guard_core::Engine;
use std::{
    ffi::CStr,
    sync::Arc,
    time::{Duration, SystemTime},
};
fn interfaces() -> Vec<u8> {
    let mut bytes = Vec::new();
    unsafe {
        let mut head = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return bytes;
        }
        let mut current = head;
        while !current.is_null() {
            let item = &*current;
            if !item.ifa_addr.is_null() && !item.ifa_name.is_null() {
                let address = &*item.ifa_addr;
                if address.sa_family as i32 == libc::AF_INET
                    || address.sa_family as i32 == libc::AF_INET6
                {
                    bytes.extend_from_slice(CStr::from_ptr(item.ifa_name).to_bytes());
                    bytes.extend_from_slice(&item.ifa_flags.to_ne_bytes());
                    bytes.extend_from_slice(std::slice::from_raw_parts(
                        item.ifa_addr.cast::<u8>(),
                        address.sa_len as usize,
                    ));
                }
            }
            current = item.ifa_next;
        }
        libc::freeifaddrs(head);
    }
    bytes
}
pub fn start(engine: Arc<Engine>) {
    std::thread::spawn(move || {
        let mut last = interfaces();
        let mut wall = SystemTime::now();
        while !engine.stop.is_cancelled() {
            std::thread::sleep(Duration::from_millis(500));
            let current = interfaces();
            let now = SystemTime::now();
            if current != last
                || now
                    .duration_since(wall)
                    .map_or(true, |d| d > Duration::from_secs(2))
            {
                engine.invalidate();
            }
            last = current;
            wall = now;
        }
    });
}
