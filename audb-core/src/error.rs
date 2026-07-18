use audb_protocol::{AudbError, ErrorCode};
use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
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
