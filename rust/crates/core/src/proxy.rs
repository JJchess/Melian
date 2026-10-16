use crate::gate::{Gate, Permit};
use anyhow::{bail, Context, Result};
use std::{
    net::{Ipv4Addr, SocketAddr},
    os::windows::io::AsRawSocket,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

pub struct Proxy {
    listener: TcpListener,
    gate: Arc<Gate>,
    stop: CancellationToken,
}
impl Proxy {
    pub fn bind(port: u16, gate: Arc<Gate>, stop: CancellationToken) -> Result<Self> {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        use windows::Win32::Networking::WinSock::{
            setsockopt, SOCKET, SOL_SOCKET, SO_EXCLUSIVEADDRUSE,
        };
        unsafe {
            let value = 1i32.to_ne_bytes();
            if setsockopt(
                SOCKET(socket.as_raw_socket() as usize),
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                Some(&value),
            ) != 0
            {
                bail!("无法独占代理端口");
            }
        }
        socket.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())?;
        socket.listen(128)?;
        socket.set_nonblocking(true)?;
        let listener = TcpListener::from_std(socket.into())?;
        Ok(Self {
            listener,
            gate,
            stop,
        })
    }
    pub fn port(&self) -> u16 {
        self.listener.local_addr().unwrap().port()
    }
    pub async fn run(self) {
        loop {
            tokio::select! {
                _=self.stop.cancelled()=>break,
                next=self.listener.accept()=>match next {
                    Ok((client,_))=>{
                        let gate=self.gate.clone(); let stop=self.stop.clone();
                        tokio::spawn(async move { tokio::select! { _=stop.cancelled()=>{}, _=serve(client,gate)=>{} } });
                    },
                    Err(_)=>{ self.gate.revoke(); break; }
                }
            }
        }
    }
}
async fn header(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 2048];
    loop {
        let n = stream.read(&mut buffer).await?;
        if n == 0 {
            bail!("连接已关闭");
        }
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            if index + 4 > 16384 {
                bail!("HTTP 标头过长");
            }
            let extra = bytes.split_off(index + 4);
            return Ok((bytes, extra));
        }
        if bytes.len() > 16384 {
            bail!("HTTP 标头过长");
        }
    }
}
pub fn sanitize_header(bytes: &[u8]) -> Result<(bool, Vec<u8>)> {
    let text = std::str::from_utf8(bytes)?;
    let mut lines = text.split("\r\n");
    let first = lines.next().context("缺少请求行")?;
    let parts: Vec<_> = first.split(' ').collect();
    if parts.len() != 3
        || !matches!(parts[2], "HTTP/1.0" | "HTTP/1.1")
        || parts[0].is_empty()
        || !parts[0]
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b == b'-')
    {
        bail!("无效请求行");
    }
    let connect = parts[0] == "CONNECT";
    if connect {
        let authority = parts[1].parse::<hyper::http::uri::Authority>()?;
        if authority.host().is_empty()
            || authority.port_u16().is_none()
            || parts[1].contains(['@', '/', '?', '#', '\\'])
        {
            bail!("无效 CONNECT");
        }
    } else {
        let uri = parts[1].parse::<hyper::Uri>()?;
        if uri.scheme_str() != Some("http")
            || uri.host().is_none()
            || parts[1].contains(['@', '#', '\\'])
        {
            bail!("仅支持绝对 HTTP 代理请求");
        }
    }
    let mut sanitized = format!("{first}\r\n");
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').context("无效标头")?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_|~".contains(&b) || b == 96)
            || value.bytes().any(|b| (b < 32 && b != 9) || b == 127)
        {
            bail!("无效标头");
        }
        if !name.eq_ignore_ascii_case("proxy-authorization")
            && !name.eq_ignore_ascii_case("proxy-connection")
        {
            sanitized.push_str(line);
            sanitized.push_str("\r\n");
        }
    }
    sanitized.push_str("\r\n");
    Ok((connect, sanitized.into_bytes()))
}
async fn serve(mut client: TcpStream, gate: Arc<Gate>) {
    let Some(permit) = gate.permit() else {
        let _=client.write_all(b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").await;
        return;
    };
    let Ok((tracked, client_handle)) = track_socket(client, &gate, &permit) else {
        return;
    };
    client = tracked;
    let _client_handle = client_handle;
    let revoked = permit.revoked.clone();
    tokio::select! {
        _=revoked.cancelled()=>{},
        _=async {
            let handshake=tokio::time::timeout(Duration::from_secs(10),handshake(&mut client,&gate,&permit)).await;
            if let Ok(Ok((mut upstream,_upstream_handle)))=handshake {
                if gate.current(&permit) { let _=tokio::io::copy_bidirectional(&mut client,&mut upstream).await; }
            } else { let _=client.write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").await; }
        }=>{}
    }
}
fn track_socket(
    stream: TcpStream,
    gate: &Gate,
    permit: &Permit,
) -> Result<(TcpStream, Arc<std::net::TcpStream>)> {
    let stream = stream.into_std()?;
    let handle = Arc::new(stream.try_clone()?);
    if !gate.track(permit, &handle) {
        bail!("已阻断");
    }
    Ok((TcpStream::from_std(stream)?, handle))
}
async fn handshake(
    client: &mut TcpStream,
    gate: &Gate,
    permit: &Permit,
) -> Result<(TcpStream, Arc<std::net::TcpStream>)> {
    let (bytes, extra) = header(client).await?;
    let (connect, sanitized) = sanitize_header(&bytes)?;
    if !gate.current(permit) {
        bail!("已阻断");
    }
    let upstream = TcpStream::connect((Ipv4Addr::LOCALHOST, permit.port)).await?;
    let (mut upstream, handle) = track_socket(upstream, gate, permit)?;
    if !gate.current(permit) {
        bail!("已阻断");
    }
    upstream.write_all(&sanitized).await?;
    if connect {
        let (response, tail) = header(&mut upstream).await?;
        let text = std::str::from_utf8(&response)?;
        let mut first = text.lines().next().unwrap_or("").split_whitespace();
        let version = first.next().unwrap_or("");
        let status = first.next().unwrap_or("").parse::<u16>().unwrap_or(0);
        if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || !(200..300).contains(&status) {
            bail!("上游拒绝隧道");
        }
        if !gate.current(permit) {
            bail!("已阻断");
        }
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        client.write_all(&tail).await?;
    }
    if !gate.current(permit) {
        bail!("已阻断");
    }
    upstream.write_all(&extra).await?;
    Ok((upstream, handle))
}
