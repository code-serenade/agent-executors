#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::HashMap,
    fs::File,
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};

use agent_executor_core::{Error, Result};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    task::JoinHandle,
};

use super::types::{ExecutionOutput, ExecutionStatus, ExecutionStdin};

pub(super) enum WaitOutcome {
    Exited(ExitStatus),
    TimedOut,
}

pub(super) fn configure_command(
    cmd: &mut Command,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    stdin: Option<&ExecutionStdin>,
) -> Result<()> {
    configure_process_group(cmd);
    cmd.kill_on_drop(true);

    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    if let Some(env) = env {
        cmd.envs(env);
    }

    configure_stdin(cmd, stdin)?;
    configure_output(cmd);
    Ok(())
}

pub(super) fn spawn_child(cmd: &mut Command) -> Result<Child> {
    cmd.spawn().map_err(Error::tool_io)
}

pub(super) struct ProcessGroupGuard {
    #[cfg(unix)]
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    pub(super) fn new(child: &Child, enabled: bool) -> Self {
        #[cfg(unix)]
        {
            let pid = enabled.then(|| child.id()).flatten();
            Self { pid }
        }

        #[cfg(not(unix))]
        {
            let _ = (child, enabled);
            Self {}
        }
    }

    pub(super) fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.pid = None;
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            kill_process_group(pid);
        }
    }
}

pub(super) fn take_output_reader<R>(
    pipe: &mut Option<R>,
    max_bytes: Option<usize>,
) -> Option<JoinHandle<io::Result<CapturedBytes>>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    pipe.take().map(|reader| spawn_reader(reader, max_bytes))
}

pub(super) async fn write_stdin(
    child: &mut Child,
    stdin_content: Option<&ExecutionStdin>,
) -> Result<()> {
    let Some(stdin_content) = stdin_content else {
        return Ok(());
    };

    let Some(mut stdin) = child.stdin.take() else {
        if matches!(
            stdin_content,
            ExecutionStdin::Text(_) | ExecutionStdin::Bytes(_)
        ) {
            return Err(Error::tool_io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stdin pipe not available",
            )));
        }
        return Ok(());
    };

    match stdin_content {
        ExecutionStdin::Text(text) => stdin
            .write_all(text.as_bytes())
            .await
            .map_err(Error::tool_io)?,
        ExecutionStdin::Bytes(bytes) => stdin.write_all(bytes).await.map_err(Error::tool_io)?,
        ExecutionStdin::File(_) | ExecutionStdin::Null => {}
    }

    stdin.flush().await.map_err(Error::tool_io)?;
    drop(stdin);
    Ok(())
}

pub(super) async fn wait_for_child(
    child: &mut Child,
    timeout_ms: Option<u64>,
) -> Result<WaitOutcome> {
    match timeout_ms {
        Some(timeout_ms) => wait_with_timeout(child, timeout_ms).await,
        None => child
            .wait()
            .await
            .map(WaitOutcome::Exited)
            .map_err(Error::tool_io),
    }
}

pub(super) async fn collect_output(
    handle: Option<JoinHandle<io::Result<CapturedBytes>>>,
) -> Result<CapturedOutput> {
    let Some(handle) = handle else {
        return Ok(CapturedOutput::empty());
    };

    let captured = handle
        .await
        .map_err(|_| Error::tool_io(io::Error::other("output reader thread panicked")))?
        .map_err(Error::tool_io)?;

    Ok(CapturedOutput {
        text: String::from_utf8_lossy(&captured.bytes).into_owned(),
        truncated: captured.truncated,
    })
}

pub(super) async fn build_output(
    outcome: WaitOutcome,
    stdout_handle: Option<JoinHandle<io::Result<CapturedBytes>>>,
    stderr_handle: Option<JoinHandle<io::Result<CapturedBytes>>>,
    fail_on_non_zero: bool,
    duration_ms: u128,
) -> Result<ExecutionOutput> {
    let stdout = collect_output(stdout_handle).await?;
    let stderr = collect_output(stderr_handle).await?;

    match outcome {
        WaitOutcome::TimedOut => Ok(ExecutionOutput::timed_out(
            stdout.text,
            stderr.text,
            duration_ms,
            stdout.truncated,
            stderr.truncated,
        )),
        WaitOutcome::Exited(status) => {
            let exit_code = status.code().unwrap_or(-1);
            let output = ExecutionOutput::foreground(
                stdout.text,
                stderr.text,
                exit_code,
                duration_ms,
                stdout.truncated,
                stderr.truncated,
            );

            if fail_on_non_zero && matches!(output.status, ExecutionStatus::Failed(_)) {
                return Err(Error::tool_cmd_failed(exit_code));
            }

            Ok(output)
        }
    }
}

fn configure_stdin(cmd: &mut Command, stdin: Option<&ExecutionStdin>) -> Result<()> {
    match stdin {
        Some(ExecutionStdin::File(path)) => {
            let file = File::open(path).map_err(Error::tool_io)?;
            cmd.stdin(file);
        }
        Some(ExecutionStdin::Null) => {
            cmd.stdin(Stdio::null());
        }
        Some(_) => {
            cmd.stdin(Stdio::piped());
        }
        None => {}
    }

    Ok(())
}

fn configure_output(cmd: &mut Command) {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
}

async fn wait_with_timeout(child: &mut Child, timeout_ms: u64) -> Result<WaitOutcome> {
    let duration = Duration::from_millis(timeout_ms);
    match tokio::time::timeout(duration, child.wait()).await {
        Ok(status) => status.map(WaitOutcome::Exited).map_err(Error::tool_io),
        Err(_) => {
            kill_timed_out_child(child).await?;
            Ok(WaitOutcome::TimedOut)
        }
    }
}

async fn kill_timed_out_child(child: &mut Child) -> Result<()> {
    kill_child_process_group(child);
    child.kill().await.map_err(Error::tool_io)?;
    child.wait().await.map_err(Error::tool_io)?;
    Ok(())
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

fn spawn_reader<R>(mut reader: R, max_bytes: Option<usize>) -> JoinHandle<io::Result<CapturedBytes>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut chunk = [0; 8192];

        loop {
            let count = reader.read(&mut chunk).await?;
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

#[cfg(unix)]
fn configure_process_group(cmd: &mut Command) {
    unsafe {
        cmd.as_std_mut().pre_exec(|| {
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
    let Some(pid) = child.id() else {
        return;
    };
    kill_process_group(pid);
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    let pgid = -(pid as i32);
    unsafe {
        libc::kill(pgid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_child_process_group(_child: &Child) {}
