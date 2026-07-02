use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmdStdin {
    Text(String),
    Bytes(Vec<u8>),
    File(PathBuf),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub fail_on_non_zero: bool,
    pub stdin: Option<CmdStdin>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCmdRequest {
    pub command: String,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub fail_on_non_zero: bool,
    pub stdin: Option<CmdStdin>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliExecutionRequest {
    Command(CmdRequest),
    Shell(ShellCmdRequest),
}

pub type CliExecutionResult = CmdOutput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmdStatus {
    Success,
    Failed(i32),
    TimedOut,
    Started,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub pid: Option<u32>,
    pub status: CmdStatus,
    pub duration_ms: u128,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdSessionOutput {
    pub stdout: String,
    pub stderr: String,
}

impl CmdSessionOutput {
    pub(super) fn new(stdout: String, stderr: String) -> Self {
        Self { stdout, stderr }
    }
}

impl CmdOutput {
    pub(super) fn background(pid: u32) -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            pid: Some(pid),
            status: CmdStatus::Started,
            duration_ms: 0,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    pub(super) fn foreground(
        stdout: String,
        stderr: String,
        exit_code: i32,
        duration_ms: u128,
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        let status = match exit_code {
            0 => CmdStatus::Success,
            -1 => CmdStatus::Unknown,
            code => CmdStatus::Failed(code),
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
            status: CmdStatus::TimedOut,
            duration_ms,
            stdout_truncated,
            stderr_truncated,
        }
    }
}
