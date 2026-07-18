use crate::config::EmulatorConfig;
use crate::error::{CoreError, CoreResult};
use crate::qmp::QmpClient;
use crate::transport::{shell_quote, EmulatorTransport};
use audb_protocol::{Command, CommandOutput};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

pub struct EmulatorBackend {
    pub config: EmulatorConfig,
    pub transport: EmulatorTransport,
    pub qmp: QmpClient,
}

impl EmulatorBackend {
    pub fn new(config: EmulatorConfig) -> Self {
        let qmp = QmpClient::new(config.qmp_socket.clone());
        let transport = EmulatorTransport::new(config.clone());
        Self {
            config,
            transport,
            qmp,
        }
    }

    async fn qmp_command(
        &mut self,
        command: Command,
        socket: Option<String>,
    ) -> CoreResult<CommandOutput> {
        let custom = socket
            .as_deref()
            .is_some_and(|path| Path::new(path) != self.config.qmp_socket);
        if custom {
            let mut qmp = QmpClient::new(socket.unwrap());
            execute_qmp(&mut qmp, &mut self.transport, command).await
        } else {
            execute_qmp(&mut self.qmp, &mut self.transport, command).await
        }
    }

    pub async fn execute(&mut self, command: Command) -> CoreResult<CommandOutput> {
        let qmp_socket = match &command {
            Command::QmpStatus { socket }
            | Command::Tap { socket, .. }
            | Command::Swipe { socket, .. }
            | Command::Text { socket, .. }
            | Command::Key { socket, .. }
            | Command::Screenshot { socket } => Some(socket.clone()),
            _ => None,
        };
        if let Some(socket) = qmp_socket {
            return self.qmp_command(command, socket).await;
        }
        match command {
            Command::Ping => Ok(CommandOutput::Text("pong".into())),
            Command::Shell { root, command_line } => Ok(CommandOutput::Text(
                self.transport.exec(&command_line, root).await?,
            )),
            Command::Push {
                local_path,
                remote_path,
            } => {
                let bytes = tokio::fs::read(&local_path).await.map_err(|e| {
                    CoreError::new(
                        audb_protocol::ErrorCode::NotFound,
                        format!("Cannot read {local_path}: {e}"),
                    )
                })?;
                self.transport
                    .upload_bytes(Path::new(&remote_path), &bytes)
                    .await?;
                Ok(CommandOutput::Json(
                    json!({"localPath":local_path,"remotePath":remote_path,"bytes":bytes.len()}),
                ))
            }
            Command::Pull { remote_path } => Ok(CommandOutput::Binary(
                self.transport
                    .download_bytes(Path::new(&remote_path))
                    .await?,
            )),
            Command::Open { url } => {
                let response = self.transport.exec(&format!("gdbus call --session --dest org.sailfishos.fileservice --object-path / --method org.sailfishos.fileservice.openUrl {}", shell_quote(&url)), false).await?;
                Ok(CommandOutput::Json(
                    json!({"url":url,"opened":true,"response":response}),
                ))
            }
            Command::AppLaunch { package } => Ok(CommandOutput::Json(
                crate::app::launch(&mut self.transport, &package).await?,
            )),
            Command::AppStop { package } => Ok(CommandOutput::Json(
                crate::app::stop(&mut self.transport, &package).await?,
            )),
            Command::AppListRunning => Ok(CommandOutput::Json(Value::Array(
                crate::app::list(&mut self.transport).await?,
            ))),
            Command::AppPid { package } => Ok(CommandOutput::Json(
                json!({"package":package,"pid":crate::app::pid(&mut self.transport, &package).await?}),
            )),
            Command::AppWait {
                package,
                running,
                timeout_ms,
                interval_ms,
            } => Ok(CommandOutput::Json(
                crate::app::wait(
                    &mut self.transport,
                    &package,
                    running,
                    Duration::from_millis(timeout_ms),
                    Duration::from_millis(interval_ms),
                )
                .await?,
            )),
            Command::DisplayStatus => Ok(CommandOutput::Json(
                crate::display::status(&mut self.transport).await?,
            )),
            Command::DisplaySet { action, timeout_ms } => Ok(CommandOutput::Json(
                crate::display::set(
                    &mut self.transport,
                    &action,
                    Duration::from_millis(timeout_ms),
                )
                .await?,
            )),
            Command::ClipboardStatus | Command::ClipboardUnavailable => Err(CoreError::new(
                audb_protocol::ErrorCode::CapabilityUnavailable,
                "Aurora emulator does not expose a reliable clipboard API",
            )),
            Command::QmpStatus { .. }
            | Command::Tap { .. }
            | Command::Swipe { .. }
            | Command::Text { .. }
            | Command::Key { .. }
            | Command::Screenshot { .. } => unreachable!("QMP commands return before dispatch"),
            _ => Err(CoreError::new(
                audb_protocol::ErrorCode::CapabilityUnavailable,
                "command is not implemented yet",
            )),
        }
    }
}

async fn execute_qmp(
    qmp: &mut QmpClient,
    transport: &mut EmulatorTransport,
    command: Command,
) -> CoreResult<CommandOutput> {
    match command {
        Command::QmpStatus { .. } => {
            let status = qmp.execute("query-status", None).await?;
            let commands = qmp.execute("query-commands", None).await?;
            let names = commands.as_array().cloned().unwrap_or_default();
            Ok(CommandOutput::Json(json!({
                "connected": true,
                "status": status,
                "capabilities": {
                    "inputSendEvent": names.iter().any(|v| v["name"] == "input-send-event"),
                    "screendump": names.iter().any(|v| v["name"] == "screendump"),
                }
            })))
        }
        Command::Tap {
            x, y, duration_ms, ..
        } => Ok(CommandOutput::Text(
            crate::input::tap(qmp, x, y, duration_ms).await?,
        )),
        Command::Swipe { args, options, .. } => Ok(CommandOutput::Text(
            crate::input::swipe(qmp, &args, options).await?,
        )),
        Command::Text { text, delay_ms, .. } => Ok(CommandOutput::Text(
            crate::input::text(qmp, &text, delay_ms).await?,
        )),
        Command::Key { name, .. } => Ok(CommandOutput::Text(crate::input::key(qmp, &name).await?)),
        Command::Screenshot { .. } => Ok(CommandOutput::Binary(
            crate::screenshot::capture(transport, qmp).await?,
        )),
        _ => Err(CoreError::runtime("invalid QMP command dispatch")),
    }
}
