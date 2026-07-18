use crate::error::{CoreError, CoreResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub struct QmpClient {
    socket: PathBuf,
    stream: Option<BufReader<UnixStream>>,
    next_id: u64,
}

impl QmpClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            stream: None,
            next_id: 1,
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub async fn connect(&mut self) -> CoreResult<()> {
        let stream = UnixStream::connect(&self.socket).await.map_err(|e| {
            CoreError::qmp(format!("Cannot connect to {}: {e}", self.socket.display()))
        })?;
        let mut reader = BufReader::new(stream);
        let greeting = read_json_line(&mut reader).await?;
        if greeting.get("QMP").is_none() {
            return Err(CoreError::qmp("Invalid QMP greeting"));
        }
        self.stream = Some(reader);
        self.execute_connected("qmp_capabilities", None).await?;
        Ok(())
    }

    async fn ensure_connected(&mut self) -> CoreResult<()> {
        if self.stream.is_none() {
            self.connect().await?;
        }
        Ok(())
    }

    pub async fn execute(&mut self, command: &str, arguments: Option<Value>) -> CoreResult<Value> {
        self.ensure_connected().await?;
        self.execute_connected(command, arguments).await
    }

    async fn execute_connected(
        &mut self,
        command: &str,
        arguments: Option<Value>,
    ) -> CoreResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"execute": command, "id": id});
        if let Some(arguments) = arguments {
            request["arguments"] = arguments;
        }
        let reader = self
            .stream
            .as_mut()
            .ok_or_else(|| CoreError::qmp("QMP is not connected"))?;
        let payload = serde_json::to_vec(&request).map_err(CoreError::from)?;
        reader
            .get_mut()
            .write_all(&payload)
            .await
            .map_err(|e| CoreError::qmp(e.to_string()))?;
        reader
            .get_mut()
            .write_all(b"\r\n")
            .await
            .map_err(|e| CoreError::qmp(e.to_string()))?;
        reader
            .get_mut()
            .flush()
            .await
            .map_err(|e| CoreError::qmp(e.to_string()))?;
        loop {
            let response = read_json_line(reader).await?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(CoreError::qmp(format!("QMP {command} failed: {error}")));
            }
            return Ok(response.get("return").cloned().unwrap_or(Value::Null));
        }
    }

    pub fn disconnect(&mut self) {
        self.stream = None;
    }
}

async fn read_json_line(reader: &mut BufReader<UnixStream>) -> CoreResult<Value> {
    let mut line = String::new();
    let count = reader
        .read_line(&mut line)
        .await
        .map_err(|e| CoreError::qmp(e.to_string()))?;
    if count == 0 {
        return Err(CoreError::qmp("QMP connection closed"));
    }
    serde_json::from_str(line.trim()).map_err(|e| CoreError::qmp(format!("Invalid QMP JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn qmp_handshake_and_command() {
        let dir = tempdir().unwrap();
        let socket = dir.path().join("qmp.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut io = BufReader::new(stream);
            io.get_mut().write_all(b"{\"QMP\":{}}\r\n").await.unwrap();
            for _ in 0..2 {
                let mut line = String::new();
                io.read_line(&mut line).await.unwrap();
                let value: Value = serde_json::from_str(&line).unwrap();
                let id = value["id"].as_u64().unwrap();
                io.get_mut()
                    .write_all(format!("{{\"return\":{{}},\"id\":{id}}}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let mut client = QmpClient::new(socket);
        client.connect().await.unwrap();
        client.execute("query-status", None).await.unwrap();
    }
}
