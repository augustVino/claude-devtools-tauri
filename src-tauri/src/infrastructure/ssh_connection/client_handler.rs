//! russh client handler — 接受所有 host key。

use async_trait::async_trait;
use russh::client;

/// russh client handler that accepts all host keys (matching Electron default).
#[derive(Clone)]
pub struct SshClientHandler;

#[async_trait]
impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> { Ok(true) }
}
