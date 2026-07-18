use anyhow::{anyhow, Context, Result};
use audb_core::{EmulatorBackend, EmulatorConfig};
use audb_protocol::{
    recv_message, send_message, AudbError, Command, CommandOutput, CommandResult, ErrorCode,
    Request, Response, PROTOCOL_VERSION,
};
use directories::BaseDirs;
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
    Ok(base.cache_dir().join("audb/audb.sock"))
}

pub async fn run() -> Result<()> {
    let socket = socket_path()?;
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent)?;
    }
    if socket.exists() {
        fs::remove_file(&socket)?;
    }
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("Cannot bind {}", socket.display()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let backend = Arc::new(Mutex::new(EmulatorBackend::new(
        EmulatorConfig::load_or_default()?,
    )));
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

async fn serve(mut stream: UnixStream, backend: Arc<Mutex<EmulatorBackend>>) -> Result<()> {
    loop {
        let request: Request = recv_message(&mut stream).await?;
        let shutdown = matches!(request.command, Command::Shutdown);
        let result = if request.protocol_version != PROTOCOL_VERSION {
            CommandResult::Error {
                error: AudbError {
                    code: ErrorCode::ProtocolMismatch,
                    message: format!(
                        "Protocol {} required, got {}",
                        PROTOCOL_VERSION, request.protocol_version
                    ),
                },
                data: None,
            }
        } else if shutdown {
            CommandResult::Success {
                output: CommandOutput::Empty,
            }
        } else {
            match backend.lock().await.execute(request.command).await {
                Ok(output) => CommandResult::Success { output },
                Err(error) => CommandResult::Error {
                    error: error.into(),
                    data: None,
                },
            }
        };
        let response = Response {
            id: request.id,
            protocol_version: PROTOCOL_VERSION,
            result,
        };
        send_message(&mut stream, &response).await?;
        if shutdown {
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(25)).await;
                std::process::exit(0);
            });
            return Ok(());
        }
    }
}

pub async fn typed_request(command: Command) -> std::result::Result<CommandOutput, AudbError> {
    let mut stream = connect_or_spawn().await.map_err(internal_error)?;
    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    send_message(
        &mut stream,
        &Request {
            id,
            protocol_version: PROTOCOL_VERSION,
            command,
        },
    )
    .await
    .map_err(internal_error)?;
    let response: Response = recv_message(&mut stream).await.map_err(internal_error)?;
    if response.id != id || response.protocol_version != PROTOCOL_VERSION {
        return Err(AudbError {
            code: ErrorCode::ProtocolMismatch,
            message: "Invalid daemon response".into(),
        });
    }
    match response.result {
        CommandResult::Success { output } => Ok(output),
        CommandResult::Error { error, .. } => Err(error),
    }
}

fn internal_error(error: impl std::fmt::Display) -> AudbError {
    AudbError {
        code: ErrorCode::InternalError,
        message: error.to_string(),
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
