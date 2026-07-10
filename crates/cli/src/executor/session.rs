use std::{collections::HashMap, io, process::ExitStatus, time::Duration};

use agent_executor_core::{Error, Result};
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
pub struct ProcessRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

impl ProcessRequest {
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
pub enum ProcessStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    Output {
        stream: ProcessStream,
        bytes: Vec<u8>,
    },
    Exited(ProcessExit),
    IoError {
        stream: Option<ProcessStream>,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessExit {
    pub exit_code: Option<i32>,
}

impl From<ExitStatus> for ProcessExit {
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
pub struct ProcessBackend;

impl ProcessBackend {
    pub fn start(&self, request: ProcessRequest) -> Result<StartedProcess> {
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

        Ok(StartedProcess {
            pid,
            control: ProcessControl { command_tx },
            event_rx,
        })
    }
}

pub struct StartedProcess {
    pid: u32,
    control: ProcessControl,
    event_rx: mpsc::UnboundedReceiver<ProcessEvent>,
}

impl StartedProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn control(&self) -> ProcessControl {
        self.control.clone()
    }

    pub async fn recv(&mut self) -> Option<ProcessEvent> {
        self.event_rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<ProcessEvent> {
        self.event_rx.try_recv().ok()
    }
}

#[derive(Clone)]
pub struct ProcessControl {
    command_tx: mpsc::Sender<ProcessCommand>,
}

impl std::fmt::Debug for ProcessControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessControl").finish_non_exhaustive()
    }
}

impl ProcessControl {
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

    pub async fn stop(&self) -> Result<ProcessExit> {
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
        reply_tx: oneshot::Sender<Result<ProcessExit>>,
    },
}

async fn run_process(
    mut child: Child,
    mut command_rx: mpsc::Receiver<ProcessCommand>,
    event_tx: mpsc::UnboundedSender<ProcessEvent>,
) {
    let mut stdin = child.stdin.take();
    let stdout_task = child.stdout.take().map(|stdout| {
        tokio::spawn(forward_output(
            stdout,
            ProcessStream::Stdout,
            event_tx.clone(),
        ))
    });
    let stderr_task = child.stderr.take().map(|stderr| {
        tokio::spawn(forward_output(
            stderr,
            ProcessStream::Stderr,
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
                                let exit = ProcessExit::from(status);
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
                    Ok(Some(status)) => break ProcessExit::from(status),
                    Ok(None) => {}
                    Err(error) => {
                        let _ = event_tx.send(ProcessEvent::IoError {
                            stream: None,
                            message: error.to_string(),
                        });
                        break ProcessExit { exit_code: None };
                    }
                }
            }
        }
    };

    drop(stdin);
    join_output_task(stdout_task, &event_tx).await;
    join_output_task(stderr_task, &event_tx).await;
    let _ = event_tx.send(ProcessEvent::Exited(exit));
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
    stream: ProcessStream,
    event_tx: mpsc::UnboundedSender<ProcessEvent>,
) where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => return,
            Ok(count) => {
                if event_tx
                    .send(ProcessEvent::Output {
                        stream,
                        bytes: buffer[.. count].to_vec(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = event_tx.send(ProcessEvent::IoError {
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
    event_tx: &mpsc::UnboundedSender<ProcessEvent>,
) {
    if let Some(task) = task
        && task.await.is_err()
    {
        let _ = event_tx.send(ProcessEvent::IoError {
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

    use tokio::time::timeout;

    use super::{ProcessBackend, ProcessEvent, ProcessRequest, ProcessStream};

    #[test]
    fn session_start_requires_a_tokio_runtime() {
        let error = match ProcessBackend.start(ProcessRequest::new("echo")) {
            Ok(_) => panic!("managed process should require a Tokio runtime"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("Tokio runtime"));
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn session_forwards_stdout_stderr_and_exit() {
        let mut process = ProcessBackend
            .start(ProcessRequest {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "printf out; printf err >&2".to_string()],
                cwd: None,
                env: None,
            })
            .unwrap();

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit = None;
        while let Some(event) = timeout(Duration::from_secs(1), process.recv())
            .await
            .unwrap()
        {
            match event {
                ProcessEvent::Output {
                    stream: ProcessStream::Stdout,
                    bytes,
                } => stdout.extend(bytes),
                ProcessEvent::Output {
                    stream: ProcessStream::Stderr,
                    bytes,
                } => stderr.extend(bytes),
                ProcessEvent::Exited(status) => {
                    exit = Some(status);
                    break;
                }
                ProcessEvent::IoError { message, .. } => {
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
        let mut process = ProcessBackend
            .start(ProcessRequest {
                program: "sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "read line; printf 'reply:%s' \"$line\"".to_string(),
                ],
                cwd: None,
                env: None,
            })
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
                ProcessEvent::Output {
                    stream: ProcessStream::Stdout,
                    bytes,
                } => stdout.extend(bytes),
                ProcessEvent::Exited(_) => break,
                ProcessEvent::Output { .. } => {}
                ProcessEvent::IoError { message, .. } => {
                    panic!("unexpected process I/O error: {message}")
                }
            }
        }

        assert_eq!(stdout, b"reply:hello");
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn session_stop_terminates_the_process() {
        let mut process = ProcessBackend
            .start(ProcessRequest {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "sleep 10".to_string()],
                cwd: None,
                env: None,
            })
            .unwrap();

        let exit = process.control().stop().await.unwrap();
        assert_ne!(exit.exit_code, Some(0));

        loop {
            let event = timeout(Duration::from_secs(1), process.recv())
                .await
                .unwrap();
            match event {
                Some(ProcessEvent::Exited(observed)) => {
                    assert_eq!(observed, exit);
                    break;
                }
                Some(_) => {}
                None => panic!("process event stream closed before exit"),
            }
        }
    }
}
