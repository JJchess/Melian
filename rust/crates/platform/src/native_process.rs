//! A one-shot interactive task gives our executable the real user registry
//! view. Package identity / Desktop Bridge flags alone do not reliably remove
//! an inherited registry silo. The temporary task has no trigger and is removed
//! after a current-user-only pipe confirms the child's PID.
use anyhow::{bail, Context, Result};
use std::{
    io::{Read, Write},
    os::windows::io::AsRawHandle,
    path::Path,
    time::Duration,
};
use windows::{
    core::{Interface, BSTR, VARIANT},
    Win32::{
        Foundation::*,
        System::{Pipes::GetNamedPipeClientProcessId, TaskScheduler::*, Threading::*},
        UI::Shell::IsUserAnAdmin,
    },
};

pub struct Child {
    handle: HANDLE,
    pid: u32,
}
impl Drop for Child {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}
impl Child {
    pub fn id(&self) -> u32 {
        self.pid
    }
    pub fn try_wait(&mut self) -> Result<Option<u32>> {
        unsafe {
            match WaitForSingleObject(self.handle, 0) {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 1;
                    GetExitCodeProcess(self.handle, &mut code)?;
                    Ok(Some(code))
                }
                _ => bail!("无法查询子进程状态"),
            }
        }
    }
    pub fn wait(&mut self) -> Result<u32> {
        unsafe {
            if WaitForSingleObject(self.handle, INFINITE) != WAIT_OBJECT_0 {
                bail!("等待子进程失败");
            }
        }
        self.try_wait()?.context("子进程未结束")
    }
}
struct Task {
    scheduler: crate::startup::Scheduler,
    name: BSTR,
}
impl Drop for Task {
    fn drop(&mut self) {
        unsafe {
            let _ = self.scheduler.folder.DeleteTask(&self.name, 0);
        }
    }
}
fn pipe(nonce: &str) -> Result<String> {
    uuid::Uuid::parse_str(nonce)?;
    Ok(format!(
        r"\\.\pipe\{}-launch-{nonce}",
        guard_protocol::pipe_name()
    ))
}
/// Called before UI / installer work. No filesystem paths are accepted.
pub fn signal_started() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(index) = args.iter().position(|arg| arg == "--native-launch") {
        let path = pipe(args.get(index + 1).context("原生启动标识缺失")?)?;
        let mut stream = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        writeln!(stream, "{}", std::process::id())?;
        let mut ack = [0];
        stream.read_exact(&mut ack)?;
        if ack[0] != b'1' {
            bail!("原生启动确认无效");
        }
    }
    Ok(())
}
pub fn forwarded_args(args: &[String]) -> Vec<String> {
    let mut result = vec![];
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--native-launch" {
            iter.next();
        } else {
            result.push(arg.clone());
        }
    }
    result
}
pub fn spawn(path: &Path, args: &[String]) -> Result<Child> {
    let nonce = uuid::Uuid::new_v4().to_string();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let server = {
        let _enter = runtime.enter();
        crate::ipc::create_server(&pipe(&nonce)?, true)?
    };
    let scheduler = crate::startup::Scheduler::connect()?;
    let name = BSTR::from(format!("{}-launch-{nonce}", guard_protocol::pipe_name()));
    let task = Task { scheduler, name };
    unsafe {
        let sid = BSTR::from(crate::ipc::current_sid()?);
        let definition = task.scheduler.service.NewTask(0)?;
        let principal = definition.Principal()?;
        principal.SetUserId(&sid)?;
        principal.SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)?;
        // Retain the caller's existing level. This never requests elevation;
        // proxy-policy changes still go through the explicit Windows UAC UI.
        let policy_helper = args.iter().any(|a| {
            matches!(
                a.as_str(),
                "--desktop-proxy-enable" | "--desktop-proxy-remove"
            )
        });
        principal.SetRunLevel(if policy_helper && IsUserAnAdmin().as_bool() {
            TASK_RUNLEVEL_HIGHEST
        } else {
            TASK_RUNLEVEL_LUA
        })?;
        let settings = definition.Settings()?;
        settings.SetEnabled(VARIANT_BOOL(-1))?;
        settings.SetAllowDemandStart(VARIANT_BOOL(-1))?;
        settings.SetDisallowStartIfOnBatteries(VARIANT_BOOL(0))?;
        settings.SetStopIfGoingOnBatteries(VARIANT_BOOL(0))?;
        settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;
        let action: IExecAction = definition.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
        action.SetPath(&BSTR::from(path.to_string_lossy().as_ref()))?;
        let mut args = forwarded_args(args);
        args.extend(["--native-launch".into(), nonce]);
        action.SetArguments(&BSTR::from(
            args.iter()
                .map(|arg| crate::launch::quote_arg(arg))
                .collect::<Vec<_>>()
                .join(" "),
        ))?;
        action.SetWorkingDirectory(&BSTR::from(
            path.parent()
                .context("启动路径无效")?
                .to_string_lossy()
                .as_ref(),
        ))?;
        let registered = task.scheduler.folder.RegisterTaskDefinition(
            &task.name,
            &definition,
            TASK_CREATE.0,
            &VARIANT::from(sid.to_string().as_str()),
            &VARIANT::default(),
            TASK_LOGON_INTERACTIVE_TOKEN,
            &VARIANT::default(),
        )?;
        registered.Run(&VARIANT::default())?;
    }
    runtime.block_on(async move {
        tokio::time::timeout(Duration::from_secs(20), async move {
            server.connect().await?;
            let mut server = server;
            let pid = crate::ipc::read::<u32>(&mut server)
                .await?
                .context("原生启动确认丢失")?;
            let mut actual_pid = 0;
            unsafe {
                GetNamedPipeClientProcessId(HANDLE(server.as_raw_handle()), &mut actual_pid)?;
            }
            if actual_pid != pid {
                bail!("原生启动身份不匹配");
            }
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )?
            };
            let child = Child { handle, pid };
            crate::ipc::write(&mut server, &1u32).await?;
            Ok::<_, anyhow::Error>(child)
        })
        .await
        .context("原生启动确认超时")?
    })
}
