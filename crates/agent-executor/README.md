# agent-executor

Facade crate for the `agent-executors` workspace.

Use this crate when you want one dependency and feature-gated access to executor
implementations.

```toml
[dependencies]
agent-executor = { version = "0.1.0", features = ["cli"] }
```

```rust
use agent_executor::{cli::{CliExecutionRequest, CliExecutor, CommandRequest}, Result};

async fn run() -> Result<()> {
    let output = CliExecutor::default().execute(CliExecutionRequest::Command(CommandRequest {
        program: "echo".to_string(),
        args: vec!["hello".to_string()],
        cwd: None,
        env: None,
        timeout_ms: Some(1_000),
        fail_on_non_zero: true,
        stdin: None,
        background: false,
    })).await?;

    assert_eq!(output.stdout.trim(), "hello");
    Ok(())
}
```
