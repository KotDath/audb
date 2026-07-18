use crate::error::{CoreError, CoreResult};
use crate::transport::{shell_quote, EmulatorTransport};
use audb_protocol::TrackPosition;
use regex::Regex;
use serde_json::{json, Value};

async fn dbus(
    transport: &mut EmulatorTransport,
    destination: &str,
    object: &str,
    interface: &str,
    method: &str,
    args: &str,
    root: bool,
) -> CoreResult<String> {
    let suffix = if args.is_empty() {
        String::new()
    } else {
        format!(" {args}")
    };
    transport.exec(&format!("gdbus call --system --dest {destination} --object-path {object} --method {interface}.{method}{suffix}"), root).await
}

fn property_string(raw: &str, key: &str) -> Option<String> {
    Regex::new(&format!(r"'{}':\s*<'([^']*)'>", regex::escape(key)))
        .ok()?
        .captures(raw)?
        .get(1)
        .map(|v| v.as_str().into())
}
fn property_bool(raw: &str, key: &str) -> Option<bool> {
    Regex::new(&format!(r"'{}':\s*<(true|false)>", regex::escape(key)))
        .ok()?
        .captures(raw)?
        .get(1)
        .map(|v| v.as_str() == "true")
}
fn section_string(raw: &str, section: &str, key: &str) -> Option<String> {
    let section = Regex::new(&format!(
        r"(?s)'{}':\s*<\{{(.*?)\}}>",
        regex::escape(section)
    ))
    .ok()?
    .captures(raw)?
    .get(1)?
    .as_str()
    .to_string();
    property_string(&section, key)
}

async fn connman(
    transport: &mut EmulatorTransport,
    interface: &str,
    method: &str,
    args: &str,
    root: bool,
) -> CoreResult<String> {
    dbus(transport, "net.connman", "/", interface, method, args, root).await
}

pub async fn network_status(transport: &mut EmulatorTransport) -> CoreResult<Value> {
    let manager = connman(transport, "net.connman.Manager", "GetProperties", "", false).await?;
    let services = connman(transport, "net.connman.Manager", "GetServices", "", false).await?;
    let nameservers: Vec<Value> = Regex::new(r"(?s)'Nameservers':\s*<\[([^]]*)\]>")
        .unwrap()
        .captures(&services)
        .and_then(|c| c.get(1))
        .map(|m| {
            Regex::new(r"'([^']+)'")
                .unwrap()
                .captures_iter(m.as_str())
                .filter_map(|c| c.get(1).map(|m| Value::String(m.as_str().into())))
                .collect()
        })
        .unwrap_or_default();
    let proxy = Regex::new(r"(?s)'Proxy':\s*<\{(.*?)\}>")
        .unwrap()
        .captures(&services)
        .and_then(|c| c.get(1))
        .and_then(|m| property_string(m.as_str(), "Method"));
    Ok(
        json!({"state":property_string(&manager,"State"),"offline":property_bool(&manager,"OfflineMode"),"service":{"type":property_string(&services,"Type"),"name":property_string(&services,"Name"),"state":property_string(&services,"State"),"interface":property_string(&services,"Interface"),"address":section_string(&services,"IPv4","Address"),"netmask":section_string(&services,"IPv4","Netmask"),"gateway":section_string(&services,"IPv4","Gateway"),"nameservers":nameservers,"proxyMethod":proxy}}),
    )
}

pub async fn network_interfaces(transport: &mut EmulatorTransport) -> CoreResult<Value> {
    let raw = transport.exec("for i in /sys/class/net/*; do n=${i##*/}; test \"$n\" = lo && continue; echo IFACE=$n; cat $i/operstate 2>/dev/null; cat $i/address 2>/dev/null; cat $i/mtu 2>/dev/null; done", true).await?;
    let mut result = Vec::new();
    let mut current: Option<serde_json::Map<String, Value>> = None;
    for line in raw.lines() {
        if let Some(name) = line.strip_prefix("IFACE=") {
            if let Some(v) = current.take() {
                result.push(Value::Object(v));
            }
            let mut v = serde_json::Map::new();
            v.insert("name".into(), json!(name));
            current = Some(v);
        } else if let Some(v) = current.as_mut() {
            if !v.contains_key("state") {
                v.insert("state".into(), json!(line));
            } else if !v.contains_key("address") {
                v.insert("address".into(), json!(line));
            } else {
                v.insert("mtu".into(), json!(line.parse::<u64>().unwrap_or(0)));
            }
        }
    }
    if let Some(v) = current {
        result.push(Value::Object(v));
    }
    Ok(Value::Array(result))
}

pub async fn network_traffic(transport: &mut EmulatorTransport) -> CoreResult<Value> {
    let raw=transport.exec("for i in /sys/class/net/*; do n=${i##*/}; test \"$n\" = lo && continue; echo $n $(cat $i/statistics/rx_bytes) $(cat $i/statistics/tx_bytes) $(cat $i/statistics/rx_packets) $(cat $i/statistics/tx_packets) $(cat $i/statistics/rx_errors) $(cat $i/statistics/tx_errors); done",true).await?;
    Ok(Value::Array(raw.lines().filter_map(|line|{let f:Vec<_>=line.split_whitespace().collect();if f.len()!=7{return None}Some(json!({"interface":f[0],"rxBytes":f[1].parse::<u64>().ok()?,"txBytes":f[2].parse::<u64>().ok()?,"rxPackets":f[3].parse::<u64>().ok()?,"txPackets":f[4].parse::<u64>().ok()?,"rxErrors":f[5].parse::<u64>().ok()?,"txErrors":f[6].parse::<u64>().ok()?}))}).collect()))
}

pub async fn proxy_get(transport: &mut EmulatorTransport) -> CoreResult<Value> {
    let active = connman(
        transport,
        "org.sailfishos.connman.GlobalProxy",
        "GetProperty",
        &shell_quote("Active"),
        false,
    )
    .await?;
    let config = connman(
        transport,
        "org.sailfishos.connman.GlobalProxy",
        "GetProperty",
        &shell_quote("Configuration"),
        false,
    )
    .await?;
    let servers = Regex::new(r"https?://[^'\], ]+")
        .unwrap()
        .find_iter(&config)
        .map(|m| json!(m.as_str()))
        .collect::<Vec<_>>();
    Ok(
        json!({"active":active.contains("<true>"),"method":property_string(&config,"Method"),"servers":servers,"raw":config}),
    )
}
pub async fn proxy_set(
    transport: &mut EmulatorTransport,
    host: &str,
    port: u16,
) -> CoreResult<Value> {
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c))
    {
        return Err(CoreError::invalid("Invalid proxy host or port"));
    }
    let config = format!("<{{'Method': <'manual'>, 'Servers': <['http://{host}:{port}']}}>");
    connman(
        transport,
        "org.sailfishos.connman.GlobalProxy",
        "SetProperty",
        &format!("{} {}", shell_quote("Configuration"), shell_quote(&config)),
        true,
    )
    .await?;
    connman(
        transport,
        "org.sailfishos.connman.GlobalProxy",
        "SetProperty",
        &format!("{} {}", shell_quote("Active"), shell_quote("<true>")),
        true,
    )
    .await?;
    proxy_get(transport).await
}
pub async fn proxy_clear(transport: &mut EmulatorTransport) -> CoreResult<Value> {
    connman(
        transport,
        "org.sailfishos.connman.GlobalProxy",
        "SetProperty",
        &format!(
            "{} {}",
            shell_quote("Configuration"),
            shell_quote("<{'Method': <'direct'>}>")
        ),
        true,
    )
    .await?;
    connman(
        transport,
        "org.sailfishos.connman.GlobalProxy",
        "SetProperty",
        &format!("{} {}", shell_quote("Active"), shell_quote("<false>")),
        true,
    )
    .await?;
    proxy_get(transport).await
}
pub async fn offline(transport: &mut EmulatorTransport, enabled: bool) -> CoreResult<Value> {
    let value = if enabled { "true" } else { "false" };
    connman(
        transport,
        "net.connman.Manager",
        "SetProperty",
        &format!(
            "{} {}",
            shell_quote("OfflineMode"),
            shell_quote(&format!("<{value}>"))
        ),
        true,
    )
    .await?;
    Ok(json!({"offlineRequested":enabled}))
}

async fn geo(transport: &mut EmulatorTransport, method: &str, args: &str) -> CoreResult<String> {
    dbus(
        transport,
        "ru.omp.GeoclueEmulationManagement",
        "/ru/omp/GeoclueEmulationManagement",
        "ru.omp.GeoclueEmulationManagement",
        method,
        args,
        true,
    )
    .await
}
pub async fn location_set(
    transport: &mut EmulatorTransport,
    lat: f64,
    lon: f64,
    alt: f64,
) -> CoreResult<Value> {
    if !lat.is_finite()
        || !lon.is_finite()
        || !alt.is_finite()
        || !(-90.0..=90.0).contains(&lat)
        || !(-180.0..=180.0).contains(&lon)
    {
        return Err(CoreError::invalid("Latitude/longitude are out of range"));
    }
    geo(transport, "setPosition", &format!("{lat} {lon} {alt}")).await?;
    Ok(json!({"latitude":lat,"longitude":lon,"altitude":alt}))
}
pub async fn track_load(
    transport: &mut EmulatorTransport,
    positions: &[TrackPosition],
    looped: Option<bool>,
    speed: Option<i32>,
    default_interval: Option<bool>,
) -> CoreResult<Value> {
    if positions.is_empty() {
        return Err(CoreError::invalid("Track must contain positions"));
    }
    let mut encoded = Vec::new();
    for (i, p) in positions.iter().enumerate() {
        if !p.latitude.is_finite()
            || !p.longitude.is_finite()
            || !p.altitude.is_finite()
            || !(-90.0..=90.0).contains(&p.latitude)
            || !(-180.0..=180.0).contains(&p.longitude)
            || p.interval < 0
        {
            return Err(CoreError::invalid(format!("Invalid track position {i}")));
        }
        encoded.push(format!(
            "<{{'latitude': <{}>, 'longitude': <{}>, 'altitude': <{}>, 'interval': <{}>}}>",
            p.latitude, p.longitude, p.altitude, p.interval
        ));
    }
    geo(
        transport,
        "loadTrack",
        &shell_quote(&format!("[{}]", encoded.join(", "))),
    )
    .await?;
    let mut result = json!({"track":"loaded","positions":positions});
    if let Some(v) = looped {
        geo(
            transport,
            "setTrackLooped",
            if v { "true" } else { "false" },
        )
        .await?;
        result["loop"] = json!(v)
    }
    if let Some(v) = default_interval {
        geo(
            transport,
            "setTrackIntervalMode",
            if v { "true" } else { "false" },
        )
        .await?;
        result["defaultInterval"] = json!(v)
    }
    if let Some(v) = speed {
        if v <= 0 {
            return Err(CoreError::invalid("track speed must be > 0"));
        }
        geo(transport, "setTrackSpeed", &v.to_string()).await?;
        result["speed"] = json!(v)
    }
    Ok(result)
}
pub async fn track_action(
    transport: &mut EmulatorTransport,
    action: &str,
    index: Option<i32>,
) -> CoreResult<Value> {
    let (method, args) = match action {
        "start" => ("startTrack", String::new()),
        "pause" => ("pauseTrack", String::new()),
        "resume" => ("resumeTrack", String::new()),
        "stop" => ("stopTrack", String::new()),
        "goto" => {
            let i =
                index.ok_or_else(|| CoreError::invalid("location track goto requires an index"))?;
            if i < 0 {
                return Err(CoreError::invalid("track index must be >= 0"));
            }
            ("goToPositionOnTrack", i.to_string())
        }
        _ => {
            return Err(CoreError::invalid(format!(
                "Unknown track action: {action}"
            )))
        }
    };
    geo(transport, method, &args).await?;
    Ok(if action == "goto" {
        json!({"track":"goto","index":index})
    } else {
        json!({"track":action})
    })
}

const SENSORS: [&str; 9] = [
    "accelerometer",
    "als",
    "compass",
    "gyroscope",
    "magnetometer",
    "orientation",
    "proximity",
    "rotation",
    "tap",
];
async fn sensor_call(
    transport: &mut EmulatorTransport,
    method: &str,
    args: &str,
) -> CoreResult<String> {
    dbus(
        transport,
        "ru.omp.SensorfwEmulationManagement",
        "/ru/omp/SensorfwEmulationManagement",
        "ru.omp.SensorfwEmulationManagement",
        method,
        args,
        true,
    )
    .await
}
pub fn sensor_list() -> Value {
    Value::Array(
        SENSORS
            .into_iter()
            .map(|name| json!({"name":name,"emulatable":true}))
            .collect(),
    )
}
pub async fn sensor_enable(
    transport: &mut EmulatorTransport,
    sensor: &str,
    enabled: bool,
) -> CoreResult<Value> {
    if !SENSORS.contains(&sensor) {
        return Err(CoreError::invalid(format!("Unknown sensor: {sensor}")));
    }
    sensor_call(
        transport,
        if enabled {
            "enableSensor"
        } else {
            "disableSensor"
        },
        &shell_quote(sensor),
    )
    .await?;
    Ok(json!({"sensor":sensor,"enabled":enabled}))
}
pub async fn sensor_vector(
    transport: &mut EmulatorTransport,
    sensor: &str,
    x: i32,
    y: i32,
    z: i32,
) -> CoreResult<Value> {
    let method = match sensor {
        "accelerometer" => "setAccelerometerValues",
        "gyroscope" => "setGyroscopeValues",
        "magnetometer" => "setMagnetometerValues",
        _ => {
            return Err(CoreError::invalid(
                "Vector values are supported for accelerometer, gyroscope and magnetometer",
            ))
        }
    };
    sensor_call(transport, method, &format!("{x} {y} {z}")).await?;
    Ok(json!({"sensor":sensor,"x":x,"y":y,"z":z}))
}
pub async fn sensor_scalar(
    transport: &mut EmulatorTransport,
    sensor: &str,
    value: i32,
) -> CoreResult<Value> {
    let method = match sensor {
        "als" => "setAlsValue",
        "proximity" => "setProximitySensorValue",
        "tap" => "setTapSensorValue",
        _ => {
            return Err(CoreError::invalid(
                "Scalar values are supported for als, proximity and tap",
            ))
        }
    };
    sensor_call(transport, method, &value.to_string()).await?;
    Ok(json!({"sensor":sensor,"value":value}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn property_parsers() {
        let raw = "{'State': <'online'>, 'OfflineMode': <false>}";
        assert_eq!(property_string(raw, "State").as_deref(), Some("online"));
        assert_eq!(property_bool(raw, "OfflineMode"), Some(false));
    }
}
