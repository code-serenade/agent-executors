# agent-executor

Facade crate for the `agent-executors` workspace.

Use this crate when you want one dependency and feature-gated access to executor
implementations.

```toml
[dependencies]
agent-executor = { version = "0.1.0", features = ["cli"] }
```

```rust
use agent_executor::{cli::{CliExecutionRequest, CliExecutor, CommandRequest}, Executor, Result};

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

Enable the `patch` feature to apply structured file patches:

```toml
[dependencies]
agent-executor = { version = "0.1.0", features = ["patch"] }
```

```rust
use agent_executor::{
    patch::{PatchExecutionRequest, PatchExecutor, PatchStatus},
    Executor,
    Result,
};

async fn apply() -> Result<()> {
    let output = PatchExecutor::default().execute(PatchExecutionRequest {
        patch: "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch".to_string(),
        cwd: std::env::current_dir().expect("current directory"),
        dry_run: false,
    }).await?;

    assert_eq!(output.status, PatchStatus::Applied);
    Ok(())
}
```
