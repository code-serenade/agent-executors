use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchExecutionRequest {
    pub patch: String,
    pub cwd: PathBuf,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchExecutionResult {
    pub status: PatchStatus,
    pub changed_files: Vec<PatchFileChange>,
    pub preview: Vec<PatchPreview>,
    pub diagnostics: Vec<String>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchStatus {
    Applied,
    DryRun,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchFileChange {
    Add {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        move_path: Option<PathBuf>,
    },
    Delete {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchPreview {
    Add {
        path: PathBuf,
        content: String,
    },
    Update {
        path: PathBuf,
        move_path: Option<PathBuf>,
        before: String,
        after: String,
        unified_diff: String,
    },
    Delete {
        path: PathBuf,
        content: String,
    },
}
