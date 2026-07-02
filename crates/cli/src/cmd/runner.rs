use std::process::Command;

use agent_executor_core::Result;

use super::{
    policy::CommandPolicy,
    process::{self, WaitOutcome},
    shell::build_shell_command,
    types::{CmdOutput, CmdRequest, CmdStdin, ShellCmdRequest},
};

#[derive(Debug, Clone, Default)]
pub struct CmdRunner {
    pub(crate) policy: CommandPolicy,
}

impl CmdRunner {
    pub fn new(policy: CommandPolicy) -> Self {
        Self { policy }
    }

    pub fn run(&self, req: CmdRequest) -> Result<CmdOutput> {
        self.policy.validate_command(&req)?;
        let mut cmd = Command::new(&req.program);
        cmd.args(&req.args);

        self.run_inner(&mut cmd, RunParts::from(req))
    }

    pub fn run_shell(&self, req: ShellCmdRequest) -> Result<CmdOutput> {
        self.policy.validate_shell(&req)?;
        let mut cmd = build_shell_command(&req.command);

        self.run_inner(&mut cmd, RunParts::from(req))
    }

    pub(crate) fn run_inner(&self, cmd: &mut Command, parts: RunParts) -> Result<CmdOutput> {
        process::configure_command(
            cmd,
            parts.cwd,
            parts.env,
            parts.stdin.as_ref(),
            parts.background,
        )?;
        let mut child = process::spawn_child(cmd)?;
        let stdout_handle = process::take_output_reader(&mut child.stdout);
        let stderr_handle = process::take_output_reader(&mut child.stderr);

        process::write_stdin(&mut child, parts.stdin.as_ref())?;

        if parts.background {
            return Ok(CmdOutput::background(child.id()));
        }

        let outcome = match process::wait_for_child(&mut child, parts.timeout_ms) {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = process::collect_output(stdout_handle);
                let _ = process::collect_output(stderr_handle);
                return Err(err);
            }
        };

        process::build_output(
            outcome,
            stdout_handle,
            stderr_handle,
            parts.fail_on_non_zero,
        )
    }

    pub(crate) fn spawn_session_command(
        &self,
        mut cmd: Command,
        req: SessionStartParts,
    ) -> Result<std::process::Child> {
        process::configure_command(&mut cmd, req.cwd, req.env, req.stdin.as_ref(), true)?;
        let mut child = process::spawn_child(&mut cmd)?;
        process::write_stdin(&mut child, req.stdin.as_ref())?;
        Ok(child)
    }
}

pub(crate) struct RunParts {
    cwd: Option<String>,
    env: Option<std::collections::HashMap<String, String>>,
    timeout_ms: Option<u64>,
    fail_on_non_zero: bool,
    stdin: Option<CmdStdin>,
    background: bool,
}

impl From<CmdRequest> for RunParts {
    fn from(req: CmdRequest) -> Self {
        Self {
            cwd: req.cwd,
            env: req.env,
            timeout_ms: req.timeout_ms,
            fail_on_non_zero: req.fail_on_non_zero,
            stdin: req.stdin,
            background: req.background,
        }
    }
}

impl From<ShellCmdRequest> for RunParts {
    fn from(req: ShellCmdRequest) -> Self {
        Self {
            cwd: req.cwd,
            env: req.env,
            timeout_ms: req.timeout_ms,
            fail_on_non_zero: req.fail_on_non_zero,
            stdin: req.stdin,
            background: req.background,
        }
    }
}

pub(crate) struct SessionStartParts {
    pub(crate) cwd: Option<String>,
    pub(crate) env: Option<std::collections::HashMap<String, String>>,
    pub(crate) stdin: Option<super::types::CmdStdin>,
}

impl From<WaitOutcome> for super::session::CmdSessionStatus {
    fn from(outcome: WaitOutcome) -> Self {
        match outcome {
            WaitOutcome::Exited(status) => {
                super::session::CmdSessionStatus::Exited(status.code().unwrap_or(-1))
            }
            WaitOutcome::TimedOut => super::session::CmdSessionStatus::TimedOut,
        }
    }
}
