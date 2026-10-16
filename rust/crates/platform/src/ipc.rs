use anyhow::{bail, Result};
use guard_protocol::{pipe_path, wide, Request, Response, MAX_WIRE_BYTES};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions},
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

pub async fn read<T: DeserializeOwned>(stream: &mut (impl AsyncRead + Unpin)) -> Result<Option<T>> {
    let mut data = Vec::new();
    let mut byte = [0];
    loop {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            return if data.is_empty() {
                Ok(None)
            } else {
                Ok(Some(serde_json::from_slice(&data)?))
            };
        }
        if byte[0] == 10 {
            return Ok(Some(serde_json::from_slice(&data)?));
        }
        data.push(byte[0]);
        if data.len() >= MAX_WIRE_BYTES {
            bail!("本机消息超过长度限制");
        }
    }
}
pub async fn write<T: Serialize>(stream: &mut (impl AsyncWrite + Unpin), value: &T) -> Result<()> {
    let mut data = serde_json::to_vec(value)?;
    if data.len() >= MAX_WIRE_BYTES {
        bail!("本机消息超过长度限制");
    }
    data.push(10);
    stream.write_all(&data).await?;
    stream.flush().await?;
    Ok(())
}
pub async fn send(request: Request) -> Result<Response> {
    let duration = if request.action == "launchDesktop" {
        180
    } else if request.action == "switchPreferred" {
        15
    } else {
        3
    };
    tokio::time::timeout(Duration::from_secs(duration), async {
        let mut pipe = loop {
            match ClientOptions::new().open(pipe_path()) {
                Ok(pipe) => break pipe,
                Err(e) if e.raw_os_error() == Some(231) => {
                    tokio::time::sleep(Duration::from_millis(25)).await
                }
                Err(e) => return Err(e.into()),
            }
        };
        write(&mut pipe, &request).await?;
        read(&mut pipe)
            .await?
            .ok_or_else(|| anyhow::anyhow!("守卫未返回响应"))
    })
    .await?
}
pub fn current_sid() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut size = 0;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        // Allocate pointer-aligned storage for TOKEN_USER.
        let mut data = vec![
            0usize;
            (size as usize + std::mem::size_of::<usize>() - 1)
                / std::mem::size_of::<usize>()
        ];
        let result = GetTokenInformation(
            token,
            TokenUser,
            Some(data.as_mut_ptr().cast()),
            size,
            &mut size,
        );
        let _ = CloseHandle(token);
        result?;
        let user = &*(data.as_ptr().cast::<TOKEN_USER>());
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid)?;
        let text = sid.to_string()?;
        LocalFree(HLOCAL(sid.0.cast()));
        Ok(text)
    }
}
pub fn create_server(path: &str, first: bool) -> Result<NamedPipeServer> {
    unsafe {
        let sddl = wide(format!("D:P(A;;GA;;;{})", current_sid()?));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let result = ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(32)
            .create_with_security_attributes_raw(
                path,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            );
        LocalFree(HLOCAL(descriptor.0));
        Ok(result?)
    }
}
