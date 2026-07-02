use std::io;

use agent_executor_core::{Error, Result};

use super::types::{CmdRequest, ShellCmdRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPolicy {
    pub allow_shell: bool,
    pub allow_background: bool,
    pub max_timeout_ms: Option<u64>,
}

impl Default for CommandPolicy {
    fn default() -> Self {
        Self {
            allow_shell: true,
            allow_background: true,
            max_timeout_ms: None,
        }
    }
}

impl CommandPolicy {
    pub(crate) fn validate_command(&self, req: &CmdRequest) -> Result<()> {
        self.validate_background(req.background)?;
        self.validate_timeout(req.timeout_ms)
    }

    pub(crate) fn validate_command_session(&self, req: &CmdRequest) -> Result<()> {
        self.validate_background(true)?;
        self.validate_timeout(req.timeout_ms)
    }

    pub(crate) fn validate_shell(&self, req: &ShellCmdRequest) -> Result<()> {
        if !self.allow_shell {
            return Err(policy_error("shell commands are not allowed by policy"));
        }

        self.validate_background(req.background)?;
        self.validate_timeout(req.timeout_ms)
    }

    pub(crate) fn validate_shell_session(&self, req: &ShellCmdRequest) -> Result<()> {
        if !self.allow_shell {
            return Err(policy_error("shell commands are not allowed by policy"));
        }

        self.validate_background(true)?;
        self.validate_timeout(req.timeout_ms)
    }

    fn validate_background(&self, background: bool) -> Result<()> {
        if background && !self.allow_background {
            return Err(policy_error(
                "background commands are not allowed by policy",
            ));
        }

        Ok(())
    }

    fn validate_timeout(&self, timeout_ms: Option<u64>) -> Result<()> {
        let (Some(max), Some(timeout_ms)) = (self.max_timeout_ms, timeout_ms) else {
            return Ok(());
        };

        if timeout_ms > max {
            return Err(policy_error(format!(
                "timeout {timeout_ms}ms exceeds policy maximum {max}ms"
            )));
        }

        Ok(())
    }
}

fn policy_error(message: impl Into<String>) -> Error {
    Error::tool_io(io::Error::new(
        io::ErrorKind::PermissionDenied,
        message.into(),
    ))
}
