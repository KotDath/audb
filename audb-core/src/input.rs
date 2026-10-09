use crate::error::{CoreError, CoreResult};
use crate::qmp::QmpClient;
use audb_protocol::SwipeOptions;
use serde_json::{json, Value};
use std::time::Duration;

const ABS_MAX: f64 = 32767.0;
#[derive(Debug, Clone, Copy)]
pub(crate) struct Geometry {
    pub width: i32,
    pub height: i32,
}
impl Geometry {
    pub async fn query(qmp: &mut QmpClient) -> CoreResult<Self> {
        let (width, height) = crate::screenshot::qmp_geometry(qmp).await?;
        if width < 2 || height < 2 {
            return Err(CoreError::invalid(
                "QMP touchscreen requires dimensions of at least 2x2",
            ));
        }
        Ok(Self {
            width: width as i32,
            height: height as i32,
        })
    }
    fn check(self, x: i32, y: i32) -> CoreResult<()> {
        if !(0..self.width).contains(&x) || !(0..self.height).contains(&y) {
            return Err(CoreError::invalid(format!(
                "Coordinates outside {}x{} QMP display: {x},{y}",
                self.width, self.height
            )));
        }
        Ok(())
    }
}

fn abs(value: i32, extent: i32) -> i32 {
    (value as f64 * ABS_MAX / (extent - 1) as f64).round() as i32
}

fn mtt(kind: &str, tracking_id: i32, axis: &str, value: i32) -> Value {
    json!({"type":"mtt","data":{"type":kind,"slot":0,"tracking-id":tracking_id,"axis":axis,"value":value}})
}

fn touch(down: bool) -> Value {
    json!({"type":"btn","data":{"button":"touch","down":down}})
}

async fn events(qmp: &mut QmpClient, events: Vec<Value>) -> CoreResult<()> {
    qmp.execute("input-send-event", Some(json!({"events": events})))
        .await?;
    Ok(())
}

pub async fn tap(qmp: &mut QmpClient, x: i32, y: i32, duration_ms: u64) -> CoreResult<String> {
    if !(1..=3000).contains(&duration_ms) {
        return Err(CoreError::invalid("Tap duration must be 1..3000 ms"));
    }
    let geometry = Geometry::query(qmp).await?;
    geometry.check(x, y)?;
    let (x_abs, y_abs) = (abs(x, geometry.width), abs(y, geometry.height));
    events(
        qmp,
        vec![
            mtt("begin", 1001, "x", x_abs),
            mtt("begin", 1001, "y", y_abs),
            touch(true),
            mtt("data", 1001, "x", x_abs),
            mtt("data", 1001, "y", y_abs),
        ],
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(duration_ms)).await;
    events(
        qmp,
        vec![
            mtt("end", -1, "x", x_abs),
            mtt("end", -1, "y", y_abs),
            touch(false),
        ],
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    Ok(format!("tap({x}, {y}) via QMP multitouch"))
}

fn direction_coords(direction: &str, width: i32, height: i32) -> Option<(i32, i32, i32, i32)> {
    let cx = width / 2;
    let cy = height / 2;
    let mx = (width / 10).clamp(1, width - 1);
    let edge_x = (width / 8).clamp(1, width - 1);
    let edge_y = (height / 8).clamp(1, height - 1);
    Some(match direction {
        "up" => (
            cx,
            (height as f64 * 0.78) as i32,
            cx,
            (height as f64 * 0.22) as i32,
        ),
        "down" => (
            cx,
            (height as f64 * 0.22) as i32,
            cx,
            (height as f64 * 0.78) as i32,
        ),
        "left" => (width - mx, cy, mx, cy),
        "right" => (mx, cy, width - mx, cy),
        "edge-up" => (cx, (height - 3).max(1), cx, edge_y),
        "edge-down" => (cx, 30.min(height - 1), cx, (height - 3).max(1)),
        "edge-left" => ((width - 3).max(1), cy, edge_x, cy),
        "edge-right" => (3.min(width - 1), cy, width - edge_x, cy),
        _ => return None,
    })
}

pub async fn swipe(
    qmp: &mut QmpClient,
    args: &[String],
    options: SwipeOptions,
) -> CoreResult<String> {
    let geometry = Geometry::query(qmp).await?;
    let (x1, y1, x2, y2, description, mode) = if args.len() == 4 {
        let values = args
            .iter()
            .map(|v| v.parse::<i32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| CoreError::invalid("swipe expects a direction or X1 Y1 X2 Y2"))?;
        (
            values[0],
            values[1],
            values[2],
            values[3],
            format!("{},{} -> {},{}", values[0], values[1], values[2], values[3]),
            "scroll",
        )
    } else if args.len() == 1 {
        let original = args[0].as_str();
        let (base, mode) = if let Some(base) = original.strip_prefix("fast-") {
            (base, "gesture")
        } else if let Some(base) = original.strip_prefix("long-") {
            (base, "long")
        } else if original.starts_with("edge-") {
            (original, "gesture")
        } else {
            (original, "scroll")
        };
        let coords = direction_coords(base, geometry.width, geometry.height)
            .ok_or_else(|| CoreError::invalid(format!("Unknown swipe direction: {original}")))?;
        (
            coords.0,
            coords.1,
            coords.2,
            coords.3,
            original.to_string(),
            mode,
        )
    } else {
        return Err(CoreError::invalid(
            "swipe expects a direction or X1 Y1 X2 Y2",
        ));
    };

    geometry.check(x1, y1)?;
    geometry.check(x2, y2)?;
    let (default_steps, default_duration, default_hold, settle) = match mode {
        "gesture" => (60, 500, 50, 800),
        "long" => (80, 1500, 50, 800),
        _ => (
            42,
            900,
            160,
            if (y2 - y1).abs() >= (x2 - x1).abs() {
                3000
            } else {
                450
            },
        ),
    };
    let steps = options.steps.unwrap_or(default_steps);
    let duration = options.duration_ms.unwrap_or(default_duration);
    let hold = options.hold_ms.unwrap_or(default_hold);
    if !(1..=240).contains(&steps) || !(40..=3000).contains(&duration) || hold > 1000 {
        return Err(CoreError::invalid(
            "Swipe requires steps 1..240, duration 40..3000 ms, hold 0..1000 ms",
        ));
    }
    let (sx, sy, ex, ey) = (
        abs(x1, geometry.width),
        abs(y1, geometry.height),
        abs(x2, geometry.width),
        abs(y2, geometry.height),
    );
    events(
        qmp,
        vec![
            mtt("begin", 1001, "x", sx),
            mtt("begin", 1001, "y", sy),
            touch(true),
            mtt("data", 1001, "x", sx),
            mtt("data", 1001, "y", sy),
        ],
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(hold)).await;
    for step in 1..=steps {
        let x = sx + ((ex - sx) as i64 * step as i64 / steps as i64) as i32;
        let y = sy + ((ey - sy) as i64 * step as i64 / steps as i64) as i32;
        events(
            qmp,
            vec![
                mtt("update", 1001, "x", x),
                mtt("update", 1001, "y", y),
                mtt("data", 1001, "x", x),
                mtt("data", 1001, "y", y),
            ],
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(duration / steps as u64)).await;
    }
    events(
        qmp,
        vec![
            mtt("end", -1, "x", ex),
            mtt("end", -1, "y", ey),
            touch(false),
        ],
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(settle)).await;
    Ok(format!("swipe({description}) via QMP multitouch [{mode}: steps={steps}, dur={duration}ms, hold={hold}ms]"))
}

async fn send_key(qmp: &mut QmpClient, qcode: &str, down: bool) -> CoreResult<()> {
    events(
        qmp,
        vec![json!({"type":"key","data":{"down":down,"key":{"type":"qcode","data":qcode}}})],
    )
    .await
}

fn qcode(character: char) -> Option<(String, bool)> {
    let lower = character.to_ascii_lowercase();
    let shifted = character.is_ascii_uppercase() || "!@#$%^&*()_+{}:\"<>?|~".contains(character);
    let code = match lower {
        'a'..='z' => return Some((lower.to_string(), shifted)),
        '0'..='9' => return Some((lower.to_string(), false)),
        ' ' => "spc",
        '\n' => "ret",
        '\t' => "tab",
        ',' => "comma",
        '.' => "dot",
        '/' | '?' => "slash",
        '-' | '_' => "minus",
        '=' | '+' => "equal",
        '[' | '{' => "bracket_left",
        ']' | '}' => "bracket_right",
        '\\' | '|' => "backslash",
        '`' | '~' => "grave_accent",
        ';' | ':' => "semicolon",
        '\'' | '"' => "apostrophe",
        '!' => "1",
        '@' => "2",
        '#' => "3",
        '$' => "4",
        '%' => "5",
        '^' => "6",
        '&' => "7",
        '*' => "8",
        '(' => "9",
        ')' => "0",
        '<' => "comma",
        '>' => "dot",
        _ => return None,
    };
    Some((code.to_string(), shifted))
}

pub async fn text(qmp: &mut QmpClient, value: &str, delay_ms: u64) -> CoreResult<String> {
    audb_protocol::input::validate_text(value, delay_ms).map_err(CoreError::invalid)?;
    if value.chars().any(|c| qcode(c).is_none()) {
        return Err(CoreError::new(
            audb_protocol::ErrorCode::CapabilityUnavailable,
            "QMP cannot type this text; install audb-agent for Unicode input. Nothing was sent",
        ));
    }
    for character in value.chars() {
        let (code, shifted) = qcode(character).expect("validated before input");
        if shifted {
            send_key(qmp, "shift", true).await?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        send_key(qmp, &code, true).await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        send_key(qmp, &code, false).await?;
        if shifted {
            tokio::time::sleep(Duration::from_millis(20)).await;
            send_key(qmp, "shift", false).await?;
        }
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    Ok(format!(
        "text({} code points) via QMP keyboard; requires a matching guest keyboard layout",
        value.chars().count()
    ))
}

pub async fn key(qmp: &mut QmpClient, name: &str) -> CoreResult<String> {
    let normalized = name.to_ascii_lowercase();
    let code = match normalized.as_str() {
        "volumeup" | "volup" | "vol+" => "volumeup",
        "volumedown" | "voldown" | "vol-" => "volumedown",
        "enter" | "return" => "ret",
        "escape" | "esc" => "esc",
        "del" | "delete" => "delete",
        "space" => "spc",
        "capslock" => "caps_lock",
        other => other,
    };
    send_key(qmp, code, true).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    send_key(qmp, code, false).await?;
    Ok(format!("key({normalized}) → qcode {code}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn qmp_rejects_unicode_before_connecting_or_typing_ascii_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let mut qmp = QmpClient::new(dir.path().join("absent.sock"));
        let error = text(&mut qmp, "ASCII Привет", 0).await.unwrap_err();
        assert_eq!(error.code, audb_protocol::ErrorCode::CapabilityUnavailable);
        assert!(error.message.contains("Nothing was sent"));
    }
    #[test]
    fn pixel_mapping_matches_python() {
        assert_eq!(abs(359, 360), 32767);
        assert_eq!(abs(799, 800), 32767);
    }
    #[test]
    fn edge_coordinates_are_in_bounds() {
        assert_eq!(
            direction_coords("edge-right", 360, 800),
            Some((3, 400, 315, 400))
        );
    }
    async fn mock_display(
        socket: &std::path::Path,
        malformed: bool,
    ) -> tokio::task::JoinHandle<Vec<Value>> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut io = BufReader::new(stream);
            io.get_mut().write_all(b"{\"QMP\":{}}\n").await.unwrap();
            let mut inputs = Vec::new();
            let mut dumps = 0;
            loop {
                let mut line = String::new();
                if io.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                match request["execute"].as_str().unwrap() {
                    "qmp_capabilities" => {}
                    "screendump" => {
                        let png: &[u8] = if malformed {
                            b"incomplete image"
                        } else if dumps == 0 {
                            include_bytes!("../tests/qmp-portrait.png")
                        } else {
                            include_bytes!("../tests/qmp-landscape.png")
                        };
                        tokio::fs::write(request["arguments"]["filename"].as_str().unwrap(), png)
                            .await
                            .unwrap();
                        dumps += 1;
                    }
                    "input-send-event" => inputs.push(request["arguments"]["events"].clone()),
                    command => panic!("Unexpected command {command}"),
                }
                io.get_mut()
                    .write_all(format!("{}\n", json!({"return":{},"id":request["id"]})).as_bytes())
                    .await
                    .unwrap();
            }
            inputs
        })
    }
    #[tokio::test]
    async fn qmp_touch_refreshes_geometry_after_resize_and_rejects_out_of_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("qmp.sock");
        let server = mock_display(&socket, false).await;
        let mut qmp = QmpClient::new(socket);
        tap(&mut qmp, 719, 1279, 1).await.unwrap();
        tap(&mut qmp, 799, 359, 1).await.unwrap();
        assert_eq!(
            tap(&mut qmp, 800, 359, 1).await.unwrap_err().code,
            audb_protocol::ErrorCode::InvalidArgument
        );
        swipe(
            &mut qmp,
            &["0".into(), "0".into(), "799".into(), "359".into()],
            SwipeOptions {
                steps: Some(1),
                duration_ms: Some(40),
                hold_ms: Some(0),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            swipe(
                &mut qmp,
                &["0".into(), "0".into(), "900".into(), "359".into()],
                SwipeOptions::default()
            )
            .await
            .unwrap_err()
            .code,
            audb_protocol::ErrorCode::InvalidArgument
        );
        assert_eq!(
            swipe(
                &mut qmp,
                &[
                    i32::MIN.to_string(),
                    "0".into(),
                    i32::MAX.to_string(),
                    "0".into()
                ],
                SwipeOptions::default()
            )
            .await
            .unwrap_err()
            .code,
            audb_protocol::ErrorCode::InvalidArgument
        );
        drop(qmp);
        let inputs = server.await.unwrap();
        assert_eq!(inputs.len(), 7);
        for index in [0, 2] {
            assert_eq!(inputs[index][0]["data"]["value"], 32767);
            assert_eq!(inputs[index][1]["data"]["value"], 32767);
        }
        assert_eq!(inputs[4][0]["data"]["value"], 0);
        assert_eq!(inputs[5][0]["data"]["value"], 32767);
        assert_eq!(inputs[5][1]["data"]["value"], 32767);
        assert_eq!(inputs[6][2]["data"]["down"], false);
    }
    #[tokio::test]
    async fn missing_qmp_geometry_sends_no_touch_events() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("qmp.sock");
        let server = mock_display(&socket, true).await;
        let mut qmp = QmpClient::new(socket);
        assert!(tap(&mut qmp, 10, 10, 1).await.is_err());
        drop(qmp);
        assert!(server.await.unwrap().is_empty());
    }
    #[test]
    fn all_direction_aliases_stay_inside_current_geometry() {
        for (width, height) in [(720, 1280), (800, 360), (2, 2)] {
            for name in [
                "up",
                "down",
                "left",
                "right",
                "edge-up",
                "edge-down",
                "edge-left",
                "edge-right",
            ] {
                let (x1, y1, x2, y2) = direction_coords(name, width, height).unwrap();
                let geometry = Geometry { width, height };
                geometry.check(x1, y1).unwrap();
                geometry.check(x2, y2).unwrap();
            }
        }
    }
}
