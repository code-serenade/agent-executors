# agent-executor-patch

Small Rust executor primitives for applying structured file patches.

## Features

- Parse Codex-style patch text.
- Add, update, and delete files under a requested working directory.
- Support dry-run requests that report changed files without writing.
- Apply patch policy checks before touching the filesystem.
- Keep patch execution behind one async Rust API that agent actions can await.

## Install

```toml
[dependencies]
agent-executor-patch = "0.1.0"
```

## Apply a Patch

```rust
use agent_executor_patch::{Executor, PatchExecutionRequest, PatchExecutor};

async fn run() -> agent_executor_patch::Result<()> {
    let output = PatchExecutor::default().execute(PatchExecutionRequest {
        patch: "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch".to_string(),
        cwd: std::env::current_dir().expect("current directory"),
        dry_run: false,
    }).await?;

    assert_eq!(output.status, agent_executor_patch::PatchStatus::Applied);
    Ok(())
}
```

## Notes

- Patch paths must be relative and must stay inside `cwd`.
- Dry-run validates and reports the target file changes, but does not write.
- Parse, policy, and hunk-match failures are returned as policy errors.
