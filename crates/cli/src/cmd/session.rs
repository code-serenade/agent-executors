use std::{
    collections::HashMap,
    process::{Child, Command},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use agent_executor_core::{Error, Result};

use super::{
    policy::CommandPolicy,
    process::{self, SessionOutputCapture},
    runner::{CmdRunner, SessionStartParts},
    shell::build_shell_command,
    types::{CmdRequest, CmdSessionOutput, ShellCmdRequest},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CmdSession {
    pub id: u64,
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmdSessionStatus {
    Running { pid: u32 },
    Exited(i32),
    TimedOut,
    Unknown,
}

#[derive(Debug, Default)]
pub struct CmdSessionManager {
    runner: CmdRunner,
    next_id: AtomicU64,
    entries: Mutex<HashMap<u64, SessionEntry>>,
}

impl CmdSessionManager {
    // Public API
    pub fn new(policy: CommandPolicy) -> Self {
        Self {
            runner: CmdRunner::new(policy),
            next_id: AtomicU64::new(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn start(&self, req: CmdRequest) -> Result<CmdSession> {
        self.runner.policy.validate_command_session(&req)?;
        let mut cmd = Command::new(&req.program);
        cmd.args(&req.args);

        self.start_inner(
            cmd,
            SessionStartParts {
                cwd: req.cwd,
                env: req.env,
                stdin: req.stdin,
            },
        )
    }

    pub fn start_shell(&self, req: ShellCmdRequest) -> Result<CmdSession> {
        self.runner.policy.validate_shell_session(&req)?;
        let cmd = build_shell_command(&req.command);

        self.start_inner(
            cmd,
            SessionStartParts {
                cwd: req.cwd,
                env: req.env,
                stdin: req.stdin,
            },
        )
    }

    pub fn status(&self, session: CmdSession) -> Result<CmdSessionStatus> {
        let mut entries = self.lock_entries()?;
        let Some(entry) = entries.get_mut(&session.id) else {
            return Ok(CmdSessionStatus::Unknown);
        };

        if let Some(status) = &entry.final_status {
            return Ok(status.clone());
        }

        match entry.child.try_wait().map_err(Error::tool_io)? {
            Some(status) => {
                let status = CmdSessionStatus::Exited(status.code().unwrap_or(-1));
                entry.final_status = Some(status.clone());
                Ok(status)
            }
            None => Ok(CmdSessionStatus::Running { pid: session.pid }),
        }
    }

    pub fn output(&self, session: CmdSession) -> Result<CmdSessionOutput> {
        let entries = self.lock_entries()?;
        let Some(entry) = entries.get(&session.id) else {
            return Ok(CmdSessionOutput::new(String::new(), String::new()));
        };

        let (stdout, stderr) = entry.output.snapshot()?;
        Ok(CmdSessionOutput::new(stdout, stderr))
    }

    pub fn stop(&self, session: CmdSession) -> Result<CmdSessionStatus> {
        let mut entries = self.lock_entries()?;
        let Some(entry) = entries.get_mut(&session.id) else {
            return Ok(CmdSessionStatus::Unknown);
        };

        if let Some(status) = &entry.final_status {
            return Ok(status.clone());
        }

        match entry.child.try_wait().map_err(Error::tool_io)? {
            Some(status) => {
                let status = CmdSessionStatus::Exited(status.code().unwrap_or(-1));
                entry.final_status = Some(status.clone());
                Ok(status)
            }
            None => {
                let status = process::stop_child(&mut entry.child)?;
                let status = CmdSessionStatus::Exited(status.code().unwrap_or(-1));
                entry.final_status = Some(status.clone());
                Ok(status)
            }
        }
    }
}

impl CmdSessionManager {
    // Internal helpers
    fn start_inner(&self, cmd: Command, parts: SessionStartParts) -> Result<CmdSession> {
        let mut child = self.runner.spawn_session_command(cmd, parts)?;
        let pid = child.id();
        let output = process::capture_session_output(&mut child);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.lock_entries()?.insert(
            id,
            SessionEntry {
                child,
                output,
                final_status: None,
            },
        );

        Ok(CmdSession { id, pid })
    }

    fn lock_entries(&self) -> Result<std::sync::MutexGuard<'_, HashMap<u64, SessionEntry>>> {
        self.entries
            .lock()
            .map_err(|_| Error::tool_io(std::io::Error::other("session manager lock poisoned")))
    }
}

#[derive(Debug)]
struct SessionEntry {
    child: Child,
    output: SessionOutputCapture,
    final_status: Option<CmdSessionStatus>,
}
