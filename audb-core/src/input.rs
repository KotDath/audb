use crate::error::{CoreError, CoreResult};
use crate::qmp::QmpClient;
use audb_protocol::SwipeOptions;
use serde_json::{json, Value};
use std::time::Duration;

const ABS_MAX: f64 = 32767.0;
const WIDTH: i32 = 360;
const HEIGHT: i32 = 800;

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
    if !(0..WIDTH).contains(&x) || !(0..HEIGHT).contains(&y) {
        return Err(CoreError::invalid(format!(
            "tap coordinates outside {WIDTH}x{HEIGHT}: {x},{y}"
        )));
    }
    let (x_abs, y_abs) = (abs(x, WIDTH), abs(y, HEIGHT));
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

fn direction_coords(direction: &str) -> Option<(i32, i32, i32, i32)> {
    let cx = WIDTH / 2;
    let cy = HEIGHT / 2;
    let mx = (WIDTH / 10).max(20);
    let edge = (WIDTH / 8).max(30);
    Some(match direction {
        "up" => (
            cx,
            (HEIGHT as f64 * 0.78) as i32,
            cx,
            (HEIGHT as f64 * 0.22) as i32,
        ),
        "down" => (
            cx,
            (HEIGHT as f64 * 0.22) as i32,
            cx,
            (HEIGHT as f64 * 0.78) as i32,
        ),
        "left" => (WIDTH - mx, cy, mx, cy),
        "right" => (mx, cy, WIDTH - mx, cy),
        "edge-up" => (cx, HEIGHT - 3, cx, edge),
        "edge-down" => (cx, 30, cx, HEIGHT - 3),
        "edge-left" => (WIDTH - 3, cy, edge, cy),
        "edge-right" => (3, cy, WIDTH - edge, cy),
        _ => return None,
    })
}

pub async fn swipe(
    qmp: &mut QmpClient,
    args: &[String],
    options: SwipeOptions,
) -> CoreResult<String> {
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
        let coords = direction_coords(base)
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
    let steps = options.steps.unwrap_or(default_steps).max(1);
    let duration = options.duration_ms.unwrap_or(default_duration);
    let hold = options.hold_ms.unwrap_or(default_hold);
    let (sx, sy, ex, ey) = (
        abs(x1, WIDTH),
        abs(y1, HEIGHT),
        abs(x2, WIDTH),
        abs(y2, HEIGHT),
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
    let mut unsupported = String::new();
    for character in value.chars() {
        let Some((code, shifted)) = qcode(character) else {
            unsupported.push(character);
            continue;
        };
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
    let suffix = if unsupported.is_empty() {
        String::new()
    } else {
        format!(" (unsupported chars skipped: {unsupported})")
    };
    Ok(format!("text(\"{value}\") via QMP keyboard{suffix}"))
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
    #[test]
    fn pixel_mapping_matches_python() {
        assert_eq!(abs(359, 360), 32767);
        assert_eq!(abs(799, 800), 32767);
    }
    #[test]
    fn edge_coordinates_are_in_bounds() {
        assert_eq!(direction_coords("edge-right"), Some((3, 400, 315, 400)));
    }
}
