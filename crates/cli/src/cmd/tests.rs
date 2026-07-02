use std::{collections::HashSet, io::Write};

use tempfile::NamedTempFile;

use super::*;

#[test]
fn command_success_returns_structured_success() {
    let output = CmdTool::run(CmdRequest {
        program: "echo".to_string(),
        args: vec!["hello".to_string()],
        cwd: None,
        env: None,
        timeout_ms: None,
        fail_on_non_zero: false,
        stdin: None,
        background: false,
    })
    .unwrap();

    assert_eq!(output.exit_code, 0);
    assert_eq!(output.status, CmdStatus::Success);
    assert_eq!(output.stdout.trim(), "hello");
    assert!(!output.stdout_truncated);
    assert!(!output.stderr_truncated);
    assert!(output.pid.is_none());
}

#[test]
fn unified_execute_runs_command_requests() {
    let output = CmdTool::execute(CliExecutionRequest::Command(CmdRequest {
        program: "echo".to_string(),
        args: vec!["hello".to_string()],
        cwd: None,
        env: None,
        timeout_ms: None,
        fail_on_non_zero: false,
        stdin: None,
        background: false,
    }))
    .unwrap();

    assert_eq!(output.status, CmdStatus::Success);
    assert_eq!(output.stdout.trim(), "hello");
}

#[test]
fn shell_command_supports_shell_syntax() {
    let command = if cfg!(target_os = "windows") {
        "echo hello pipe | findstr pipe"
    } else {
        "echo 'hello pipe' | grep pipe"
    };

    let output = CmdTool::run_shell(ShellCmdRequest {
        command: command.to_string(),
        cwd: None,
        env: None,
        timeout_ms: None,
        fail_on_non_zero: false,
        stdin: None,
        background: false,
    })
    .unwrap();

    assert_eq!(output.status, CmdStatus::Success);
    assert!(output.stdout.contains("hello pipe"));
}

#[test]
fn timeout_is_structured_output() {
    let output = CmdTool::run(CmdRequest {
        program: "sleep".to_string(),
        args: vec!["2".to_string()],
        cwd: None,
        env: None,
        timeout_ms: Some(100),
        fail_on_non_zero: false,
        stdin: None,
        background: false,
    })
    .unwrap();

    assert_eq!(output.status, CmdStatus::TimedOut);
    assert_eq!(output.exit_code, -1);
    assert!(output.duration_ms >= 50);
}

#[test]
fn non_zero_exit_can_be_observed_without_error() {
    let output = CmdTool::run_shell(ShellCmdRequest {
        command: exit_command(9),
        cwd: None,
        env: None,
        timeout_ms: None,
        fail_on_non_zero: false,
        stdin: None,
        background: false,
    })
    .unwrap();

    assert_eq!(output.exit_code, 9);
    assert_eq!(output.status, CmdStatus::Failed(9));
}

#[test]
fn non_zero_exit_can_fail() {
    let result = CmdTool::run_shell(ShellCmdRequest {
        command: exit_command(7),
        cwd: None,
        env: None,
        timeout_ms: None,
        fail_on_non_zero: true,
        stdin: None,
        background: false,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("exit code 7"));
}

#[test]
fn stdin_text_bytes_file_and_null_are_supported() {
    let text = CmdTool::run(cat_request(Some(CmdStdin::Text("hello text".to_string())))).unwrap();
    assert_eq!(text.stdout, "hello text");

    let bytes = CmdTool::run(cat_request(Some(CmdStdin::Bytes(b"hello bytes".to_vec())))).unwrap();
    assert_eq!(bytes.stdout, "hello bytes");

    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "hello file").unwrap();
    let file = CmdTool::run(cat_request(Some(CmdStdin::File(
        temp_file.path().to_path_buf(),
    ))))
    .unwrap();
    assert_eq!(file.stdout, "hello file");

    let null = CmdTool::run(cat_request(Some(CmdStdin::Null))).unwrap();
    assert!(null.stdout.is_empty());
}

#[test]
fn policy_can_reject_shell_and_large_timeout() {
    let no_shell = CmdRunner::new(CommandPolicy {
        allow_shell: false,
        ..CommandPolicy::default()
    });
    let shell_result = no_shell.run_shell(ShellCmdRequest {
        command: "echo nope".to_string(),
        cwd: None,
        env: None,
        timeout_ms: None,
        fail_on_non_zero: false,
        stdin: None,
        background: false,
    });
    assert!(shell_result.is_err());

    let bounded = CmdRunner::new(CommandPolicy {
        max_timeout_ms: Some(10),
        ..CommandPolicy::default()
    });
    let timeout_result = bounded.run(CmdRequest {
        program: "echo".to_string(),
        args: vec!["hello".to_string()],
        cwd: None,
        env: None,
        timeout_ms: Some(20),
        fail_on_non_zero: false,
        stdin: None,
        background: false,
    });
    assert!(timeout_result.is_err());
}

#[test]
fn policy_can_restrict_program_cwd_and_env() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runner = CmdRunner::new(CommandPolicy {
        allowed_programs: Some(HashSet::from(["echo".to_string()])),
        allowed_cwd_roots: vec![temp_dir.path().to_path_buf()],
        allowed_env_vars: Some(HashSet::from(["SAFE_ENV".to_string()])),
        ..CommandPolicy::default()
    });

    let allowed = runner.run(CmdRequest {
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
    });
    assert!(allowed.is_ok());

    let blocked_program = runner.run(CmdRequest {
        program: "cat".to_string(),
        args: vec![],
        cwd: Some(temp_dir.path().display().to_string()),
        env: None,
        timeout_ms: None,
        fail_on_non_zero: false,
        stdin: Some(CmdStdin::Null),
        background: false,
    });
    assert!(blocked_program.is_err());

    let blocked_env = runner.run(CmdRequest {
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
    });
    assert!(blocked_env.is_err());
}

#[test]
fn policy_can_limit_captured_output_size() {
    let runner = CmdRunner::new(CommandPolicy {
        max_output_bytes: Some(5),
        ..CommandPolicy::default()
    });

    let output = runner
        .run_shell(ShellCmdRequest {
            command: output_limit_command(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        })
        .unwrap();

    assert_eq!(output.stdout, "12345");
    assert!(output.stdout_truncated);
    assert!(!output.stderr_truncated);
}

#[test]
fn session_manager_can_start_query_and_stop() {
    let manager = CmdSessionManager::default();
    let session = manager
        .start(CmdRequest {
            program: "sleep".to_string(),
            args: vec!["5".to_string()],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: true,
        })
        .unwrap();

    assert!(session.pid > 0);
    assert_eq!(
        manager.status(session).unwrap(),
        CmdSessionStatus::Running { pid: session.pid }
    );
    assert!(matches!(
        manager.stop(session).unwrap(),
        CmdSessionStatus::Exited(_)
    ));
    assert!(matches!(
        manager.status(session).unwrap(),
        CmdSessionStatus::Exited(_)
    ));
}

#[test]
fn session_manager_captures_output_snapshot() {
    let manager = CmdSessionManager::default();
    let session = manager
        .start_shell(ShellCmdRequest {
            command: session_output_command(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: true,
        })
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(100));
    let output = manager.output(session).unwrap();
    assert_eq!(output.stdout, "hello");
    assert!(output.stderr.is_empty());
    let _ = manager.stop(session).unwrap();
}

#[cfg(not(target_os = "windows"))]
#[test]
fn non_utf8_stdout_is_preserved_lossily() {
    let output = CmdTool::run_shell(ShellCmdRequest {
        command: "printf '\\377\\376abc'".to_string(),
        cwd: None,
        env: None,
        timeout_ms: None,
        fail_on_non_zero: false,
        stdin: None,
        background: false,
    })
    .unwrap();

    assert!(output.stdout.contains("abc"));
    assert!(!output.stdout.is_empty());
}

fn cat_request(stdin: Option<CmdStdin>) -> CmdRequest {
    CmdRequest {
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
    if cfg!(target_os = "windows") {
        format!("cmd /c exit {code}")
    } else {
        format!("sh -c 'exit {code}'")
    }
}

fn session_output_command() -> String {
    if cfg!(target_os = "windows") {
        "echo hello & timeout /t 1 > nul".to_string()
    } else {
        "printf hello; sleep 1".to_string()
    }
}

fn output_limit_command() -> String {
    if cfg!(target_os = "windows") {
        "echo 123456789".to_string()
    } else {
        "printf 123456789".to_string()
    }
}
