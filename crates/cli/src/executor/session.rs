use std::{collections::HashMap, io, process::ExitStatus, time::Duration};

use agent_executor_core::{Error, Result, SessionExecutor};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot},
    time::{self, MissedTickBehavior},
};

use super::process;

const CONTROL_CHANNEL_CAPACITY: usize = 16;
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A process request that keeps stdin and both output streams attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliProcessRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

impl CliProcessRequest {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliProcessStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliProcessEvent {
    Output {
        stream: CliProcessStream,
        bytes: Vec<u8>,
    },
    Exited(CliProcessExit),
    IoError {
        stream: Option<CliProcessStream>,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliProcessExit {
    pub exit_code: Option<i32>,
}

impl From<ExitStatus> for CliProcessExit {
    fn from(status: ExitStatus) -> Self {
        Self {
            exit_code: status.code(),
        }
    }
}

/// The OS-facing backend for a long-running process.
///
/// It intentionally has no task, agent, capsule, or session-registry knowledge.
#[derive(Debug, Clone, Default)]
pub struct CliProcessExecutor;

impl CliProcessExecutor {
    fn start_inner(&self, request: CliProcessRequest) -> Result<CliProcessSession> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            Error::tool_io(io::Error::other(format!(
                "managed process requires a Tokio runtime: {error}"
            )))
        })?;
        let mut command = Command::new(&request.program);
        command.args(&request.args);
        process::configure_session_command(&mut command, request.cwd, request.env);

        let child = process::spawn_child(&mut command)?;
        let pid = child.id().ok_or_else(|| {
            Error::tool_io(io::Error::other(
                "started process did not expose a process id",
            ))
        })?;
        let (command_tx, command_rx) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        runtime.spawn(run_process(child, command_rx, event_tx));

        Ok(CliProcessSession {
            pid,
            control: CliProcessControl { command_tx },
            event_rx,
        })
    }
}

impl SessionExecutor for CliProcessExecutor {
    type Request = CliProcessRequest;
    type Session = CliProcessSession;

    async fn start(&self, request: Self::Request) -> Result<Self::Session> {
        self.start_inner(request)
    }
}

pub struct CliProcessSession {
    pid: u32,
    control: CliProcessControl,
    event_rx: mpsc::UnboundedReceiver<CliProcessEvent>,
}

impl CliProcessSession {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn control(&self) -> CliProcessControl {
        self.control.clone()
    }

    pub async fn recv(&mut self) -> Option<CliProcessEvent> {
        self.event_rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<CliProcessEvent> {
        self.event_rx.try_recv().ok()
    }
}

#[derive(Clone)]
pub struct CliProcessControl {
    command_tx: mpsc::Sender<ProcessCommand>,
}

impl std::fmt::Debug for CliProcessControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliProcessControl").finish_non_exhaustive()
    }
}

impl CliProcessControl {
    pub async fn write_stdin(&self, bytes: impl Into<Vec<u8>>) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ProcessCommand::WriteStdin {
                bytes: bytes.into(),
                reply_tx,
            })
            .await
            .map_err(closed_process_error)?;
        reply_rx.await.map_err(closed_process_error)?
    }

    pub async fn stop(&self) -> Result<CliProcessExit> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ProcessCommand::Stop { reply_tx })
            .await
            .map_err(closed_process_error)?;
        reply_rx.await.map_err(closed_process_error)?
    }
}

enum ProcessCommand {
    WriteStdin {
        bytes: Vec<u8>,
        reply_tx: oneshot::Sender<Result<()>>,
    },
    Stop {
        reply_tx: oneshot::Sender<Result<CliProcessExit>>,
    },
}

async fn run_process(
    mut child: Child,
    mut command_rx: mpsc::Receiver<ProcessCommand>,
    event_tx: mpsc::UnboundedSender<CliProcessEvent>,
) {
    let mut stdin = child.stdin.take();
    let stdout_task = child.stdout.take().map(|stdout| {
        tokio::spawn(forward_output(
            stdout,
            CliProcessStream::Stdout,
            event_tx.clone(),
        ))
    });
    let stderr_task = child.stderr.take().map(|stderr| {
        tokio::spawn(forward_output(
            stderr,
            CliProcessStream::Stderr,
            event_tx.clone(),
        ))
    });
    let mut poll = time::interval(EXIT_POLL_INTERVAL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut commands_open = true;

    let exit = loop {
        tokio::select! {
            command = command_rx.recv(), if commands_open => {
                match command {
                    Some(ProcessCommand::WriteStdin { bytes, reply_tx }) => {
                        let _ = reply_tx.send(write_stdin(&mut stdin, &bytes).await);
                    }
                    Some(ProcessCommand::Stop { reply_tx }) => {
                        match process::stop_child(&mut child).await {
                            Ok(status) => {
                                let exit = CliProcessExit::from(status);
                                let _ = reply_tx.send(Ok(exit));
                                break exit;
                            }
                            Err(error) => {
                                let _ = reply_tx.send(Err(error));
                            }
                        }
                    }
                    None => commands_open = false,
                }
            }
            _ = poll.tick() => {
                match child.try_wait() {
                    Ok(Some(status)) => break CliProcessExit::from(status),
                    Ok(None) => {}
                    Err(error) => {
                        let _ = event_tx.send(CliProcessEvent::IoError {
                            stream: None,
                            message: error.to_string(),
                        });
                        break CliProcessExit { exit_code: None };
                    }
                }
            }
        }
    };

    drop(stdin);
    join_output_task(stdout_task, &event_tx).await;
    join_output_task(stderr_task, &event_tx).await;
    let _ = event_tx.send(CliProcessEvent::Exited(exit));
}

async fn write_stdin(stdin: &mut Option<ChildStdin>, bytes: &[u8]) -> Result<()> {
    let stdin = stdin.as_mut().ok_or_else(|| {
        Error::tool_io(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "process stdin is not available",
        ))
    })?;
    stdin.write_all(bytes).await.map_err(Error::tool_io)?;
    stdin.flush().await.map_err(Error::tool_io)
}

async fn forward_output<R>(
    mut reader: R,
    stream: CliProcessStream,
    event_tx: mpsc::UnboundedSender<CliProcessEvent>,
) where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => return,
            Ok(count) => {
                if event_tx
                    .send(CliProcessEvent::Output {
                        stream,
                        bytes: buffer[.. count].to_vec(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = event_tx.send(CliProcessEvent::IoError {
                    stream: Some(stream),
                    message: error.to_string(),
                });
                return;
            }
        }
    }
}

async fn join_output_task(
    task: Option<tokio::task::JoinHandle<()>>,
    event_tx: &mpsc::UnboundedSender<CliProcessEvent>,
) {
    if let Some(task) = task
        && task.await.is_err()
    {
        let _ = event_tx.send(CliProcessEvent::IoError {
            stream: None,
            message: "process output reader panicked".to_string(),
        });
    }
}

fn closed_process_error<T>(_error: T) -> Error {
    Error::tool_io(io::Error::new(
        io::ErrorKind::BrokenPipe,
        "process control channel is closed",
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_executor_core::SessionExecutor;
    use tokio::time::timeout;

    use super::{CliProcessEvent, CliProcessExecutor, CliProcessRequest, CliProcessStream};

    #[test]
    fn session_start_requires_a_tokio_runtime() {
        let error = match CliProcessExecutor.start_inner(CliProcessRequest::new("echo")) {
            Ok(_) => panic!("managed process should require a Tokio runtime"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("Tokio runtime"));
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn session_forwards_stdout_stderr_and_exit() {
        let mut process = CliProcessExecutor
            .start(CliProcessRequest {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "printf out; printf err >&2".to_string()],
                cwd: None,
                env: None,
            })
            .await
            .unwrap();

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit = None;
        while let Some(event) = timeout(Duration::from_secs(1), process.recv())
            .await
            .unwrap()
        {
            match event {
                CliProcessEvent::Output {
                    stream: CliProcessStream::Stdout,
                    bytes,
                } => stdout.extend(bytes),
                CliProcessEvent::Output {
                    stream: CliProcessStream::Stderr,
                    bytes,
                } => stderr.extend(bytes),
                CliProcessEvent::Exited(status) => {
                    exit = Some(status);
                    break;
                }
                CliProcessEvent::IoError { message, .. } => {
                    panic!("unexpected process I/O error: {message}")
                }
            }
        }

        assert_eq!(stdout, b"out");
        assert_eq!(stderr, b"err");
        assert_eq!(exit.expect("process should exit").exit_code, Some(0));
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn session_accepts_stdin_and_returns_response() {
        let mut process = CliProcessExecutor
            .start(CliProcessRequest {
                program: "sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "read line; printf 'reply:%s' \"$line\"".to_string(),
                ],
                cwd: None,
                env: None,
            })
            .await
            .unwrap();

        process
            .control()
            .write_stdin(b"hello\n".to_vec())
            .await
            .unwrap();

        let mut stdout = Vec::new();
        while let Some(event) = timeout(Duration::from_secs(1), process.recv())
            .await
            .unwrap()
        {
            match event {
                CliProcessEvent::Output {
                    stream: CliProcessStream::Stdout,
                    bytes,
                } => stdout.extend(bytes),
                CliProcessEvent::Exited(_) => break,
                CliProcessEvent::Output { .. } => {}
                CliProcessEvent::IoError { message, .. } => {
                    panic!("unexpected process I/O error: {message}")
                }
            }
        }

        assert_eq!(stdout, b"reply:hello");
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn session_stop_terminates_the_process() {
        let mut process = CliProcessExecutor
            .start(CliProcessRequest {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "sleep 10".to_string()],
                cwd: None,
                env: None,
            })
            .await
            .unwrap();

        let exit = process.control().stop().await.unwrap();
        assert_ne!(exit.exit_code, Some(0));

        loop {
            let event = timeout(Duration::from_secs(1), process.recv())
                .await
                .unwrap();
            match event {
                Some(CliProcessEvent::Exited(observed)) => {
                    assert_eq!(observed, exit);
                    break;
                }
                Some(_) => {}
                None => panic!("process event stream closed before exit"),
            }
        }
    }
}
