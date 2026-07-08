use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use agent_executor_core::{Error, Executor, Result};

use crate::{
    parser::{PatchAction, PatchOperation, UpdateHunk, parse_patch},
    policy::PatchPolicy,
    types::{PatchExecutionRequest, PatchExecutionResult, PatchFileChange, PatchStatus},
};

#[derive(Debug, Clone, Default)]
pub struct PatchExecutor {
    pub(super) policy: PatchPolicy,
}

impl PatchExecutor {
    pub fn new(policy: PatchPolicy) -> Self {
        Self { policy }
    }
}

impl Executor for PatchExecutor {
    type Request = PatchExecutionRequest;
    type Output = PatchExecutionResult;

    async fn execute(&self, request: Self::Request) -> Result<Self::Output> {
        let started_at = Instant::now();
        self.policy
            .validate_request(&request.patch, request.cwd.as_path())?;

        let action = parse_patch(&request.patch)?;
        self.policy.validate_action(&action)?;
        validate_paths(&request.cwd, &action)?;

        let changed_files = changed_files_for_action(&action);
        if request.dry_run {
            validate_apply_action(&request.cwd, &action)?;
            return Ok(PatchExecutionResult {
                status: PatchStatus::DryRun,
                changed_files,
                diagnostics: Vec::new(),
                duration_ms: started_at.elapsed().as_millis(),
            });
        }

        apply_action(&request.cwd, &action)?;
        Ok(PatchExecutionResult {
            status: PatchStatus::Applied,
            changed_files,
            diagnostics: Vec::new(),
            duration_ms: started_at.elapsed().as_millis(),
        })
    }
}

fn validate_paths(cwd: &Path, action: &PatchAction) -> Result<()> {
    for op in &action.operations {
        let path = match op {
            PatchOperation::Add { path, .. }
            | PatchOperation::Update { path, .. }
            | PatchOperation::Delete { path } => path,
        };
        resolve_patch_path(cwd, path)?;
    }
    Ok(())
}

fn changed_files_for_action(action: &PatchAction) -> Vec<PatchFileChange> {
    action
        .operations
        .iter()
        .map(|op| match op {
            PatchOperation::Add { path, .. } => PatchFileChange::Add { path: path.clone() },
            PatchOperation::Update { path, .. } => PatchFileChange::Update { path: path.clone() },
            PatchOperation::Delete { path } => PatchFileChange::Delete { path: path.clone() },
        })
        .collect()
}

fn apply_action(cwd: &Path, action: &PatchAction) -> Result<()> {
    for op in &action.operations {
        match op {
            PatchOperation::Add { path, lines } => {
                let path = resolve_patch_path(cwd, path)?;
                validate_add_target(&path)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(Error::tool_io)?;
                }
                fs::write(path, lines_to_text(lines)).map_err(Error::tool_io)?;
            }
            PatchOperation::Update { path, hunks } => {
                let path = resolve_patch_path(cwd, path)?;
                let original = fs::read_to_string(&path).map_err(Error::tool_io)?;
                let updated = apply_hunks(&original, hunks)?;
                fs::write(path, updated).map_err(Error::tool_io)?;
            }
            PatchOperation::Delete { path } => {
                let path = resolve_patch_path(cwd, path)?;
                fs::remove_file(path).map_err(Error::tool_io)?;
            }
        }
    }
    Ok(())
}

fn validate_apply_action(cwd: &Path, action: &PatchAction) -> Result<()> {
    for op in &action.operations {
        match op {
            PatchOperation::Add { path, .. } => {
                let path = resolve_patch_path(cwd, path)?;
                validate_add_target(&path)?;
            }
            PatchOperation::Update { path, hunks } => {
                let path = resolve_patch_path(cwd, path)?;
                let original = fs::read_to_string(path).map_err(Error::tool_io)?;
                apply_hunks(&original, hunks)?;
            }
            PatchOperation::Delete { path } => {
                let path = resolve_patch_path(cwd, path)?;
                if !path.exists() {
                    return Err(Error::tool_policy(format!(
                        "cannot delete `{}` because it does not exist",
                        path.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_add_target(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(Error::tool_policy(format!(
            "cannot add `{}` because it already exists",
            path.display()
        )));
    }
    Ok(())
}

fn apply_hunks(original: &str, hunks: &[UpdateHunk]) -> Result<String> {
    let mut lines = split_text_lines(original);
    let mut search_start = 0;

    for hunk in hunks {
        let Some(index) = find_subsequence(&lines, &hunk.old_lines, search_start) else {
            return Err(Error::tool_policy("update hunk did not match target file"));
        };

        lines.splice(
            index .. index + hunk.old_lines.len(),
            hunk.new_lines.clone(),
        );
        search_start = index + hunk.new_lines.len();
    }

    Ok(lines_to_text(&lines))
}

fn find_subsequence(haystack: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(haystack.len()));
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .skip(start)
        .find_map(|(index, window)| (window == needle).then_some(index))
}

fn split_text_lines(text: &str) -> Vec<String> {
    text.lines().map(ToString::to_string).collect()
}

fn lines_to_text(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn resolve_patch_path(cwd: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Err(Error::tool_policy(format!(
            "patch path `{}` must be relative",
            path.display()
        )));
    }

    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        )
    }) {
        return Err(Error::tool_policy(format!(
            "patch path `{}` must stay inside cwd",
            path.display()
        )));
    }

    Ok(cwd.join(path))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use agent_executor_core::{ErrorCategory, Executor};
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn applies_add_update_and_delete_operations() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("old.txt"), "old\n").unwrap();
        fs::write(dir.path().join("gone.txt"), "gone\n").unwrap();

        let output = PatchExecutor::default()
            .execute(PatchExecutionRequest {
                cwd: dir.path().to_path_buf(),
                dry_run: false,
                patch: r#"*** Begin Patch
*** Add File: new.txt
+new
*** Update File: old.txt
@@
-old
+updated
*** Delete File: gone.txt
*** End Patch"#
                    .to_string(),
            })
            .await
            .unwrap();

        assert_eq!(output.status, PatchStatus::Applied);
        assert_eq!(
            fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "new\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("old.txt")).unwrap(),
            "updated\n"
        );
        assert!(!dir.path().join("gone.txt").exists());
    }

    #[tokio::test]
    async fn dry_run_reports_changes_without_writing() {
        let dir = tempdir().unwrap();
        let output = PatchExecutor::default()
            .execute(PatchExecutionRequest {
                cwd: dir.path().to_path_buf(),
                dry_run: true,
                patch: r#"*** Begin Patch
*** Add File: new.txt
+new
*** End Patch"#
                    .to_string(),
            })
            .await
            .unwrap();

        assert_eq!(output.status, PatchStatus::DryRun);
        assert_eq!(
            output.changed_files,
            vec![PatchFileChange::Add {
                path: PathBuf::from("new.txt")
            }]
        );
        assert!(!dir.path().join("new.txt").exists());
    }

    #[tokio::test]
    async fn policy_can_reject_delete_and_large_patch() {
        let dir = tempdir().unwrap();
        let delete_blocked = PatchExecutor::new(PatchPolicy {
            allow_delete: false,
            ..PatchPolicy::default()
        })
        .execute(PatchExecutionRequest {
            cwd: dir.path().to_path_buf(),
            dry_run: true,
            patch: r#"*** Begin Patch
*** Delete File: old.txt
*** End Patch"#
                .to_string(),
        })
        .await
        .unwrap_err();
        assert_eq!(delete_blocked.category(), ErrorCategory::Policy);

        let size_blocked = PatchExecutor::new(PatchPolicy {
            max_patch_bytes: Some(8),
            ..PatchPolicy::default()
        })
        .execute(PatchExecutionRequest {
            cwd: dir.path().to_path_buf(),
            dry_run: true,
            patch: "*** Begin Patch\n*** End Patch".to_string(),
        })
        .await
        .unwrap_err();
        assert_eq!(size_blocked.category(), ErrorCategory::Policy);
    }

    #[tokio::test]
    async fn policy_can_restrict_cwd_roots() {
        let allowed = tempdir().unwrap();
        let blocked = tempdir().unwrap();
        let runner = PatchExecutor::new(PatchPolicy {
            allowed_cwd_roots: vec![allowed.path().to_path_buf()],
            ..PatchPolicy::default()
        });

        let result = runner
            .execute(PatchExecutionRequest {
                cwd: blocked.path().to_path_buf(),
                dry_run: true,
                patch: r#"*** Begin Patch
*** Add File: new.txt
+new
*** End Patch"#
                    .to_string(),
            })
            .await;

        assert_eq!(result.unwrap_err().category(), ErrorCategory::Policy);
    }

    #[tokio::test]
    async fn rejects_paths_that_escape_cwd() {
        let dir = tempdir().unwrap();
        let result = PatchExecutor::default()
            .execute(PatchExecutionRequest {
                cwd: dir.path().to_path_buf(),
                dry_run: true,
                patch: r#"*** Begin Patch
*** Add File: ../escape.txt
+nope
*** End Patch"#
                    .to_string(),
            })
            .await;

        assert_eq!(result.unwrap_err().category(), ErrorCategory::Policy);
    }
}
