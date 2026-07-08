use std::path::PathBuf;

use agent_executor_core::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchAction {
    pub(crate) operations: Vec<PatchOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PatchOperation {
    Add {
        path: PathBuf,
        lines: Vec<String>,
    },
    Update {
        path: PathBuf,
        move_path: Option<PathBuf>,
        hunks: Vec<UpdateHunk>,
    },
    Delete {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateHunk {
    pub(crate) old_lines: Vec<String>,
    pub(crate) new_lines: Vec<String>,
}

pub(crate) fn parse_patch(input: &str) -> Result<PatchAction> {
    let lines = input.lines().collect::<Vec<_>>();
    let mut parser = Parser { lines, index: 0 };
    parser.parse()
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    index: usize,
}

impl Parser<'_> {
    fn parse(&mut self) -> Result<PatchAction> {
        self.expect_exact("*** Begin Patch")?;
        let mut operations = Vec::new();

        while !self.peek_is("*** End Patch") {
            if self.is_done() {
                return Err(parse_error("missing `*** End Patch`"));
            }

            if let Some(path) = self.take_prefixed("*** Add File: ") {
                operations.push(self.parse_add(path)?);
            } else if let Some(path) = self.take_prefixed("*** Update File: ") {
                operations.push(self.parse_update(path)?);
            } else if let Some(path) = self.take_prefixed("*** Delete File: ") {
                operations.push(PatchOperation::Delete {
                    path: PathBuf::from(path),
                });
            } else {
                return Err(parse_error(format!(
                    "unsupported patch line `{}`",
                    self.current().unwrap_or_default()
                )));
            }
        }

        self.expect_exact("*** End Patch")?;
        if operations.is_empty() {
            return Err(parse_error("patch contains no file operations"));
        }

        Ok(PatchAction { operations })
    }

    fn parse_add(&mut self, path: String) -> Result<PatchOperation> {
        let mut lines = Vec::new();
        while !self.peek_starts_operation() && !self.peek_is("*** End Patch") {
            let Some(line) = self.next() else {
                return Err(parse_error("unterminated add file operation"));
            };
            let Some(content) = line.strip_prefix('+') else {
                return Err(parse_error("add file lines must start with `+`"));
            };
            lines.push(content.to_string());
        }

        Ok(PatchOperation::Add {
            path: PathBuf::from(path),
            lines,
        })
    }

    fn parse_update(&mut self, path: String) -> Result<PatchOperation> {
        let move_path = self.take_prefixed("*** Move to: ").map(PathBuf::from);
        let mut hunks = Vec::new();
        while !self.peek_starts_operation() && !self.peek_is("*** End Patch") {
            self.expect_hunk_header()?;
            let mut old_lines = Vec::new();
            let mut new_lines = Vec::new();

            while !self.peek_is_hunk_header()
                && !self.peek_starts_operation()
                && !self.peek_is("*** End Patch")
            {
                let Some(line) = self.next() else {
                    return Err(parse_error("unterminated update hunk"));
                };

                if line == "*** End of File" {
                    continue;
                } else if let Some(content) = line.strip_prefix('-') {
                    old_lines.push(content.to_string());
                } else if let Some(content) = line.strip_prefix('+') {
                    new_lines.push(content.to_string());
                } else if let Some(content) = line.strip_prefix(' ') {
                    old_lines.push(content.to_string());
                    new_lines.push(content.to_string());
                } else {
                    return Err(parse_error(
                        "update hunk lines must start with ` `, `-`, or `+`",
                    ));
                }
            }

            if old_lines.is_empty() && new_lines.is_empty() {
                return Err(parse_error("empty update hunk"));
            }
            hunks.push(UpdateHunk {
                old_lines,
                new_lines,
            });
        }

        if hunks.is_empty() {
            return Err(parse_error("update file operation contains no hunks"));
        }

        Ok(PatchOperation::Update {
            path: PathBuf::from(path),
            move_path,
            hunks,
        })
    }

    fn expect_hunk_header(&mut self) -> Result<()> {
        let Some(line) = self.next() else {
            return Err(parse_error("expected update hunk header"));
        };
        if line == "@@" || line.starts_with("@@ ") {
            Ok(())
        } else {
            Err(parse_error(format!(
                "expected update hunk header, got `{line}`"
            )))
        }
    }

    fn expect_exact(&mut self, expected: &str) -> Result<()> {
        match self.next() {
            Some(line) if line == expected => Ok(()),
            Some(line) => Err(parse_error(format!("expected `{expected}`, got `{line}`"))),
            None => Err(parse_error(format!("expected `{expected}`"))),
        }
    }

    fn take_prefixed(&mut self, prefix: &str) -> Option<String> {
        let path = self.current()?.strip_prefix(prefix)?.to_string();
        self.index += 1;
        Some(path)
    }

    fn peek_starts_operation(&self) -> bool {
        self.current().is_some_and(|line| {
            line.starts_with("*** Add File: ")
                || line.starts_with("*** Update File: ")
                || line.starts_with("*** Delete File: ")
        })
    }

    fn peek_is_hunk_header(&self) -> bool {
        self.current()
            .is_some_and(|line| line == "@@" || line.starts_with("@@ "))
    }

    fn peek_is(&self, expected: &str) -> bool {
        self.current().is_some_and(|line| line == expected)
    }

    fn current(&self) -> Option<&str> {
        self.lines.get(self.index).copied()
    }

    fn next(&mut self) -> Option<&str> {
        let line = self.lines.get(self.index).copied()?;
        self.index += 1;
        Some(line)
    }

    fn is_done(&self) -> bool {
        self.index >= self.lines.len()
    }
}

fn parse_error(message: impl Into<String>) -> Error {
    Error::tool_policy(message)
}
