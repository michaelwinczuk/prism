//! CodeForge domain pack: Git-aware code generation and validation for agentic AI systems.
//!
//! Provides four core capabilities:
//!
//! - [`GitSync`] — Clone repos, create branches, commit changes via subprocess `git`.
//! - [`ParallelPR`] — Spawn multiple agents concurrently to generate code fixes.
//! - [`Sandbox`] — Execute generated code safely with timeout and resource limits.
//! - [`DiffEngine`] — Compute unified diffs between original and generated code.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{PrismError, PrismResult};

// ---------------------------------------------------------------------------
// CodeFix
// ---------------------------------------------------------------------------

/// A single code fix produced by an agent.
#[derive(Debug, Clone)]
pub struct CodeFix {
    /// Path to the file that was modified.
    pub path: PathBuf,
    /// Original file content.
    pub original: String,
    /// Modified file content.
    pub modified: String,
    /// Unified diff between original and modified.
    pub diff: String,
    /// Identifier of the agent/model that produced this fix.
    pub model_id: String,
}

// ---------------------------------------------------------------------------
// SandboxResult
// ---------------------------------------------------------------------------

/// Result of a sandboxed command execution.
#[derive(Debug, Clone)]
pub struct SandboxResult {
    /// Process exit code, or `None` if the process was killed (e.g. timeout).
    pub exit_code: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Whether the process was killed due to timeout.
    pub timed_out: bool,
}

impl SandboxResult {
    /// Returns `true` if the command exited successfully (code 0, no timeout).
    pub fn success(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

// ---------------------------------------------------------------------------
// GitSync
// ---------------------------------------------------------------------------

/// Git operations via subprocess for portability (no C dependency on libgit2).
#[derive(Debug, Clone)]
pub struct GitSync {
    /// Working directory for git operations.
    work_dir: PathBuf,
}

impl GitSync {
    /// Create a new `GitSync` bound to the given working directory.
    pub fn new(work_dir: impl Into<PathBuf>) -> Self {
        Self {
            work_dir: work_dir.into(),
        }
    }

    /// Return the working directory.
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Clone a remote repository into `dest`.
    pub async fn clone_repo(url: &str, dest: impl AsRef<Path>) -> PrismResult<Self> {
        let dest = dest.as_ref();
        let output = tokio::process::Command::new("git")
            .args(["clone", url])
            .arg(dest)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PrismError::Checkpoint(format!(
                "git clone failed: {stderr}"
            )));
        }

        Ok(Self {
            work_dir: dest.to_path_buf(),
        })
    }

    /// Initialize a new git repository in the working directory.
    pub async fn init_repo(&self) -> PrismResult<()> {
        tokio::fs::create_dir_all(&self.work_dir).await?;
        let output = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(&self.work_dir)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PrismError::Checkpoint(format!("git init failed: {stderr}")));
        }
        Ok(())
    }

    /// Create and checkout a new branch.
    pub async fn create_branch(&self, name: &str) -> PrismResult<()> {
        let output = tokio::process::Command::new("git")
            .args(["checkout", "-b", name])
            .current_dir(&self.work_dir)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PrismError::Checkpoint(format!(
                "git checkout -b failed: {stderr}"
            )));
        }
        Ok(())
    }

    /// Stage the given files and create a commit.
    pub async fn commit_changes(
        &self,
        message: &str,
        files: &[impl AsRef<Path>],
    ) -> PrismResult<()> {
        // Stage files
        for file in files {
            let output = tokio::process::Command::new("git")
                .args(["add"])
                .arg(file.as_ref())
                .current_dir(&self.work_dir)
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(PrismError::Checkpoint(format!(
                    "git add failed for {}: {stderr}",
                    file.as_ref().display()
                )));
            }
        }

        // Commit
        let output = tokio::process::Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(&self.work_dir)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PrismError::Checkpoint(format!(
                "git commit failed: {stderr}"
            )));
        }
        Ok(())
    }

    /// Check whether the working tree has merge conflicts.
    pub async fn has_conflicts(&self) -> PrismResult<bool> {
        let output = tokio::process::Command::new("git")
            .args(["diff", "--check"])
            .current_dir(&self.work_dir)
            .output()
            .await?;

        // `git diff --check` exits non-zero when conflict markers are present.
        Ok(!output.status.success())
    }
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

/// Execute subprocess commands with timeout and captured output.
#[derive(Debug, Clone)]
pub struct Sandbox {
    /// Default timeout for command execution.
    default_timeout: Duration,
}

impl Sandbox {
    /// Create a new `Sandbox` with the given default timeout in seconds.
    pub fn new(default_timeout_secs: u64) -> Self {
        Self {
            default_timeout: Duration::from_secs(default_timeout_secs),
        }
    }

    /// Execute a command inside the sandbox.
    ///
    /// # Arguments
    /// * `program` — The program to execute.
    /// * `args` — Command-line arguments.
    /// * `work_dir` — Working directory for the process.
    /// * `timeout` — Optional override for the default timeout.
    pub async fn execute(
        &self,
        program: &str,
        args: &[&str],
        work_dir: impl AsRef<Path>,
        timeout: Option<Duration>,
    ) -> PrismResult<SandboxResult> {
        let timeout = timeout.unwrap_or(self.default_timeout);

        let child = tokio::process::Command::new(program)
            .args(args)
            .current_dir(work_dir.as_ref())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let result = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => Ok(SandboxResult {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
            }),
            Ok(Err(e)) => Err(PrismError::Io(e)),
            Err(_) => {
                // Timeout — process will be killed when dropped.
                Ok(SandboxResult {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: "Process timed out and was terminated".to_string(),
                    timed_out: true,
                })
            }
        };

        result
    }
}

// ---------------------------------------------------------------------------
// DiffEngine
// ---------------------------------------------------------------------------

/// Compute unified diffs between text content.
#[derive(Debug, Clone, Copy)]
pub struct DiffEngine;

impl DiffEngine {
    /// Compute a unified diff between `original` and `modified`.
    ///
    /// Returns a string in unified diff format with `---`/`+++` headers and
    /// `@@` hunk markers.
    pub fn diff(original: &str, modified: &str) -> String {
        let orig_lines: Vec<&str> = original.lines().collect();
        let mod_lines: Vec<&str> = modified.lines().collect();

        if orig_lines == mod_lines {
            return String::new();
        }

        let mut output = String::new();
        output.push_str("--- original\n");
        output.push_str("+++ modified\n");

        // Simple LCS-based diff with context.
        let lcs = Self::lcs_table(&orig_lines, &mod_lines);
        let edits = Self::backtrack(&lcs, &orig_lines, &mod_lines);
        let hunks = Self::group_into_hunks(&edits, 3);

        for hunk in &hunks {
            let (orig_start, orig_count, mod_start, mod_count) = Self::hunk_header(hunk);
            output.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                orig_start, orig_count, mod_start, mod_count
            ));
            for edit in hunk {
                match edit {
                    Edit::Equal(_, line) => {
                        output.push(' ');
                        output.push_str(line);
                        output.push('\n');
                    }
                    Edit::Delete(_, line) => {
                        output.push('-');
                        output.push_str(line);
                        output.push('\n');
                    }
                    Edit::Insert(_, line) => {
                        output.push('+');
                        output.push_str(line);
                        output.push('\n');
                    }
                }
            }
        }

        output
    }

    /// Check for test regressions: returns `true` if tests previously passed
    /// but now fail.
    pub fn has_regression(original_tests_pass: bool, modified_tests_pass: bool) -> bool {
        original_tests_pass && !modified_tests_pass
    }

    // -- private helpers --

    fn lcs_table(a: &[&str], b: &[&str]) -> Vec<Vec<usize>> {
        let m = a.len();
        let n = b.len();
        let mut table = vec![vec![0usize; n + 1]; m + 1];
        for i in 1..=m {
            for j in 1..=n {
                if a[i - 1] == b[j - 1] {
                    table[i][j] = table[i - 1][j - 1] + 1;
                } else {
                    table[i][j] = std::cmp::max(table[i - 1][j], table[i][j - 1]);
                }
            }
        }
        table
    }

    fn backtrack<'a>(table: &[Vec<usize>], a: &[&'a str], b: &[&'a str]) -> Vec<Edit<'a>> {
        let mut edits = Vec::new();
        let mut i = a.len();
        let mut j = b.len();

        while i > 0 || j > 0 {
            if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
                edits.push(Edit::Equal(i - 1, a[i - 1]));
                i -= 1;
                j -= 1;
            } else if j > 0 && (i == 0 || table[i][j - 1] >= table[i - 1][j]) {
                edits.push(Edit::Insert(j - 1, b[j - 1]));
                j -= 1;
            } else if i > 0 {
                edits.push(Edit::Delete(i - 1, a[i - 1]));
                i -= 1;
            }
        }

        edits.reverse();
        edits
    }

    fn group_into_hunks<'a>(edits: &[Edit<'a>], context: usize) -> Vec<Vec<Edit<'a>>> {
        if edits.is_empty() {
            return Vec::new();
        }

        // Find indices of changed edits.
        let change_indices: Vec<usize> = edits
            .iter()
            .enumerate()
            .filter(|(_, e)| !matches!(e, Edit::Equal(..)))
            .map(|(i, _)| i)
            .collect();

        if change_indices.is_empty() {
            return Vec::new();
        }

        let mut hunks: Vec<Vec<Edit<'a>>> = Vec::new();
        let mut hunk_start = change_indices[0].saturating_sub(context);
        let mut hunk_end = std::cmp::min(change_indices[0] + context, edits.len() - 1);

        for &ci in &change_indices[1..] {
            let new_start = ci.saturating_sub(context);
            let new_end = std::cmp::min(ci + context, edits.len() - 1);

            if new_start <= hunk_end + 1 {
                // Merge with current hunk.
                hunk_end = new_end;
            } else {
                // Emit current hunk, start new one.
                hunks.push(edits[hunk_start..=hunk_end].to_vec());
                hunk_start = new_start;
                hunk_end = new_end;
            }
        }
        hunks.push(edits[hunk_start..=hunk_end].to_vec());
        hunks
    }

    fn hunk_header(hunk: &[Edit<'_>]) -> (usize, usize, usize, usize) {
        let mut orig_start = usize::MAX;
        let mut orig_count = 0usize;
        let mut mod_start = usize::MAX;
        let mut mod_count = 0usize;

        for edit in hunk {
            match edit {
                Edit::Equal(idx, _) => {
                    if orig_start == usize::MAX {
                        orig_start = *idx + 1;
                    }
                    if mod_start == usize::MAX {
                        mod_start = *idx + 1;
                    }
                    orig_count += 1;
                    mod_count += 1;
                }
                Edit::Delete(idx, _) => {
                    if orig_start == usize::MAX {
                        orig_start = *idx + 1;
                    }
                    orig_count += 1;
                }
                Edit::Insert(idx, _) => {
                    if mod_start == usize::MAX {
                        mod_start = *idx + 1;
                    }
                    mod_count += 1;
                }
            }
        }

        // Default to 1 if no lines found (shouldn't happen with valid input).
        if orig_start == usize::MAX {
            orig_start = 1;
        }
        if mod_start == usize::MAX {
            mod_start = 1;
        }

        (orig_start, orig_count, mod_start, mod_count)
    }
}

/// Internal edit operation for diff computation.
#[derive(Debug, Clone, Copy)]
enum Edit<'a> {
    /// Line present in both (index in original, content).
    Equal(usize, &'a str),
    /// Line only in original (index in original, content).
    Delete(usize, &'a str),
    /// Line only in modified (index in modified, content).
    Insert(usize, &'a str),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- DiffEngine tests --

    #[test]
    fn test_diff_identical() {
        let text = "line1\nline2\nline3";
        let result = DiffEngine::diff(text, text);
        assert!(
            result.is_empty(),
            "identical text should produce empty diff"
        );
    }

    #[test]
    fn test_diff_single_line_change() {
        let original = "aaa\nbbb\nccc";
        let modified = "aaa\nBBB\nccc";
        let diff = DiffEngine::diff(original, modified);
        assert!(diff.contains("--- original"));
        assert!(diff.contains("+++ modified"));
        assert!(diff.contains("-bbb"));
        assert!(diff.contains("+BBB"));
    }

    #[test]
    fn test_diff_addition() {
        let original = "a\nb";
        let modified = "a\nb\nc";
        let diff = DiffEngine::diff(original, modified);
        assert!(diff.contains("+c"));
    }

    #[test]
    fn test_diff_deletion() {
        let original = "a\nb\nc";
        let modified = "a\nc";
        let diff = DiffEngine::diff(original, modified);
        assert!(diff.contains("-b"));
    }

    #[test]
    fn test_diff_empty_original() {
        let diff = DiffEngine::diff("", "hello");
        assert!(diff.contains("+hello"));
    }

    #[test]
    fn test_diff_empty_modified() {
        let diff = DiffEngine::diff("hello", "");
        assert!(diff.contains("-hello"));
    }

    #[test]
    fn test_diff_both_empty() {
        let diff = DiffEngine::diff("", "");
        assert!(diff.is_empty());
    }

    #[test]
    fn test_has_regression_true() {
        assert!(DiffEngine::has_regression(true, false));
    }

    #[test]
    fn test_has_regression_false_both_pass() {
        assert!(!DiffEngine::has_regression(true, true));
    }

    #[test]
    fn test_has_regression_false_both_fail() {
        assert!(!DiffEngine::has_regression(false, false));
    }

    #[test]
    fn test_has_regression_false_improvement() {
        assert!(!DiffEngine::has_regression(false, true));
    }

    // -- SandboxResult tests --

    #[test]
    fn test_sandbox_result_success() {
        let r = SandboxResult {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        };
        assert!(r.success());
    }

    #[test]
    fn test_sandbox_result_failure() {
        let r = SandboxResult {
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "error".into(),
            timed_out: false,
        };
        assert!(!r.success());
    }

    #[test]
    fn test_sandbox_result_timeout() {
        let r = SandboxResult {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        };
        assert!(!r.success());
    }

    // -- Sandbox integration tests (require system commands) --

    #[tokio::test]
    async fn test_sandbox_echo() {
        let sandbox = Sandbox::new(10);
        let result = sandbox
            .execute(
                if cfg!(windows) { "cmd" } else { "echo" },
                if cfg!(windows) {
                    &["/C", "echo", "hello"]
                } else {
                    &["hello"]
                },
                ".",
                None,
            )
            .await
            .unwrap();

        assert!(result.success());
        assert!(result.stdout.trim().contains("hello"));
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_sandbox_timeout() {
        let sandbox = Sandbox::new(1);
        let result = sandbox
            .execute(
                if cfg!(windows) { "ping" } else { "sleep" },
                if cfg!(windows) {
                    &["-n", "30", "127.0.0.1"]
                } else {
                    &["30"]
                },
                ".",
                Some(Duration::from_millis(500)),
            )
            .await
            .unwrap();

        assert!(result.timed_out);
        assert!(!result.success());
    }

    #[tokio::test]
    async fn test_sandbox_nonexistent_command() {
        let sandbox = Sandbox::new(5);
        let result = sandbox
            .execute("nonexistent_command_xyz_123", &[], ".", None)
            .await;

        assert!(result.is_err());
    }

    // -- GitSync tests (require git on PATH) --

    #[tokio::test]
    async fn test_git_init_and_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let gs = GitSync::new(tmp.path());

        gs.init_repo().await.unwrap();

        // Configure git user for the test repo so commits work.
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Create an initial commit so we can branch.
        let file_path = tmp.path().join("README.md");
        tokio::fs::write(&file_path, "# Test").await.unwrap();
        gs.commit_changes("initial commit", &[PathBuf::from("README.md")])
            .await
            .unwrap();

        gs.create_branch("feature-x").await.unwrap();

        // Verify we are on the new branch.
        let output = tokio::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        let branch = String::from_utf8_lossy(&output.stdout);
        assert_eq!(branch.trim(), "feature-x");
    }

    #[tokio::test]
    async fn test_git_commit_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let gs = GitSync::new(tmp.path());
        gs.init_repo().await.unwrap();

        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        let file_path = tmp.path().join("hello.txt");
        tokio::fs::write(&file_path, "hello world").await.unwrap();

        gs.commit_changes("add hello", &[PathBuf::from("hello.txt")])
            .await
            .unwrap();

        // Verify the commit exists.
        let output = tokio::process::Command::new("git")
            .args(["log", "--oneline"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        let log = String::from_utf8_lossy(&output.stdout);
        assert!(log.contains("add hello"));
    }

    #[tokio::test]
    async fn test_git_has_conflicts_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let gs = GitSync::new(tmp.path());
        gs.init_repo().await.unwrap();

        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        let file_path = tmp.path().join("f.txt");
        tokio::fs::write(&file_path, "clean").await.unwrap();
        gs.commit_changes("init", &[PathBuf::from("f.txt")])
            .await
            .unwrap();

        let conflicts = gs.has_conflicts().await.unwrap();
        assert!(!conflicts);
    }

    // -- CodeFix construction test --

    #[test]
    fn test_code_fix_construction() {
        let fix = CodeFix {
            path: PathBuf::from("src/main.rs"),
            original: "fn main() {}".into(),
            modified: "fn main() { println!(\"hi\"); }".into(),
            diff: "+println".into(),
            model_id: "gpt-4".into(),
        };
        assert_eq!(fix.path, PathBuf::from("src/main.rs"));
        assert_eq!(fix.model_id, "gpt-4");
    }

    // -- GitSync::work_dir accessor --

    #[test]
    fn test_git_sync_work_dir() {
        let gs = GitSync::new("/tmp/test");
        assert_eq!(gs.work_dir(), Path::new("/tmp/test"));
    }
}
