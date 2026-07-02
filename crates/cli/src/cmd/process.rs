use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::Duration,
};

use agent_executor_core::{Error, Result};
use wait_timeout::ChildExt;

use super::types::{CmdOutput, CmdStatus, CmdStdin};

pub(crate) enum WaitOutcome {
    Exited(ExitStatus),
    TimedOut,
}

pub(crate) fn configure_command(
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

pub(crate) fn spawn_child(cmd: &mut Command) -> Result<Child> {
    cmd.spawn().map_err(Error::tool_io)
}

pub(crate) fn take_output_reader<R>(
    pipe: &mut Option<R>,
) -> Option<thread::JoinHandle<io::Result<Vec<u8>>>>
where
    R: Read + Send + 'static,
{
    pipe.take().map(spawn_reader)
}

pub(crate) fn write_stdin(child: &mut Child, stdin_content: Option<&CmdStdin>) -> Result<()> {
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

pub(crate) fn wait_for_child(child: &mut Child, timeout_ms: Option<u64>) -> Result<WaitOutcome> {
    match timeout_ms {
        Some(timeout_ms) => wait_with_timeout(child, timeout_ms),
        None => child
            .wait()
            .map(WaitOutcome::Exited)
            .map_err(Error::tool_io),
    }
}

pub(crate) fn collect_output(
    handle: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
) -> Result<String> {
    let Some(handle) = handle else {
        return Ok(String::new());
    };

    let bytes = handle
        .join()
        .map_err(|_| Error::tool_io(io::Error::other("output reader thread panicked")))?
        .map_err(Error::tool_io)?;

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn build_output(
    outcome: WaitOutcome,
    stdout_handle: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
    stderr_handle: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
    fail_on_non_zero: bool,
) -> Result<CmdOutput> {
    let stdout = collect_output(stdout_handle)?;
    let stderr = collect_output(stderr_handle)?;

    match outcome {
        WaitOutcome::TimedOut => Ok(CmdOutput::timed_out(stdout, stderr)),
        WaitOutcome::Exited(status) => {
            let exit_code = status.code().unwrap_or(-1);
            let output = CmdOutput::foreground(stdout, stderr, exit_code);

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
    child.kill().map_err(Error::tool_io)?;
    child.wait().map_err(Error::tool_io)?;
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
