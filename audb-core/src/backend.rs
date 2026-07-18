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
            | Command::Screenshot { socket }
            | Command::VisualFps { socket, .. } => Some(socket.clone()),
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
            Command::Info { category } => Ok(CommandOutput::Json(
                crate::system::info(&mut self.transport, category.as_deref()).await?,
            )),
            Command::Logs { options } => Ok(CommandOutput::Text(
                crate::system::logs(&mut self.transport, options).await?,
            )),
            Command::PackageList { filter } => Ok(CommandOutput::Json(
                crate::system::package_list(&mut self.transport, filter.as_deref()).await?,
            )),
            Command::PackageInstall { name, bytes } => Ok(CommandOutput::Json(
                crate::system::package_install(&mut self.transport, &name, &bytes).await?,
            )),
            Command::PackageUninstall { package } => Ok(CommandOutput::Json(
                crate::system::package_uninstall(&mut self.transport, &package).await?,
            )),
            Command::AppLaunch { package } => Ok(CommandOutput::Json(
                crate::app::launch(&mut self.transport, &package).await?,
            )),
            Command::AppStop { package } => Ok(CommandOutput::Json(
                crate::app::stop(&mut self.transport, &package).await?,
            )),
            Command::AppListRunning => Ok(CommandOutput::Json(Value::Array(
                crate::app::list(&mut self.transport).await?,
            ))),
            Command::AppPid { package } => {
                let pid = crate::app::pid(&mut self.transport, &package).await?;
                match pid {
                    Some(pid) => Ok(CommandOutput::Json(json!({"package":package,"pid":pid}))),
                    None => Err(CoreError::new(
                        audb_protocol::ErrorCode::AppNotRunning,
                        format!("Application is not running: {package}"),
                    )),
                }
            }
            Command::AppWait {
                package,
                running,
                timeout_ms,
                interval_ms,
            } => {
                let result = crate::app::wait(
                    &mut self.transport,
                    &package,
                    running,
                    Duration::from_millis(timeout_ms),
                    Duration::from_millis(interval_ms),
                )
                .await?;
                if result["matched"] == false {
                    Err(CoreError::new(
                        audb_protocol::ErrorCode::AppWaitTimeout,
                        format!("Timed out waiting for {package}"),
                    ))
                } else {
                    Ok(CommandOutput::Json(result))
                }
            }
            Command::AppClearData { package, confirm } => Ok(CommandOutput::Json(
                crate::appdata::clear(&mut self.transport, &package, confirm).await?,
            )),
            Command::SandboxPaths { package } => Ok(CommandOutput::Json(
                crate::appdata::paths(&mut self.transport, &package).await?,
            )),
            Command::SandboxList {
                package,
                root,
                path,
            } => Ok(CommandOutput::Json(
                crate::appdata::list(&mut self.transport, &package, &root, &path).await?,
            )),
            Command::SandboxPull {
                package,
                root,
                path,
            } => Ok(CommandOutput::Binary(
                crate::appdata::pull(&mut self.transport, &package, &root, &path).await?,
            )),
            Command::SandboxSqlite {
                package,
                root,
                path,
                query,
            } => Ok(CommandOutput::Json(
                crate::appdata::sqlite(&mut self.transport, &package, &root, &path, &query).await?,
            )),
            Command::DisplayStatus => Ok(CommandOutput::Json(
                crate::display::status(&mut self.transport).await?,
            )),
            Command::DisplaySet { action, timeout_ms } => {
                let result = crate::display::set(
                    &mut self.transport,
                    &action,
                    Duration::from_millis(timeout_ms),
                )
                .await?;
                if result["verified"] == false {
                    Err(CoreError::new(
                        audb_protocol::ErrorCode::DisplayStateTimeout,
                        "MCE did not reach the requested state",
                    ))
                } else {
                    Ok(CommandOutput::Json(result))
                }
            }
            Command::PerfSnapshot {
                package,
                sample_interval_ms,
            } => Ok(CommandOutput::Json(
                crate::diagnostics::perf_snapshot(
                    &mut self.transport,
                    &package,
                    Duration::from_millis(sample_interval_ms),
                )
                .await?,
            )),
            Command::PerfMonitor {
                package,
                duration_ms,
                interval_ms,
            } => Ok(CommandOutput::Json(
                crate::diagnostics::perf_monitor(
                    &mut self.transport,
                    &package,
                    Duration::from_millis(duration_ms),
                    Duration::from_millis(interval_ms),
                )
                .await?,
            )),
            Command::CrashList {
                package,
                since,
                lines,
            } => Ok(CommandOutput::Json(
                crate::diagnostics::crash_list(
                    &mut self.transport,
                    package.as_deref(),
                    since.as_deref(),
                    lines,
                )
                .await?,
            )),
            Command::CrashWatch {
                package,
                timeout_ms,
                interval_ms,
            } => Ok(CommandOutput::Json(
                crate::diagnostics::crash_watch(
                    &mut self.transport,
                    &package,
                    Duration::from_millis(timeout_ms),
                    Duration::from_millis(interval_ms),
                )
                .await?,
            )),
            Command::CrashClear { package } => Ok(CommandOutput::Json(
                crate::diagnostics::crash_clear(package.as_deref())?,
            )),
            Command::NetworkStatus => Ok(CommandOutput::Json(
                crate::emulator_api::network_status(&mut self.transport).await?,
            )),
            Command::NetworkInterfaces => Ok(CommandOutput::Json(
                crate::emulator_api::network_interfaces(&mut self.transport).await?,
            )),
            Command::NetworkTraffic => Ok(CommandOutput::Json(
                crate::emulator_api::network_traffic(&mut self.transport).await?,
            )),
            Command::NetworkProxyGet => Ok(CommandOutput::Json(
                crate::emulator_api::proxy_get(&mut self.transport).await?,
            )),
            Command::NetworkProxySet { host, port } => Ok(CommandOutput::Json(
                crate::emulator_api::proxy_set(&mut self.transport, &host, port).await?,
            )),
            Command::NetworkProxyClear => Ok(CommandOutput::Json(
                crate::emulator_api::proxy_clear(&mut self.transport).await?,
            )),
            Command::NetworkOffline { enabled } => Ok(CommandOutput::Json(
                crate::emulator_api::offline(&mut self.transport, enabled).await?,
            )),
            Command::LocationSet {
                latitude,
                longitude,
                altitude,
            } => Ok(CommandOutput::Json(
                crate::emulator_api::location_set(
                    &mut self.transport,
                    latitude,
                    longitude,
                    altitude,
                )
                .await?,
            )),
            Command::LocationTrackLoad {
                positions,
                looped,
                speed,
                default_interval,
            } => Ok(CommandOutput::Json(
                crate::emulator_api::track_load(
                    &mut self.transport,
                    &positions,
                    looped,
                    speed,
                    default_interval,
                )
                .await?,
            )),
            Command::LocationTrackAction { action, index } => Ok(CommandOutput::Json(
                crate::emulator_api::track_action(&mut self.transport, &action, index).await?,
            )),
            Command::SensorList => Ok(CommandOutput::Json(crate::emulator_api::sensor_list())),
            Command::SensorEnable { sensor, enabled } => Ok(CommandOutput::Json(
                crate::emulator_api::sensor_enable(&mut self.transport, &sensor, enabled).await?,
            )),
            Command::SensorVector { sensor, x, y, z } => Ok(CommandOutput::Json(
                crate::emulator_api::sensor_vector(&mut self.transport, &sensor, x, y, z).await?,
            )),
            Command::SensorScalar { sensor, value } => Ok(CommandOutput::Json(
                crate::emulator_api::sensor_scalar(&mut self.transport, &sensor, value).await?,
            )),
            Command::ClipboardStatus => Ok(CommandOutput::Json(
                json!({"available":false,"reason":"Aurora emulator does not expose a reliable global clipboard API without an application-side helper"}),
            )),
            Command::ClipboardUnavailable => Err(CoreError::new(
                audb_protocol::ErrorCode::CapabilityUnavailable,
                "Aurora emulator does not expose a reliable clipboard API",
            )),
            Command::QmpStatus { .. }
            | Command::Tap { .. }
            | Command::Swipe { .. }
            | Command::Text { .. }
            | Command::Key { .. }
            | Command::Screenshot { .. }
            | Command::VisualFps { .. } => unreachable!("QMP commands return before dispatch"),
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
        Command::VisualFps {
            duration_ms,
            interval_ms,
            freeze_threshold_ms,
            ..
        } => Ok(CommandOutput::Json(
            crate::diagnostics::visual(
                transport,
                qmp,
                Duration::from_millis(duration_ms),
                Duration::from_millis(interval_ms),
                Duration::from_millis(freeze_threshold_ms),
            )
            .await?,
        )),
        _ => Err(CoreError::runtime("invalid QMP command dispatch")),
    }
}
