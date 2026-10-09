use crate::config::EmulatorConfig;
use crate::devices::{DeviceConfig, DeviceKind};
use crate::error::{CoreError, CoreResult};
use audb_protocol::ErrorCode;
use russh::client::{self, Handle};
use russh::keys::{ssh_key, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Preferred};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::borrow::Cow;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Common transport; legacy SDK SSH remains compatible, physical devices use
/// OpenSSH configuration, SSH agent and known_hosts without accepting new keys.
pub struct DeviceTransport {
    config: DeviceConfig,
    legacy: Option<LegacyTransport>,
}

impl DeviceTransport {
    pub fn new(config: DeviceConfig) -> Self {
        let legacy = if config.kind == DeviceKind::Emulator {
            config.emulator_config().ok().map(LegacyTransport::new)
        } else {
            None
        };
        Self { config, legacy }
    }
    pub fn config(&self) -> &DeviceConfig {
        &self.config
    }
    pub async fn exec(&mut self, command: &str, root: bool) -> CoreResult<String> {
        Ok(self.exec_raw(command, root).await?.trim().to_owned())
    }
    pub async fn exec_raw(&mut self, command: &str, root: bool) -> CoreResult<String> {
        if let Some(legacy) = &mut self.legacy {
            return tokio::time::timeout(Duration::from_secs(300), legacy.exec(command, root))
                .await
                .map_err(|_| {
                    CoreError::new(
                        ErrorCode::OutcomeUnknown,
                        "SSH command timed out; result unknown and command was not repeated",
                    )
                })?;
        }
        let mut process = self.openssh("ssh", root)?;
        process.arg("--").arg(&self.config.host).arg(command);
        let output = run_process(process, None).await?;
        check_output(&output)?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
    pub async fn ping(&mut self) -> bool {
        self.exec("true", false).await.is_ok()
    }
    /// Send structured input over SSH stdin, without placing it in shell arguments.
    pub async fn exec_stdin(&mut self, command: &str, input: Vec<u8>) -> CoreResult<String> {
        Ok(String::from_utf8_lossy(&self.exec_stdin_bytes(command, input).await?).into_owned())
    }
    /// Binary stdout must never pass through UTF-8 lossy conversion.
    pub async fn exec_stdin_bytes(&mut self, command: &str, input: Vec<u8>) -> CoreResult<Vec<u8>> {
        if let Some(legacy) = &mut self.legacy {
            let result =
                exec_session_input_bytes(legacy.session(false).await?, command, Some(&input)).await;
            if result.is_err() {
                legacy.clear_session(false);
            }
            return result;
        }
        let mut process = self.openssh("ssh", false)?;
        process.arg("--").arg(&self.config.host).arg(command);
        let output = run_process(process, Some(input)).await?;
        check_output(&output)?;
        Ok(output.stdout)
    }
    /// Bootstrap only: use root SSH or a one-shot devel-su credential on stdin.
    pub async fn exec_privileged(
        &mut self,
        command: &str,
        password: Option<&audb_protocol::RootPassword>,
    ) -> CoreResult<String> {
        let checked = format!("test \"$(id -u)\" = 0 && {command}");
        if self.config.ssh_user == "root" && self.config.root_user.is_none() {
            return self.exec_raw(&checked, false).await;
        }
        if self.config.root_user.is_some() && password.is_none() {
            return self.exec_raw(&checked, true).await;
        }
        let password =
            password
                .ok_or_else(|| {
                    CoreError::new(ErrorCode::RootAccessRequired,
            "Use --root-password-stdin, an interactive terminal, or configure --root-user")
                })?
                .expose();
        if password.is_empty() || password.len() > 4096 || password.contains(['\n', '\r', '\0']) {
            return Err(CoreError::invalid("Invalid root credential"));
        }
        let mut process = self.openssh("ssh", false)?;
        process
            .arg("--")
            .arg(&self.config.host)
            .arg(format!("devel-su /bin/sh -c {}", shell_quote(&checked)));
        let mut output = run_process(process, Some(format!("{password}\n").into_bytes())).await?;
        // No PTY is allocated, so terminal echo cannot reveal the credential.
        output.stdout = String::from_utf8_lossy(&output.stdout)
            .replace(password, "[REDACTED]")
            .into_bytes();
        output.stderr = String::from_utf8_lossy(&output.stderr)
            .replace(password, "[REDACTED]")
            .into_bytes();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            if stderr.contains("auth failed")
                || stderr.contains("authentication failed")
                || stderr.contains("devel-su: sorry")
            {
                return Err(CoreError::new(
                    ErrorCode::AuthenticationFailed,
                    "devel-su authentication failed",
                ));
            }
        }
        check_output(&output)?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
    pub async fn upload_bytes(&mut self, remote: &Path, bytes: &[u8]) -> CoreResult<()> {
        if let Some(legacy) = &mut self.legacy {
            return legacy.upload_bytes(remote, bytes).await;
        }
        let dir = tempfile::tempdir()?;
        let local = dir.path().join("upload.bin");
        tokio::fs::write(&local, bytes).await?;
        self.sftp_batch(format!(
            "put {} {}\n",
            sftp_quote(&local)?,
            sftp_quote(remote)?
        ))
        .await
    }
    /// Send the local file over SFTP without reading it into the daemon request
    /// or buffering its entire contents in memory.
    pub async fn upload_file(&mut self, local: &Path, remote: &Path) -> CoreResult<u64> {
        let mut source = tokio::fs::File::open(local).await.map_err(|e| {
            CoreError::new(
                ErrorCode::NotFound,
                format!("Cannot open {}: {e}", local.display()),
            )
        })?;
        let metadata = source.metadata().await?;
        if !metadata.is_file() {
            return Err(CoreError::invalid("Upload source must be a regular file"));
        }
        if let Some(legacy) = &mut self.legacy {
            let session = legacy.session(false).await?;
            let sftp = sftp(session).await?;
            let mut dest = sftp
                .open_with_flags(
                    remote.to_string_lossy().to_string(),
                    OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                )
                .await
                .map_err(|e| CoreError::ssh(format!("SFTP open failed: {e}")))?;
            let bytes = tokio::io::copy(&mut source, &mut dest)
                .await
                .map_err(|e| CoreError::ssh(format!("SFTP file transfer failed: {e}")))?;
            dest.shutdown()
                .await
                .map_err(|e| CoreError::ssh(format!("SFTP close failed: {e}")))?;
            return Ok(bytes);
        }
        self.sftp_batch(format!(
            "put {} {}\n",
            sftp_quote(local)?,
            sftp_quote(remote)?
        ))
        .await?;
        Ok(metadata.len())
    }
    pub async fn download_bytes(&mut self, remote: &Path) -> CoreResult<Vec<u8>> {
        if let Some(legacy) = &mut self.legacy {
            return legacy.download_bytes(remote).await;
        }
        let dir = tempfile::tempdir()?;
        let local = dir.path().join("download.bin");
        self.sftp_batch(format!(
            "get {} {}\n",
            sftp_quote(remote)?,
            sftp_quote(&local)?
        ))
        .await?;
        Ok(tokio::fs::read(local).await?)
    }
    async fn sftp_batch(&self, batch: String) -> CoreResult<()> {
        let mut process = self.openssh("sftp", false)?;
        let host = if self.config.host.parse::<std::net::Ipv6Addr>().is_ok() {
            format!("[{}]", self.config.host)
        } else {
            self.config.host.clone()
        };
        process.args(["-b", "-", "--"]).arg(host);
        check_output(&run_process(process, Some(batch.into_bytes())).await?)
    }
    fn openssh(&self, program: &str, root: bool) -> CoreResult<tokio::process::Command> {
        let user = if root {
            self.config.root_user.as_deref().ok_or_else(|| {
                CoreError::new(
                    ErrorCode::CapabilityUnavailable,
                    "Root SSH is not configured; privileged operations require audb-agent",
                )
            })?
        } else {
            &self.config.ssh_user
        };
        let mut process = tokio::process::Command::new(program);
        process
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
        ]);
        process.arg("-o").arg(format!("User={user}"));
        process
            .arg("-o")
            .arg(format!("Port={}", self.config.ssh_port));
        if let Some(key) = &self.config.ssh_key {
            process.arg("-i").arg(key);
        }
        Ok(process)
    }
    pub fn disconnect(&mut self) {
        if let Some(legacy) = &mut self.legacy {
            legacy.disconnect();
        }
    }
}

async fn run_process(
    mut process: tokio::process::Command,
    input: Option<Vec<u8>>,
) -> CoreResult<std::process::Output> {
    let input = input.map(zeroize::Zeroizing::new);
    if input.is_some() {
        process.stdin(Stdio::piped());
    }
    let mut child = process
        .spawn()
        .map_err(|e| CoreError::ssh(format!("Cannot start OpenSSH: {e}")))?;
    let stdin = child.stdin.take();
    let operation = async {
        let write = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                stdin.write_all(&input).await?;
                stdin.shutdown().await?;
            }
            Ok::<_, std::io::Error>(())
        };
        let (_, output) = tokio::try_join!(write, child.wait_with_output())?;
        Ok::<_, std::io::Error>(output)
    };
    tokio::time::timeout(Duration::from_secs(300), operation)
        .await
        .map_err(|_| {
            CoreError::new(
                ErrorCode::OutcomeUnknown,
                "SSH operation timed out; it was not repeated",
            )
        })?
        .map_err(|e| {
            CoreError::new(
                ErrorCode::OutcomeUnknown,
                format!("SSH operation interrupted: {e}; it was not repeated"),
            )
        })
}

fn check_output(output: &std::process::Output) -> CoreResult<()> {
    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code();
    let message = String::from_utf8_lossy(&output.stderr);
    Err(CoreError::new(
        if code == Some(255) || code.is_none() {
            ErrorCode::OutcomeUnknown
        } else {
            ErrorCode::RemoteCommandFailed
        },
        format!("SSH operation failed (exit {code:?}): {}", message.trim()),
    )
    .with_data(
        serde_json::json!({"exitCode": code, "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": message, "retried": false}),
    ))
}

fn sftp_quote(path: &Path) -> CoreResult<String> {
    let value = path
        .to_str()
        .ok_or_else(|| CoreError::invalid("SFTP path must be UTF-8"))?;
    if value.chars().any(char::is_control) {
        return Err(CoreError::invalid("SFTP path contains control characters"));
    }
    let mut escaped = String::new();
    for c in value.chars() {
        // sftp's quoted argument parser escapes glob metacharacters itself.
        // Pre-escaping them would introduce literal backslashes in put targets.
        // See openssh-portable/sftp.c makeargv() and undo_glob_escape().
        if "\\\"".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    if value.starts_with('-') {
        escaped.insert_str(0, "./");
    }
    Ok(format!("\"{escaped}\""))
}

pub struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub struct LegacyTransport {
    config: EmulatorConfig,
    user: Option<Handle<ClientHandler>>,
    root: Option<Handle<ClientHandler>>,
}

impl LegacyTransport {
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
        // Reconnect before sending, never retry a command after it was sent.
        if self.root.as_ref().is_some_and(Handle::is_closed) {
            self.root = None;
        }
        if self.user.as_ref().is_some_and(Handle::is_closed) {
            self.user = None;
        }
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
        let result = exec_session(self.session(root).await?, command).await;
        if result.is_err() {
            self.clear_session(root);
        }
        result
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
    exec_session_input(session, command, None).await
}
async fn exec_session_input(
    session: &mut Handle<ClientHandler>,
    command: &str,
    input: Option<&[u8]>,
) -> CoreResult<String> {
    Ok(
        String::from_utf8_lossy(&exec_session_input_bytes(session, command, input).await?)
            .into_owned(),
    )
}
async fn exec_session_input_bytes(
    session: &mut Handle<ClientHandler>,
    command: &str,
    input: Option<&[u8]>,
) -> CoreResult<Vec<u8>> {
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| CoreError::ssh(e.to_string()))?;
    channel.exec(true, command).await.map_err(|e| {
        CoreError::new(
            ErrorCode::OutcomeUnknown,
            format!("SSH exec interrupted: {e}; command was not repeated"),
        )
    })?;
    if let Some(input) = input {
        channel.data(input).await.map_err(|e| {
            CoreError::new(
                ErrorCode::OutcomeUnknown,
                format!("SSH stdin interrupted; command was not repeated: {e}"),
            )
        })?;
        channel.eof().await.map_err(|e| {
            CoreError::new(
                ErrorCode::OutcomeUnknown,
                format!("SSH EOF interrupted; command was not repeated: {e}"),
            )
        })?;
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut code = None;
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
            _ => {}
        }
    }
    let out = String::from_utf8_lossy(&stdout).to_string();
    let code = code.ok_or_else(|| {
        CoreError::new(
            audb_protocol::ErrorCode::OutcomeUnknown,
            "SSH channel closed without exit status; command was not repeated",
        )
    })?;
    if code != 0 {
        let err = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(CoreError::new(
            audb_protocol::ErrorCode::RemoteCommandFailed,
            if err.is_empty() {
                format!("Command failed (rc={code}): {out}")
            } else {
                format!("Command failed (rc={code}): {err}")
            },
        )
        .with_data(serde_json::json!({"exitCode": code, "stdout": out,
            "stderr": String::from_utf8_lossy(&stderr), "retried": false})));
    }
    Ok(stdout)
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
    #[test]
    fn sftp_paths_cannot_add_commands_or_expand_globs() {
        assert!(sftp_quote(Path::new("a\nrm anything")).is_err());
        assert_eq!(
            sftp_quote(Path::new("a \"[*]?\\b")).unwrap(),
            "\"a \\\"[*]?\\\\b\""
        );
    }
}
