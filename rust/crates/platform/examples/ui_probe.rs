use windows::core::Interface;
use windows::{
    core::w,
    Win32::{
        System::Com::*,
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
};
fn main() -> anyhow::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let hwnd = FindWindowW(w!("ClaudeGuard.Rust.Island"), None)?;
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let element = automation.ElementFromHandle(hwnd)?;
        println!(
            "type={:?}, id={}, help={}, provider={}",
            element.CurrentControlType()?,
            element.CurrentAutomationId()?,
            element.CurrentHelpText()?,
            element.CurrentProviderDescription()?
        );
        if std::env::args().any(|a| a == "--open")
            && element.CurrentControlType()? == UIA_ButtonControlTypeId
        {
            let invoke: IUIAutomationInvokePattern =
                element.GetCurrentPattern(UIA_InvokePatternId)?.cast()?;
            invoke.Invoke()?;
            std::thread::sleep(std::time::Duration::from_millis(800));
        }
        println!("root: {}", element.CurrentName()?);
        let list = element.FindAll(TreeScope_Children, &automation.CreateTrueCondition()?)?;
        for i in 0..list.Length()? {
            let child = list.GetElement(i)?;
            println!(
                "{}: {} ({:?})",
                i,
                child.CurrentName()?,
                child.CurrentControlType()?
            );
        }
        Ok(())
    }
}
