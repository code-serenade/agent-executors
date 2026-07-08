use std::path::{Path, PathBuf};

use agent_executor_core::{Error, Result};

use crate::parser::{PatchAction, PatchOperation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPolicy {
    pub allowed_cwd_roots: Vec<PathBuf>,
    pub allow_add: bool,
    pub allow_update: bool,
    pub allow_delete: bool,
    pub max_patch_bytes: Option<usize>,
}

impl Default for PatchPolicy {
    fn default() -> Self {
        Self {
            allowed_cwd_roots: Vec::new(),
            allow_add: true,
            allow_update: true,
            allow_delete: true,
            max_patch_bytes: None,
        }
    }
}

impl PatchPolicy {
    pub(crate) fn validate_request(&self, patch: &str, cwd: &Path) -> Result<()> {
        if let Some(max_patch_bytes) = self.max_patch_bytes
            && patch.len() > max_patch_bytes
        {
            return Err(policy_error(format!(
                "patch size {} bytes exceeds policy maximum {max_patch_bytes} bytes",
                patch.len()
            )));
        }

        self.validate_cwd(cwd)
    }

    pub(crate) fn validate_action(&self, action: &PatchAction) -> Result<()> {
        for op in &action.operations {
            match op {
                PatchOperation::Add { .. } if !self.allow_add => {
                    return Err(policy_error("adding files is not allowed by policy"));
                }
                PatchOperation::Update { .. } if !self.allow_update => {
                    return Err(policy_error("updating files is not allowed by policy"));
                }
                PatchOperation::Delete { .. } if !self.allow_delete => {
                    return Err(policy_error("deleting files is not allowed by policy"));
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn validate_cwd(&self, cwd: &Path) -> Result<()> {
        if self.allowed_cwd_roots.is_empty() {
            return Ok(());
        }

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
}

fn canonicalize_policy_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    path.as_ref().canonicalize().map_err(Error::tool_io)
}

fn policy_error(message: impl Into<String>) -> Error {
    Error::tool_policy(message)
}
