use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum ToolError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Policy rejected execution: {0}")]
    Policy(String),

    #[error("Command failed with exit code {0}")]
    CommandFailed(i32),

    #[error("Command timed out")]
    Timeout,
}
