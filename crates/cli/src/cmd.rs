mod policy;
mod process;
mod runner;
mod session;
mod shell;
#[cfg(test)]
mod tests;
mod types;

pub use policy::CommandPolicy;
pub use runner::CmdRunner;
pub use session::{CmdSession, CmdSessionManager, CmdSessionStatus};
pub use types::{
    CliExecutionRequest, CliExecutionResult, CmdOutput, CmdRequest, CmdSessionOutput, CmdStatus,
    CmdStdin, ShellCmdRequest,
};

pub struct CmdTool;

impl CmdTool {
    pub fn run(req: CmdRequest) -> agent_executor_core::Result<CmdOutput> {
        CmdRunner::default().run(req)
    }

    pub fn run_shell(req: ShellCmdRequest) -> agent_executor_core::Result<CmdOutput> {
        CmdRunner::default().run_shell(req)
    }

    pub fn execute(req: CliExecutionRequest) -> agent_executor_core::Result<CliExecutionResult> {
        CmdRunner::default().execute(req)
    }
}
