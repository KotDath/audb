/// Terminal output formatting utilities
///
/// This module provides functions for printing colored, formatted messages to the terminal.
/// It replaces the previous macro-based approach with a more maintainable function-based design.
use colored::Colorize;

/// Output level determines the color and prefix of the message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputLevel {
    /// Informational messages (blue)
    Info,
    /// Success messages (green)
    Success,
    /// Warning messages (yellow)
    Warning,
    /// Error messages (red)
    Error,
    /// State messages (cyan)
    State,
}

impl OutputLevel {
    /// Get the color for this output level
    fn color(&self) -> colored::Color {
        match self {
            OutputLevel::Info => colored::Color::Blue,
            OutputLevel::Success => colored::Color::Green,
            OutputLevel::Warning => colored::Color::Yellow,
            OutputLevel::Error => colored::Color::Red,
            OutputLevel::State => colored::Color::Cyan,
        }
    }

    /// Get the prefix label for this output level
    fn label(&self) -> &'static str {
        match self {
            OutputLevel::Info => "info",
            OutputLevel::Success => "success",
            OutputLevel::Warning => "warning",
            OutputLevel::Error => "error",
            OutputLevel::State => "state",
        }
    }

    /// Should this level exit with error code 1?
    fn should_error_exit(&self) -> bool {
        matches!(self, OutputLevel::Error)
    }
}

/// Print a formatted message to stdout
///
/// # Example
/// ```
/// use audb::tools::macros::{print_msg, OutputLevel};
///
/// print_msg(OutputLevel::Info, "Starting operation");
/// print_msg(OutputLevel::Success, &format!("Completed {} items", 42));
/// ```
pub fn print_msg(level: OutputLevel, message: impl AsRef<str>) {
    let label = level.label().color(level.color()).bold();
    println!("{}: {}", label, message.as_ref());
}

/// Print an error message
pub fn print_error(message: impl AsRef<str>) {
    print_msg(OutputLevel::Error, message);
}

/// Print an info message
pub fn print_info(message: impl AsRef<str>) {
    print_msg(OutputLevel::Info, message);
}

/// Print a warning message
pub fn print_warning(message: impl AsRef<str>) {
    print_msg(OutputLevel::Warning, message);
}

/// Print a success message
pub fn print_success(message: impl AsRef<str>) {
    print_msg(OutputLevel::Success, message);
}

/// Print a state message
pub fn print_state(message: impl AsRef<str>) {
    print_msg(OutputLevel::State, message);
}

/// Print a message and exit the program
///
/// # Arguments
/// * `level` - The output level (determines color and exit code)
/// * `message` - The message to print
/// * `lowercase` - Whether to convert the message to lowercase (for compatibility with old behavior)
///
/// # Exit Codes
/// * Error level: exits with code 1
/// * Other levels: exits with code 0
pub fn exit_with_msg(level: OutputLevel, message: impl AsRef<str>, lowercase: bool) -> ! {
    let msg = if lowercase {
        message.as_ref().to_lowercase()
    } else {
        message.as_ref().to_string()
    };

    let label = level.label().color(level.color()).bold();
    println!("{}: {}", label, msg);

    let exit_code = if level.should_error_exit() { 1 } else { 0 };
    std::process::exit(exit_code);
}

/// Print an info message and exit with code 0
///
/// Note: The message is converted to lowercase for backward compatibility
pub fn exit_info(message: impl AsRef<str>) -> ! {
    exit_with_msg(OutputLevel::Info, message, true);
}

/// Print an error message and exit with code 1
///
/// Note: The message is converted to lowercase for backward compatibility
pub fn exit_error(message: impl AsRef<str>) -> ! {
    exit_with_msg(OutputLevel::Error, message, true);
}

// Keep the old macros for backward compatibility during transition
// These can be removed once all call sites are updated

/// Pretty print error (deprecated - use `print_error` function instead)
#[deprecated(since = "0.1.0", note = "Use print_error function instead")]
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_error {
    ($arg:tt) => {
        $crate::tools::macros::print_error($arg)
    };
    ($($arg:tt)*) => {
        $crate::tools::macros::print_error(&format!($($arg)*))
    };
}

/// Pretty print info (deprecated - use `print_info` function instead)
#[deprecated(since = "0.1.0", note = "Use print_info function instead")]
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_info {
    ($arg:tt) => {
        $crate::tools::macros::print_info($arg)
    };
    ($($arg:tt)*) => {
        $crate::tools::macros::print_info(&format!($($arg)*))
    };
}

/// Pretty print warning (deprecated - use `print_warning` function instead)
#[deprecated(since = "0.1.0", note = "Use print_warning function instead")]
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_warning {
    ($arg:tt) => {
        $crate::tools::macros::print_warning($arg)
    };
    ($($arg:tt)*) => {
        $crate::tools::macros::print_warning(&format!($($arg)*))
    };
}

/// Pretty print success (deprecated - use `print_success` function instead)
#[deprecated(since = "0.1.0", note = "Use print_success function instead")]
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_success {
    ($arg:tt) => {
        $crate::tools::macros::print_success($arg)
    };
    ($($arg:tt)*) => {
        $crate::tools::macros::print_success(&format!($($arg)*))
    };
}

/// Pretty print state (deprecated - use `print_state` function instead)
#[deprecated(since = "0.1.0", note = "Use print_state function instead")]
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_state {
    ($arg:tt) => {
        $crate::tools::macros::print_state($arg)
    };
    ($($arg:tt)*) => {
        $crate::tools::macros::print_state(&format!($($arg)*))
    };
}

/// Print info and exit (deprecated - use `exit_info` function instead)
#[deprecated(since = "0.1.0", note = "Use exit_info function instead")]
#[allow(unused_macros)]
#[macro_export]
macro_rules! exit_info {
    ($arg:tt) => {
        $crate::tools::macros::exit_info($arg)
    };
    ($($arg:tt)*) => {
        $crate::tools::macros::exit_info(&format!($($arg)*))
    };
}

/// Print error and exit (deprecated - use `exit_error` function instead)
#[deprecated(since = "0.1.0", note = "Use exit_error function instead")]
#[allow(unused_macros)]
#[macro_export]
macro_rules! exit_error {
    ($arg:tt) => {
        $crate::tools::macros::exit_error($arg)
    };
    ($($arg:tt)*) => {
        $crate::tools::macros::exit_error(&format!($($arg)*))
    };
}
