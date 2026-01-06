use russh::client::Handle;
use russh::client::{self};
use russh::keys::ssh_key;
use russh::keys::PrivateKeyWithHashAlg;
use russh::{ChannelMsg, Preferred};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::borrow::Cow;
use std::fs::File;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use anyhow::{anyhow, Result};

use crate::tools::shell_escape::escape_single_quote;

const DEFAULT_USER: &str = "defaultuser";

pub struct SshClient {}

impl client::Handler for SshClient {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _server_public_key: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

impl SshClient {
    pub fn connect(
        host: &str,
        port: u16,
        key_path: &Path,
    ) -> Result<Handle<SshClient>> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(Self::_connect(host, port, key_path))
        })
    }

    pub fn exec(
        session: &mut Handle<SshClient>,
        command: &str,
    ) -> Result<Vec<String>> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(Self::_exec(session, command))
        })
    }

    /// Execute command as root using devel-su (Aurora OS)
    ///
    /// Uses the `echo 'password' | devel-su sh -c 'command'` pattern to automate
    /// devel-su password input through SSH exec channels.
    ///
    /// # Security
    /// This function properly escapes the password and command to prevent shell injection.
    /// Both parameters are escaped for use in single-quote contexts.
    pub fn exec_as_devel_su(
        session: &mut Handle<SshClient>,
        command: &str,
        password: &str,
    ) -> Result<Vec<String>> {
        if password.is_empty() {
            return Err(anyhow!(
                "Root password not configured. Use 'audb device add' to set the root password."
            ));
        }

        // Escape password and command for single-quote context to prevent shell injection
        let password_escaped = escape_single_quote(password);
        let command_escaped = escape_single_quote(command);

        // Use echo pipe pattern: echo 'password' | devel-su sh -c 'command'
        let devel_su_command = format!(
            "echo '{}' | devel-su sh -c '{}'",
            password_escaped, command_escaped
        );

        Self::exec(session, &devel_su_command)
    }

    /// Read file contents as base64 string via SSH exec
    /// Useful for reading files owned by root when used with exec_as_devel_su
    pub fn read_file_base64(
        session: &mut Handle<SshClient>,
        remote_path: &Path,
        password: &str,
    ) -> Result<String> {
        let command = format!("base64 {}", remote_path.display());
        let output = Self::exec_as_devel_su(session, &command, password)?;

        if output.is_empty() {
            return Err(anyhow!("File is empty or could not be read"));
        }

        // Join lines and remove whitespace
        Ok(output.join("").replace(['\n', '\r'], ""))
    }

    pub fn upload(
        session: &mut Handle<SshClient>,
        local_path: &Path,
        remote_path: &Path,
    ) -> Result<()> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(Self::_upload(session, local_path, remote_path))
        })
    }

    pub fn download(
        session: &mut Handle<SshClient>,
        remote_path: &Path,
        local_path: &Path,
    ) -> Result<()> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(Self::_download(session, remote_path, local_path))
        })
    }

    pub fn test_connection(
        host: &str,
        port: u16,
        key_path: &Path,
    ) -> bool {
        match Self::connect(host, port, key_path) {
            Ok(mut session) => {
                Self::exec(&mut session, "echo test").is_ok()
            }
            Err(_) => false,
        }
    }

    async fn _connect(
        host: &str,
        port: u16,
        key_path: &Path,
    ) -> Result<Handle<SshClient>> {
        Self::_connect_with_user(DEFAULT_USER, host, port, key_path).await
    }

    async fn _connect_with_user(
        user: &str,
        host: &str,
        port: u16,
        key_path: &Path,
    ) -> Result<Handle<SshClient>> {
        let timeout_session = Duration::from_secs(30);
        let timeout_connect = Duration::from_secs(5);
        let config = client::Config {
            inactivity_timeout: Some(timeout_session),
            preferred: Preferred {
                kex: Cow::Owned(vec![
                    russh::kex::CURVE25519_PRE_RFC_8731,
                    russh::kex::EXTENSION_SUPPORT_AS_CLIENT,
                ]),
                ..Default::default()
            },
            ..<_>::default()
        };
        let config = Arc::new(config);
        let sh = SshClient {};
        let mut session = match tokio::time::timeout(timeout_connect, client::connect(config, (host, port), sh)).await?
        {
            Ok(session) => session,
            Err(err) => return Err(anyhow!("Connection error: {}", err)),
        };
        let secret_key = Arc::new(russh::keys::load_secret_key(key_path, None)?);
        let key_pair = PrivateKeyWithHashAlg::new(secret_key, session.best_supported_rsa_hash().await?.flatten());
        let result = session.authenticate_publickey(user, key_pair).await?;
        if !result.success() {
            return Err(anyhow!("Failed to authenticate via SSH as {}", user));
        }
        Ok(session)
    }

    async fn _exec(
        session: &mut Handle<SshClient>,
        command: &str,
    ) -> Result<Vec<String>> {
        let mut code = None;
        let mut stdout: Vec<String> = vec![];
        let mut stderr: Vec<String> = vec![];
        let mut channel = session.channel_open_session().await?;
        channel.exec(true, command).await?;
        loop {
            let Some(msg) = channel.wait().await else {
                break;
            };
            match msg {
                ChannelMsg::Data { ref data } => {
                    match str::from_utf8(data.as_ref()) {
                        Ok(out_line) => {
                            let line = out_line.trim().to_string();
                            stdout.push(line)
                        },
                        Err(_) => return Err(anyhow!("Failed to process SSH connection data")),
                    };
                }
                ChannelMsg::ExtendedData { ref data, ext } => {
                    // ext == 1 means stderr
                    if ext == 1 {
                        match str::from_utf8(data.as_ref()) {
                            Ok(err_line) => {
                                let line = err_line.trim().to_string();
                                stderr.push(line)
                            },
                            Err(_) => return Err(anyhow!("Failed to process SSH stderr data")),
                        };
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    code = Some(exit_status);
                }
                _ => {}
            }
        }
        if let Some(code) = code {
            if code != 0 {
                let error_msg = if !stderr.is_empty() {
                    stderr.join("\n")
                } else if !stdout.is_empty() {
                    stdout.join("\n")
                } else {
                    format!("Command failed with exit code {}", code)
                };
                return Err(anyhow!("{}", error_msg));
            }
        }
        Ok(stdout)
    }

    async fn _upload(
        session: &mut Handle<SshClient>,
        local_path: &Path,
        remote_path: &Path,
    ) -> Result<()> {
        let sftp_session = Self::_sftp_session(session).await?;

        let file = File::open(local_path)?;
        let size = file.metadata()?.len();
        if size == 0 {
            return Err(anyhow!("File is empty"));
        }

        let mut sftp_file = sftp_session
            .open_with_flags(
                remote_path.to_string_lossy().to_string(),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE | OpenFlags::READ,
            )
            .await?;

        let data = fs::read(local_path)?;
        sftp_file.write_all(&data).await?;

        Ok(())
    }


    async fn _download(
        session: &mut Handle<SshClient>,
        remote_path: &Path,
        local_path: &Path,
    ) -> Result<()> {
        let sftp_session = Self::_sftp_session(session).await?;

        let mut sftp_file = sftp_session
            .open_with_flags(
                remote_path.to_string_lossy().to_string(),
                OpenFlags::READ,
            )
            .await
            .map_err(|e| anyhow!("Failed to open remote file {}: {}", remote_path.display(), e))?;

        // Read file contents
        use tokio::io::AsyncReadExt;
        let mut data = Vec::new();
        sftp_file.read_to_end(&mut data).await
            .map_err(|e| anyhow!("Failed to read remote file: {}", e))?;

        // Write to local file
        fs::write(local_path, &data)
            .map_err(|e| anyhow!("Failed to write local file {}: {}", local_path.display(), e))?;

        Ok(())
    }

    async fn _sftp_session(session: &mut Handle<SshClient>) -> Result<SftpSession> {
        let channel = session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await
            .map_err(|e| anyhow!("Failed to request SFTP subsystem: {}", e))?;
        Ok(SftpSession::new(channel.into_stream()).await?)
    }
}
