use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStdin {
    Text(String),
    Bytes(Vec<u8>),
    File(PathBuf),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub fail_on_non_zero: bool,
    pub stdin: Option<ExecutionStdin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRequest {
    pub command: String,
    pub shell: ShellKind,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub fail_on_non_zero: bool,
    pub stdin: Option<ExecutionStdin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellKind {
    Sh,
    Zsh,
    Bash,
    Cmd,
    Custom(PathBuf),
}

impl ShellRequest {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            shell: ShellKind::default(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
        }
    }

    pub fn with_shell(mut self, shell: ShellKind) -> Self {
        self.shell = shell;
        self
    }
}

impl Default for ShellKind {
    fn default() -> Self {
        if cfg!(target_os = "windows") {
            Self::Cmd
        } else {
            Self::Sh
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliExecutionRequest {
    Command(CommandRequest),
    Shell(ShellRequest),
}

pub type CliExecutionResult = ExecutionOutput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStatus {
    Success,
    Failed(i32),
    TimedOut,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub pid: Option<u32>,
    pub status: ExecutionStatus,
    pub duration_ms: u128,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

impl ExecutionOutput {
    pub(super) fn foreground(
        stdout: String,
        stderr: String,
        exit_code: i32,
        duration_ms: u128,
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        let status = match exit_code {
            0 => ExecutionStatus::Success,
            -1 => ExecutionStatus::Unknown,
            code => ExecutionStatus::Failed(code),
        };

        Self {
            stdout,
            stderr,
            exit_code,
            pid: None,
            status,
            duration_ms,
            stdout_truncated,
            stderr_truncated,
        }
    }

    pub(super) fn timed_out(
        stdout: String,
        stderr: String,
        duration_ms: u128,
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        Self {
            stdout,
            stderr,
            exit_code: -1,
            pid: None,
            status: ExecutionStatus::TimedOut,
            duration_ms,
            stdout_truncated,
            stderr_truncated,
        }
    }
}
