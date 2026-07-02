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
}

impl CmdOutput {
    pub(crate) fn background(pid: u32) -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            pid: Some(pid),
            status: CmdStatus::Started,
        }
    }

    pub(crate) fn foreground(stdout: String, stderr: String, exit_code: i32) -> Self {
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
        }
    }

    pub(crate) fn timed_out(stdout: String, stderr: String) -> Self {
        Self {
            stdout,
            stderr,
            exit_code: -1,
            pid: None,
            status: CmdStatus::TimedOut,
        }
    }
}
