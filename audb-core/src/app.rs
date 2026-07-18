use crate::error::{CoreError, CoreResult};
use crate::transport::{shell_quote, EmulatorTransport};
use regex::Regex;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const PREFIX: &str = "gdbus call --system --dest ru.omp.RuntimeManager --object-path /ru/omp/RuntimeManager/Control1 --method ru.omp.RuntimeManager.Control1";

pub fn validate_package(package: &str) -> CoreResult<()> {
    let valid = package.contains('.')
        && package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    if valid {
        Ok(())
    } else {
        Err(CoreError::invalid(format!(
            "Invalid package name: {package:?}"
        )))
    }
}

async fn call(
    transport: &mut EmulatorTransport,
    method: &str,
    argument: Option<&str>,
) -> CoreResult<String> {
    let suffix = argument
        .map(|v| format!(" {}", shell_quote(v)))
        .unwrap_or_default();
    transport
        .exec(&format!("{PREFIX}.{method}{suffix}"), false)
        .await
}

pub fn parse_running(raw: &str) -> Vec<Value> {
    let regex = Regex::new(r"\('([^']+)',\s*(-?\d+),").unwrap();
    regex.captures_iter(raw).filter_map(|capture| Some(json!({"package": capture.get(1)?.as_str(), "pid": capture.get(2)?.as_str().parse::<i64>().ok()?}))).collect()
}

pub async fn list(transport: &mut EmulatorTransport) -> CoreResult<Vec<Value>> {
    Ok(parse_running(
        &call(transport, "GetRunningApplications", None).await?,
    ))
}

pub async fn pid(transport: &mut EmulatorTransport, package: &str) -> CoreResult<Option<i64>> {
    validate_package(package)?;
    Ok(list(transport)
        .await?
        .into_iter()
        .find(|item| item["package"] == package)
        .and_then(|item| item["pid"].as_i64()))
}

pub async fn launch(transport: &mut EmulatorTransport, package: &str) -> CoreResult<Value> {
    validate_package(package)?;
    let raw = call(transport, "Start", Some(package)).await?;
    Ok(json!({"package": package, "launched": true, "response": raw}))
}

pub async fn stop(transport: &mut EmulatorTransport, package: &str) -> CoreResult<Value> {
    validate_package(package)?;
    let raw = call(transport, "Terminate", Some(package)).await?;
    Ok(json!({"package": package, "stopped": true, "response": raw}))
}

pub async fn wait(
    transport: &mut EmulatorTransport,
    package: &str,
    running: bool,
    timeout: Duration,
    interval: Duration,
) -> CoreResult<Value> {
    validate_package(package)?;
    let deadline = Instant::now() + timeout;
    loop {
        let current = pid(transport, package).await?;
        if current.is_some() == running || Instant::now() >= deadline {
            return Ok(
                json!({"package":package,"running":current.is_some(),"pid":current,"matched":current.is_some() == running}),
            );
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_runtime_manager_shape() {
        let rows = parse_running("([('ru.test.App', 123, 0, 0, {})], [('pending', 'x')])");
        assert_eq!(rows[0]["pid"], 123);
    }
    #[test]
    fn rejects_shell_injection() {
        assert!(validate_package("x;rm -rf /").is_err());
    }
}
