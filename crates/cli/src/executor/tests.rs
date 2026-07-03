use std::{collections::HashSet, io::Write};

use agent_executor_core::{ErrorCategory, Executor};
use tempfile::NamedTempFile;

use super::*;

#[tokio::test]
async fn command_success_returns_structured_success() {
    let output = CliExecutor::default()
        .execute(CliExecutionRequest::Command(CommandRequest {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await
        .unwrap();

    assert_eq!(output.exit_code, 0);
    assert_eq!(output.status, ExecutionStatus::Success);
    assert_eq!(output.stdout.trim(), "hello");
    assert!(!output.stdout_truncated);
    assert!(!output.stderr_truncated);
    assert!(output.pid.is_none());
}

#[tokio::test]
async fn unified_execute_runs_command_requests() {
    let output = CliExecutor::default()
        .execute(CliExecutionRequest::Command(CommandRequest {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await
        .unwrap();

    assert_eq!(output.status, ExecutionStatus::Success);
    assert_eq!(output.stdout.trim(), "hello");
}

#[tokio::test]
async fn cli_executor_implements_core_executor_trait() {
    async fn run_with_trait<E>(
        executor: &E,
        request: E::Request,
    ) -> agent_executor_core::Result<E::Output>
    where
        E: Executor,
    {
        executor.execute(request).await
    }

    let output = run_with_trait(
        &CliExecutor::default(),
        CliExecutionRequest::Command(CommandRequest {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }),
    )
    .await
    .unwrap();

    assert_eq!(output.status, ExecutionStatus::Success);
    assert_eq!(output.stdout.trim(), "hello");
}

#[tokio::test]
async fn shell_command_supports_shell_syntax() {
    let command = if cfg!(target_os = "windows") {
        "echo hello pipe | findstr pipe"
    } else {
        "echo 'hello pipe' | grep pipe"
    };

    let output = CliExecutor::default()
        .execute(CliExecutionRequest::Shell(ShellRequest {
            command: command.to_string(),
            shell: ShellKind::default(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await
        .unwrap();

    assert_eq!(output.status, ExecutionStatus::Success);
    assert!(output.stdout.contains("hello pipe"));
}

#[tokio::test]
async fn timeout_is_structured_output() {
    let output = CliExecutor::default()
        .execute(CliExecutionRequest::Command(CommandRequest {
            program: "sleep".to_string(),
            args: vec!["2".to_string()],
            cwd: None,
            env: None,
            timeout_ms: Some(100),
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await
        .unwrap();

    assert_eq!(output.status, ExecutionStatus::TimedOut);
    assert_eq!(output.exit_code, -1);
    assert!(output.duration_ms >= 50);
}

#[tokio::test]
async fn non_zero_exit_can_be_observed_without_error() {
    let output = CliExecutor::default()
        .execute(CliExecutionRequest::Shell(ShellRequest {
            command: exit_command(9),
            shell: ShellKind::default(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await
        .unwrap();

    assert_eq!(output.exit_code, 9);
    assert_eq!(output.status, ExecutionStatus::Failed(9));
}

#[tokio::test]
async fn non_zero_exit_can_fail() {
    let result = CliExecutor::default()
        .execute(CliExecutionRequest::Shell(ShellRequest {
            command: exit_command(7),
            shell: ShellKind::default(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: true,
            stdin: None,
            background: false,
        }))
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("exit code 7"));
}

#[tokio::test]
async fn stdin_text_bytes_file_and_null_are_supported() {
    let text = CliExecutor::default()
        .execute(CliExecutionRequest::Command(cat_request(Some(
            ExecutionStdin::Text("hello text".to_string()),
        ))))
        .await
        .unwrap();
    assert_eq!(text.stdout, "hello text");

    let bytes = CliExecutor::default()
        .execute(CliExecutionRequest::Command(cat_request(Some(
            ExecutionStdin::Bytes(b"hello bytes".to_vec()),
        ))))
        .await
        .unwrap();
    assert_eq!(bytes.stdout, "hello bytes");

    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "hello file").unwrap();
    let file = CliExecutor::default()
        .execute(CliExecutionRequest::Command(cat_request(Some(
            ExecutionStdin::File(temp_file.path().to_path_buf()),
        ))))
        .await
        .unwrap();
    assert_eq!(file.stdout, "hello file");

    let null = CliExecutor::default()
        .execute(CliExecutionRequest::Command(cat_request(Some(
            ExecutionStdin::Null,
        ))))
        .await
        .unwrap();
    assert!(null.stdout.is_empty());
}

#[tokio::test]
async fn policy_can_reject_shell_and_large_timeout() {
    let no_shell = CliExecutor::new(CommandPolicy {
        allow_shell: false,
        ..CommandPolicy::default()
    });
    let shell_result = no_shell
        .execute(CliExecutionRequest::Shell(ShellRequest {
            command: "echo nope".to_string(),
            shell: ShellKind::default(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await;
    let shell_error = shell_result.unwrap_err();
    assert_eq!(shell_error.category(), ErrorCategory::Policy);

    let bounded = CliExecutor::new(CommandPolicy {
        max_timeout_ms: Some(10),
        ..CommandPolicy::default()
    });
    let timeout_result = bounded
        .execute(CliExecutionRequest::Command(CommandRequest {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: None,
            env: None,
            timeout_ms: Some(20),
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await;
    let timeout_error = timeout_result.unwrap_err();
    assert_eq!(timeout_error.category(), ErrorCategory::Policy);
}

#[tokio::test]
async fn policy_can_restrict_program_cwd_and_env() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runner = CliExecutor::new(CommandPolicy {
        allowed_programs: Some(HashSet::from(["echo".to_string()])),
        allowed_cwd_roots: vec![temp_dir.path().to_path_buf()],
        allowed_env_vars: Some(HashSet::from(["SAFE_ENV".to_string()])),
        ..CommandPolicy::default()
    });

    let allowed = runner
        .execute(CliExecutionRequest::Command(CommandRequest {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: Some(temp_dir.path().display().to_string()),
            env: Some(std::collections::HashMap::from([(
                "SAFE_ENV".to_string(),
                "1".to_string(),
            )])),
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await;
    assert!(allowed.is_ok());

    let blocked_program = runner
        .execute(CliExecutionRequest::Command(CommandRequest {
            program: "cat".to_string(),
            args: vec![],
            cwd: Some(temp_dir.path().display().to_string()),
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: Some(ExecutionStdin::Null),
            background: false,
        }))
        .await;
    assert!(blocked_program.is_err());

    let blocked_env = runner
        .execute(CliExecutionRequest::Command(CommandRequest {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: Some(temp_dir.path().display().to_string()),
            env: Some(std::collections::HashMap::from([(
                "UNSAFE_ENV".to_string(),
                "1".to_string(),
            )])),
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await;
    assert!(blocked_env.is_err());
}

#[tokio::test]
async fn policy_can_limit_captured_output_size() {
    let runner = CliExecutor::new(CommandPolicy {
        max_output_bytes: Some(5),
        ..CommandPolicy::default()
    });

    let output = runner
        .execute(CliExecutionRequest::Shell(ShellRequest {
            command: output_limit_command(),
            shell: ShellKind::default(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await
        .unwrap();

    assert_eq!(output.stdout, "12345");
    assert!(output.stdout_truncated);
    assert!(!output.stderr_truncated);
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn shell_request_can_choose_zsh() {
    if std::process::Command::new("zsh")
        .arg("-c")
        .arg("true")
        .status()
        .is_err()
    {
        return;
    }

    let output = CliExecutor::default()
        .execute(CliExecutionRequest::Shell(
            ShellRequest::new("echo $ZSH_VERSION").with_shell(ShellKind::Zsh),
        ))
        .await
        .unwrap();

    assert_eq!(output.status, ExecutionStatus::Success);
    assert!(!output.stdout.trim().is_empty());
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn non_utf8_stdout_is_preserved_lossily() {
    let output = CliExecutor::default()
        .execute(CliExecutionRequest::Shell(ShellRequest {
            command: "printf '\\377\\376abc'".to_string(),
            shell: ShellKind::default(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        }))
        .await
        .unwrap();

    assert!(output.stdout.contains("abc"));
    assert!(!output.stdout.is_empty());
}

fn cat_request(stdin: Option<ExecutionStdin>) -> CommandRequest {
    CommandRequest {
        program: "cat".to_string(),
        args: vec![],
        cwd: None,
        env: None,
        timeout_ms: Some(1_000),
        fail_on_non_zero: false,
        stdin,
        background: false,
    }
}

fn exit_command(code: i32) -> String {
    format!("exit {code}")
}

fn output_limit_command() -> String {
    if cfg!(target_os = "windows") {
        "echo 123456789".to_string()
    } else {
        "printf 123456789".to_string()
    }
}
