use anyhow::Result;
use guard_protocol::{desktop_exe, install_dir, pipe_name};
use windows::{
    core::{Interface, BSTR, VARIANT},
    Win32::{
        Foundation::VARIANT_BOOL,
        System::{Com::*, TaskScheduler::*},
    },
};

struct Apartment(bool);
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() }
        }
    }
}
pub(crate) struct Scheduler {
    pub(crate) service: ITaskService,
    pub(crate) folder: ITaskFolder,
    _apartment: Apartment,
}
impl Scheduler {
    pub(crate) fn connect() -> Result<Self> {
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            // UI threads already use STA; callers on another COM apartment remain valid.
            if initialized.is_err() && initialized.0 != 0x80010106u32 as i32 {
                initialized.ok()?;
            }
            let apartment = Apartment(initialized.is_ok());
            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let empty = VARIANT::default();
            service.Connect(&empty, &empty, &empty, &empty)?;
            let folder = service.GetFolder(&BSTR::from("\\"))?;
            Ok(Self {
                service,
                folder,
                _apartment: apartment,
            })
        }
    }
}
fn missing(error: &windows::core::Error) -> bool {
    matches!(error.code().0 as u32, 0x80070002 | 0x8004130f)
}
pub fn enabled() -> Result<Option<bool>> {
    let scheduler = Scheduler::connect()?;
    unsafe {
        match scheduler.folder.GetTask(&BSTR::from(pipe_name())) {
            Ok(task) => Ok(Some(task.Enabled()?.as_bool())),
            Err(e) if missing(&e) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
pub fn set_enabled(enabled: bool) -> Result<()> {
    let scheduler = Scheduler::connect()?;
    let name = BSTR::from(pipe_name());
    unsafe {
        if !enabled {
            return match scheduler.folder.DeleteTask(&name, 0) {
                Ok(()) => Ok(()),
                Err(e) if missing(&e) => Ok(()),
                Err(e) => Err(e.into()),
            };
        }
        let sid = BSTR::from(crate::ipc::current_sid()?);
        let task = scheduler.service.NewTask(0)?;
        task.RegistrationInfo()?
            .SetDescription(&BSTR::from("Claude 网络守卫：当前用户登录后启动"))?;
        let principal = task.Principal()?;
        principal.SetUserId(&sid)?;
        principal.SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)?;
        principal.SetRunLevel(TASK_RUNLEVEL_LUA)?;
        let settings = task.Settings()?;
        settings.SetEnabled(VARIANT_BOOL(-1))?;
        settings.SetStartWhenAvailable(VARIANT_BOOL(-1))?;
        settings.SetDisallowStartIfOnBatteries(VARIANT_BOOL(0))?;
        settings.SetStopIfGoingOnBatteries(VARIANT_BOOL(0))?;
        settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;
        settings.SetMultipleInstances(TASK_INSTANCES_IGNORE_NEW)?;
        settings.SetRestartCount(3)?;
        settings.SetRestartInterval(&BSTR::from("PT1M"))?;
        let trigger: ILogonTrigger = task.Triggers()?.Create(TASK_TRIGGER_LOGON)?.cast()?;
        trigger.SetUserId(&sid)?;
        trigger.SetDelay(&BSTR::from("PT10S"))?;
        let action: IExecAction = task.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
        action.SetPath(&BSTR::from(desktop_exe().to_string_lossy().as_ref()))?;
        action.SetArguments(&BSTR::from("--autostart --native-context"))?;
        action.SetWorkingDirectory(&BSTR::from(install_dir().to_string_lossy().as_ref()))?;
        scheduler.folder.RegisterTaskDefinition(
            &name,
            &task,
            TASK_CREATE_OR_UPDATE.0,
            &VARIANT::from(sid.to_string().as_str()),
            &VARIANT::default(),
            TASK_LOGON_INTERACTIVE_TOKEN,
            &VARIANT::default(),
        )?;
    }
    Ok(())
}
