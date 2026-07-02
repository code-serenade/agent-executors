mod tools;

use std::{fmt, io};

/// Opaque workspace error type.
///
/// Specific low-level error variants stay private so executor crates can evolve
/// without forcing callers to match on implementation details.
#[derive(Debug)]
pub struct Error {
    inner: Box<ErrorKind>,
}

#[derive(Debug, thiserror::Error)]
enum ErrorKind {
    #[error(transparent)]
    Tools(#[from] tools::ToolError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.source()
    }
}

impl Error {
    /// Creates an IO error associated with tool execution.
    pub fn tool_io(err: io::Error) -> Self {
        Self {
            inner: Box::new(ErrorKind::Tools(tools::ToolError::Io(err))),
        }
    }

    /// Creates a command failure error associated with tool execution.
    pub fn tool_cmd_failed(code: i32) -> Self {
        Self {
            inner: Box::new(ErrorKind::Tools(tools::ToolError::CommandFailed(code))),
        }
    }

    /// Creates a timeout error associated with tool execution.
    pub fn tool_timeout() -> Self {
        Self {
            inner: Box::new(ErrorKind::Tools(tools::ToolError::Timeout)),
        }
    }
}

/// Workspace result type.
pub type Result<T> = std::result::Result<T, Error>;
