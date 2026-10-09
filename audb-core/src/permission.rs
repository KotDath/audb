//! Common agent-backed permissions for physical devices and emulators.
use crate::{physical_input, transport::DeviceTransport, CoreError, CoreResult};
use audb_protocol::{ErrorCode, PermissionAction};
use serde_json::{json, Value};

pub async fn execute(
    t: &mut DeviceTransport,
    application_id: &str,
    action: PermissionAction,
) -> CoreResult<Value> {
    let capability = physical_input::call(t, json!({"command":"permission_capabilities"}))
        .await
        .map_err(|e| {
            if e.message
                .contains("unknown variant `permission_capabilities`")
            {
                CoreError::new(
                    ErrorCode::CapabilityUnavailable,
                    "Upgrade audb-agent using setup-device for permission support",
                )
            } else {
                e
            }
        })?;
    if capability["permissions"] != true || capability["promptSemantics"] != "aurora-boolean" {
        return Err(CoreError::new(
            ErrorCode::CapabilityUnavailable,
            "This agent does not support Aurora permission management; upgrade using setup-device",
        ));
    }
    physical_input::call(
        t,
        json!({"command":"permission", "application_id":application_id,"action":action}),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::{server, Channel, ChannelId, CryptoVec};
    use std::{collections::HashMap, sync::Arc, time::Duration};
    struct AgentServer {
        input: HashMap<ChannelId, Vec<u8>>,
        log: Arc<std::sync::Mutex<Vec<Value>>>,
    }
    impl server::Handler for AgentServer {
        type Error = russh::Error;
        async fn auth_publickey(
            &mut self,
            user: &str,
            _: &russh::keys::ssh_key::PublicKey,
        ) -> Result<server::Auth, Self::Error> {
            assert_eq!(user, "defaultuser");
            Ok(server::Auth::Accept)
        }
        async fn channel_open_session(
            &mut self,
            _: Channel<server::Msg>,
            _: &mut server::Session,
        ) -> Result<bool, Self::Error> {
            Ok(true)
        }
        async fn exec_request(
            &mut self,
            channel: ChannelId,
            command: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            assert_eq!(command, physical_input::AGENT_COMMAND.as_bytes());
            session.channel_success(channel)?;
            Ok(())
        }
        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            let input = self.input.entry(channel).or_default();
            input.extend_from_slice(data);
            if input.ends_with(b"\n") {
                let action: Value = serde_json::from_slice(input).unwrap();
                self.log.lock().unwrap().push(action.clone());
                if action["command"] == "text" && action["text"] == "lost" {
                    session.data(channel, CryptoVec::from("{\"ok\":false,\"error\":{\"code\":\"OUTCOME_UNKNOWN\",\"message\":\"lost after dispatch\"}}\n"))?;
                    session.exit_status_request(channel, 0)?;
                    session.eof(channel)?;
                    session.close(channel)?;
                    return Ok(());
                }
                let reply = if action["command"] == "permission_capabilities" {
                    json!({"permissions":true,"promptSemantics":"aurora-boolean"})
                } else if action["command"] == "status" {
                    json!({"capabilities":{"text":true,"key":true}})
                } else if action["command"] == "text" || action["command"] == "key" {
                    json!({"backend":"agent","action":action})
                } else {
                    json!({"applicationId":action["application_id"],"granted":["UserDirs"],"uid":100123})
                };
                let bytes = format!("{}\n", json!({"ok":true,"data":reply}));
                session.data(channel, CryptoVec::from(bytes.as_bytes()))?;
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Ok(())
        }
    }
    #[tokio::test]
    async fn emulator_permissions_and_input_use_user_ssh_stdin_without_qmp_or_replay() {
        use russh::keys::ssh_key::{rand_core::OsRng, Algorithm, LineEnding, PrivateKey};
        let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key");
        std::fs::write(
            &key_path,
            key.to_openssh(LineEnding::LF).unwrap().as_bytes(),
        )
        .unwrap();
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let server_log = log.clone();
        let task = tokio::spawn(async move {
            let (stream, _) = socket.accept().await.unwrap();
            let config = server::Config {
                keys: vec![key],
                auth_rejection_time: Duration::ZERO,
                ..Default::default()
            };
            server::run_stream(
                Arc::new(config),
                stream,
                AgentServer {
                    input: HashMap::new(),
                    log: server_log,
                },
            )
            .await
            .unwrap()
            .await
            .unwrap();
        });
        let config = crate::config::EmulatorConfig {
            ssh_port: port,
            ssh_key: key_path,
            qmp_socket: dir.path().join("absent-qmp.sock"),
            ..Default::default()
        };
        let mut backend = crate::DeviceBackend::new(config.into());
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            backend.execute(audb_protocol::Command::Permission {
                application_id: "test-app".into(),
                action: PermissionAction::List,
            }),
        )
        .await
        .unwrap()
        .unwrap();
        let audb_protocol::CommandOutput::Json(data) = output else {
            panic!("Expected permission JSON")
        };
        assert_eq!(data["granted"], json!(["UserDirs"]));
        for command in [
            audb_protocol::Command::Text {
                text: "Привет $\"\\ 🦀".into(),
                delay_ms: 0,
                socket: None,
            },
            audb_protocol::Command::Key {
                name: "backspace".into(),
                socket: None,
            },
        ] {
            let result = backend.execute(command).await.unwrap();
            let audb_protocol::CommandOutput::Json(data) = result else {
                panic!("Expected input JSON")
            };
            assert_eq!(data["backend"], "agent");
        }
        assert_eq!(
            backend
                .execute(audb_protocol::Command::Text {
                    text: "lost".into(),
                    delay_ms: 0,
                    socket: None
                })
                .await
                .unwrap_err()
                .code,
            ErrorCode::OutcomeUnknown
        );
        let requests = log.lock().unwrap();
        assert_eq!(requests.len(), 8);
        assert_eq!(requests[1]["action"]["operation"], "list");
        assert!(!requests[1].to_string().contains("uid"));
        assert_eq!(requests[3]["text"], "Привет $\"\\ 🦀");
        assert_eq!(requests[5]["name"], "backspace");
        assert_eq!(requests.iter().filter(|r| r["text"] == "lost").count(), 1);
        task.abort();
    }
}
