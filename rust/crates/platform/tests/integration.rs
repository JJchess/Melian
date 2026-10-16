#![cfg(windows)]
use guard_platform::{host::Sessions, integration, ipc, launch};
use guard_protocol::{pipe_name, Request, State, EXTENSION_ID};
use std::{ffi::OsStr, path::Path};
use tokio::io::AsyncWriteExt;
fn args() -> Vec<String> {
    [
        "--resume",
        "a b",
        "literal\"quote",
        "$(not-a-command)",
        "中文",
        "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
fn command() -> std::process::Command {
    launch::claude_command(
        Path::new("C:/fake/claude.exe"),
        Path::new("C:/"),
        &args(),
        17890,
    )
}
#[test]
fn original_arguments() {
    assert_eq!(
        command()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        args()
    );
    assert_eq!(command().get_current_dir(), Some(Path::new("C:/")));
}
#[test]
fn controlled_http_proxy() {
    for name in ["HTTP_PROXY", "HTTPS_PROXY"] {
        assert!(command()
            .get_envs()
            .any(|(k, v)| k == name && v == Some(OsStr::new("http://127.0.0.1:17890"))));
    }
}
#[test]
fn no_wildcard_bypass() {
    assert!(command()
        .get_envs()
        .any(|(k, v)| k == "NO_PROXY" && v == Some(OsStr::new("localhost,127.0.0.1,::1"))));
    assert!(!command()
        .get_envs()
        .any(|(k, v)| k == "ALL_PROXY" && v.is_some()));
}
#[tokio::test]
async fn json_roundtrip() {
    let r = Request {
        cwd: Some("D:\\中文 目录".into()),
        args: Some(vec!["a\nb".into(), "\"x\"".into()]),
        ..Request::action("launch")
    };
    let (mut a, mut b) = tokio::io::duplex(65536);
    ipc::write(&mut a, &r).await.unwrap();
    let read: Request = ipc::read(&mut b).await.unwrap().unwrap();
    assert_eq!(read.cwd, r.cwd);
    assert_eq!(read.args, r.args);
}
#[tokio::test]
async fn wire_limit() {
    let (mut a, mut b) = tokio::io::duplex(70000);
    a.write_all(&vec![b'a'; 65537]).await.unwrap();
    drop(a);
    assert!(ipc::read::<Request>(&mut b).await.is_err());
}
#[tokio::test]
async fn pipe_eof() {
    let (a, mut b) = tokio::io::duplex(16);
    drop(a);
    assert!(ipc::read::<Request>(&mut b).await.unwrap().is_none());
}
#[test]
fn missing_claude_rejected() {
    assert!(launch::resolve_claude(Some("C:/missing-claude-guard-test/claude.exe")).is_err());
}
#[test]
fn fixed_extension_id() {
    assert_eq!(EXTENSION_ID.len(), 32);
    assert!(EXTENSION_ID.bytes().all(|b| (b'a'..=b'p').contains(&b)));
}
#[test]
fn user_pipe_name() {
    let name = pipe_name();
    assert!(name.starts_with("ClaudeGuard-"));
    assert!(!name.contains('\\'));
    assert!(ipc::current_sid().unwrap().starts_with("S-1-"));
}
#[test]
fn system_proxy_is_conservative() {
    let p = integration::system_proxy();
    assert!(!p.enabled || !p.supported || !p.server.is_empty());
}
fn sessions() -> (Sessions, String, String) {
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    let mut s = Sessions::default();
    for (id, protected) in [(&a, true), (&b, false)] {
        s.report(&Request {
            client_id: Some(id.clone()),
            browser: Some("chrome".into()),
            connected: true,
            protected,
            ..Request::action("browserStatus")
        })
        .unwrap();
    }
    (s, a, b)
}
#[test]
fn unprotected_profile_does_not_get_command() {
    let (mut s, _, b) = sessions();
    s.queue_web(true).unwrap();
    assert!(s.take_command(&b, true).is_none());
}
#[test]
fn protected_profile_gets_command() {
    let (mut s, a, _) = sessions();
    s.queue_web(true).unwrap();
    assert_eq!(s.take_command(&a, true).as_deref(), Some("openClaude"));
    let mut state = State::default();
    s.merge(&mut state);
    assert!(state.browsers["chrome"].protected);
}
#[test]
fn command_is_one_shot() {
    let (mut s, a, _) = sessions();
    s.queue_web(true).unwrap();
    s.take_command(&a, true).unwrap();
    assert!(s.take_command(&a, true).is_none());
}
#[test]
fn blocked_launch_rejected() {
    let (mut s, a, _) = sessions();
    assert!(s.queue_web(false).is_err());
    s.queue_web(true).unwrap();
    assert!(s.take_command(&a, false).is_none());
}
#[test]
fn disconnected_session_rejected() {
    let (mut s, a, _) = sessions();
    s.report(&Request {
        client_id: Some(a),
        browser: Some("chrome".into()),
        ..Request::action("browserStatus")
    })
    .unwrap();
    assert!(s.queue_web(true).is_err());
}
#[test]
fn shell_quotes_and_front_only() {
    assert_eq!(launch::quote_arg("a\"b\\"), "\"a\\\"b\\\\\"");
    assert_eq!(launch::quote_arg(""), "\"\"");
    assert!(launch::validate_front_args(&["--bg".into()]).is_err());
    assert!(launch::validate_front_args(&["daemon".into()]).is_err());
}
