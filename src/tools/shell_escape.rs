/// Shell escaping utilities for safe command construction
///
/// This module provides functions to safely escape shell arguments for use in
/// shell commands, preventing shell injection vulnerabilities.

/// Escape a string for use in a single-quote context in shell commands.
///
/// In a single-quote context, the only character that needs escaping is the
/// single quote itself. This is done by closing the quote, escaping the quote,
/// and reopening the quote: '\''
///
/// # Example
/// ```
/// use audb::tools::shell_escape::escape_single_quote;
///
/// let password = "my'password";
/// let escaped = escape_single_quote(password);
/// assert_eq!(escaped, "my'\\''password");
/// ```
///
/// # Security
/// This function should be used whenever constructing shell commands that
/// incorporate user-provided or external data within single quotes.
pub fn escape_single_quote(s: &str) -> String {
    s.replace('\'', r"'\''")
}

/// Wrapper type for shell-escaped strings
///
/// This newtype pattern ensures that strings used in shell contexts
/// are properly escaped at compile time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEscaped(String);

impl ShellEscaped {
    /// Create a new shell-escaped string for single-quote context
    pub fn single_quote(s: &str) -> Self {
        Self(escape_single_quote(s))
    }

    /// Get the escaped string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume and return the inner string
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for ShellEscaped {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ShellEscaped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_single_quote_no_quotes() {
        assert_eq!(escape_single_quote("password"), "password");
    }

    #[test]
    fn test_escape_single_quote_with_quotes() {
        assert_eq!(escape_single_quote("pass'word"), "pass'\\''word");
    }

    #[test]
    fn test_escape_single_quote_multiple_quotes() {
        assert_eq!(
            escape_single_quote("'multiple'quotes'"),
            "'\\''multiple'\\''quotes'\\''",
        );
    }

    #[test]
    fn test_shell_escaped_wrapper() {
        let escaped = ShellEscaped::single_quote("test'value");
        assert_eq!(escaped.as_str(), "test'\\''value");
        assert_eq!(escaped.to_string(), "test'\\''value");
    }
}
