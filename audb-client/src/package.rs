use audb_core::{config::cache_dir, EmulatorConfig};
use audb_protocol::{AudbError, ErrorCode};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn error(code: ErrorCode, message: impl Into<String>) -> AudbError {
    AudbError {
        code,
        message: message.into(),
        data: None,
    }
}
fn check_rpm(path: &str) -> Result<PathBuf, AudbError> {
    let path = std::fs::canonicalize(path)
        .map_err(|e| error(ErrorCode::NotFound, format!("RPM not found: {path}: {e}")))?;
    if path.extension().and_then(|v| v.to_str()) != Some("rpm") {
        return Err(error(
            ErrorCode::InvalidArgument,
            format!("File must be .rpm: {}", path.display()),
        ));
    }
    Ok(path)
}
fn run(mut command: Command, timeout: Duration) -> Result<Output, AudbError> {
    let mut child = command
        .spawn()
        .map_err(|e| error(ErrorCode::RuntimeError, e.to_string()))?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|e| error(ErrorCode::RuntimeError, e.to_string()))?
            .is_some()
        {
            return child
                .wait_with_output()
                .map_err(|e| error(ErrorCode::RuntimeError, e.to_string()));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            return Err(error(ErrorCode::RuntimeError, "command timed out"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn image() -> Result<String, AudbError> {
    let mut c = Command::new("docker");
    c.args(["images", "--format", "{{.Repository}}:{{.Tag}}"]);
    let out = run(c, Duration::from_secs(10))?;
    if !out.status.success() {
        return Err(error(
            ErrorCode::RuntimeError,
            "Docker not available. Cannot sign/validate packages.",
        ));
    }
    let names = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    names
        .iter()
        .find(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("aurora") && v.contains("build-tools")
        })
        .or_else(|| {
            names.iter().find(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("aurora")
                    && (v.contains("build") || v.contains("sdk") || v.contains("engine"))
            })
        })
        .cloned()
        .ok_or_else(|| {
            error(
                ErrorCode::NotFound,
                "No aurora-build-tools Docker image found",
            )
        })
}
fn keys(
    config: &EmulatorConfig,
    key: Option<&str>,
    cert: Option<&str>,
) -> Result<(PathBuf, PathBuf), AudbError> {
    if let (Some(k), Some(c)) = (key, cert) {
        return Ok((
            std::fs::canonicalize(k).map_err(|e| error(ErrorCode::NotFound, e.to_string()))?,
            std::fs::canonicalize(c).map_err(|e| error(ErrorCode::NotFound, e.to_string()))?,
        ));
    }
    if key.is_some() != cert.is_some() {
        return Err(error(
            ErrorCode::InvalidArgument,
            "--key and --cert must be provided together",
        ));
    }
    let sdk = (
        config.sdk_root.join("package-signing/regular_key.pem"),
        config.sdk_root.join("package-signing/regular_cert.pem"),
    );
    if sdk.0.exists() && sdk.1.exists() {
        let cache = cache_dir().map_err(|e| error(e.code, e.message))?;
        std::fs::create_dir_all(&cache)
            .map_err(|e| error(ErrorCode::InternalError, e.to_string()))?;
        let ck = cache.join("regular_key.pem");
        let cc = cache.join("regular_cert.pem");
        if !ck.exists() {
            std::fs::copy(&sdk.0, &ck)
                .map_err(|e| error(ErrorCode::InternalError, e.to_string()))?;
        }
        if !cc.exists() {
            std::fs::copy(&sdk.1, &cc)
                .map_err(|e| error(ErrorCode::InternalError, e.to_string()))?;
        }
        return Ok(sdk);
    }
    let cache = cache_dir().map_err(|e| error(e.code, e.message))?;
    let cached = (
        cache.join("regular_key.pem"),
        cache.join("regular_cert.pem"),
    );
    if cached.0.exists() && cached.1.exists() {
        return Ok(cached);
    }
    Err(error(
        ErrorCode::NotFound,
        "Aurora package signing keys not found in SDK or audb cache",
    ))
}
pub fn sign(
    config: &EmulatorConfig,
    rpm: &str,
    key: Option<&str>,
    cert: Option<&str>,
) -> Result<Value, AudbError> {
    let rpm = check_rpm(rpm)?;
    let (key, cert) = keys(config, key, cert)?;
    let image = image()?;
    let temp = tempfile::tempdir().map_err(|e| error(ErrorCode::InternalError, e.to_string()))?;
    let stage = temp.path().join("stage");
    std::fs::create_dir(&stage).map_err(|e| error(ErrorCode::InternalError, e.to_string()))?;
    let name = rpm.file_name().unwrap();
    for (src, dst) in [
        (&rpm, stage.join(name)),
        (&key, stage.join("regular_key.pem")),
        (&cert, stage.join("regular_cert.pem")),
    ] {
        std::fs::copy(src, dst).map_err(|e| error(ErrorCode::InternalError, e.to_string()))?;
    }
    let script=format!("rpmsign-external sign --force --key=/project/regular_key.pem --cert=/project/regular_cert.pem /project/{}",name.to_string_lossy());
    let mut c = Command::new("docker");
    c.args([
        "run",
        "--rm",
        "-v",
        &format!("{}:/project", stage.display()),
        &image,
        "/bin/bash",
        "-c",
        &script,
    ]);
    let out = run(c, Duration::from_secs(120))?;
    if !out.status.success() {
        return Err(error(
            ErrorCode::RuntimeError,
            format!(
                "rpmsign-external failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        ));
    }
    std::fs::copy(stage.join(name), &rpm)
        .map_err(|e| error(ErrorCode::InternalError, e.to_string()))?;
    Ok(json!({"signed":true,"rpm":rpm,"key":key,"cert":cert,"image":image}))
}
pub fn validate(rpm: &str) -> Result<Value, AudbError> {
    let rpm = check_rpm(rpm)?;
    let image = image()?;
    let dir = rpm.parent().unwrap_or(Path::new("."));
    let name = rpm.file_name().unwrap().to_string_lossy();
    let script = format!("rpm-validator -p regular /project/{name}");
    let mut c = Command::new("docker");
    c.args([
        "run",
        "--rm",
        "-v",
        &format!("{}:/project", dir.display()),
        &image,
        "/bin/bash",
        "-c",
        &script,
    ]);
    let out = run(c, Duration::from_secs(120))?;
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let passed = out.status.success() && !output.contains("(ERROR)");
    Ok(json!({"passed":passed,"rpm":rpm,"output":output.trim(),"image":image}))
}
