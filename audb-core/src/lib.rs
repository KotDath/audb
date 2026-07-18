pub mod app;
pub mod backend;
pub mod config;
pub mod display;
pub mod emulator;
pub mod error;
pub mod input;
pub mod qmp;
pub mod screenshot;
pub mod setup;
pub mod transport;

pub use backend::EmulatorBackend;
pub use config::EmulatorConfig;
pub use error::{CoreError, CoreResult};
