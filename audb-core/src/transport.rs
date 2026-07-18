use crate::config::EmulatorConfig;
use crate::error::{CoreError, CoreResult};
use russh::client::{self, Handle};
use russh::keys::{ssh_key, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Preferred};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub struct EmulatorTransport {
    config: EmulatorConfig,
    user: Option<Handle<ClientHandler>>,
    root: Option<Handle<ClientHandler>>,
}

impl EmulatorTransport {
    pub fn new(config: EmulatorConfig) -> Self {
        Self {
            config,
            user: None,
            root: None,
        }
    }
    pub fn config(&self) -> &EmulatorConfig {
        &self.config
    }

    async fn connect_as(&self, user: &str) -> CoreResult<Handle<ClientHandler>> {
        let config = client::Config {
            // The daemon intentionally keeps SSH sessions between CLI invocations.  A short
            // client-side inactivity timeout made the first command after an idle minute fail
            // with `Channel send error`; let the server own session lifetime instead.
            inactivity_timeout: None,
            preferred: Preferred {
                kex: Cow::Owned(vec![
                    russh::kex::CURVE25519_PRE_RFC_8731,
                    russh::kex::EXTENSION_SUPPORT_AS_CLIENT,
                ]),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut session = tokio::time::timeout(
            Duration::from_secs(5),
            client::connect(
                Arc::new(config),
                (self.config.host.as_str(), self.config.ssh_port),
                ClientHandler,
            ),
        )
        .await
        .map_err(|_| CoreError::ssh("SSH connection timeout"))?
        .map_err(|e| CoreError::ssh(format!("SSH connection failed: {e}")))?;
        let key = Arc::new(
            russh::keys::load_secret_key(&self.config.ssh_key, None)
                .map_err(|e| CoreError::ssh(format!("Cannot load SSH key: {e}")))?,
        );
        let key = PrivateKeyWithHashAlg::new(
            key,
            session
                .best_supported_rsa_hash()
                .await
                .map_err(|e| CoreError::ssh(e.to_string()))?
                .flatten(),
        );
        let auth = session
            .authenticate_publickey(user, key)
            .await
            .map_err(|e| CoreError::ssh(format!("SSH authentication failed: {e}")))?;
        if !auth.success() {
            return Err(CoreError::ssh(format!(
                "SSH authentication failed for {user}"
            )));
        }
        Ok(session)
    }

    async fn session(&mut self, root: bool) -> CoreResult<&mut Handle<ClientHandler>> {
        if root {
            if self.root.is_none() {
                self.root = Some(self.connect_as(&self.config.root_user).await?);
            }
            Ok(self.root.as_mut().unwrap())
        } else {
            if self.user.is_none() {
                self.user = Some(self.connect_as(&self.config.ssh_user).await?);
            }
            Ok(self.user.as_mut().unwrap())
        }
    }

    pub async fn exec(&mut self, command: &str, root: bool) -> CoreResult<String> {
        let first = exec_session(self.session(root).await?, command).await;
        if first.is_ok() {
            return first;
        }

        self.clear_session(root);
        let retry = exec_session(self.session(root).await?, command).await;
        if retry.is_err() {
            self.clear_session(root);
        }
        retry
    }

    pub async fn ping(&mut self) -> bool {
        self.exec("true", false).await.is_ok()
    }

    pub async fn upload_bytes(&mut self, remote: &Path, bytes: &[u8]) -> CoreResult<()> {
        let session = self.session(false).await?;
        let sftp = sftp(session).await?;
        let mut file = sftp
            .open_with_flags(
                remote.to_string_lossy().to_string(),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|e| CoreError::ssh(format!("SFTP open failed: {e}")))?;
        file.write_all(bytes)
            .await
            .map_err(|e| CoreError::ssh(format!("SFTP write failed: {e}")))?;
        file.shutdown()
            .await
            .map_err(|e| CoreError::ssh(format!("SFTP close failed: {e}")))?;
        Ok(())
    }

    pub async fn download_bytes(&mut self, remote: &Path) -> CoreResult<Vec<u8>> {
        let session = self.session(false).await?;
        let sftp = sftp(session).await?;
        let mut file = sftp
            .open_with_flags(remote.to_string_lossy().to_string(), OpenFlags::READ)
            .await
            .map_err(|e| CoreError::ssh(format!("SFTP open {} failed: {e}", remote.display())))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .await
            .map_err(|e| CoreError::ssh(format!("SFTP read failed: {e}")))?;
        Ok(bytes)
    }

    pub fn disconnect(&mut self) {
        self.user = None;
        self.root = None;
    }

    fn clear_session(&mut self, root: bool) {
        if root {
            self.root = None;
        } else {
            self.user = None;
        }
    }
}

async fn exec_session(session: &mut Handle<ClientHandler>, command: &str) -> CoreResult<String> {
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| CoreError::ssh(e.to_string()))?;
    channel
        .exec(true, command)
        .await
        .map_err(|e| CoreError::ssh(e.to_string()))?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut code = 0;
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => code = exit_status,
            _ => {}
        }
    }
    let out = String::from_utf8_lossy(&stdout).trim().to_string();
    if code != 0 {
        let err = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(CoreError::ssh(if err.is_empty() {
            format!("Command failed (rc={code}): {out}")
        } else {
            format!("Command failed (rc={code}): {err}")
        }));
    }
    Ok(out)
}

async fn sftp(session: &mut Handle<ClientHandler>) -> CoreResult<SftpSession> {
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| CoreError::ssh(e.to_string()))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| CoreError::ssh(e.to_string()))?;
    SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| CoreError::ssh(e.to_string()))
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quote_handles_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
}
