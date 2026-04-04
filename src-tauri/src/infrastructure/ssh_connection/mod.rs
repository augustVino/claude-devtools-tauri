//! SSH connection manager -- manages russh SSH connections and SFTP sessions.

mod agent_discovery;
mod client_handler;
mod connection_state;
mod connect_flow;      // establish_raw_connection + build_connected_bundle
mod remote_path_resolver;
mod ssh_config_merge;
mod test_flow;         // test() reuses establish_raw_connection
mod types;             // ConnectRequest / RawConnection / ConnectedBundle

pub use client_handler::SshClientHandler;
pub use types::{ConnectRequest, ConnectedBundle, RawConnection};

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, RwLock, watch};

use crate::infrastructure::fs_provider::FsProvider;
use crate::infrastructure::ssh_auth;
use crate::infrastructure::ssh_config_parser::SshConfigParser;
use crate::infrastructure::ssh_fs_provider::SshFsProvider;
use crate::types::ssh::{
    SshAuthMethod, SshConfigHostEntry, SshConnectionConfig, SshConnectionStatus, SshTestResult,
};
#[cfg(test)]
use crate::types::ssh::SshConnectionState;

// Re-export internal state for tests
pub(crate) use connection_state::SshConnection;

// Constants
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// SSH connection manager -- single-connection lifecycle model.
///
/// - `connect()` auto-disconnects any existing connection.
/// - `disconnect()` drops the current connection (no params).
/// - Status changes are broadcast via `tokio::sync::broadcast`.
pub struct SshConnectionManager {
    /// Current active connection (single-connection model).
    connection: Arc<RwLock<Option<SshConnection>>>,
    /// SSH config parser for host resolution.
    config_parser: Option<SshConfigParser>,
    /// Broadcast sender for status change events.
    event_sender: broadcast::Sender<SshConnectionStatus>,
}

impl SshConnectionManager {
    /// Create a new SSH connection manager.
    ///
    /// Initializes the broadcast channel (capacity 16) and attempts to
    /// load the default SSH config (`~/.ssh/config`). If the config file
    /// does not exist, the parser is `None` and config-based methods
    /// return empty results.
    pub fn new() -> Self {
        let (event_sender, _) = broadcast::channel::<SshConnectionStatus>(16);

        let config_parser = match SshConfigParser::from_default_path() {
            Ok(Some(parser)) => Some(parser),
            Ok(None) => None,
            Err(e) => {
                log::warn!("Failed to load SSH config: {}", e);
                None
            }
        };

        Self {
            connection: Arc::new(RwLock::new(None)),
            config_parser,
            event_sender,
        }
    }

    /// Subscribe to connection status change events.
    ///
    /// Returns a `broadcast::Receiver` that the event bridge can poll
    /// to forward events to the Tauri frontend.
    pub fn subscribe(&self) -> broadcast::Receiver<SshConnectionStatus> {
        self.event_sender.subscribe()
    }

    /// Connect to an SSH server.
    ///
    /// Auto-disconnects any existing connection first (Electron-aligned behavior).
    /// Merges the provided config with `~/.ssh/config` if available.
    ///
    /// # Events emitted
    /// 1. `Connecting` (after auto-disconnect, if any)
    /// 2. `Connected` or `Error` (after connection attempt)
    pub async fn connect(
        &self,
        config: SshConnectionConfig,
    ) -> Result<SshConnectionStatus, String> {
        // 1. Auto-disconnect any existing connection
        let _ = self.disconnect().await;

        // 2. Capture original host before merge
        let request = ConnectRequest::new(config);

        // 3. Emit Connecting status
        let connecting_status = SshConnectionStatus::connecting(request.config.host.clone());
        let _ = self.event_sender.send(connecting_status.clone());

        // 4~10. Core connection (delegated to free function)
        let request_host = request.config.host.clone();
        match connect_flow::establish_raw_connection(&request, self.config_parser.as_ref()).await {
            Ok(raw) => {
                // 11~12. Build business layer bundle
                let bundle = match connect_flow::build_connected_bundle(request, raw).await {
                    Ok(b) => b,
                    Err(e) => {
                        let status = SshConnectionStatus::error(
                            request_host,
                            e.clone(),
                        );
                        let _ = self.event_sender.send(status.clone());
                        return Ok(status);
                    }
                };

                // 13. Monitor stop channel
                let (monitor_stop, monitor_stop_rx) = watch::channel(false);

                // 13.5. Clone resources for health monitor
                let monitor_sftp = bundle.fs_provider.sftp_arc();
                let monitor_event_sender = self.event_sender.clone();
                let monitor_connection = Arc::clone(&self.connection);

                // 14. Store connection
                {
                    let mut conn = self.connection.write().await;
                    *conn = Some(SshConnection {
                        config: bundle.merged_config.clone(),
                        status: bundle.status.clone(),
                        remote_projects_path: Some(bundle.remote_projects_path.clone()),
                        session: bundle.session,
                        fs_provider: bundle.fs_provider,
                        monitor_stop,
                    });
                }

                // 14.5. Start health monitor
                {
                    let host = bundle.merged_config.host.clone();
                    tokio::spawn(async move {
                        let mut stop_rx = monitor_stop_rx;
                        let sftp = monitor_sftp;
                        let sender = monitor_event_sender;
                        let connection_lock = monitor_connection;

                        loop {
                            tokio::select! {
                                _ = stop_rx.changed() => {
                                    log::info!("SSH health monitor: stop signal received");
                                    return;
                                }
                                _ = tokio::time::sleep(HEALTH_CHECK_INTERVAL) => {}
                            }

                            if *stop_rx.borrow() {
                                return;
                            }

                            // Health check 1: russh session internal state (via read lock)
                            {
                                let conn = connection_lock.read().await;
                                if let Some(ref c) = *conn {
                                    if c.session.is_closed() {
                                        log::warn!(
                                            "SSH health monitor: session closed, connection lost"
                                        );
                                        drop(conn);
                                        break;
                                    }
                                } else {
                                    // Connection already cleaned up
                                    return;
                                }
                            }

                            // Health check 2: SFTP probe
                            let probe =
                                tokio::time::timeout(HEALTH_CHECK_TIMEOUT, sftp.metadata("/")).await;

                            match probe {
                                Ok(Ok(_)) => {}
                                Ok(Err(e)) => {
                                    log::warn!(
                                        "SSH health monitor: SFTP probe failed: {}, connection lost",
                                        e
                                    );
                                    break;
                                }
                                Err(_) => {
                                    log::warn!(
                                        "SSH health monitor: SFTP probe timed out, connection lost"
                                    );
                                    break;
                                }
                            }
                        }

                        // Health check failed — clean up safely.
                        // Use take() to atomically claim the connection, preventing
                        // double-cleanup if disconnect() is called concurrently.
                        log::info!(
                            "SSH health monitor: cleaning up disconnected session"
                        );

                        let taken = {
                            let mut conn = connection_lock.write().await;
                            conn.take()
                        };

                        if let Some(mut c) = taken {
                            let _ = c.fs_provider.dispose_async().await;
                            let _ = c
                                .session
                                .disconnect(russh::Disconnect::ByApplication, "", "")
                                .await;
                            let _ = sender.send(SshConnectionStatus::disconnected());
                        } else {
                            log::info!(
                                "SSH health monitor: connection already cleaned up by disconnect()"
                            );
                        }
                    });
                }

                // 15. Emit Connected
                let _ = self.event_sender.send(bundle.status.clone());
                Ok(bundle.status)
            }
            Err(e) => {
                let error_status =
                    SshConnectionStatus::error(request.config.host.clone(), e.clone());
                let _ = self.event_sender.send(error_status.clone());
                Ok(error_status)
            }
        }
    }

    /// Disconnect the current SSH connection.
    ///
    /// Takes no parameters -- disconnects whatever is active (Electron-aligned).
    /// No-op if already disconnected.
    ///
    /// # Events emitted
    /// - `Disconnected` (if there was an active connection)
    pub async fn disconnect(&self) -> Result<SshConnectionStatus, String> {
        // Atomically claim the connection via take() — prevents double-cleanup
        // if the health monitor is concurrently cleaning up.
        let taken = {
            let mut conn = self.connection.write().await;
            conn.take()
        };

        let status = SshConnectionStatus::disconnected();

        if let Some(mut connection) = taken {
            // Signal monitor to stop (no-op if already exited)
            let _ = connection.monitor_stop.send(true);

            // Dispose SFTP resources
            connection.fs_provider.dispose_async().await;

            // Graceful SSH disconnect
            let _ = connection
                .session
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await;

            let _ = self.event_sender.send(status.clone());
        }

        Ok(status)
    }

    /// Get the current active connection state.
    ///
    /// Returns `Disconnected` if no connection is active.
    pub async fn get_active_state(&self) -> SshConnectionStatus {
        let conn = self.connection.read().await;
        match conn.as_ref() {
            Some(c) => c.status.clone(),
            None => SshConnectionStatus::disconnected(),
        }
    }

    /// Get all host entries from the SSH config.
    ///
    /// Returns an empty vec if no SSH config is loaded.
    pub fn get_config_hosts(&self) -> Vec<SshConfigHostEntry> {
        match &self.config_parser {
            Some(parser) => parser.get_hosts(),
            None => Vec::new(),
        }
    }

    /// Resolve a host alias from the SSH config.
    ///
    /// Returns `None` if the alias is not found or no SSH config is loaded.
    pub fn resolve_host_config(&self, alias: &str) -> Option<SshConfigHostEntry> {
        self.config_parser.as_ref()?.resolve_host(alias)
    }

    /// Get the remote projects path from the current connection.
    ///
    /// Returns `None` if not connected.
    pub async fn get_remote_projects_path(&self) -> Option<String> {
        let conn = self.connection.read().await;
        conn.as_ref()
            .and_then(|c| c.remote_projects_path.clone())
    }

    /// Get the FsProvider for the active SSH connection.
    ///
    /// Returns `Some(Arc<dyn FsProvider>)` wrapping an SFTP provider
    /// when connected, `None` otherwise.
    pub async fn get_provider(&self) -> Option<Arc<dyn FsProvider>> {
        let conn = self.connection.read().await;
        conn.as_ref()
            .map(|c| Arc::new(c.fs_provider.clone()) as Arc<dyn FsProvider>)
    }

    /// Get the host from the currently connected session's config.
    ///
    /// Returns `None` if not connected. Unlike `get_active_state().host`,
    /// this reads from the stored config (not the status snapshot),
    /// so it's available even during teardown.
    pub async fn get_connected_host(&self) -> Option<String> {
        let conn = self.connection.read().await;
        conn.as_ref().map(|c| c.config.host.clone())
    }
}

impl Default for SshConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
