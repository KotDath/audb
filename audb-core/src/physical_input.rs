//! Agent-backed physical input; uses the existing unprivileged SSH session.
use crate::{transport::DeviceTransport, CoreError, CoreResult};
use audb_protocol::{Command, CommandOutput, ErrorCode};
use serde_json::{json, Value};

pub(crate) const AGENT_COMMAND: &str = "if test -x /usr/bin/audb-agentctl; then exec /usr/bin/audb-agentctl; else printf '%s\\n' '{\"ok\":false,\"error\":{\"code\":\"AGENT_UNAVAILABLE\",\"message\":\"Run audb setup-device to install audb-agent\"}}'; fi";

pub async fn call(t: &mut DeviceTransport, action: Value) -> CoreResult<Value> {
    let mut bytes = serde_json::to_vec(&action)?;
    bytes.push(b'\n');
    let output = t.exec_stdin(AGENT_COMMAND, bytes).await?;
    parse(&output)
}
pub(crate) fn parse(output: &str) -> CoreResult<Value> {
    let response: Value = serde_json::from_str(output).map_err(|e| {
        CoreError::new(
            ErrorCode::OutcomeUnknown,
            format!("Invalid agent response; action was not repeated: {e}"),
        )
    })?;
    if response["ok"] == true {
        return response.get("data").cloned().ok_or_else(|| {
            CoreError::new(ErrorCode::OutcomeUnknown, "Agent response has no data")
        });
    }
    let code = match response["error"]["code"].as_str() {
        Some("INVALID_ARGUMENT") => ErrorCode::InvalidArgument,
        Some("INPUT_NOT_FOCUSED") => ErrorCode::InputNotFocused,
        Some("INPUT_BUSY") => ErrorCode::InputBusy,
        Some("OUTCOME_UNKNOWN") => ErrorCode::OutcomeUnknown,
        Some("AGENT_UNAVAILABLE") => ErrorCode::CapabilityUnavailable,
        Some("APP_NOT_FOUND") => ErrorCode::AppNotFound,
        Some("PERMISSION_NOT_DECLARED") => ErrorCode::PermissionNotDeclared,
        Some("PROMPT_ENABLED") => ErrorCode::PromptEnabled,
        Some("PERMISSION_DENIED") => ErrorCode::PermissionDenied,
        Some("PERMISSION_VERIFY_FAILED") => ErrorCode::PermissionVerifyFailed,
        Some("PERMISSION_SERVICE_UNAVAILABLE") => ErrorCode::PermissionServiceUnavailable,
        Some("CAPABILITY_UNAVAILABLE") => ErrorCode::CapabilityUnavailable,
        Some("PROTOCOL_MISMATCH") => ErrorCode::ProtocolMismatch,
        _ => ErrorCode::RuntimeError,
    };
    Err(CoreError::new(
        code,
        response["error"]["message"]
            .as_str()
            .unwrap_or("Physical input failed"),
    )
    .with_data(response.get("data").cloned().unwrap_or(response)))
}
pub async fn execute(t: &mut DeviceTransport, command: Command) -> CoreResult<CommandOutput> {
    if matches!(command, Command::Screenshot { .. }) {
        return Ok(CommandOutput::Binary(
            crate::screenshot::capture_physical(t).await?,
        ));
    }
    let action = match command {
        Command::Text { text, delay_ms, .. } => {
            audb_protocol::input::validate_text(&text, delay_ms).map_err(CoreError::invalid)?;
            json!({"command":"text","text":text,"delay_ms":delay_ms})
        }
        Command::Key { name, .. } => {
            if audb_protocol::input::key_code(&name).is_none() {
                return Err(CoreError::invalid("Unsupported key name"));
            }
            json!({"command":"key","name":name})
        }
        Command::Tap {
            x, y, duration_ms, ..
        } => json!({"command":"tap","x":x,"y":y,"duration_ms":duration_ms}),
        Command::Swipe { args, options, .. } => {
            json!({"command":"swipe","args":args,"duration_ms":options.duration_ms,"hold_ms":options.hold_ms,"steps":options.steps})
        }
        _ => {
            return Err(CoreError::new(
                ErrorCode::CapabilityUnavailable,
                "This physical operation is not implemented by audb-agent",
            ))
        }
    };
    Ok(CommandOutput::Json(call(t, action).await?))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn result_and_unknown_outcome_are_preserved() {
        assert_eq!(
            parse(r#"{"ok":true,"data":{"backend":"uinput"}}"#).unwrap()["backend"],
            "uinput"
        );
        assert_eq!(
            parse(r#"{"ok":false,"error":{"code":"OUTCOME_UNKNOWN","message":"lost"}}"#)
                .unwrap_err()
                .code,
            ErrorCode::OutcomeUnknown
        );
        assert_eq!(parse("broken").unwrap_err().code, ErrorCode::OutcomeUnknown);
        assert_eq!(
            parse(r#"{"ok":false,"error":{"code":"AGENT_UNAVAILABLE","message":"setup"}}"#)
                .unwrap_err()
                .code,
            ErrorCode::CapabilityUnavailable
        );
    }
}
