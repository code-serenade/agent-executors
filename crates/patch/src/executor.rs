use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use agent_executor_core::{Error, Executor, Result};

use crate::{
    parser::{PatchAction, PatchOperation, UpdateHunk, parse_patch},
    policy::PatchPolicy,
    types::{
        PatchExecutionRequest, PatchExecutionResult, PatchFileChange, PatchPreview, PatchStatus,
    },
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

        let action = match parse_patch(&request.patch) {
            Ok(action) => action,
            Err(err) => {
                return Ok(rejected_result(
                    started_at,
                    Vec::new(),
                    vec![err.to_string()],
                ));
            }
        };
        self.policy.validate_action(&action)?;
        validate_paths(&request.cwd, &action)?;

        let changed_files = changed_files_for_action(&action);
        let preview = match preview_action(&request.cwd, &action) {
            Ok(preview) => preview,
            Err(message) => {
                return Ok(rejected_result(started_at, changed_files, vec![message]));
            }
        };
        if request.dry_run {
            return Ok(PatchExecutionResult {
                status: PatchStatus::DryRun,
                changed_files,
                preview,
                diagnostics: Vec::new(),
                duration_ms: started_at.elapsed().as_millis(),
            });
        }

        apply_preview(&request.cwd, &preview)?;
        Ok(PatchExecutionResult {
            status: PatchStatus::Applied,
            changed_files,
            preview,
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
        if let PatchOperation::Update {
            move_path: Some(move_path),
            ..
        } = op
        {
            resolve_patch_path(cwd, move_path)?;
        }
    }
    Ok(())
}

fn changed_files_for_action(action: &PatchAction) -> Vec<PatchFileChange> {
    action
        .operations
        .iter()
        .map(|op| match op {
            PatchOperation::Add { path, .. } => PatchFileChange::Add { path: path.clone() },
            PatchOperation::Update {
                path, move_path, ..
            } => PatchFileChange::Update {
                path: path.clone(),
                move_path: move_path.clone(),
            },
            PatchOperation::Delete { path } => PatchFileChange::Delete { path: path.clone() },
        })
        .collect()
}

fn preview_action(
    cwd: &Path,
    action: &PatchAction,
) -> std::result::Result<Vec<PatchPreview>, String> {
    let mut preview = Vec::new();
    for op in &action.operations {
        match op {
            PatchOperation::Add { path, lines } => {
                let path = resolve_patch_path_for_preview(cwd, path)?;
                validate_add_target(&path).map_err(|err| err.to_string())?;
                preview.push(PatchPreview::Add {
                    path: path
                        .strip_prefix(cwd)
                        .unwrap_or(path.as_path())
                        .to_path_buf(),
                    content: lines_to_text(lines),
                });
            }
            PatchOperation::Update {
                path,
                move_path,
                hunks,
            } => {
                let path = resolve_patch_path_for_preview(cwd, path)?;
                let original = fs::read_to_string(&path)
                    .map_err(|err| format!("cannot update `{}`: {err}", path.display()))?;
                let updated = apply_hunks(&original, hunks)?;
                let move_path = match move_path {
                    Some(move_path) => {
                        let target = resolve_patch_path_for_preview(cwd, move_path)?;
                        if target != path && target.exists() {
                            return Err(format!(
                                "cannot move `{}` to `{}` because target already exists",
                                path.display(),
                                target.display()
                            ));
                        }
                        Some(
                            target
                                .strip_prefix(cwd)
                                .unwrap_or(target.as_path())
                                .to_path_buf(),
                        )
                    }
                    None => None,
                };
                preview.push(PatchPreview::Update {
                    path: path
                        .strip_prefix(cwd)
                        .unwrap_or(path.as_path())
                        .to_path_buf(),
                    move_path,
                    unified_diff: unified_diff_for_update(
                        path.strip_prefix(cwd).unwrap_or(path.as_path()),
                        &original,
                        &updated,
                    ),
                    before: original,
                    after: updated,
                });
            }
            PatchOperation::Delete { path } => {
                let path = resolve_patch_path_for_preview(cwd, path)?;
                let content = fs::read_to_string(&path)
                    .map_err(|err| format!("cannot delete `{}`: {err}", path.display()))?;
                preview.push(PatchPreview::Delete {
                    path: path
                        .strip_prefix(cwd)
                        .unwrap_or(path.as_path())
                        .to_path_buf(),
                    content,
                });
            }
        }
    }
    Ok(preview)
}

fn apply_preview(cwd: &Path, preview: &[PatchPreview]) -> Result<()> {
    for change in preview {
        match change {
            PatchPreview::Add { path, content } => {
                let path = resolve_patch_path(cwd, path)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(Error::tool_io)?;
                }
                fs::write(path, content).map_err(Error::tool_io)?;
            }
            PatchPreview::Update {
                path,
                move_path,
                after,
                ..
            } => {
                let path = resolve_patch_path(cwd, path)?;
                let target = match move_path {
                    Some(move_path) => resolve_patch_path(cwd, move_path)?,
                    None => path.clone(),
                };
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(Error::tool_io)?;
                }
                fs::write(&target, after).map_err(Error::tool_io)?;
                if target != path {
                    fs::remove_file(path).map_err(Error::tool_io)?;
                }
            }
            PatchPreview::Delete { path, .. } => {
                let path = resolve_patch_path(cwd, path)?;
                if !path.exists() {
                    return Err(Error::tool_io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "cannot delete `{}` because it does not exist",
                            path.display()
                        ),
                    )));
                }
                fs::remove_file(path).map_err(Error::tool_io)?;
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

fn apply_hunks(original: &str, hunks: &[UpdateHunk]) -> std::result::Result<String, String> {
    let mut lines = split_text_lines(original);
    let mut search_start = 0;

    for hunk in hunks {
        let Some(index) = find_subsequence(&lines, &hunk.old_lines, search_start) else {
            return Err("update hunk did not match target file".to_string());
        };

        lines.splice(
            index .. index + hunk.old_lines.len(),
            hunk.new_lines.clone(),
        );
        search_start = index + hunk.new_lines.len();
    }

    Ok(lines_to_text(&lines))
}

fn unified_diff_for_update(path: &Path, before: &str, after: &str) -> String {
    let mut diff = String::new();
    diff.push_str("--- ");
    diff.push_str(&path.display().to_string());
    diff.push('\n');
    diff.push_str("+++ ");
    diff.push_str(&path.display().to_string());
    diff.push('\n');
    diff.push_str("@@\n");
    for line in split_text_lines(before) {
        diff.push('-');
        diff.push_str(&line);
        diff.push('\n');
    }
    for line in split_text_lines(after) {
        diff.push('+');
        diff.push_str(&line);
        diff.push('\n');
    }
    diff
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

fn resolve_patch_path_for_preview(cwd: &Path, path: &Path) -> std::result::Result<PathBuf, String> {
    resolve_patch_path(cwd, path).map_err(|err| err.to_string())
}

fn rejected_result(
    started_at: Instant,
    changed_files: Vec<PatchFileChange>,
    diagnostics: Vec<String>,
) -> PatchExecutionResult {
    PatchExecutionResult {
        status: PatchStatus::Rejected,
        changed_files,
        preview: Vec::new(),
        diagnostics,
        duration_ms: started_at.elapsed().as_millis(),
    }
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
        assert_eq!(
            output.preview,
            vec![PatchPreview::Add {
                path: PathBuf::from("new.txt"),
                content: "new\n".to_string(),
            }]
        );
        assert!(!dir.path().join("new.txt").exists());
    }

    #[tokio::test]
    async fn rejected_hunk_is_structured_output_without_writing() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("old.txt"), "actual\n").unwrap();

        let output = PatchExecutor::default()
            .execute(PatchExecutionRequest {
                cwd: dir.path().to_path_buf(),
                dry_run: false,
                patch: r#"*** Begin Patch
*** Update File: old.txt
@@
-expected
+updated
*** End Patch"#
                    .to_string(),
            })
            .await
            .unwrap();

        assert_eq!(output.status, PatchStatus::Rejected);
        assert_eq!(
            output.changed_files,
            vec![PatchFileChange::Update {
                path: PathBuf::from("old.txt"),
                move_path: None,
            }]
        );
        assert!(output.diagnostics[0].contains("hunk"));
        assert_eq!(
            fs::read_to_string(dir.path().join("old.txt")).unwrap(),
            "actual\n"
        );
    }

    #[tokio::test]
    async fn update_can_move_file() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("old.txt"), "old\n").unwrap();

        let output = PatchExecutor::default()
            .execute(PatchExecutionRequest {
                cwd: dir.path().to_path_buf(),
                dry_run: false,
                patch: r#"*** Begin Patch
*** Update File: old.txt
*** Move to: renamed.txt
@@
-old
+updated
*** End Patch"#
                    .to_string(),
            })
            .await
            .unwrap();

        assert_eq!(output.status, PatchStatus::Applied);
        assert_eq!(
            output.changed_files,
            vec![PatchFileChange::Update {
                path: PathBuf::from("old.txt"),
                move_path: Some(PathBuf::from("renamed.txt")),
            }]
        );
        assert!(!dir.path().join("old.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("renamed.txt")).unwrap(),
            "updated\n"
        );
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
