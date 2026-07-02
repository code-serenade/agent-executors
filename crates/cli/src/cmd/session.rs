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
    runner::{CmdRunner, SessionStartParts},
    shell::build_shell_command,
    types::{CmdRequest, ShellCmdRequest},
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
    children: Mutex<HashMap<u64, Child>>,
}

impl CmdSessionManager {
    pub fn new(policy: CommandPolicy) -> Self {
        Self {
            runner: CmdRunner::new(policy),
            next_id: AtomicU64::new(1),
            children: Mutex::new(HashMap::new()),
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
        let mut children = self.lock_children()?;
        let Some(child) = children.get_mut(&session.id) else {
            return Ok(CmdSessionStatus::Unknown);
        };

        match child.try_wait().map_err(Error::tool_io)? {
            Some(status) => {
                children.remove(&session.id);
                Ok(CmdSessionStatus::Exited(status.code().unwrap_or(-1)))
            }
            None => Ok(CmdSessionStatus::Running { pid: session.pid }),
        }
    }

    pub fn stop(&self, session: CmdSession) -> Result<CmdSessionStatus> {
        let mut children = self.lock_children()?;
        let Some(mut child) = children.remove(&session.id) else {
            return Ok(CmdSessionStatus::Unknown);
        };

        match child.try_wait().map_err(Error::tool_io)? {
            Some(status) => Ok(CmdSessionStatus::Exited(status.code().unwrap_or(-1))),
            None => {
                child.kill().map_err(Error::tool_io)?;
                let status = child.wait().map_err(Error::tool_io)?;
                Ok(CmdSessionStatus::Exited(status.code().unwrap_or(-1)))
            }
        }
    }

    fn start_inner(&self, cmd: Command, parts: SessionStartParts) -> Result<CmdSession> {
        let child = self.runner.spawn_session_command(cmd, parts)?;
        let pid = child.id();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.lock_children()?.insert(id, child);

        Ok(CmdSession { id, pid })
    }

    fn lock_children(&self) -> Result<std::sync::MutexGuard<'_, HashMap<u64, Child>>> {
        self.children
            .lock()
            .map_err(|_| Error::tool_io(std::io::Error::other("session manager lock poisoned")))
    }
}
