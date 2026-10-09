use anyhow::{anyhow, Context, Result};
use audb_core::{devices::DeviceRegistry, DeviceBackend};
use audb_protocol::{
    recv_message, send_message, AudbError, Command, CommandOutput, CommandResult, ErrorCode,
    Request, Response, PROTOCOL_VERSION,
};
use directories::BaseDirs;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub fn socket_path() -> Result<PathBuf> {
    let base = BaseDirs::new().ok_or_else(|| anyhow!("Cannot determine home directory"))?;
    Ok(base
        .cache_dir()
        .join(format!("audb/audb-v{PROTOCOL_VERSION}.sock")))
}

pub async fn run() -> Result<()> {
    let socket = socket_path()?;
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent)?;
    }
    // Concurrent CLI invocations can both spawn a daemon. Only one may bind
    // or remove its socket; losers leave the winning daemon intact.
    let owner = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(socket.with_extension("lock"))?;
    fs2::FileExt::try_lock_exclusive(&owner).context("audb daemon is already running")?;
    if socket.exists() {
        fs::remove_file(&socket)?;
    }
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("Cannot bind {}", socket.display()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let backend = Arc::new(Mutex::new(HashMap::new()));
    loop {
        let (stream, _) = listener.accept().await?;
        let backend = Arc::clone(&backend);
        tokio::spawn(async move {
            if let Err(error) = serve(stream, backend).await {
                tracing::debug!("client disconnected: {error}");
            }
        });
    }
}

type Runtimes = Arc<Mutex<HashMap<String, Arc<Mutex<DeviceBackend>>>>>;

async fn serve(mut stream: UnixStream, backend: Runtimes) -> Result<()> {
    loop {
        let request: Request = recv_message(&mut stream).await?;
        let shutdown = matches!(request.command, Command::Shutdown)
            && request.protocol_version == PROTOCOL_VERSION
            && request.timeout_ms > 0
            && request.timeout_ms <= 86_400_000;
        let result = if request.protocol_version != PROTOCOL_VERSION {
            CommandResult::Error {
                error: AudbError {
                    code: ErrorCode::ProtocolMismatch,
                    message: format!(
                        "Protocol {} required, got {}",
                        PROTOCOL_VERSION, request.protocol_version
                    ),
                    data: None,
                },
                data: None,
            }
        } else if request.timeout_ms == 0 || request.timeout_ms > 86_400_000 {
            CommandResult::Error {
                error: AudbError {
                    code: ErrorCode::InvalidArgument,
                    message: "timeoutMs must be between 1 and 86400000".into(),
                    data: None,
                },
                data: None,
            }
        } else if shutdown {
            CommandResult::Success {
                output: CommandOutput::Empty,
            }
        } else {
            let execution = async {
                tokio::time::timeout(Duration::from_millis(request.timeout_ms),
                    execute_device(&backend, request.device_id.as_deref(), request.command)).await
                    .map_err(|_| audb_core::CoreError::new(ErrorCode::OutcomeUnknown,
                        "Command deadline exceeded; result may be unknown and command was not repeated"))?
            };
            tokio::pin!(execution);
            let executed = tokio::select! {
                result = &mut execution => result,
                disconnected = wait_for_disconnect(&stream) => {
                    disconnected?;
                    return Ok(());
                }
            };
            match executed {
                Ok(output) => CommandResult::Success { output },
                Err(error) => {
                    let data = error.data.clone();
                    CommandResult::Error {
                        error: error.into(),
                        data,
                    }
                }
            }
        };
        let response = Response {
            id: request.id,
            protocol_version: PROTOCOL_VERSION,
            device_id: request.device_id,
            result,
        };
        send_message(&mut stream, &response).await?;
        if shutdown {
            // Keep this stream alive until the process exits so a CLI waiting
            // for EOF cannot immediately reconnect to a daemon about to exit.
            tokio::time::sleep(Duration::from_millis(25)).await;
            std::process::exit(0);
        }
    }
}

async fn execute_device(
    runtimes: &Runtimes,
    id: Option<&str>,
    command: Command,
) -> audb_core::CoreResult<CommandOutput> {
    let registry = DeviceRegistry::load()?;
    let config = registry.resolve(id)?.clone();
    let runtime = {
        let mut map = runtimes.lock().await;
        map.retain(|id, _| registry.devices.iter().any(|d| &d.id == id));
        map.entry(config.id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(DeviceBackend::new(config.clone()))))
            .clone()
    };
    let mut backend = runtime.lock().await;
    // Re-read after waiting: queued requests must not use removed or updated configuration.
    let current = DeviceRegistry::load()?.get(&config.id)?.clone();
    if current != backend.config {
        *backend = DeviceBackend::new(current);
    }
    backend.execute(command).await
}

async fn wait_for_disconnect(stream: &UnixStream) -> Result<()> {
    let mut byte = [0_u8; 1];
    loop {
        stream.readable().await?;
        match stream.try_read(&mut byte) {
            Ok(0) => return Ok(()),
            Ok(_) => return Err(anyhow!("pipelined daemon requests are not supported")),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

pub async fn typed_request(
    device_id: Option<&str>,
    command: Command,
    timeout_ms: u64,
) -> std::result::Result<CommandOutput, AudbError> {
    let shutdown = matches!(&command, Command::Shutdown);
    let mut stream = connect_or_spawn().await.map_err(internal_error)?;
    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    send_message(
        &mut stream,
        &Request {
            id,
            protocol_version: PROTOCOL_VERSION,
            device_id: device_id.map(str::to_owned),
            timeout_ms,
            command,
        },
    )
    .await
    .map_err(|e| unknown_result("Daemon request delivery failed", e))?;
    let response: Response = tokio::time::timeout(
        Duration::from_millis(timeout_ms + 5_000),
        recv_message(&mut stream),
    )
    .await
    .map_err(|_| AudbError {
        code: ErrorCode::OutcomeUnknown,
        message: "Daemon response timed out; command was not repeated".into(),
        data: None,
    })?
    .map_err(|e| unknown_result("Daemon response was lost", e))?;
    if response.id != id
        || response.protocol_version != PROTOCOL_VERSION
        || response.device_id.as_deref() != device_id
    {
        return Err(AudbError {
            code: ErrorCode::ProtocolMismatch,
            message: "Invalid daemon response".into(),
            data: None,
        });
    }
    if shutdown && matches!(&response.result, CommandResult::Success { .. }) {
        use tokio::io::AsyncReadExt;
        let mut byte = [0_u8; 1];
        tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte))
            .await
            .map_err(|_| internal_error("Daemon did not shut down"))?
            .map_err(internal_error)?;
    }
    match response.result {
        CommandResult::Success { output } => Ok(output),
        CommandResult::Error { mut error, data } => {
            error.data = data;
            Err(error)
        }
    }
}

fn unknown_result(context: &str, error: impl std::fmt::Display) -> AudbError {
    AudbError {
        code: ErrorCode::OutcomeUnknown,
        message: format!("{context}: {error}; result may be unknown and command was not repeated"),
        data: None,
    }
}

fn internal_error(error: impl std::fmt::Display) -> AudbError {
    AudbError {
        code: ErrorCode::InternalError,
        message: error.to_string(),
        data: None,
    }
}

async fn connect_or_spawn() -> Result<UnixStream> {
    let socket = socket_path()?;
    if let Ok(stream) = UnixStream::connect(&socket).await {
        return Ok(stream);
    }
    let executable = std::env::current_exe()?;
    let mut process = ProcessCommand::new(executable);
    process
        .arg("__daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    process.spawn().context("Cannot start audb daemon")?;
    for _ in 0..100 {
        if let Ok(stream) = UnixStream::connect(&socket).await {
            return Ok(stream);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err(anyhow!("audb daemon did not create {}", socket.display()))
}
