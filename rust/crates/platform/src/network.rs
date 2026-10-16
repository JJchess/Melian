use anyhow::Result;
use guard_core::Engine;
use std::sync::Arc;
use windows::Win32::{
    Foundation::HANDLE,
    NetworkManagement::IpHelper::{
        CancelMibChangeNotify2, NotifyUnicastIpAddressChange, MIB_NOTIFICATION_TYPE,
        MIB_UNICASTIPADDRESS_ROW,
    },
    Networking::WinSock::AF_UNSPEC,
};
pub struct NetworkMonitor {
    handle: HANDLE,
    context: Box<Arc<Engine>>,
}
impl NetworkMonitor {
    pub fn new(engine: Arc<Engine>) -> Result<Self> {
        unsafe {
            let context = Box::new(engine);
            let mut handle = HANDLE::default();
            NotifyUnicastIpAddressChange(
                AF_UNSPEC,
                Some(changed),
                Some((&*context as *const Arc<Engine>).cast()),
                false,
                &mut handle,
            )
            .ok()?;
            Ok(Self { handle, context })
        }
    }
}
unsafe extern "system" fn changed(
    context: *const std::ffi::c_void,
    _row: *const MIB_UNICASTIPADDRESS_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    if !context.is_null() {
        (&*context.cast::<Arc<Engine>>()).invalidate();
    }
}
impl Drop for NetworkMonitor {
    fn drop(&mut self) {
        unsafe {
            let _ = CancelMibChangeNotify2(self.handle);
        }
        let _ = &self.context;
    }
}
