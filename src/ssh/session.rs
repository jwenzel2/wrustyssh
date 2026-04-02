use russh::client;
use russh::{ChannelMsg, Disconnect};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::app::{SshCommand, SshEvent};
use crate::error::AppError;
use crate::models::connection::{AuthMethod, ConnectionProfile};
use crate::ssh::algorithms::preferred_algorithms;
use crate::ssh::handler::ClientHandler;
use crate::ssh::tunnel;
use crate::storage::paths;

pub struct SessionConnection {
    pub handle: client::Handle<ClientHandler>,
    pub cloudflared: Option<Child>,
}

struct ProcessStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl AsyncRead for ProcessStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.stdout) }.poll_read(cx, buf)
    }
}

impl AsyncWrite for ProcessStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.stdin) }.poll_write(cx, data)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.stdin) }.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.stdin) }.poll_shutdown(cx)
    }
}

/// Establish an authenticated SSH session. Returns the session handle.
/// This is shared between terminal sessions and SFTP sessions.
pub async fn establish_session(
    profile: &ConnectionProfile,
    password: Option<&Zeroizing<String>>,
    key_passphrase: Option<&Zeroizing<String>>,
    event_tx: async_channel::Sender<SshEvent>,
) -> Result<SessionConnection, AppError> {
    let config = Arc::new(client::Config {
        preferred: preferred_algorithms(),
        ..Default::default()
    });

    let handler = ClientHandler::new(
        event_tx.clone(),
        profile.hostname.clone(),
        profile.port,
    );

    let (mut session, cloudflared) = if profile.use_cloudflare_tunnel {
        let (stream, child) = connect_via_cloudflare_tunnel(profile)?;
        let session = client::connect_stream(config, stream, handler)
            .await
            .map_err(|e| AppError::Connection(e.to_string()))?;
        (session, Some(child))
    } else {
        let addr = format!("{}:{}", profile.hostname, profile.port);
        let session = client::connect(config, &addr, handler)
            .await
            .map_err(|e| AppError::Connection(e.to_string()))?;
        (session, None)
    };

    // Authenticate
    let authenticated = match profile.auth_method {
        AuthMethod::Password => {
            let pw = password
                .map(|p| p.as_str())
                .ok_or_else(|| AppError::Auth("Password required".into()))?;
            session
                .authenticate_password(&profile.username, pw)
                .await
                .map_err(|e| AppError::Auth(e.to_string()))?
        }
        AuthMethod::PublicKey => {
            let key_id = profile
                .key_pair_id
                .ok_or_else(|| AppError::Auth("No key pair selected".into()))?;
            let key_path = paths::private_key_path(&key_id);
            let key_pass = key_passphrase.map(|s| s.as_str());
            let key_pair = russh_keys::load_secret_key(&key_path, key_pass)
                .map_err(|e| AppError::Auth(e.to_string()))?;
            session
                .authenticate_publickey(&profile.username, Arc::new(key_pair))
                .await
                .map_err(|e| AppError::Auth(e.to_string()))?
        }
        AuthMethod::Both => {
            let key_id = profile
                .key_pair_id
                .ok_or_else(|| AppError::Auth("No key pair selected".into()))?;
            let key_path = paths::private_key_path(&key_id);
            let key_pass = key_passphrase.map(|s| s.as_str());
            let key_pair = russh_keys::load_secret_key(&key_path, key_pass)
                .map_err(|e| AppError::Auth(e.to_string()))?;
            let pk_ok = session
                .authenticate_publickey(&profile.username, Arc::new(key_pair))
                .await
                .map_err(|e| AppError::Auth(e.to_string()))?;

            if !pk_ok {
                let pw = password
                    .map(|p| p.as_str())
                    .ok_or_else(|| AppError::Auth("Password required for fallback".into()))?;
                session
                    .authenticate_password(&profile.username, pw)
                    .await
                    .map_err(|e| AppError::Auth(e.to_string()))?
            } else {
                true
            }
        }
    };

    if !authenticated {
        return Err(AppError::Auth("Authentication failed".into()));
    }

    Ok(SessionConnection {
        handle: session,
        cloudflared,
    })
}

/// Spawn an SSH session task.  Returns the command sender for controlling the session.
pub fn spawn_session(
    profile: ConnectionProfile,
    password: Option<Zeroizing<String>>,
    key_passphrase: Option<Zeroizing<String>>,
    terminal_type: String,
    initial_cols: u32,
    initial_rows: u32,
    event_tx: async_channel::Sender<SshEvent>,
) -> async_channel::Sender<SshCommand> {
    let (cmd_tx, cmd_rx) = async_channel::bounded::<SshCommand>(256);

    let rt = crate::runtime();
    rt.spawn(async move {
        if let Err(e) = run_session(
            profile,
            password,
            key_passphrase,
            terminal_type,
            initial_cols,
            initial_rows,
            event_tx.clone(),
            cmd_rx,
        )
        .await
        {
            let _ = event_tx.send(SshEvent::Error(e.to_string())).await;
            let _ = event_tx
                .send(SshEvent::Disconnected(Some(e.to_string())))
                .await;
        }
    });

    cmd_tx
}

async fn run_session(
    profile: ConnectionProfile,
    password: Option<Zeroizing<String>>,
    key_passphrase: Option<Zeroizing<String>>,
    terminal_type: String,
    initial_cols: u32,
    initial_rows: u32,
    event_tx: async_channel::Sender<SshEvent>,
    cmd_rx: async_channel::Receiver<SshCommand>,
) -> Result<(), AppError> {
    let session_connection = establish_session(
        &profile,
        password.as_ref(),
        key_passphrase.as_ref(),
        event_tx.clone(),
    )
    .await?;
    let _cloudflared = session_connection.cloudflared;
    let session = session_connection.handle;

    let _ = event_tx.send(SshEvent::Connected).await;

    // Open a session channel with a PTY
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| AppError::Connection(e.to_string()))?;

    channel
        .request_pty(
            false,
            &terminal_type,
            initial_cols.max(20),
            initial_rows.max(5),
            0,
            0,
            &[],
        )
        .await
        .map_err(|e| AppError::Connection(e.to_string()))?;

    channel
        .request_shell(false)
        .await
        .map_err(|e| AppError::Connection(e.to_string()))?;

    // Start enabled tunnels
    let session_handle = Arc::new(Mutex::new(session));
    log::info!(
        "Starting {} tunnels for profile '{}'",
        profile.tunnels.len(),
        profile.name
    );
    for tc in &profile.tunnels {
        if tc.enabled {
            log::info!(
                "Starting tunnel '{}': {}:{} -> {}:{}",
                tc.name,
                tc.local_host,
                tc.local_port,
                tc.remote_host,
                tc.remote_port
            );
            tunnel::start_tunnel(session_handle.clone(), tc.clone(), event_tx.clone());
        }
    }

    // Main data loop
    let mut channel = channel;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Ok(SshCommand::SendData(data)) => {
                        channel.data(&data[..]).await
                            .map_err(|e| AppError::Connection(e.to_string()))?;
                    }
                    Ok(SshCommand::Resize { cols, rows }) => {
                        channel.window_change(cols, rows, 0, 0).await
                            .map_err(|e| AppError::Connection(e.to_string()))?;
                    }
                    Ok(SshCommand::StartTunnel(tc)) => {
                        tunnel::start_tunnel(session_handle.clone(), tc, event_tx.clone());
                    }
                    Ok(SshCommand::StopTunnel(_id)) => {
                        // Tunnel stop is handled via drop of the tunnel task
                    }
                    Ok(SshCommand::Disconnect) | Err(_) => {
                        let _ = channel.eof().await;
                        let sess = session_handle.lock().await;
                        sess.disconnect(Disconnect::ByApplication, "User disconnected", "en")
                            .await
                            .map_err(|e| AppError::Connection(e.to_string()))?;
                        let _ = event_tx.send(SshEvent::Disconnected(None)).await;
                        return Ok(());
                    }
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        let _ = event_tx.send(SshEvent::Data(data.to_vec())).await;
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let _ = event_tx.send(SshEvent::Data(data.to_vec())).await;
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        log::info!("Remote process exited with status {exit_status}");
                    }
                    Some(ChannelMsg::Eof) | None => {
                        let _ = event_tx.send(SshEvent::Disconnected(None)).await;
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }
}

fn connect_via_cloudflare_tunnel(
    profile: &ConnectionProfile,
) -> Result<(ProcessStream, Child), AppError> {
    let cloudflared = resolve_cloudflared_path()?;
    let mut child = Command::new(&cloudflared)
        .arg("access")
        .arg("ssh")
        .arg("--hostname")
        .arg(&profile.hostname)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            AppError::Connection(format!(
                "Failed to start cloudflared for '{}': {e}. Make sure cloudflared is installed and on PATH.",
                profile.name
            ))
        })?;

    let stdin = child.stdin.take().ok_or_else(|| {
        AppError::Connection("cloudflared did not expose stdin for SSH transport".into())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        AppError::Connection("cloudflared did not expose stdout for SSH transport".into())
    })?;

    Ok((ProcessStream { stdin, stdout }, child))
}

fn resolve_cloudflared_path() -> Result<PathBuf, AppError> {
    let path_name = PathBuf::from("cloudflared");
    if command_exists(&path_name) {
        return Ok(path_name);
    }

    let mut candidates = Vec::new();
    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        candidates.push(
            Path::new(&program_files_x86)
                .join("cloudflared")
                .join("cloudflared.exe"),
        );
    }
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(
            Path::new(&program_files)
                .join("cloudflared")
                .join("cloudflared.exe"),
        );
    }

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(AppError::Connection(
        "Failed to start cloudflared: program not found. Make sure cloudflared is installed and on PATH.".into(),
    ))
}

fn command_exists(command: &Path) -> bool {
    if command.components().count() > 1 {
        return command.is_file();
    }

    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path_var).any(|dir| {
        let exe = dir.join(format!("{}.exe", command.display()));
        let bare = dir.join(command);
        exe.is_file() || bare.is_file()
    })
}
