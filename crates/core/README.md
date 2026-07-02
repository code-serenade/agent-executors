# agent-executor-core

Shared primitives for the `agent-executors` workspace.

This crate owns common API types that should stay stable across executor crates.
At the moment that means the workspace-wide `Error` and `Result` types.
