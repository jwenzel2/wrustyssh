use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

use std::path::{Path, PathBuf};

use crate::app::SshEvent;
use crate::error::AppError;
use crate::models::connection::ConnectionProfile;
use crate::ssh::session::establish_session;

#[derive(Debug)]
pub enum SftpCommand {
    ListDir(String),
    Upload { local: PathBuf, remote: String },
    Download { remote: String, local: PathBuf },
    MkDir(String),
    Remove(String),
    Rename { from: String, to: String },
    Disconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpConflictDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpConflictDecision {
    KeepExisting,
    ReplaceWithIncoming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SftpConflictResponse {
    pub decision: SftpConflictDecision,
    pub apply_to_all: bool,
}

#[derive(Debug, Clone)]
pub enum SftpEvent {
    Connected,
    DirListing {
        path: String,
        entries: Vec<SftpEntry>,
    },
    TransferProgress {
        name: String,
        bytes: u64,
        total: u64,
    },
    TransferComplete {
        name: String,
    },
    TransferConflict {
        path: String,
        direction: SftpConflictDirection,
        is_dir: bool,
        response_tx: async_channel::Sender<SftpConflictResponse>,
    },
    Error(String),
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Spawn an SFTP session task. Returns the command sender.
pub fn spawn_sftp_session(
    profile: ConnectionProfile,
    password: Option<Zeroizing<String>>,
    key_passphrase: Option<Zeroizing<String>>,
    event_tx: async_channel::Sender<SftpEvent>,
) -> async_channel::Sender<SftpCommand> {
    let (cmd_tx, cmd_rx) = async_channel::bounded::<SftpCommand>(64);

    let rt = crate::runtime();
    rt.spawn(async move {
        if let Err(e) =
            run_sftp_session(profile, password, key_passphrase, event_tx.clone(), cmd_rx).await
        {
            let _ = event_tx.send(SftpEvent::Error(e.to_string())).await;
            let _ = event_tx.send(SftpEvent::Disconnected).await;
        }
    });

    cmd_tx
}

async fn run_sftp_session(
    profile: ConnectionProfile,
    password: Option<Zeroizing<String>>,
    key_passphrase: Option<Zeroizing<String>>,
    event_tx: async_channel::Sender<SftpEvent>,
    cmd_rx: async_channel::Receiver<SftpCommand>,
) -> Result<(), AppError> {
    // We need a separate event channel for the SSH layer (we ignore its events)
    let (ssh_event_tx, _ssh_event_rx) = async_channel::bounded::<SshEvent>(16);

    let session_connection = establish_session(
        &profile,
        password.as_ref(),
        key_passphrase.as_ref(),
        ssh_event_tx,
    )
    .await?;
    let _cloudflared = session_connection.cloudflared;
    let session = session_connection.handle;

    // Open SFTP subsystem
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| AppError::Connection(format!("Failed to open channel: {e}")))?;

    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| AppError::Connection(format!("Failed to request SFTP subsystem: {e}")))?;

    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| AppError::Connection(format!("Failed to initialize SFTP session: {e}")))?;

    let _ = event_tx.send(SftpEvent::Connected).await;

    // Command loop
    while let Ok(cmd) = cmd_rx.recv().await {
        match cmd {
            SftpCommand::ListDir(path) => match sftp.read_dir(&path).await {
                Ok(entries) => {
                    let mut listing = Vec::new();
                    for entry in entries {
                        let name = entry.file_name();
                        if name == "." || name == ".." {
                            continue;
                        }
                        let metadata = entry.metadata();
                        listing.push(SftpEntry {
                            name,
                            is_dir: metadata.is_dir(),
                            size: metadata.size.unwrap_or(0),
                            modified: metadata.mtime.map(|t| t as u64),
                        });
                    }
                    listing.sort_by(|a, b| {
                        b.is_dir
                            .cmp(&a.is_dir)
                            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    });
                    let _ = event_tx
                        .send(SftpEvent::DirListing {
                            path,
                            entries: listing,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(SftpEvent::Error(format!("Failed to list {path}: {e}")))
                        .await;
                }
            },
            SftpCommand::Upload { local, remote } => {
                let mut conflict_policy = ConflictPolicy::default();
                if let Err(msg) =
                    upload_entry_recursive(&sftp, &event_tx, local, remote, &mut conflict_policy)
                        .await
                {
                    let _ = event_tx.send(SftpEvent::Error(msg)).await;
                }
            }
            SftpCommand::Download { remote, local } => {
                let mut conflict_policy = ConflictPolicy::default();
                if let Err(msg) =
                    download_entry_recursive(&sftp, &event_tx, remote, local, &mut conflict_policy)
                        .await
                {
                    let _ = event_tx.send(SftpEvent::Error(msg)).await;
                }
            }
            SftpCommand::MkDir(path) => {
                if let Err(msg) = ensure_remote_dir(&sftp, &path).await {
                    let _ = event_tx.send(SftpEvent::Error(msg)).await;
                }
            }
            SftpCommand::Remove(path) => {
                if let Err(msg) = remove_remote_entry_recursive(&sftp, &path).await {
                    let _ = event_tx.send(SftpEvent::Error(msg)).await;
                }
            }
            SftpCommand::Rename { from, to } => {
                if let Err(e) = sftp.rename(&from, &to).await {
                    let _ = event_tx
                        .send(SftpEvent::Error(format!(
                            "Failed to rename {from} -> {to}: {e}"
                        )))
                        .await;
                }
            }
            SftpCommand::Disconnect => {
                let _ = event_tx.send(SftpEvent::Disconnected).await;
                return Ok(());
            }
        }
    }

    let _ = event_tx.send(SftpEvent::Disconnected).await;
    Ok(())
}

fn remote_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

fn join_remote_path(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{name}")
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

fn join_remote_with_relative(base: &str, relative: &Path) -> String {
    let mut current = base.to_string();
    for component in relative.components() {
        if let std::path::Component::Normal(segment) = component {
            current = join_remote_path(&current, &segment.to_string_lossy());
        }
    }
    current
}

#[derive(Default)]
struct ConflictPolicy {
    apply_all: Option<SftpConflictDecision>,
}

async fn ask_transfer_conflict(
    event_tx: &async_channel::Sender<SftpEvent>,
    path: &str,
    direction: SftpConflictDirection,
    is_dir: bool,
    conflict_policy: &mut ConflictPolicy,
) -> Result<SftpConflictDecision, String> {
    if let Some(decision) = conflict_policy.apply_all {
        return Ok(decision);
    }

    let (response_tx, response_rx) = async_channel::bounded::<SftpConflictResponse>(1);
    event_tx
        .send(SftpEvent::TransferConflict {
            path: path.to_string(),
            direction,
            is_dir,
            response_tx,
        })
        .await
        .map_err(|e| format!("Failed to request conflict resolution for {path}: {e}"))?;

    response_rx
        .recv()
        .await
        .map_err(|e| format!("Conflict resolution canceled for {path}: {e}"))
        .map(|response| {
            if response.apply_to_all {
                conflict_policy.apply_all = Some(response.decision);
            }
            response.decision
        })
}

async fn ensure_remote_dir(sftp: &SftpSession, path: &str) -> Result<(), String> {
    let normalized = path.trim_end_matches('/');
    if normalized.is_empty() || normalized == "." || normalized == "/" {
        return Ok(());
    }

    let mut current = if normalized.starts_with('/') {
        "/".to_string()
    } else {
        ".".to_string()
    };

    for segment in normalized
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
    {
        current = join_remote_path(&current, segment);
        match sftp.metadata(&current).await {
            Ok(metadata) => {
                if metadata.is_dir() {
                    continue;
                }

                sftp.remove_file(&current)
                    .await
                    .map_err(|e| format!("Failed to replace non-directory {current}: {e}"))?;
                sftp.create_dir(&current)
                    .await
                    .map_err(|e| format!("Failed to create directory {current}: {e}"))?;
            }
            Err(_) => {
                sftp.create_dir(&current)
                    .await
                    .map_err(|e| format!("Failed to create directory {current}: {e}"))?;
            }
        }
    }

    Ok(())
}

async fn upload_file(
    sftp: &SftpSession,
    event_tx: &async_channel::Sender<SftpEvent>,
    local_file: &Path,
    remote_file: &str,
    conflict_policy: &mut ConflictPolicy,
) -> Result<(), String> {
    if let Ok(existing) = sftp.metadata(remote_file).await {
        match ask_transfer_conflict(
            event_tx,
            remote_file,
            SftpConflictDirection::Upload,
            existing.is_dir(),
            conflict_policy,
        )
        .await?
        {
            SftpConflictDecision::KeepExisting => return Ok(()),
            SftpConflictDecision::ReplaceWithIncoming => {
                remove_remote_entry_recursive(sftp, remote_file).await?;
            }
        }
    }

    let display_name = local_file
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| remote_basename(remote_file));

    let data = tokio::fs::read(local_file)
        .await
        .map_err(|e| format!("Failed to read local file {}: {e}", local_file.display()))?;

    let total = data.len() as u64;
    let _ = event_tx
        .send(SftpEvent::TransferProgress {
            name: display_name.clone(),
            bytes: 0,
            total,
        })
        .await;

    let mut file = sftp
        .open_with_flags(
            remote_file,
            russh_sftp::protocol::OpenFlags::CREATE
                | russh_sftp::protocol::OpenFlags::TRUNCATE
                | russh_sftp::protocol::OpenFlags::WRITE,
        )
        .await
        .map_err(|e| format!("Failed to open remote file {remote_file}: {e}"))?;

    let chunk_size = 32768;
    let mut written = 0u64;
    for chunk in data.chunks(chunk_size) {
        file.write_all(chunk)
            .await
            .map_err(|e| format!("Upload failed for {display_name}: {e}"))?;
        written += chunk.len() as u64;
        let _ = event_tx
            .send(SftpEvent::TransferProgress {
                name: display_name.clone(),
                bytes: written,
                total,
            })
            .await;
    }

    file.shutdown()
        .await
        .map_err(|e| format!("Finalizing upload failed for {display_name}: {e}"))?;

    let _ = event_tx
        .send(SftpEvent::TransferComplete { name: display_name })
        .await;

    Ok(())
}

async fn upload_entry_recursive(
    sftp: &SftpSession,
    event_tx: &async_channel::Sender<SftpEvent>,
    local: PathBuf,
    remote: String,
    conflict_policy: &mut ConflictPolicy,
) -> Result<(), String> {
    let metadata = tokio::fs::metadata(&local)
        .await
        .map_err(|e| format!("Failed to read local path {}: {e}", local.display()))?;

    if metadata.is_dir() {
        if let Ok(existing) = sftp.metadata(&remote).await {
            if !existing.is_dir() {
                match ask_transfer_conflict(
                    event_tx,
                    &remote,
                    SftpConflictDirection::Upload,
                    existing.is_dir(),
                    conflict_policy,
                )
                .await?
                {
                    SftpConflictDecision::KeepExisting => return Ok(()),
                    SftpConflictDecision::ReplaceWithIncoming => {
                        remove_remote_entry_recursive(sftp, &remote).await?;
                    }
                }
            }
        }

        ensure_remote_dir(sftp, &remote).await?;

        let mut stack = vec![local.clone()];
        while let Some(local_dir) = stack.pop() {
            let dir_iter = std::fs::read_dir(&local_dir).map_err(|e| {
                format!(
                    "Failed to read local directory {}: {e}",
                    local_dir.display()
                )
            })?;

            for entry in dir_iter {
                let entry = entry.map_err(|e| {
                    format!(
                        "Failed to read directory entry in {}: {e}",
                        local_dir.display()
                    )
                })?;
                let local_entry = entry.path();
                let relative = local_entry.strip_prefix(&local).map_err(|e| {
                    format!(
                        "Failed to compute relative path for {}: {e}",
                        local_entry.display()
                    )
                })?;
                let remote_entry = join_remote_with_relative(&remote, relative);

                let file_type = entry.file_type().map_err(|e| {
                    format!(
                        "Failed to inspect local entry {}: {e}",
                        local_entry.display()
                    )
                })?;

                if file_type.is_dir() {
                    if let Ok(existing) = sftp.metadata(&remote_entry).await {
                        if !existing.is_dir() {
                            match ask_transfer_conflict(
                                event_tx,
                                &remote_entry,
                                SftpConflictDirection::Upload,
                                existing.is_dir(),
                                conflict_policy,
                            )
                            .await?
                            {
                                SftpConflictDecision::KeepExisting => continue,
                                SftpConflictDecision::ReplaceWithIncoming => {
                                    remove_remote_entry_recursive(sftp, &remote_entry).await?;
                                }
                            }
                        }
                    }
                    ensure_remote_dir(sftp, &remote_entry).await?;
                    stack.push(local_entry);
                } else if file_type.is_file() {
                    upload_file(sftp, event_tx, &local_entry, &remote_entry, conflict_policy)
                        .await?;
                }
            }
        }

        let _ = event_tx
            .send(SftpEvent::TransferComplete {
                name: remote_basename(&remote),
            })
            .await;

        Ok(())
    } else {
        let remote_file = if remote.ends_with('/') {
            let base = remote.trim_end_matches('/');
            let file_name = local
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            join_remote_path(base, &file_name)
        } else {
            remote
        };

        upload_file(sftp, event_tx, &local, &remote_file, conflict_policy).await
    }
}

async fn remove_local_entry_recursive(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    if path.is_dir() {
        tokio::fs::remove_dir_all(path)
            .await
            .map_err(|e| format!("Failed to remove local directory {}: {e}", path.display()))
    } else {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| format!("Failed to remove local file {}: {e}", path.display()))
    }
}

async fn download_file_to_local(
    sftp: &SftpSession,
    event_tx: &async_channel::Sender<SftpEvent>,
    remote_file: &str,
    local_file: &Path,
    conflict_policy: &mut ConflictPolicy,
) -> Result<(), String> {
    if local_file.exists() {
        match ask_transfer_conflict(
            event_tx,
            &local_file.display().to_string(),
            SftpConflictDirection::Download,
            local_file.is_dir(),
            conflict_policy,
        )
        .await?
        {
            SftpConflictDecision::KeepExisting => return Ok(()),
            SftpConflictDecision::ReplaceWithIncoming => {
                remove_local_entry_recursive(local_file).await?;
            }
        }
    }

    let display_name = remote_basename(remote_file);

    let total = sftp
        .metadata(remote_file)
        .await
        .ok()
        .and_then(|metadata| metadata.size)
        .unwrap_or(0);

    let _ = event_tx
        .send(SftpEvent::TransferProgress {
            name: display_name.clone(),
            bytes: 0,
            total,
        })
        .await;

    let mut remote_handle = sftp
        .open(remote_file)
        .await
        .map_err(|e| format!("Failed to open remote file {remote_file}: {e}"))?;

    let mut data = Vec::new();
    remote_handle
        .read_to_end(&mut data)
        .await
        .map_err(|e| format!("Failed to read remote file {remote_file}: {e}"))?;

    if let Some(parent) = local_file.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create local directory {}: {e}", parent.display()))?;
    }

    tokio::fs::write(local_file, &data)
        .await
        .map_err(|e| format!("Failed to write {}: {e}", local_file.display()))?;

    let _ = event_tx
        .send(SftpEvent::TransferProgress {
            name: display_name.clone(),
            bytes: data.len() as u64,
            total,
        })
        .await;

    let _ = event_tx
        .send(SftpEvent::TransferComplete { name: display_name })
        .await;

    Ok(())
}

async fn download_entry_recursive(
    sftp: &SftpSession,
    event_tx: &async_channel::Sender<SftpEvent>,
    remote: String,
    local: PathBuf,
    conflict_policy: &mut ConflictPolicy,
) -> Result<(), String> {
    let metadata = sftp
        .metadata(&remote)
        .await
        .map_err(|e| format!("Failed to stat remote path {remote}: {e}"))?;

    if metadata.is_dir() {
        let local_root = if local.is_dir() {
            local.join(remote_basename(&remote))
        } else {
            local
        };

        if local_root.exists() {
            if !local_root.is_dir() {
                match ask_transfer_conflict(
                    event_tx,
                    &local_root.display().to_string(),
                    SftpConflictDirection::Download,
                    local_root.is_dir(),
                    conflict_policy,
                )
                .await?
                {
                    SftpConflictDecision::KeepExisting => return Ok(()),
                    SftpConflictDecision::ReplaceWithIncoming => {
                        remove_local_entry_recursive(&local_root).await?;
                    }
                }
            }
        }

        tokio::fs::create_dir_all(&local_root).await.map_err(|e| {
            format!(
                "Failed to create local directory {}: {e}",
                local_root.display()
            )
        })?;

        let mut stack = vec![(remote.clone(), local_root.clone())];
        while let Some((remote_dir, local_dir)) = stack.pop() {
            let entries = sftp
                .read_dir(&remote_dir)
                .await
                .map_err(|e| format!("Failed to list {remote_dir}: {e}"))?;

            for entry in entries {
                let name = entry.file_name();
                if name == "." || name == ".." {
                    continue;
                }

                let remote_child = join_remote_path(&remote_dir, &name);
                let local_child = local_dir.join(&name);
                if entry.metadata().is_dir() {
                    if local_child.exists() {
                        if !local_child.is_dir() {
                            match ask_transfer_conflict(
                                event_tx,
                                &local_child.display().to_string(),
                                SftpConflictDirection::Download,
                                local_child.is_dir(),
                                conflict_policy,
                            )
                            .await?
                            {
                                SftpConflictDecision::KeepExisting => continue,
                                SftpConflictDecision::ReplaceWithIncoming => {
                                    remove_local_entry_recursive(&local_child).await?;
                                }
                            }
                        }
                    }

                    tokio::fs::create_dir_all(&local_child).await.map_err(|e| {
                        format!(
                            "Failed to create local directory {}: {e}",
                            local_child.display()
                        )
                    })?;
                    stack.push((remote_child, local_child));
                } else {
                    download_file_to_local(
                        sftp,
                        event_tx,
                        &remote_child,
                        &local_child,
                        conflict_policy,
                    )
                    .await?;
                }
            }
        }

        let _ = event_tx
            .send(SftpEvent::TransferComplete {
                name: remote_basename(&remote),
            })
            .await;

        Ok(())
    } else {
        let local_target = if local.is_dir() {
            local.join(remote_basename(&remote))
        } else {
            local
        };

        download_file_to_local(sftp, event_tx, &remote, &local_target, conflict_policy).await
    }
}

async fn remove_remote_entry_recursive(sftp: &SftpSession, path: &str) -> Result<(), String> {
    let mut stack: Vec<(String, bool)> = vec![(path.to_string(), false)];

    while let Some((current, visited)) = stack.pop() {
        match sftp.metadata(&current).await {
            Ok(metadata) => {
                if metadata.is_dir() {
                    if visited {
                        sftp.remove_dir(&current)
                            .await
                            .map_err(|e| format!("Failed to remove directory {current}: {e}"))?;
                    } else {
                        stack.push((current.clone(), true));
                        let entries = sftp
                            .read_dir(&current)
                            .await
                            .map_err(|e| format!("Failed to list {current}: {e}"))?;

                        for entry in entries {
                            let name = entry.file_name();
                            if name == "." || name == ".." {
                                continue;
                            }
                            stack.push((join_remote_path(&current, &name), false));
                        }
                    }
                } else {
                    sftp.remove_file(&current)
                        .await
                        .map_err(|e| format!("Failed to remove file {current}: {e}"))?;
                }
            }
            Err(_) => {
                if let Err(_file_err) = sftp.remove_file(&current).await {
                    if let Err(dir_err) = sftp.remove_dir(&current).await {
                        return Err(format!("Failed to remove {current}: {dir_err}"));
                    }
                }
            }
        }
    }

    Ok(())
}
