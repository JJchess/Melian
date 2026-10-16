use anyhow::{bail, Context, Result};
use guard_protocol::{Request, Response, MAX_WIRE_BYTES};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

fn directory() -> PathBuf {
    std::env::temp_dir().join("melian")
}
pub fn socket_path() -> PathBuf {
    directory().join("control.sock")
}
fn private_directory() -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let path = directory();
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.permissions().mode() & 0o077 != 0
    {
        bail!("本机通信目录权限不安全");
    }
    Ok(())
}
pub struct Server {
    pub listener: UnixListener,
    _lock: fs::File,
}
impl Server {
    pub fn bind() -> Result<Self> {
        private_directory()?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory().join("instance.lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("Melian 已在运行");
        }
        let path = socket_path();
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
                bail!("本机通信路径被占用");
            }
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            _lock: lock,
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(socket_path());
    }
}
pub async fn read<T: DeserializeOwned>(stream: &mut (impl AsyncRead + Unpin)) -> Result<Option<T>> {
    let mut data = Vec::new();
    let mut byte = [0];
    loop {
        if stream.read(&mut byte).await? == 0 {
            return if data.is_empty() {
                Ok(None)
            } else {
                bail!("本机消息不完整")
            };
        }
        if byte[0] == b'\n' {
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
    data.push(b'\n');
    stream.write_all(&data).await?;
    stream.flush().await?;
    Ok(())
}
pub fn check_peer(stream: &UnixStream) -> Result<()> {
    if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
        bail!("拒绝其他用户的本机连接");
    }
    Ok(())
}
pub async fn send(request: Request) -> Result<Response> {
    tokio::time::timeout(Duration::from_secs(15), async {
        private_directory()?;
        let mut stream = UnixStream::connect(socket_path())
            .await
            .context("请先打开 Melian")?;
        check_peer(&stream)?;
        write(&mut stream, &request).await?;
        read(&mut stream).await?.context("守卫未返回状态")
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[tokio::test]
    async fn messages_round_trip_with_same_user_credentials() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        check_peer(&client).unwrap();
        check_peer(&server).unwrap();
        let message = json!({"action":"launch", "args":["空格 参数", "\"quoted\"", "$(literal)"]});
        write(&mut client, &message).await.unwrap();
        assert_eq!(read::<Value>(&mut server).await.unwrap(), Some(message));
        client.shutdown().await.unwrap();
        assert!(read::<Value>(&mut server).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn truncated_and_oversized_messages_are_rejected() {
        let mut truncated = b"{\"action\":\"status\"}".as_slice();
        assert!(read::<Value>(&mut truncated).await.is_err());
        let oversized = vec![b' '; MAX_WIRE_BYTES];
        assert!(read::<Value>(&mut oversized.as_slice()).await.is_err());
        assert!(write(&mut tokio::io::sink(), &"x".repeat(MAX_WIRE_BYTES))
            .await
            .is_err());
    }
}
