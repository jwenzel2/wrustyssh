use russh::client;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::app::SshEvent;
use crate::models::tunnel::TunnelConfig;

/// Start a local port forwarding tunnel in a background Tokio task.
pub fn start_tunnel(
    session: Arc<Mutex<client::Handle<crate::ssh::handler::ClientHandler>>>,
    config: TunnelConfig,
    event_tx: async_channel::Sender<SshEvent>,
) {
    let tunnel_id = config.id;
    tokio::spawn(async move {
        match run_tunnel(session, &config, event_tx.clone()).await {
            Ok(()) => {}
            Err(e) => {
                let _ = event_tx
                    .send(SshEvent::TunnelFailed(tunnel_id, e.to_string()))
                    .await;
            }
        }
    });
}

async fn run_tunnel(
    session: Arc<Mutex<client::Handle<crate::ssh::handler::ClientHandler>>>,
    config: &TunnelConfig,
    event_tx: async_channel::Sender<SshEvent>,
) -> Result<(), anyhow::Error> {
    let bind_addr = format!("{}:{}", config.local_host, config.local_port);
    log::info!("Tunnel '{}': binding to {}", config.name, bind_addr);
    let listener = TcpListener::bind(&bind_addr).await.map_err(|e| {
        log::error!(
            "Tunnel '{}': failed to bind {}: {}",
            config.name,
            bind_addr,
            e
        );
        e
    })?;

    let _ = event_tx.send(SshEvent::TunnelEstablished(config.id)).await;

    let remote_host = config.remote_host.clone();
    let remote_port = config.remote_port as u32;

    log::info!(
        "Tunnel '{}': listening, forwarding to {}:{}",
        config.name,
        remote_host,
        remote_port
    );

    loop {
        let (mut tcp_stream, peer_addr) = listener.accept().await?;
        log::info!(
            "Tunnel '{}': accepted connection from {}",
            config.name,
            peer_addr
        );
        let session = session.clone();
        let remote_host = remote_host.clone();
        let tunnel_name = config.name.clone();

        tokio::spawn(async move {
            let sess = session.lock().await;
            let channel = match sess
                .channel_open_direct_tcpip(&remote_host, remote_port, "127.0.0.1", 0)
                .await
            {
                Ok(ch) => ch,
                Err(e) => {
                    log::error!(
                        "Tunnel '{}': failed to open direct-tcpip channel: {e}",
                        tunnel_name
                    );
                    return;
                }
            };
            drop(sess);

            let (mut tcp_read, mut tcp_write) = tcp_stream.split();
            let mut channel = channel;

            let mut buf = vec![0u8; 8192];
            loop {
                tokio::select! {
                    n = tcp_read.read(&mut buf) => {
                        match n {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if channel.data(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    msg = channel.wait() => {
                        match msg {
                            Some(russh::ChannelMsg::Data { data }) => {
                                if tcp_write.write_all(&data).await.is_err() {
                                    break;
                                }
                            }
                            Some(russh::ChannelMsg::Eof) | None => break,
                            _ => {}
                        }
                    }
                }
            }
        });
    }
}
