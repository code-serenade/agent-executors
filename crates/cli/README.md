# agent-executor-cli

Small Rust executor primitives for running local commands and shell scripts.

## Features

- Run commands with args, cwd, env, stdin, timeout, and output capture.
- Run shell commands when shell syntax is needed.
- Choose the shell for shell commands (`sh`, `zsh`, `bash`, `cmd.exe`, or a custom path).
- Return structured command status for success, non-zero exits, timeout, and background start.
- Apply command policy checks before execution.
- Limit captured output and mark truncated stdout/stderr.
- Measure command duration.
- Keep command execution behind a small Rust API that agents can call later.

## Install

```toml
[dependencies]
agent-executor-cli = "0.1.0"
```

## Run a Command

```rust
use agent_executor_cli::{CliExecutionRequest, CommandRequest, CliExecutor};

let output = CliExecutor::default().execute(CliExecutionRequest::Command(CommandRequest {
    program: "echo".to_string(),
    args: vec!["hello".to_string()],
    cwd: None,
    env: None,
    timeout_ms: Some(1_000),
    fail_on_non_zero: true,
    stdin: None,
    background: false,
}))?;

assert_eq!(output.stdout.trim(), "hello");
assert_eq!(output.status, agent_executor_cli::ExecutionStatus::Success);
assert!(!output.stdout_truncated);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Notes

- `ShellRequest` passes the command string to the selected shell. Do not use it with untrusted input.
- Command output is collected into memory.
- Timeout returns `ExecutionStatus::TimedOut` after killing the direct child process.
- On Unix, timeout tries to kill the process group before killing the direct child.
- On Windows, process cleanup currently targets the direct child.
