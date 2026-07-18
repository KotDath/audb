use crate::app;
use crate::config::cache_dir;
use crate::error::{CoreError, CoreResult};
use crate::qmp::QmpClient;
use crate::screenshot;
use crate::transport::{shell_quote, EmulatorTransport};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn numbers(raw: &str) -> HashMap<String, u64> {
    raw.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((k.into(), v.trim().parse().ok()?))
        })
        .collect()
}
async fn sample(t: &mut EmulatorTransport, package: &str) -> CoreResult<HashMap<String, u64>> {
    app::validate_package(package)?;
    let base = "/sys/fs/cgroup";
    let scope = "user.slice/runtime-manager-helper.service/user-100000";
    let cpu = format!("{base}/cpu,cpuacct/{scope}/{package}/app");
    let mem = format!("{base}/memory/{scope}/{package}/app");
    let io = format!("{base}/blkio/{scope}/{package}/app");
    let command=format!("test -d {cpu:?} || exit 44; echo cpu_ns=$(cat {cpu:?}/cpuacct.usage); echo cpu_user_ticks=$(awk '/^user /{{print $2}}' {cpu:?}/cpuacct.stat); echo cpu_system_ticks=$(awk '/^system /{{print $2}}' {cpu:?}/cpuacct.stat); echo memory_bytes=$(cat {mem:?}/memory.usage_in_bytes); echo memory_peak_bytes=$(cat {mem:?}/memory.max_usage_in_bytes); echo rss_bytes=$(awk '/^rss /{{print $2}}' {mem:?}/memory.stat); echo cache_bytes=$(awk '/^cache /{{print $2}}' {mem:?}/memory.stat); echo page_faults=$(awk '/^pgfault /{{print $2}}' {mem:?}/memory.stat); echo major_page_faults=$(awk '/^pgmajfault /{{print $2}}' {mem:?}/memory.stat); echo oom_events=$(cat {mem:?}/memory.failcnt); echo processes=$(wc -l < {cpu:?}/cgroup.procs); pids=$(cat {cpu:?}/cgroup.procs); threads=0; fds=0; for p in $pids; do test -d /proc/$p/task && threads=$((threads+$(ls -1 /proc/$p/task | wc -l))); test -d /proc/$p/fd && fds=$((fds+$(ls -1 /proc/$p/fd | wc -l))); done; echo threads=$threads; echo open_fds=$fds; if test -f {io:?}/blkio.throttle.io_service_bytes; then awk '$2==\"Read\"{{r+=$3}} $2==\"Write\"{{w+=$3}} END{{print \"io_read_bytes=\"r; print \"io_write_bytes=\"w}}' {io:?}/blkio.throttle.io_service_bytes; else echo io_read_bytes=0; echo io_write_bytes=0; fi; echo cpu_cores=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)");
    match t.exec(&command, true).await {
        Ok(v) => Ok(numbers(&v)),
        Err(e) if e.message.contains("rc=44") => Err(CoreError::new(
            audb_protocol::ErrorCode::AppNotRunning,
            format!("Application is not running: {package}"),
        )),
        Err(e) => Err(e),
    }
}
fn get(v: &HashMap<String, u64>, key: &str) -> u64 {
    v.get(key).copied().unwrap_or(0)
}
pub async fn perf_snapshot(
    t: &mut EmulatorTransport,
    package: &str,
    interval: Duration,
) -> CoreResult<Value> {
    if interval.is_zero() || interval > Duration::from_secs(10) {
        return Err(CoreError::invalid(
            "sample interval must be > 0 and <= 10 seconds",
        ));
    }
    let started = Instant::now();
    let first = sample(t, package).await?;
    tokio::time::sleep(interval).await;
    let second = sample(t, package).await?;
    let wall = started.elapsed().as_nanos().max(1) as f64;
    let cores = get(&second, "cpu_cores").max(1);
    let cpu =
        get(&second, "cpu_ns").saturating_sub(get(&first, "cpu_ns")) as f64 / wall / cores as f64
            * 100.0;
    Ok(
        json!({"package":package,"timestamp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64(),"sampleIntervalMs":wall/1_000_000.0,"cpuPercent":cpu,"cpuCores":cores,"cpuTotalNs":get(&second,"cpu_ns"),"cpuUserTicks":get(&second,"cpu_user_ticks"),"cpuSystemTicks":get(&second,"cpu_system_ticks"),"memoryBytes":get(&second,"memory_bytes"),"memoryPeakBytes":get(&second,"memory_peak_bytes"),"rssBytes":get(&second,"rss_bytes"),"cacheBytes":get(&second,"cache_bytes"),"processes":get(&second,"processes"),"threads":get(&second,"threads"),"openFiles":get(&second,"open_fds"),"pageFaults":get(&second,"page_faults"),"majorPageFaults":get(&second,"major_page_faults"),"oomEvents":get(&second,"oom_events"),"ioReadBytes":get(&second,"io_read_bytes"),"ioWriteBytes":get(&second,"io_write_bytes")}),
    )
}
pub async fn perf_monitor(
    t: &mut EmulatorTransport,
    package: &str,
    duration: Duration,
    interval: Duration,
) -> CoreResult<Value> {
    if duration.is_zero() || duration > Duration::from_secs(300) {
        return Err(CoreError::invalid(
            "duration must be > 0 and <= 300 seconds",
        ));
    }
    if interval.is_zero() || interval > duration {
        return Err(CoreError::invalid("interval must be > 0 and <= duration"));
    }
    let started = Instant::now();
    let mut samples = Vec::new();
    while started.elapsed() < duration {
        samples.push(
            perf_snapshot(
                t,
                package,
                interval.min(duration.saturating_sub(started.elapsed())),
            )
            .await?,
        );
    }
    let cpus: Vec<f64> = samples
        .iter()
        .filter_map(|v| v["cpuPercent"].as_f64())
        .collect();
    let mems: Vec<u64> = samples
        .iter()
        .filter_map(|v| v["memoryBytes"].as_u64())
        .collect();
    Ok(
        json!({"package":package,"durationMs":started.elapsed().as_secs_f64()*1000.0,"summary":{"sampleCount":samples.len(),"cpuAveragePercent":if cpus.is_empty(){0.0}else{cpus.iter().sum::<f64>()/cpus.len()as f64},"cpuMaxPercent":cpus.iter().copied().fold(0.0,f64::max),"memoryAverageBytes":if mems.is_empty(){0}else{mems.iter().sum::<u64>()/mems.len()as u64},"memoryMaxBytes":mems.iter().copied().max().unwrap_or(0)},"samples":samples}),
    )
}

pub async fn visual(
    t: &mut EmulatorTransport,
    q: &mut QmpClient,
    duration: Duration,
    interval: Duration,
    threshold: Duration,
) -> CoreResult<Value> {
    if duration.is_zero()
        || duration > Duration::from_secs(60)
        || interval < Duration::from_millis(50)
        || interval > duration
        || threshold < interval
        || threshold > duration
    {
        return Err(CoreError::invalid(
            "invalid visual-fps duration, interval, or freeze threshold",
        ));
    }
    let started = Instant::now();
    let mut frames = Vec::new();
    loop {
        let data = screenshot::capture(t, q).await?;
        frames.push((Instant::now(), Sha256::digest(&data).to_vec()));
        if started.elapsed() >= duration {
            break;
        }
        tokio::time::sleep(interval.min(duration.saturating_sub(started.elapsed()))).await;
    }
    let elapsed = started.elapsed();
    let changes = frames.windows(2).filter(|v| v[0].1 != v[1].1).count();
    let mut static_start = frames[0].0;
    let mut longest = Duration::ZERO;
    for pair in frames.windows(2) {
        if pair[0].1 != pair[1].1 {
            longest = longest.max(pair[1].0.duration_since(static_start));
            static_start = pair[1].0;
        }
    }
    longest = longest.max(Instant::now().duration_since(static_start));
    Ok(
        json!({"backend":"lipstick-dbus/qmp-screendump","rendererFps":Value::Null,"metric":"changed-frame-rate","durationMs":elapsed.as_secs_f64()*1000.0,"samples":frames.len(),"sampleRateHz":frames.len()as f64/elapsed.as_secs_f64(),"changedFrames":changes,"changedFrameRateHz":changes as f64/elapsed.as_secs_f64(),"longestStaticMs":longest.as_secs_f64()*1000.0,"freezeThresholdMs":threshold.as_secs_f64()*1000.0,"freezeDetected":longest>=threshold,"limitations":"Detects visible pixel changes only; this is not Qt renderer FPS."}),
    )
}

fn marker_path() -> CoreResult<std::path::PathBuf> {
    Ok(cache_dir()?.join("crash-markers.json"))
}
fn markers() -> CoreResult<serde_json::Map<String, Value>> {
    let path = marker_path()?;
    if !path.exists() {
        return Ok(Default::default());
    }
    Ok(serde_json::from_slice::<Value>(&std::fs::read(path)?)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default())
}
pub fn crash_clear(package: Option<&str>) -> CoreResult<Value> {
    if let Some(v) = package {
        app::validate_package(v)?
    }
    let mut data = markers()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    data.insert(format!("emulator:{}", package.unwrap_or("*")), json!(now));
    let path = marker_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?
    }
    std::fs::write(path, serde_json::to_vec_pretty(&data)?)?;
    Ok(json!({"package":package,"clearedAt":now,"journalModified":false}))
}
fn crash_since(package: Option<&str>, since: Option<&str>) -> CoreResult<String> {
    if let Some(v) = since {
        return Ok(v.into());
    }
    let data = markers()?;
    let exact = data
        .get(&format!("emulator:{}", package.unwrap_or("*")))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let wildcard = data
        .get("emulator:*")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let value = exact.max(wildcard);
    Ok(if value > 0.0 {
        format!("@{value}")
    } else {
        "24 hours ago".into()
    })
}
pub async fn crash_list(
    t: &mut EmulatorTransport,
    package: Option<&str>,
    since: Option<&str>,
    lines: usize,
) -> CoreResult<Value> {
    if let Some(v) = package {
        app::validate_package(v)?
    }
    let since = crash_since(package, since)?;
    let raw = t
        .exec(
            &format!(
                "journalctl --since {} -n {} --no-pager --no-hostname -o short-iso",
                shell_quote(&since),
                lines.clamp(1, 20_000)
            ),
            true,
        )
        .await?;
    let patterns = [
        (
            "oom",
            Regex::new("(?i)out of memory|oom-kill|killed process").unwrap(),
        ),
        ("segfault", Regex::new("(?i)segfault|sigsegv").unwrap()),
        (
            "abort",
            Regex::new("(?i)sigabrt|aborted|assert(?:ion)? failed").unwrap(),
        ),
        (
            "fatal",
            Regex::new("(?i)\\bfatal\\b|uncaught exception|core dumped").unwrap(),
        ),
        (
            "terminated",
            Regex::new("(?i)RuntimeManager.*(?:terminated|failed)|exit status").unwrap(),
        ),
    ];
    let events = raw
        .lines()
        .filter(|l| {
            package.is_none_or(|p| l.to_ascii_lowercase().contains(&p.to_ascii_lowercase()))
        })
        .filter_map(|line| {
            patterns
                .iter()
                .find(|(_, r)| r.is_match(line))
                .map(|(kind, _)| json!({"type":kind,"line":line}))
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"package":package,"since":since,"count":events.len(),"events":events,"nativeBacktraceAvailable":false}),
    )
}
pub async fn crash_watch(
    t: &mut EmulatorTransport,
    package: &str,
    timeout: Duration,
    interval: Duration,
) -> CoreResult<Value> {
    app::validate_package(package)?;
    if timeout.is_zero() || timeout > Duration::from_secs(300) {
        return Err(CoreError::invalid("timeout must be > 0 and <= 300 seconds"));
    }
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let running = app::pid(t, package).await?.is_some();
    let started = Instant::now();
    while started.elapsed() < timeout {
        let found = crash_list(t, Some(package), Some(&format!("@{wall}")), 2000).await?;
        if found["count"].as_u64().unwrap_or(0) > 0 {
            return Ok(
                json!({"detected":true,"reason":"journal","package":package,"events":found["events"],"count":found["count"],"nativeBacktraceAvailable":false}),
            );
        }
        if running && app::pid(t, package).await?.is_none() {
            return Ok(
                json!({"detected":true,"reason":"unexpected-stop","package":package,"events":[],"count":0,"nativeBacktraceAvailable":false}),
            );
        }
        tokio::time::sleep(interval).await;
    }
    Ok(
        json!({"detected":false,"reason":"timeout","package":package,"events":[],"count":0,"nativeBacktraceAvailable":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_numbers() {
        assert_eq!(numbers("a=4\nb=x").get("a"), Some(&4));
    }
}
