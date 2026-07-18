pub mod app;
pub mod appdata;
pub mod backend;
pub mod config;
pub mod diagnostics;
pub mod display;
pub mod emulator;
pub mod emulator_api;
pub mod error;
pub mod input;
pub mod qmp;
pub mod screenshot;
pub mod setup;
pub mod system;
pub mod transport;

pub use backend::EmulatorBackend;
pub use config::EmulatorConfig;
pub use error::{CoreError, CoreResult};
