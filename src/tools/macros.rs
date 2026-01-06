/// Pretty print error
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_error {
    ($arg:tt) => {
        println!("{}", format!("\x1b[1m\x1b[91merror\x1b[0m: {}", $arg))
    };
    ($($arg:tt)*) => {{
        println!("{}", format!("\x1b[1m\x1b[91merror\x1b[0m: {}", format!($($arg)*)))
    }};
}

/// Pretty print info
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_info {
    ($arg:tt) => {
        println!("{}", format!("\x1b[1m\x1b[94minfo\x1b[0m: {}", $arg))
    };
    ($($arg:tt)*) => {{
        println!("{}", format!("\x1b[1m\x1b[94minfo\x1b[0m: {}", format!($($arg)*)))
    }};
}

/// Pretty print warning
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_warning {
    ($arg:tt) => {
        println!("{}", format!("\x1b[1m\x1b[93mwarning\x1b[0m: {}", $arg))
    };
    ($($arg:tt)*) => {{
        println!("{}", format!("\x1b[1m\x1b[93mwarning\x1b[0m: {}", format!($($arg)*)))
    }};
}

/// Pretty print success
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_success {
    ($arg:tt) => {
        println!("{}", format!("\x1b[1m\x1b[32msuccess\x1b[0m: {}", $arg))
    };
    ($($arg:tt)*) => {{
        println!("{}", format!("\x1b[1m\x1b[32msuccess\x1b[0m: {}", format!($($arg)*)))
    }};
}

/// Pretty print state
#[allow(unused_macros)]
#[macro_export]
macro_rules! print_state {
    ($arg:tt) => {
        println!("{}", format!("\x1b[1m\x1b[36mstate\x1b[0m: {}", $arg))
    };
    ($($arg:tt)*) => {{
        println!("{}", format!("\x1b[1m\x1b[36mstate\x1b[0m: {}", format!($($arg)*)))
    }};
}

/// Print info and exit
#[allow(unused_macros)]
#[macro_export]
macro_rules! exit_info {
    ($arg:tt) => {{
        println!("{}", format!("\x1b[1m\x1b[94minfo\x1b[0m: {}", $arg).to_lowercase());
        std::process::exit(1);
    }};
    ($($arg:tt)*) => {{
        println!("{}", format!("\x1b[1m\x1b[94minfo\x1b[0m: {}", format!($($arg)*)).to_lowercase());
        std::process::exit(1);
    }};
}

/// Print error and exit
#[allow(unused_macros)]
#[macro_export]
macro_rules! exit_error {
    ($arg:tt) => {{
        println!("{}", format!("\x1b[1m\x1b[91merror\x1b[0m: {}", $arg).to_lowercase());
        std::process::exit(1);
    }};
    ($($arg:tt)*) => {{
        println!("{}", format!("\x1b[1m\x1b[91merror\x1b[0m: {}", format!($($arg)*)).to_lowercase());
        std::process::exit(1);
    }};
}
