use std::fmt;

/// Error type for WarpGrid async operations.
///
/// Wraps a human-readable message describing the failure. Used as the
/// error variant in streaming body items (`Stream<Item = Result<Bytes, Error>>`).
#[derive(Debug, Clone)]
pub struct Error {
    message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_from_string() {
        let err = Error::from("test error".to_string());
        assert_eq!(err.message(), "test error");
    }

    #[test]
    fn error_from_str() {
        let err = Error::from("test error");
        assert_eq!(err.message(), "test error");
    }

    #[test]
    fn error_display() {
        let err = Error::new("something failed");
        assert_eq!(format!("{err}"), "something failed");
    }

    #[test]
    fn error_is_std_error() {
        let err = Error::new("test");
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn error_is_clone() {
        let err = Error::new("original");
        let cloned = err.clone();
        assert_eq!(err.message(), cloned.message());
    }
}
