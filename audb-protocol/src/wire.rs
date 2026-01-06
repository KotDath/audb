use anyhow::{anyhow, Result};
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Send a message over an async stream with length framing
pub async fn send_message<T: Serialize, W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &T,
) -> Result<()> {
    let json = serde_json::to_vec(msg)?;
    let len = json.len() as u32;

    // Write length prefix (4 bytes, little-endian)
    writer.write_all(&len.to_le_bytes()).await?;

    // Write JSON payload
    writer.write_all(&json).await?;
    writer.flush().await?;

    Ok(())
}

/// Receive a message from an async stream with length framing
pub async fn recv_message<T: DeserializeOwned, R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<T> {
    // Read length prefix (4 bytes, little-endian)
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let len = u32::from_le_bytes(len_bytes) as usize;

    // Sanity check: reject unreasonably large messages (>100MB)
    if len > 100 * 1024 * 1024 {
        return Err(anyhow!("Message too large: {} bytes", len));
    }

    // Read JSON payload
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;

    // Deserialize
    Ok(serde_json::from_slice(&buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Command, Request, Response, CommandResult, CommandOutput};
    use tokio::io::DuplexStream;

    #[tokio::test]
    async fn test_send_recv_request() {
        let (mut client, mut server) = tokio::io::duplex(1024);

        // Send request from client
        let request = Request {
            id: 42,
            command: Command::Ping,
        };

        send_message(&mut client, &request).await.unwrap();

        // Receive on server
        let received: Request = recv_message(&mut server).await.unwrap();

        assert_eq!(received.id, 42);
        matches!(received.command, Command::Ping);
    }

    #[tokio::test]
    async fn test_send_recv_response() {
        let (mut client, mut server) = tokio::io::duplex(1024);

        // Send response from server
        let response = Response {
            id: 42,
            result: CommandResult::Success {
                output: CommandOutput::Unit,
            },
        };

        send_message(&mut server, &response).await.unwrap();

        // Receive on client
        let received: Response = recv_message(&mut client).await.unwrap();

        assert_eq!(received.id, 42);
        matches!(received.result, CommandResult::Success { .. });
    }
}
