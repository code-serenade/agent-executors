use std::time::Instant;

use agent_executor_core::Result;
use tokio::process::Command;

use super::{
    policy::CommandPolicy,
    process,
    shell::build_shell_command,
    types::{
        CliExecutionRequest, CliExecutionResult, CommandRequest, ExecutionOutput, ExecutionStdin,
        ShellRequest,
    },
};

#[derive(Debug, Clone, Default)]
pub struct CliExecutor {
    pub(super) policy: CommandPolicy,
}

impl CliExecutor {
    // Public API
    pub fn new(policy: CommandPolicy) -> Self {
        Self { policy }
    }

    pub async fn execute(&self, req: CliExecutionRequest) -> Result<CliExecutionResult> {
        match req {
            CliExecutionRequest::Command(req) => self.run(req).await,
            CliExecutionRequest::Shell(req) => self.run_shell(req).await,
        }
    }
}

impl CliExecutor {
    // Internal execution paths
    pub(super) async fn run(&self, req: CommandRequest) -> Result<ExecutionOutput> {
        self.policy.validate_command(&req)?;
        let mut cmd = Command::new(&req.program);
        cmd.args(&req.args);

        self.run_inner(&mut cmd, RunParts::from(req)).await
    }

    pub(super) async fn run_shell(&self, req: ShellRequest) -> Result<ExecutionOutput> {
        self.policy.validate_shell(&req)?;
        let mut cmd = build_shell_command(&req.shell, &req.command);

        self.run_inner(&mut cmd, RunParts::from(req)).await
    }

    pub(super) async fn run_inner(
        &self,
        cmd: &mut Command,
        parts: RunParts,
    ) -> Result<ExecutionOutput> {
        let started_at = Instant::now();
        process::configure_command(
            cmd,
            parts.cwd,
            parts.env,
            parts.stdin.as_ref(),
            parts.background,
        )?;
        let mut child = process::spawn_child(cmd)?;
        let mut process_group_guard = process::ProcessGroupGuard::new(&child, !parts.background);
        let stdout_handle =
            process::take_output_reader(&mut child.stdout, self.policy.max_output_bytes);
        let stderr_handle =
            process::take_output_reader(&mut child.stderr, self.policy.max_output_bytes);

        process::write_stdin(&mut child, parts.stdin.as_ref()).await?;

        if parts.background {
            return process::background_output(&child);
        }

        let outcome = match process::wait_for_child(&mut child, parts.timeout_ms).await {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = process::collect_output(stdout_handle).await;
                let _ = process::collect_output(stderr_handle).await;
                return Err(err);
            }
        };
        process_group_guard.disarm();

        process::build_output(
            outcome,
            stdout_handle,
            stderr_handle,
            parts.fail_on_non_zero,
            started_at.elapsed().as_millis(),
        )
        .await
    }
}

pub(super) struct RunParts {
    cwd: Option<String>,
    env: Option<std::collections::HashMap<String, String>>,
    timeout_ms: Option<u64>,
    fail_on_non_zero: bool,
    stdin: Option<ExecutionStdin>,
    background: bool,
}

impl From<CommandRequest> for RunParts {
    fn from(req: CommandRequest) -> Self {
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

impl From<ShellRequest> for RunParts {
    fn from(req: ShellRequest) -> Self {
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
