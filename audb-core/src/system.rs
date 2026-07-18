use crate::error::{CoreError, CoreResult};
use crate::transport::{shell_quote, EmulatorTransport};
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
async fn device_info(t: &mut EmulatorTransport, method: &str) -> CoreResult<String> {
    t.exec(&format!("gdbus call --system --dest ru.omp.deviceinfo --object-path /ru/omp/deviceinfo/Features --method ru.omp.deviceinfo.Features.{method}"),false).await
}
async fn mce(t: &mut EmulatorTransport, method: &str) -> CoreResult<String> {
    t.exec(&format!("gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.{method}"),false).await
}

pub async fn info(t: &mut EmulatorTransport, category: Option<&str>) -> CoreResult<Value> {
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
pub async fn logs(t: &mut EmulatorTransport, o: LogsOptions) -> CoreResult<String> {
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
pub async fn package_list(t: &mut EmulatorTransport, filter: Option<&str>) -> CoreResult<Value> {
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
pub async fn package_install(
    t: &mut EmulatorTransport,
    name: &str,
    bytes: &[u8],
) -> CoreResult<Value> {
    if !name.ends_with(".rpm") {
        return Err(CoreError::invalid("File must be .rpm"));
    }
    let filename = Path::new(name)
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| CoreError::invalid("Invalid RPM filename"))?;
    let remote = format!("/home/{}/Downloads/{filename}", t.config().ssh_user);
    t.upload_bytes(Path::new(&remote), bytes).await?;
    let result = t
        .exec(
            &format!(
                "{APM} ru.omp.APM.Install {} {}",
                shell_quote(&remote),
                shell_quote("{}")
            ),
            false,
        )
        .await;
    let _ = t
        .exec(&format!("rm -f {}", shell_quote(&remote)), false)
        .await;
    Ok(json!({"package":filename,"installed":true,"response":result?}))
}
pub async fn package_uninstall(t: &mut EmulatorTransport, package: &str) -> CoreResult<Value> {
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
}
