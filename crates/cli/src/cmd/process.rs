#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use agent_executor_core::{Error, Result};
use wait_timeout::ChildExt;

use super::types::{CmdOutput, CmdStatus, CmdStdin};

pub(super) enum WaitOutcome {
    Exited(ExitStatus),
    TimedOut,
}

pub(super) fn configure_command(
    cmd: &mut Command,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    stdin: Option<&CmdStdin>,
    background: bool,
) -> Result<()> {
    configure_process_group(cmd);

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

pub(super) fn configure_session_command(
    cmd: &mut Command,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    stdin: Option<&CmdStdin>,
) -> Result<()> {
    configure_process_group(cmd);

    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    if let Some(env) = env {
        cmd.envs(env);
    }

    configure_stdin(cmd, stdin, true)?;
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(())
}

pub(super) fn spawn_child(cmd: &mut Command) -> Result<Child> {
    cmd.spawn().map_err(Error::tool_io)
}

pub(super) fn take_output_reader<R>(
    pipe: &mut Option<R>,
    max_bytes: Option<usize>,
) -> Option<thread::JoinHandle<io::Result<CapturedBytes>>>
where
    R: Read + Send + 'static,
{
    pipe.take().map(|reader| spawn_reader(reader, max_bytes))
}

pub(super) fn write_stdin(child: &mut Child, stdin_content: Option<&CmdStdin>) -> Result<()> {
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

pub(super) fn wait_for_child(child: &mut Child, timeout_ms: Option<u64>) -> Result<WaitOutcome> {
    match timeout_ms {
        Some(timeout_ms) => wait_with_timeout(child, timeout_ms),
        None => child
            .wait()
            .map(WaitOutcome::Exited)
            .map_err(Error::tool_io),
    }
}

pub(super) fn collect_output(
    handle: Option<thread::JoinHandle<io::Result<CapturedBytes>>>,
) -> Result<CapturedOutput> {
    let Some(handle) = handle else {
        return Ok(CapturedOutput::empty());
    };

    let captured = handle
        .join()
        .map_err(|_| Error::tool_io(io::Error::other("output reader thread panicked")))?
        .map_err(Error::tool_io)?;

    Ok(CapturedOutput {
        text: String::from_utf8_lossy(&captured.bytes).into_owned(),
        truncated: captured.truncated,
    })
}

#[derive(Debug)]
pub(super) struct SessionOutputCapture {
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl SessionOutputCapture {
    pub(super) fn snapshot(&self) -> Result<(String, String)> {
        Ok((
            snapshot_buffer(&self.stdout)?,
            snapshot_buffer(&self.stderr)?,
        ))
    }
}

pub(super) fn capture_session_output(child: &mut Child) -> SessionOutputCapture {
    SessionOutputCapture {
        stdout: capture_pipe(&mut child.stdout),
        stderr: capture_pipe(&mut child.stderr),
    }
}

pub(super) fn build_output(
    outcome: WaitOutcome,
    stdout_handle: Option<thread::JoinHandle<io::Result<CapturedBytes>>>,
    stderr_handle: Option<thread::JoinHandle<io::Result<CapturedBytes>>>,
    fail_on_non_zero: bool,
    duration_ms: u128,
) -> Result<CmdOutput> {
    let stdout = collect_output(stdout_handle)?;
    let stderr = collect_output(stderr_handle)?;

    match outcome {
        WaitOutcome::TimedOut => Ok(CmdOutput::timed_out(
            stdout.text,
            stderr.text,
            duration_ms,
            stdout.truncated,
            stderr.truncated,
        )),
        WaitOutcome::Exited(status) => {
            let exit_code = status.code().unwrap_or(-1);
            let output = CmdOutput::foreground(
                stdout.text,
                stderr.text,
                exit_code,
                duration_ms,
                stdout.truncated,
                stderr.truncated,
            );

            if fail_on_non_zero && matches!(output.status, CmdStatus::Failed(_)) {
                return Err(Error::tool_cmd_failed(exit_code));
            }

            Ok(output)
        }
    }
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

fn wait_with_timeout(child: &mut Child, timeout_ms: u64) -> Result<WaitOutcome> {
    let duration = Duration::from_millis(timeout_ms);
    match child.wait_timeout(duration).map_err(Error::tool_io)? {
        Some(status) => Ok(WaitOutcome::Exited(status)),
        None => {
            kill_timed_out_child(child)?;
            Ok(WaitOutcome::TimedOut)
        }
    }
}

fn kill_timed_out_child(child: &mut Child) -> Result<()> {
    kill_child_process_group(child);
    child.kill().map_err(Error::tool_io)?;
    child.wait().map_err(Error::tool_io)?;
    Ok(())
}

pub(super) fn stop_child(child: &mut Child) -> Result<ExitStatus> {
    kill_child_process_group(child);
    child.kill().map_err(Error::tool_io)?;
    child.wait().map_err(Error::tool_io)
}

#[derive(Debug)]
pub(super) struct CapturedOutput {
    pub(super) text: String,
    pub(super) truncated: bool,
}

impl CapturedOutput {
    fn empty() -> Self {
        Self {
            text: String::new(),
            truncated: false,
        }
    }
}

#[derive(Debug)]
pub(super) struct CapturedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

fn spawn_reader<R>(
    mut reader: R,
    max_bytes: Option<usize>,
) -> thread::JoinHandle<io::Result<CapturedBytes>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut chunk = [0; 8192];

        loop {
            let count = reader.read(&mut chunk)?;
            if count == 0 {
                break;
            }

            match max_bytes {
                Some(max_bytes) if bytes.len() < max_bytes => {
                    let remaining = max_bytes - bytes.len();
                    let keep = remaining.min(count);
                    bytes.extend_from_slice(&chunk[.. keep]);
                    truncated |= keep < count;
                }
                Some(_) => {
                    truncated = true;
                }
                None => bytes.extend_from_slice(&chunk[.. count]),
            }
        }

        Ok(CapturedBytes { bytes, truncated })
    })
}

fn capture_pipe<R>(pipe: &mut Option<R>) -> Arc<Mutex<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let Some(mut reader) = pipe.take() else {
        return buffer;
    };

    let thread_buffer = Arc::clone(&buffer);
    thread::spawn(move || {
        let mut chunk = [0; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    let Ok(mut buffer) = thread_buffer.lock() else {
                        break;
                    };
                    buffer.extend_from_slice(&chunk[.. count]);
                }
                Err(_) => break,
            }
        }
    });

    buffer
}

fn snapshot_buffer(buffer: &Arc<Mutex<Vec<u8>>>) -> Result<String> {
    let bytes = buffer
        .lock()
        .map_err(|_| Error::tool_io(io::Error::other("session output lock poisoned")))?
        .clone();

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(unix)]
fn configure_process_group(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group(_cmd: &mut Command) {}

#[cfg(unix)]
fn kill_child_process_group(child: &Child) {
    let pgid = -(child.id() as i32);
    unsafe {
        libc::kill(pgid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_child_process_group(_child: &Child) {}
