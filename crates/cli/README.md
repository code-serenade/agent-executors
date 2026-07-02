# agent-executor-cli

Small Rust executor primitives for running local commands and shell scripts.

## Features

- Run commands with args, cwd, env, stdin, timeout, and output capture.
- Run shell commands when shell syntax is needed.
- Return structured command status for success, non-zero exits, timeout, and background start.
- Manage long-running command sessions with start, status, output snapshot, and stop operations.
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
use agent_executor_cli::{CmdRequest, CmdTool};

let output = CmdTool::run(CmdRequest {
    program: "echo".to_string(),
    args: vec!["hello".to_string()],
    cwd: None,
    env: None,
    timeout_ms: Some(1_000),
    fail_on_non_zero: true,
    stdin: None,
    background: false,
})?;

assert_eq!(output.stdout.trim(), "hello");
assert_eq!(output.status, agent_executor_cli::CmdStatus::Success);
assert!(!output.stdout_truncated);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Notes

- `run_shell` passes the command string to the system shell. Do not use it with untrusted input.
- Command output is collected into memory.
- Timeout returns `CmdStatus::TimedOut` after killing the direct child process.
- On Unix, timeout and session stop try to kill the process group before killing the direct child.
- On Windows, process cleanup currently targets the direct child.
