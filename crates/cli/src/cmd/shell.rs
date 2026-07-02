use std::process::Command;

pub(crate) fn build_shell_command(command: &str) -> Command {
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
