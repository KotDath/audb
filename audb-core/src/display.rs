use crate::error::{CoreError, CoreResult};
use crate::transport::{shell_quote, EmulatorTransport};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const PREFIX: &str = "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request";

async fn call(transport: &mut EmulatorTransport, method: &str, root: bool) -> CoreResult<String> {
    transport.exec(&format!("{PREFIX}.{method}"), root).await
}

fn variant(raw: &str) -> String {
    let raw = raw.trim();
    for quote in ['\'', '"'] {
        if let Some(start) = raw.find(quote) {
            if let Some(end) = raw[start + 1..].find(quote) {
                return raw[start + 1..start + 1 + end].to_string();
            }
        }
    }
    raw.trim_matches(|c| "(), ".contains(c)).to_string()
}

pub async fn status(transport: &mut EmulatorTransport) -> CoreResult<Value> {
    let display = variant(&call(transport, "get_display_status", false).await?);
    Ok(json!({
        "display": if display == "dimmed" { "dim" } else { &display },
        "lockMode": variant(&call(transport, "get_tklock_mode", false).await?),
        "touchPolicy": variant(&call(transport, "get_touch_input_policy", false).await?),
        "blankingInhibit": variant(&call(transport, "get_display_blanking_inhibit", false).await?),
    }))
}

async fn set_policy(transport: &mut EmulatorTransport, value: i32) -> CoreResult<bool> {
    let key = shell_quote("/system/osso/dsm/display/display_never_blank");
    let value = shell_quote(&format!("<int32 {value}>"));
    Ok(transport
        .exec(&format!("{PREFIX}.set_config {key} {value}"), true)
        .await?
        .to_ascii_lowercase()
        .contains("true"))
}

pub async fn set(
    transport: &mut EmulatorTransport,
    action: &str,
    timeout: Duration,
) -> CoreResult<Value> {
    let (method, field, expected, policy) = match action {
        "on" | "wake" => ("req_display_state_on", "display", "on", 1),
        "off" => ("req_display_state_off", "display", "off", 0),
        "dim" => ("req_display_state_dim", "display", "dim", 0),
        "lock" => ("req_display_state_off", "lockMode", "locked", 0),
        _ => {
            return Err(CoreError::invalid(format!(
                "Unknown display action: {action}"
            )))
        }
    };
    let policy_adjusted = set_policy(transport, policy).await?;
    call(transport, method, false).await?;
    let deadline = Instant::now() + timeout;
    loop {
        let actual = status(transport).await?;
        let verified = actual[field] == expected;
        if verified || Instant::now() >= deadline {
            return Ok(
                json!({"action":action,"expected":{field:expected},"actual":actual,"verified":verified,"policyAdjusted":policy_adjusted}),
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_variant() {
        assert_eq!(variant("('dimmed',)"), "dimmed");
    }
}
