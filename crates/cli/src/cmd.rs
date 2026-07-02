use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use agent_executor_core::{Error, Result};
use wait_timeout::ChildExt;

pub struct CmdTool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmdStdin {
    Text(String),
    Bytes(Vec<u8>),
    File(PathBuf),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub fail_on_non_zero: bool,
    pub stdin: Option<CmdStdin>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCmdRequest {
    pub command: String,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub fail_on_non_zero: bool,
    pub stdin: Option<CmdStdin>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub pid: Option<u32>,
}

impl CmdTool {
    pub fn run(req: CmdRequest) -> Result<CmdOutput> {
        let mut cmd = Command::new(&req.program);
        cmd.args(&req.args);

        run_inner(
            &mut cmd,
            req.cwd,
            req.env,
            req.timeout_ms,
            req.fail_on_non_zero,
            req.stdin,
            req.background,
        )
    }

    pub fn run_shell(req: ShellCmdRequest) -> Result<CmdOutput> {
        let mut cmd = build_shell_command(&req.command);

        run_inner(
            &mut cmd,
            req.cwd,
            req.env,
            req.timeout_ms,
            req.fail_on_non_zero,
            req.stdin,
            req.background,
        )
    }
}

fn run_inner(
    cmd: &mut Command,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    timeout_ms: Option<u64>,
    fail_on_non_zero: bool,
    stdin: Option<CmdStdin>,
    background: bool,
) -> Result<CmdOutput> {
    configure_command(cmd, cwd, env, stdin.as_ref(), background)?;
    let mut child = spawn_child(cmd)?;
    let stdout_handle = take_output_reader(&mut child.stdout);
    let stderr_handle = take_output_reader(&mut child.stderr);

    write_stdin(&mut child, stdin.as_ref())?;

    if background {
        return Ok(background_output(&child));
    }

    let status = match wait_for_child(&mut child, timeout_ms) {
        Ok(status) => status,
        Err(err) => {
            let _ = collect_output(stdout_handle);
            let _ = collect_output(stderr_handle);
            return Err(err);
        }
    };

    build_foreground_output(status, stdout_handle, stderr_handle, fail_on_non_zero)
}

fn configure_command(
    cmd: &mut Command,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    stdin: Option<&CmdStdin>,
    background: bool,
) -> Result<()> {
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    if let Some(env) = env {
        cmd.envs(env);
    }

    configure_stdin(cmd, stdin, background)?;
    configure_output(cmd, background);
    Ok(())
}

fn configure_stdin(cmd: &mut Command, stdin: Option<&CmdStdin>, background: bool) -> Result<()> {
    match stdin {
        Some(CmdStdin::File(path)) => {
            let file = File::open(path).map_err(Error::tool_io)?;
            cmd.stdin(file);
        }
        Some(CmdStdin::Null) => {
            cmd.stdin(Stdio::null());
        }
        Some(_) => {
            cmd.stdin(Stdio::piped());
        }
        None if background => {
            cmd.stdin(Stdio::null());
        }
        None => {}
    }

    Ok(())
}

fn configure_output(cmd: &mut Command, background: bool) {
    if background {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        return;
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
}

fn spawn_child(cmd: &mut Command) -> Result<std::process::Child> {
    cmd.spawn().map_err(Error::tool_io)
}

fn take_output_reader<R>(pipe: &mut Option<R>) -> Option<thread::JoinHandle<io::Result<Vec<u8>>>>
where
    R: Read + Send + 'static,
{
    pipe.take().map(spawn_reader)
}

fn background_output(child: &std::process::Child) -> CmdOutput {
    CmdOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        pid: Some(child.id()),
    }
}

fn wait_for_child(
    child: &mut std::process::Child,
    timeout_ms: Option<u64>,
) -> Result<std::process::ExitStatus> {
    match timeout_ms {
        Some(timeout_ms) => wait_with_timeout(child, timeout_ms),
        None => child.wait().map_err(Error::tool_io),
    }
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout_ms: u64,
) -> Result<std::process::ExitStatus> {
    let duration = Duration::from_millis(timeout_ms);
    match child.wait_timeout(duration).map_err(Error::tool_io)? {
        Some(status) => Ok(status),
        None => {
            kill_timed_out_child(child)?;
            Err(Error::tool_timeout())
        }
    }
}

fn kill_timed_out_child(child: &mut std::process::Child) -> Result<()> {
    child.kill().map_err(Error::tool_io)?;
    child.wait().map_err(Error::tool_io)?;
    Ok(())
}

fn write_stdin(child: &mut std::process::Child, stdin_content: Option<&CmdStdin>) -> Result<()> {
    let Some(stdin_content) = stdin_content else {
        return Ok(());
    };

    let Some(mut stdin) = child.stdin.take() else {
        if matches!(stdin_content, CmdStdin::Text(_) | CmdStdin::Bytes(_)) {
            return Err(Error::tool_io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stdin pipe not available",
            )));
        }
        return Ok(());
    };

    match stdin_content {
        CmdStdin::Text(text) => stdin.write_all(text.as_bytes()).map_err(Error::tool_io)?,
        CmdStdin::Bytes(bytes) => stdin.write_all(bytes).map_err(Error::tool_io)?,
        CmdStdin::File(_) | CmdStdin::Null => {}
    }

    stdin.flush().map_err(Error::tool_io)?;
    drop(stdin);
    Ok(())
}

fn spawn_reader<R>(mut reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(buf)
    })
}

fn collect_output(handle: Option<thread::JoinHandle<io::Result<Vec<u8>>>>) -> Result<String> {
    let Some(handle) = handle else {
        return Ok(String::new());
    };

    let bytes = handle
        .join()
        .map_err(|_| Error::tool_io(io::Error::other("output reader thread panicked")))?
        .map_err(Error::tool_io)?;

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn build_foreground_output(
    status: std::process::ExitStatus,
    stdout_handle: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
    stderr_handle: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
    fail_on_non_zero: bool,
) -> Result<CmdOutput> {
    let stdout = collect_output(stdout_handle)?;
    let stderr = collect_output(stderr_handle)?;
    let exit_code = status.code().unwrap_or(-1);

    if fail_on_non_zero && exit_code != 0 {
        return Err(Error::tool_cmd_failed(exit_code));
    }

    Ok(CmdOutput {
        stdout,
        stderr,
        exit_code,
        pid: None,
    })
}

fn build_shell_command(command: &str) -> Command {
    if cfg!(target_os = "windows") {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/c").arg(command);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn test_successful_command() {
        let req = CmdRequest {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        };
        let result = CmdTool::run(req);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout.trim(), "hello");
        assert!(output.pid.is_none());
    }

    #[test]
    fn test_shell_command() {
        let req = ShellCmdRequest {
            command: "echo 'hello from shell'".to_string(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        };
        let result = CmdTool::run_shell(req);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout.trim(), "hello from shell");
        assert!(output.pid.is_none());
    }

    #[test]
    fn test_timeout_command() {
        let req = CmdRequest {
            program: "sleep".to_string(),
            args: vec!["2".to_string()],
            cwd: None,
            env: None,
            timeout_ms: Some(100),
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        };
        let result = CmdTool::run(req);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string().to_lowercase();
        assert!(err_msg.contains("timed out"));
    }

    #[test]
    fn test_non_existent_command() {
        let req = CmdRequest {
            program: "this_command_does_not_exist_12345".to_string(),
            args: vec![],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        };
        let result = CmdTool::run(req);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string().to_lowercase();
        assert!(err_msg.contains("no such file") || err_msg.contains("not found"));
    }

    #[test]
    fn test_stdin_text() {
        let req = CmdRequest {
            program: "cat".to_string(),
            args: vec![],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: Some(CmdStdin::Text("hello stdin text".to_string())),
            background: false,
        };
        let result = CmdTool::run(req);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, "hello stdin text");
        assert!(output.pid.is_none());
    }

    #[test]
    fn test_stdin_bytes() {
        let req = CmdRequest {
            program: "cat".to_string(),
            args: vec![],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: Some(CmdStdin::Bytes(b"hello stdin bytes".to_vec())),
            background: false,
        };
        let result = CmdTool::run(req);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, "hello stdin bytes");
        assert!(output.pid.is_none());
    }

    #[test]
    fn test_stdin_file() {
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "hello stdin file").unwrap();

        let req = CmdRequest {
            program: "cat".to_string(),
            args: vec![],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: Some(CmdStdin::File(temp_file.path().to_path_buf())),
            background: false,
        };
        let result = CmdTool::run(req);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, "hello stdin file");
        assert!(output.pid.is_none());
    }

    #[test]
    fn test_stdin_null() {
        let req = CmdRequest {
            program: "cat".to_string(),
            args: vec![],
            cwd: None,
            env: None,
            timeout_ms: Some(1_000),
            fail_on_non_zero: false,
            stdin: Some(CmdStdin::Null),
            background: false,
        };
        let result = CmdTool::run(req);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.is_empty());
        assert!(output.pid.is_none());
    }

    #[test]
    fn test_background() {
        let req = CmdRequest {
            program: "sleep".to_string(),
            args: vec!["1".to_string()],
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: true,
        };
        let result = CmdTool::run(req);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.is_empty());
        assert!(output.pid.is_some());
        assert!(output.pid.unwrap() > 0);
    }

    #[test]
    fn test_shell_pipe() {
        let command = if cfg!(target_os = "windows") {
            "echo hello pipe | findstr pipe"
        } else {
            "echo 'hello pipe' | grep pipe"
        };

        let req = ShellCmdRequest {
            command: command.to_string(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        };
        let result = CmdTool::run_shell(req);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("hello pipe"));
        assert!(output.pid.is_none());
    }

    #[test]
    fn test_non_zero_exit_can_fail() {
        let req = if cfg!(target_os = "windows") {
            ShellCmdRequest {
                command: "cmd /c exit 7".to_string(),
                cwd: None,
                env: None,
                timeout_ms: None,
                fail_on_non_zero: true,
                stdin: None,
                background: false,
            }
        } else {
            ShellCmdRequest {
                command: "sh -c 'exit 7'".to_string(),
                cwd: None,
                env: None,
                timeout_ms: None,
                fail_on_non_zero: true,
                stdin: None,
                background: false,
            }
        };

        let result = CmdTool::run_shell(req);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string().to_lowercase();
        assert!(err_msg.contains("exit code 7"));
    }

    #[test]
    fn test_non_zero_exit_can_be_observed_without_error() {
        let req = if cfg!(target_os = "windows") {
            ShellCmdRequest {
                command: "cmd /c exit 9".to_string(),
                cwd: None,
                env: None,
                timeout_ms: None,
                fail_on_non_zero: false,
                stdin: None,
                background: false,
            }
        } else {
            ShellCmdRequest {
                command: "sh -c 'exit 9'".to_string(),
                cwd: None,
                env: None,
                timeout_ms: None,
                fail_on_non_zero: false,
                stdin: None,
                background: false,
            }
        };

        let result = CmdTool::run_shell(req).unwrap();
        assert_eq!(result.exit_code, 9);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_non_utf8_stdout_is_preserved_lossily() {
        let req = ShellCmdRequest {
            command: "printf '\\377\\376abc'".to_string(),
            cwd: None,
            env: None,
            timeout_ms: None,
            fail_on_non_zero: false,
            stdin: None,
            background: false,
        };

        let result = CmdTool::run_shell(req).unwrap();
        assert!(result.stdout.contains("abc"));
        assert!(!result.stdout.is_empty());
    }
}
