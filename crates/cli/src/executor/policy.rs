use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use agent_executor_core::{Error, Result};

use super::types::{CommandRequest, ShellRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPolicy {
    pub allow_shell: bool,
    pub allow_background: bool,
    pub max_timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub allowed_programs: Option<HashSet<String>>,
    pub allowed_cwd_roots: Vec<PathBuf>,
    pub allowed_env_vars: Option<HashSet<String>>,
}

impl Default for CommandPolicy {
    fn default() -> Self {
        Self {
            allow_shell: true,
            allow_background: true,
            max_timeout_ms: None,
            max_output_bytes: None,
            allowed_programs: None,
            allowed_cwd_roots: Vec::new(),
            allowed_env_vars: None,
        }
    }
}

impl CommandPolicy {
    // Internal validation API
    pub(super) fn validate_command(&self, req: &CommandRequest) -> Result<()> {
        self.validate_program(&req.program)?;
        self.validate_cwd(req.cwd.as_deref())?;
        self.validate_env(req.env.as_ref())?;
        self.validate_background(req.background)?;
        self.validate_timeout(req.timeout_ms)
    }

    pub(super) fn validate_shell(&self, req: &ShellRequest) -> Result<()> {
        if !self.allow_shell {
            return Err(policy_error("shell commands are not allowed by policy"));
        }

        self.validate_cwd(req.cwd.as_deref())?;
        self.validate_env(req.env.as_ref())?;
        self.validate_background(req.background)?;
        self.validate_timeout(req.timeout_ms)
    }
}

impl CommandPolicy {
    // Private validation helpers
    fn validate_program(&self, program: &str) -> Result<()> {
        let Some(allowed_programs) = &self.allowed_programs else {
            return Ok(());
        };

        if allowed_programs.contains(program) {
            return Ok(());
        }

        let file_name = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(program);

        if allowed_programs.contains(file_name) {
            return Ok(());
        }

        Err(policy_error(format!(
            "program `{program}` is not allowed by policy"
        )))
    }

    fn validate_cwd(&self, cwd: Option<&str>) -> Result<()> {
        if self.allowed_cwd_roots.is_empty() {
            return Ok(());
        };

        let Some(cwd) = cwd else {
            return Err(policy_error("cwd is required by policy"));
        };

        let cwd = canonicalize_policy_path(cwd)?;
        for root in &self.allowed_cwd_roots {
            let root = canonicalize_policy_path(root)?;
            if cwd.starts_with(root) {
                return Ok(());
            }
        }

        Err(policy_error(format!(
            "cwd `{}` is not allowed by policy",
            cwd.display()
        )))
    }

    fn validate_env(&self, env: Option<&HashMap<String, String>>) -> Result<()> {
        let (Some(allowed_env_vars), Some(env)) = (&self.allowed_env_vars, env) else {
            return Ok(());
        };

        for key in env.keys() {
            if !allowed_env_vars.contains(key) {
                return Err(policy_error(format!(
                    "env var `{key}` is not allowed by policy"
                )));
            }
        }

        Ok(())
    }

    fn validate_background(&self, background: bool) -> Result<()> {
        if background && !self.allow_background {
            return Err(policy_error(
                "background commands are not allowed by policy",
            ));
        }

        Ok(())
    }

    fn validate_timeout(&self, timeout_ms: Option<u64>) -> Result<()> {
        let (Some(max), Some(timeout_ms)) = (self.max_timeout_ms, timeout_ms) else {
            return Ok(());
        };

        if timeout_ms > max {
            return Err(policy_error(format!(
                "timeout {timeout_ms}ms exceeds policy maximum {max}ms"
            )));
        }

        Ok(())
    }
}

fn canonicalize_policy_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    path.as_ref().canonicalize().map_err(Error::tool_io)
}

fn policy_error(message: impl Into<String>) -> Error {
    Error::tool_policy(message)
}
