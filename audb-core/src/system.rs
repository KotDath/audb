use crate::error::{CoreError, CoreResult};
use crate::transport::{shell_quote, DeviceTransport};
use audb_protocol::LogsOptions;
use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;

fn variant(raw: &str) -> String {
    let raw = raw.trim();
    for q in ['\'', '"'] {
        if let Some(s) = raw.find(q) {
            if let Some(e) = raw[s + 1..].find(q) {
                return raw[s + 1..s + 1 + e].into();
            }
        }
    }
    raw.trim_matches(|c| "(), ".contains(c))
        .split(',')
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .into()
}
async fn device_info(t: &mut DeviceTransport, method: &str) -> CoreResult<String> {
    t.exec(&format!("gdbus call --system --dest ru.omp.deviceinfo --object-path /ru/omp/deviceinfo/Features --method ru.omp.deviceinfo.Features.{method}"),false).await
}
async fn mce(t: &mut DeviceTransport, method: &str) -> CoreResult<String> {
    t.exec(&format!("gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.{method}"),false).await
}

pub async fn info(t: &mut DeviceTransport, category: Option<&str>) -> CoreResult<Value> {
    let valid = ["device", "cpu", "memory", "storage", "battery", "features"];
    if let Some(c) = category {
        if !valid.contains(&c) {
            return Err(CoreError::invalid(format!("Unknown info category: {c}")));
        }
    }
    let mut result = serde_json::Map::new();
    if category.is_none() || category == Some("device") {
        result.insert("device".into(),json!({"model":variant(&device_info(t,"getDeviceModel").await?),"osVersion":variant(&device_info(t,"getOsVersion").await?),"screen":variant(&device_info(t,"getScreenResolution").await?)}));
    }
    if category.is_none() || category == Some("cpu") {
        let mut model = variant(&device_info(t, "getCpuModel").await?);
        if model.is_empty() {
            model = t
                .exec(
                    "awk -F ': ' '/^model name/{print $2; exit}' /proc/cpuinfo",
                    false,
                )
                .await?;
        }
        result.insert("cpu".into(),json!({"model":model,"cores":variant(&device_info(t,"getNumberCpuCores").await?).parse::<u64>().unwrap_or(0),"maxClockMhz":variant(&device_info(t,"getMaxCpuClockSpeed").await?).parse::<u64>().unwrap_or(0)}));
    }
    if category.is_none() || category == Some("memory") {
        let total = variant(&device_info(t, "getRamTotalSize").await?)
            .parse::<u64>()
            .unwrap_or(0);
        let raw=t.exec("awk '/MemAvailable/{a=$2} /MemFree/{f=$2} /^Buffers/{b=$2} /^Cached/{c=$2} END{print a,f,b,c}' /proc/meminfo",false).await?;
        let n: Vec<u64> = raw
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        result.insert("memory".into(),json!({"totalBytes":total,"availableKb":n.first(),"freeKb":n.get(1),"buffersKb":n.get(2),"cachedKb":n.get(3)}));
    }
    if category.is_none() || category == Some("storage") {
        let raw = t.exec("stat -f -c '%b %a %S' /home", false).await?;
        let n: Vec<u64> = raw
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        result.insert("storage".into(),json!({"totalBytes":n.first().zip(n.get(2)).map(|(a,b)|a*b),"availableBytes":n.get(1).zip(n.get(2)).map(|(a,b)|a*b)}));
    }
    if category.is_none() || category == Some("battery") {
        result.insert("battery".into(),json!({"level":variant(&mce(t,"get_battery_level").await?).parse::<i64>().unwrap_or(0),"charger":variant(&mce(t,"get_charger_state").await?)}));
    }
    if category.is_none() || category == Some("features") {
        let mut f = serde_json::Map::new();
        for (key, method) in [
            ("nfc", "hasNFC"),
            ("bluetooth", "hasBluetooth"),
            ("wlan", "hasWlan"),
            ("gnss", "hasGNSS"),
            ("mainCamera", "getMainCameraResolution"),
            ("frontalCamera", "getFrontalCameraResolution"),
        ] {
            f.insert(key.into(), json!(variant(&device_info(t, method).await?)));
        }
        result.insert("features".into(), Value::Object(f));
    }
    Ok(Value::Object(result))
}

fn priority(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "v" | "d" | "debug" => Some("debug"),
        "i" | "info" => Some("info"),
        "notice" => Some("notice"),
        "w" | "warning" => Some("warning"),
        "e" | "err" | "error" => Some("err"),
        "f" | "crit" | "fatal" => Some("crit"),
        "alert" => Some("alert"),
        "emerg" => Some("emerg"),
        _ => None,
    }
}
pub async fn logs(t: &mut DeviceTransport, o: LogsOptions) -> CoreResult<String> {
    if o.kernel && o.unit.is_some() {
        return Err(CoreError::invalid(
            "--kernel and --unit are mutually exclusive",
        ));
    }
    if o.clear {
        if !o.force {
            return Err(CoreError::invalid("--clear requires --force"));
        }
        t.exec("journalctl --rotate && journalctl --vacuum-time=1s", true)
            .await?;
        return Ok("Logs cleared.".into());
    }
    let mut p = vec!["journalctl".into()];
    if o.kernel {
        p.push("-k".into())
    }
    p.extend(["-n".into(), o.lines.min(100_000).to_string()]);
    if let Some(raw) = o.priority.as_deref() {
        let value = priority(raw)
            .ok_or_else(|| CoreError::invalid(format!("Unknown log priority: {raw}")))?;
        p.extend(["-p".into(), value.into()])
    }
    if let Some(v) = o.unit {
        p.extend(["-u".into(), shell_quote(&v)])
    }
    if let Some(v) = o.since {
        p.extend(["--since".into(), shell_quote(&v)])
    }
    p.extend(["--no-pager".into(), "--no-hostname".into()]);
    let mut cmd = p.join(" ");
    if let Some(v) = o.grep {
        cmd.push_str(&format!(" | grep {} || true", shell_quote(&v)))
    }
    t.exec(&cmd, true).await
}

const APM: &str = "gdbus call --system --dest ru.omp.APM --object-path /ru/omp/APM --method";
pub async fn package_list(t: &mut DeviceTransport, filter: Option<&str>) -> CoreResult<Value> {
    let raw = t
        .exec(&format!("{APM} ru.omp.APM.GetPackageList"), false)
        .await?;
    let dict = Regex::new(r"'general\.id'\s*:\s*'([^']*)'").unwrap();
    let simple = Regex::new(r"'([\w.\-]+)'").unwrap();
    let mut ids: Vec<String> = dict
        .captures_iter(&raw)
        .filter_map(|c| c.get(1).map(|m| m.as_str().into()))
        .collect();
    if ids.is_empty() {
        ids = simple
            .captures_iter(&raw)
            .filter_map(|c| c.get(1).map(|m| m.as_str().into()))
            .collect();
    }
    ids.sort();
    ids.dedup();
    if let Some(f) = filter {
        let f = f.to_ascii_lowercase();
        ids.retain(|v| v.to_ascii_lowercase().contains(&f));
    }
    Ok(json!({"packages":ids,"count":ids.len(),"filter":filter}))
}
/// Upload RPM contents over SFTP and confirm its APM registration. The daemon
/// request contains a path, never a JSON array of package bytes.
pub async fn package_install(
    t: &mut DeviceTransport,
    local: &Path,
    timeout: std::time::Duration,
) -> CoreResult<Value> {
    use crate::system_package::{parse_info, StageCleanup, QUERY_FORMAT};
    use audb_protocol::ErrorCode;
    use tokio::io::AsyncReadExt;
    if local.extension().and_then(|v| v.to_str()) != Some("rpm") {
        return Err(CoreError::invalid("File must be .rpm"));
    }
    if timeout < std::time::Duration::from_secs(1) || timeout > std::time::Duration::from_secs(3600)
    {
        return Err(CoreError::invalid(
            "Package registration timeout must be 1..3600 seconds",
        ));
    }
    let mut source = tokio::fs::File::open(local).await.map_err(|e| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("Cannot open RPM {}: {e}", local.display()),
        )
    })?;
    if !source.metadata().await?.is_file() {
        return Err(CoreError::invalid("RPM must be a regular file"));
    }
    let mut lead = [0u8; 4];
    source
        .read_exact(&mut lead)
        .await
        .map_err(|_| CoreError::invalid("File has no RPM header"))?;
    if lead != [0xed, 0xab, 0xee, 0xdb] {
        return Err(CoreError::invalid("File has no RPM header"));
    }
    drop(source);
    let dir = t
        .exec("umask 077 && mktemp -d /tmp/audb-app-rpm.XXXXXX", false)
        .await?;
    let suffix = dir
        .strip_prefix("/tmp/audb-app-rpm.")
        .filter(|s| s.len() == 6 && s.bytes().all(|c| c.is_ascii_alphanumeric()));
    if suffix.is_none() {
        return Err(CoreError::runtime(
            "Invalid application RPM staging directory",
        ));
    }
    let remote = format!("{dir}/package.rpm");
    let cleanup = format!(
        "rm -f -- {} && rmdir -- {}",
        shell_quote(&remote),
        shell_quote(&dir)
    );
    let mut guard = StageCleanup {
        config: Some(t.config().clone()),
        command: cleanup.clone(),
    };
    let mut dispatched = false;
    let result = async {
        let bytes = t.upload_file(local,Path::new(&remote)).await?;
        let package = parse_info(&t.exec(&format!("rpm -qp --queryformat {} -- {}", shell_quote(QUERY_FORMAT),shell_quote(&remote)),false).await?)?;
        let arch = t.exec("rpm --eval '%{_arch}'",false).await?;
        if package.arch != "noarch" && package.arch != arch {
            return Err(CoreError::invalid(format!("RPM architecture '{}' does not match device '{}'",package.arch,arch)));
        }
        let version = format!("{}-{}",package.version,package.release);
        let query = format!("{APM} ru.omp.APM.GetPackage {}",shell_quote(&package.name));
        let current = apm_package(t,&query).await?;
        if current.as_ref().is_some_and(|v| v.get("general.id")==Some(&package.name) && v.get("general.version")==Some(&version)) {
            return Ok(json!({"package":package.name,"rpm":package,"installed":true,"verified":true,
                "alreadyInstalled":true,"changed":false,"bytes":bytes,"verificationBackend":"APM.GetPackage"}));
        }
        // Once APM may have received the request, don't delete its queued source
        // on cancellation or an unknown outcome. Installation is never replayed.
        dispatched = true;
        guard.config = None;
        let response = t.exec(&format!("{APM} ru.omp.APM.Install {} {}", shell_quote(&remote),shell_quote("{}")),false).await?;
        let deadline = std::time::Instant::now()+timeout;
        let mut observed = None;
        loop {
            let read = tokio::time::timeout(deadline.saturating_duration_since(std::time::Instant::now()),apm_package(t,&query)).await;
            match read {
                Ok(Ok(Some(info))) => {
                    if info.get("general.id")==Some(&package.name) && info.get("general.version")==Some(&version) {
                        return Ok(json!({"package":package.name,"rpm":package,"installed":true,"verified":true,
                            "alreadyInstalled":false,"changed":true,"bytes":bytes,"response":response,"verificationBackend":"APM.GetPackage"}));
                    }
                    observed = Some(info);
                }
                Ok(Ok(None)) => {},
                Ok(Err(e)) => return Err(CoreError::new(ErrorCode::OutcomeUnknown,format!("APM installation requested, but verification failed: {}",e.message))),
                Err(_) => break,
            }
            if std::time::Instant::now()>=deadline { break; }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        Err(CoreError::new(ErrorCode::OutcomeUnknown,"APM installation was requested but the expected version was not registered before the deadline; installation was not repeated")
            .with_data(json!({"phase":"verify","requested":package,"observed":observed})))
    }.await;
    let result = match result {
        Err(mut error) if dispatched => {
            let mut data = error.data.take().unwrap_or_else(|| json!({}));
            data["installationRequested"] = json!(true);
            data["stagingRetained"] = json!(true);
            data["stagingDirectory"] = json!(dir);
            data["retried"] = json!(false);
            error.data = Some(data);
            return Err(error);
        }
        other => other,
    };
    let removed = t.exec(&cleanup, false).await;
    guard.config = None;
    match result {
        Ok(mut value) => {
            value["stagingCleanup"] = json!(removed.is_ok());
            if let Err(e) = removed {
                value["cleanupError"] = json!({"message":e.message,"directory":dir});
            }
            Ok(value)
        }
        Err(mut e) => {
            let mut data = e.data.take().unwrap_or_else(|| json!({}));
            data["stagingCleanup"] = json!(removed.is_ok());
            e.data = Some(data);
            Err(e)
        }
    }
}

/// APM returns a{ss}. Package metadata values use simple IDs/versions, but the
/// parser handles GVariant quote/backslash escaping without invoking a shell.
fn parse_apm_package(raw: &str) -> CoreResult<std::collections::BTreeMap<String, String>> {
    let pattern = Regex::new(r#"'((?:[^'\\]|\\.)*)'\s*:\s*'((?:[^'\\]|\\.)*)'"#).unwrap();
    let fields: std::collections::BTreeMap<_, _> = pattern
        .captures_iter(raw)
        .map(|c| {
            (
                c[1].replace("\\'", "'").replace("\\\\", "\\"),
                c[2].replace("\\'", "'").replace("\\\\", "\\"),
            )
        })
        .collect();
    if !fields.contains_key("general.id") || !fields.contains_key("general.version") {
        return Err(CoreError::runtime("APM package response has no ID/version"));
    }
    Ok(fields)
}
async fn apm_package(
    t: &mut DeviceTransport,
    query: &str,
) -> CoreResult<Option<std::collections::BTreeMap<String, String>>> {
    match t.exec(query, false).await {
        Ok(raw) => Ok(Some(parse_apm_package(&raw)?)),
        Err(e)
            if e.code == audb_protocol::ErrorCode::RemoteCommandFailed
                && e.message.contains("ru.omp.APM.Error.PackageNotExist") =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

pub async fn package_uninstall(t: &mut DeviceTransport, package: &str) -> CoreResult<Value> {
    if package.is_empty() {
        return Err(CoreError::invalid("Package name required"));
    }
    let response = t
        .exec(
            &format!(
                "{APM} ru.omp.APM.Remove {} {}",
                shell_quote(package),
                shell_quote("{}")
            ),
            false,
        )
        .await?;
    Ok(json!({"package":package,"uninstalled":true,"response":response}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variants() {
        assert_eq!(variant("(42,)"), "42");
        assert_eq!(variant("(uint32 2,)"), "2");
        assert_eq!(variant("(uint64 4116619264,)"), "4116619264");
        assert_eq!(priority("E"), Some("err"));
    }
    #[test]
    fn apm_verification_requires_id_and_version_and_handles_quoted_values() {
        let package=parse_apm_package("({'general.id': 'ru.test.Probe', 'general.version': '0.1.0+1-1', 'label': 'User\\'s test'},)").unwrap();
        assert_eq!(package["general.version"], "0.1.0+1-1");
        assert_eq!(package["label"], "User's test");
        assert!(parse_apm_package("({'general.id':'ru.test.Probe'},)").is_err());
        assert!(parse_apm_package("invalid output").is_err());
    }
}
