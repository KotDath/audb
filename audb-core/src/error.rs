use audb_protocol::{AudbError, ErrorCode};
use serde_json::Value;
use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
    pub data: Option<Value>,
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::RuntimeError, message)
    }

    pub fn ssh(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::SshError, message)
    }

    pub fn qmp(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::QmpError, message)
    }
}

impl From<CoreError> for AudbError {
    fn from(value: CoreError) -> Self {
        Self {
            code: value.code,
            message: value.message,
            data: value.data,
        }
    }
}

impl From<std::io::Error> for CoreError {
    fn from(value: std::io::Error) -> Self {
        Self::runtime(value.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::runtime(value.to_string())
    }
}
