use std::process::Command;

use super::types::ShellKind;

pub(super) fn build_shell_command(shell: &ShellKind, command: &str) -> Command {
    match shell {
        ShellKind::Sh => shell_c("sh", command),
        ShellKind::Zsh => shell_c("zsh", command),
        ShellKind::Bash => shell_c("bash", command),
        ShellKind::Cmd => {
            let mut cmd = Command::new("cmd.exe");
            cmd.arg("/c").arg(command);
            cmd
        }
        ShellKind::Custom(path) => shell_c(path, command),
    }
}

fn shell_c(program: impl AsRef<std::ffi::OsStr>, command: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.arg("-c").arg(command);
    cmd
}
