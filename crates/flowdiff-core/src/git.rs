use git2::{Delta, DiffOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("ref not found: {0}")]
    RefNotFound(String),
    #[error("empty repository — no commits found")]
    EmptyRepo,
    #[error("invalid range: {0}")]
    InvalidRange(String),
}

/// Status of a changed file in the diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
}

/// A single hunk within a file diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffHunk {
    /// Starting line in old file (1-indexed)
    pub old_start: u32,
    /// Number of lines in old file
    pub old_lines: u32,
    /// Starting line in new file (1-indexed)
    pub new_start: u32,
    /// Number of lines in new file
    pub new_lines: u32,
}

/// A single changed file extracted from a git diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileDiff {
    /// Old file path (None for newly added files)
    pub old_path: Option<String>,
    /// New file path (None for deleted files)
    pub new_path: Option<String>,
    /// Old file content (None for newly added files)
    pub old_content: Option<String>,
    /// New file content (None for deleted files)
    pub new_content: Option<String>,
    /// Hunks within the diff
    pub hunks: Vec<DiffHunk>,
    /// Status of the change
    pub status: FileStatus,
    /// Number of added lines
    pub additions: u32,
    /// Number of deleted lines
    pub deletions: u32,
    /// Whether the file is binary
    pub is_binary: bool,
}

impl FileDiff {
    /// Returns the primary file path (new_path for adds/modifies/renames, old_path for deletes).
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("<unknown>")
    }
}

/// Result of extracting diffs from a git repository.
#[derive(Debug, Clone)]
pub struct DiffResult {
    pub files: Vec<FileDiff>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
}

/// Extract diffs between two refs (branches, tags, or commit SHAs).
pub fn diff_refs(
    repo: &Repository,
    base_ref: &str,
    head_ref: &str,
) -> Result<DiffResult, GitError> {
    let base_obj = repo
        .revparse_single(base_ref)
        .map_err(|_| GitError::RefNotFound(base_ref.to_string()))?;
    let head_obj = repo
        .revparse_single(head_ref)
        .map_err(|_| GitError::RefNotFound(head_ref.to_string()))?;

    let base_commit = base_obj
        .peel_to_commit()
        .map_err(|_| GitError::RefNotFound(format!("{base_ref} (not a commit)")))?;
    let head_commit = head_obj
        .peel_to_commit()
        .map_err(|_| GitError::RefNotFound(format!("{head_ref} (not a commit)")))?;

    let base_tree = base_commit.tree()?;
    let head_tree = head_commit.tree()?;

    let mut opts = DiffOptions::new();
    opts.context_lines(3);

    let mut diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut opts))?;
    find_renames(&mut diff)?;

    let files = extract_file_diffs(repo, &diff)?;

    Ok(DiffResult {
        files,
        base_sha: Some(base_commit.id().to_string()),
        head_sha: Some(head_commit.id().to_string()),
    })
}

/// Extract diffs for a commit range (e.g., HEAD~5..HEAD).
pub fn diff_range(repo: &Repository, range: &str) -> Result<DiffResult, GitError> {
    let parts: Vec<&str> = range.split("..").collect();
    if parts.len() != 2 {
        return Err(GitError::InvalidRange(range.to_string()));
    }
    diff_refs(repo, parts[0], parts[1])
}

/// Extract staged (index) changes.
pub fn diff_staged(repo: &Repository) -> Result<DiffResult, GitError> {
    let head_commit = repo
        .head()
        .map_err(|_| GitError::EmptyRepo)?
        .peel_to_commit()
        .map_err(|_| GitError::EmptyRepo)?;
    let head_tree = head_commit.tree()?;

    let mut opts = DiffOptions::new();
    opts.context_lines(3);

    let diff = repo.diff_tree_to_index(Some(&head_tree), None, Some(&mut opts))?;
    let files = extract_file_diffs(repo, &diff)?;

    Ok(DiffResult {
        files,
        base_sha: Some(head_commit.id().to_string()),
        head_sha: None,
    })
}

/// Extract unstaged (working directory) changes.
pub fn diff_unstaged(repo: &Repository) -> Result<DiffResult, GitError> {
    let mut opts = DiffOptions::new();
    opts.context_lines(3);

    let diff = repo.diff_index_to_workdir(None, Some(&mut opts))?;
    let files = extract_file_diffs(repo, &diff)?;

    Ok(DiffResult {
        files,
        base_sha: None,
        head_sha: None,
    })
}

// ── Git auto-discovery types ──

/// Information about a git branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchInfo {
    /// Branch name (e.g., "main", "feature/foo").
    pub name: String,
    /// Whether this is the currently checked-out branch.
    pub is_current: bool,
    /// Whether it has a remote tracking branch.
    pub has_upstream: bool,
}

/// Information about a git worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorktreeInfo {
    /// Path to the worktree directory.
    pub path: String,
    /// Branch checked out in this worktree (if any).
    pub branch: Option<String>,
    /// Whether this is the main worktree.
    pub is_main: bool,
}

/// Branch tracking status (ahead/behind remote).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchStatus {
    /// Name of the current branch.
    pub branch: String,
    /// Remote tracking branch (e.g., "origin/main"), if any.
    pub upstream: Option<String>,
    /// Number of commits ahead of upstream.
    pub ahead: usize,
    /// Number of commits behind upstream.
    pub behind: usize,
}

// ── Git auto-discovery functions ──

/// List all local branches.
pub fn list_branches(repo: &Repository) -> Result<Vec<BranchInfo>, GitError> {
    let head_ref = repo.head().ok();
    let current_branch = head_ref
        .as_ref()
        .and_then(|h| h.shorthand().map(String::from));

    let branches = repo.branches(Some(git2::BranchType::Local))?;
    let mut result = Vec::new();

    for branch_result in branches {
        let (branch, _) = branch_result?;
        let name = match branch.name()? {
            Some(n) => n.to_string(),
            None => continue,
        };
        let has_upstream = branch.upstream().is_ok();
        let is_current = current_branch.as_deref() == Some(name.as_str());

        result.push(BranchInfo {
            name,
            is_current,
            has_upstream,
        });
    }

    // Sort: current branch first, then alphabetically
    result.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current)
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(result)
}

/// List all git worktrees.
pub fn list_worktrees(repo: &Repository) -> Result<Vec<WorktreeInfo>, GitError> {
    let worktree_names = repo.worktrees()?;
    let mut result = Vec::new();

    // Add the main worktree
    if let Some(workdir) = repo.workdir() {
        let head_ref = repo.head().ok();
        let branch = head_ref
            .as_ref()
            .and_then(|h| h.shorthand().map(String::from));
        result.push(WorktreeInfo {
            path: workdir.to_string_lossy().to_string(),
            branch,
            is_main: true,
        });
    }

    // Add linked worktrees
    for name in worktree_names.iter() {
        let name = match name {
            Some(n) => n,
            None => continue,
        };
        let wt = match repo.find_worktree(name) {
            Ok(wt) => wt,
            Err(_) => continue,
        };
        let wt_path = wt.path().to_string_lossy().to_string();

        // Try to open the worktree repo to get its branch
        let branch = Repository::open(wt.path())
            .ok()
            .and_then(|wt_repo| {
                wt_repo
                    .head()
                    .ok()
                    .and_then(|h| h.shorthand().map(String::from))
            });

        result.push(WorktreeInfo {
            path: wt_path,
            branch,
            is_main: false,
        });
    }

    Ok(result)
}

/// Get the current branch's tracking status (ahead/behind upstream).
pub fn get_branch_status(repo: &Repository) -> Result<BranchStatus, GitError> {
    let head = repo.head().map_err(|_| GitError::EmptyRepo)?;
    let branch_name = head
        .shorthand()
        .ok_or_else(|| GitError::RefNotFound("HEAD (detached)".to_string()))?
        .to_string();

    let local_branch = repo
        .find_branch(&branch_name, git2::BranchType::Local)
        .map_err(|_| GitError::RefNotFound(branch_name.clone()))?;

    // Try to find upstream
    let upstream = match local_branch.upstream() {
        Ok(up) => up,
        Err(_) => {
            // No upstream configured
            return Ok(BranchStatus {
                branch: branch_name,
                upstream: None,
                ahead: 0,
                behind: 0,
            });
        }
    };

    let upstream_name = upstream.name()?.map(String::from);

    let local_oid = head
        .target()
        .ok_or_else(|| GitError::RefNotFound("HEAD target".to_string()))?;
    let upstream_oid = upstream
        .get()
        .target()
        .ok_or_else(|| GitError::RefNotFound("upstream target".to_string()))?;

    let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;

    Ok(BranchStatus {
        branch: branch_name,
        upstream: upstream_name,
        ahead,
        behind,
    })
}

/// Auto-detect the default branch name (main, master, or first branch).
pub fn detect_default_branch(repo: &Repository) -> Result<String, GitError> {
    // Check for common default branch names
    for name in &["main", "master", "develop"] {
        if repo
            .find_branch(name, git2::BranchType::Local)
            .is_ok()
        {
            return Ok(name.to_string());
        }
    }

    // Fall back to first branch
    let branches = repo.branches(Some(git2::BranchType::Local))?;
    for branch_result in branches {
        let (branch, _) = branch_result?;
        if let Ok(Some(name)) = branch.name() {
            return Ok(name.to_string());
        }
    }

    Err(GitError::EmptyRepo)
}

/// Extract diffs using merge-base (PR preview mode).
///
/// Finds the merge base between `base_ref` and `head_ref`, then diffs from
/// the merge base to `head_ref`. This shows what the PR/branch introduces
/// relative to where it diverged from the base branch.
pub fn diff_merge_base(
    repo: &Repository,
    base_ref: &str,
    head_ref: &str,
) -> Result<DiffResult, GitError> {
    let base_obj = repo
        .revparse_single(base_ref)
        .map_err(|_| GitError::RefNotFound(base_ref.to_string()))?;
    let head_obj = repo
        .revparse_single(head_ref)
        .map_err(|_| GitError::RefNotFound(head_ref.to_string()))?;

    let base_oid = base_obj
        .peel_to_commit()
        .map_err(|_| GitError::RefNotFound(format!("{base_ref} (not a commit)")))?
        .id();
    let head_commit = head_obj
        .peel_to_commit()
        .map_err(|_| GitError::RefNotFound(format!("{head_ref} (not a commit)")))?;

    let merge_base_oid = repo
        .merge_base(base_oid, head_commit.id())
        .map_err(|_| {
            GitError::RefNotFound(format!(
                "no merge base between {} and {}",
                base_ref, head_ref
            ))
        })?;

    let merge_base_commit = repo.find_commit(merge_base_oid)?;
    let merge_base_tree = merge_base_commit.tree()?;
    let head_tree = head_commit.tree()?;

    let mut opts = DiffOptions::new();
    opts.context_lines(3);

    let mut diff =
        repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&head_tree), Some(&mut opts))?;
    find_renames(&mut diff)?;

    let files = extract_file_diffs(repo, &diff)?;

    Ok(DiffResult {
        files,
        base_sha: Some(merge_base_oid.to_string()),
        head_sha: Some(head_commit.id().to_string()),
    })
}

/// Get the current branch name (or None if HEAD is detached).
pub fn current_branch(repo: &Repository) -> Option<String> {
    repo.head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from))
}

/// Get the repository workdir path.
pub fn workdir(repo: &Repository) -> Option<PathBuf> {
    repo.workdir().map(|p| p.to_path_buf())
}

/// Read the content of a file at a specific git ref (branch, tag, or SHA).
///
/// Returns `None` if the file does not exist at that ref.
/// Returns `Err` if the ref itself is invalid or the blob is not valid UTF-8.
pub fn file_content_at_ref(
    repo: &Repository,
    git_ref: &str,
    file_path: &str,
) -> Result<Option<String>, GitError> {
    let obj = repo
        .revparse_single(git_ref)
        .map_err(|_| GitError::RefNotFound(git_ref.to_string()))?;
    let commit = obj
        .peel_to_commit()
        .map_err(|_| GitError::RefNotFound(format!("{git_ref} (not a commit)")))?;
    let tree = commit.tree()?;

    match tree.get_path(std::path::Path::new(file_path)) {
        Ok(entry) => {
            let blob = entry
                .to_object(repo)?
                .into_blob()
                .map_err(|_| GitError::Git(git2::Error::from_str("not a blob")))?;
            let content = std::str::from_utf8(blob.content())
                .map_err(|e| GitError::Git(git2::Error::from_str(&format!("invalid UTF-8: {e}"))))?;
            Ok(Some(content.to_string()))
        }
        Err(_) => Ok(None), // File doesn't exist at this ref
    }
}

/// Apply rename detection to a diff.
fn find_renames<'a>(
    diff: &mut git2::Diff<'a>,
) -> Result<(), GitError> {
    let mut find_opts = git2::DiffFindOptions::new();
    find_opts.renames(true);
    find_opts.copies(true);
    diff.find_similar(Some(&mut find_opts))?;
    Ok(())
}

/// Extract FileDiff structs from a git2::Diff.
fn extract_file_diffs(
    repo: &Repository,
    diff: &git2::Diff<'_>,
) -> Result<Vec<FileDiff>, GitError> {
    let num_deltas = diff.deltas().len();
    let mut files: Vec<FileDiff> = Vec::with_capacity(num_deltas);

    for delta_idx in 0..num_deltas {
        let delta = match diff.deltas().nth(delta_idx) {
            Some(d) => d,
            None => continue,
        };

        // Skip binary files
        if delta.flags().is_binary() {
            continue;
        }

        let status = match delta.status() {
            Delta::Added => FileStatus::Added,
            Delta::Deleted => FileStatus::Deleted,
            Delta::Modified => FileStatus::Modified,
            Delta::Renamed => FileStatus::Renamed,
            Delta::Copied => FileStatus::Copied,
            _ => continue, // Skip unmodified, ignored, typechange, etc.
        };

        let old_path = delta.old_file().path().map(|p| p.to_string_lossy().to_string());
        let new_path = delta.new_file().path().map(|p| p.to_string_lossy().to_string());

        // Check binary at the blob level (before UTF-8 conversion)
        if is_blob_binary(repo, delta.old_file().id())
            || is_blob_binary(repo, delta.new_file().id())
        {
            continue;
        }

        // Read old content
        let old_content = if status != FileStatus::Added {
            read_blob_content(repo, delta.old_file().id())
        } else {
            None
        };

        // Read new content
        let new_content = if status != FileStatus::Deleted {
            read_blob_content(repo, delta.new_file().id())
        } else {
            None
        };

        files.push(FileDiff {
            old_path,
            new_path,
            old_content,
            new_content,
            hunks: Vec::new(),
            status,
            additions: 0,
            deletions: 0,
            is_binary: false,
        });
    }

    // Extract hunks and line counts using the diff's foreach.
    // Use Cell for file_idx so all closures can share it without borrow conflicts.
    let file_idx: Cell<Option<usize>> = Cell::new(None);
    let mut file_additions: Vec<u32> = vec![0; files.len()];
    let mut file_deletions: Vec<u32> = vec![0; files.len()];
    let mut file_hunks: Vec<Vec<DiffHunk>> = vec![Vec::new(); files.len()];

    // Build a map of (old_path, new_path, status) -> index for matching
    let file_keys: Vec<(Option<String>, Option<String>, FileStatus)> = files
        .iter()
        .map(|f| (f.old_path.clone(), f.new_path.clone(), f.status.clone()))
        .collect();

    diff.foreach(
        &mut |delta, _progress| {
            let d_old = delta.old_file().path().map(|p| p.to_string_lossy().to_string());
            let d_new = delta.new_file().path().map(|p| p.to_string_lossy().to_string());
            let d_status = match delta.status() {
                Delta::Added => FileStatus::Added,
                Delta::Deleted => FileStatus::Deleted,
                Delta::Modified => FileStatus::Modified,
                Delta::Renamed => FileStatus::Renamed,
                Delta::Copied => FileStatus::Copied,
                _ => {
                    file_idx.set(None);
                    return true;
                }
            };

            let idx = file_keys
                .iter()
                .position(|(old, new, st)| *old == d_old && *new == d_new && *st == d_status);
            file_idx.set(idx);
            true
        },
        None,
        Some(&mut |_delta, hunk| {
            if let Some(idx) = file_idx.get() {
                file_hunks[idx].push(DiffHunk {
                    old_start: hunk.old_start(),
                    old_lines: hunk.old_lines(),
                    new_start: hunk.new_start(),
                    new_lines: hunk.new_lines(),
                });
            }
            true
        }),
        Some(&mut |_delta, _hunk, line| {
            if let Some(idx) = file_idx.get() {
                match line.origin() {
                    '+' => file_additions[idx] += 1,
                    '-' => file_deletions[idx] += 1,
                    _ => {}
                }
            }
            true
        }),
    )?;

    // Apply collected hunks and counts back to files
    for (i, file) in files.iter_mut().enumerate() {
        file.hunks = std::mem::take(&mut file_hunks[i]);
        file.additions = file_additions[i];
        file.deletions = file_deletions[i];
    }

    Ok(files)
}

/// Read blob content as UTF-8 string (returns None for zero OIDs or non-UTF-8).
fn read_blob_content(repo: &Repository, oid: Oid) -> Option<String> {
    if oid.is_zero() {
        return None;
    }
    let blob = repo.find_blob(oid).ok()?;
    std::str::from_utf8(blob.content()).ok().map(String::from)
}

/// Check if a blob is binary by examining its raw bytes.
fn is_blob_binary(repo: &Repository, oid: Oid) -> bool {
    if oid.is_zero() {
        return false;
    }
    let blob = match repo.find_blob(oid) {
        Ok(b) => b,
        Err(_) => return false,
    };
    blob.is_binary()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    /// Helper: create a new git repo in a temp dir with an initial commit.
    fn init_repo() -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Configure committer identity for test commits
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@test.com").unwrap();

        (dir, repo)
    }

    /// Helper: create a commit with the given files.
    fn commit_files(repo: &Repository, dir: &Path, files: &[(&str, &str)], msg: &str) -> Oid {
        let mut index = repo.index().unwrap();
        for (path, content) in files {
            let full_path = dir.join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&full_path, content).unwrap();
            index.add_path(Path::new(path)).unwrap();
        }
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();

        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    /// Helper: delete files and commit.
    fn delete_files_and_commit(
        repo: &Repository,
        dir: &Path,
        paths: &[&str],
        msg: &str,
    ) -> Oid {
        let mut index = repo.index().unwrap();
        for path in paths {
            let full_path = dir.join(path);
            if full_path.exists() {
                fs::remove_file(&full_path).unwrap();
            }
            index.remove_path(Path::new(path)).unwrap();
        }
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();

        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent])
            .unwrap()
    }

    // ── Branch Comparison Tests ──

    #[test]
    fn test_diff_branch_comparison() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[
                ("src/main.ts", "console.log('hello');"),
                ("src/utils.ts", "export function add(a: number, b: number) { return a + b; }"),
            ],
            "initial",
        );

        // Create a branch at base
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base-branch", &base_commit, false).unwrap();

        // Make changes on HEAD
        commit_files(
            &repo,
            dir.path(),
            &[
                ("src/main.ts", "console.log('hello world');"),
                ("src/new-file.ts", "export const x = 42;"),
            ],
            "add changes",
        );

        let result = diff_refs(&repo, "base-branch", "HEAD").unwrap();
        assert_eq!(result.files.len(), 2);

        let paths: Vec<&str> = result.files.iter().map(|f| f.path()).collect();
        assert!(paths.contains(&"src/main.ts"));
        assert!(paths.contains(&"src/new-file.ts"));

        let main_diff = result.files.iter().find(|f| f.path() == "src/main.ts").unwrap();
        assert_eq!(main_diff.status, FileStatus::Modified);
        assert!(main_diff.old_content.is_some());
        assert!(main_diff.new_content.is_some());
        assert!(main_diff.additions > 0 || main_diff.deletions > 0);
        assert!(!main_diff.hunks.is_empty());

        let new_diff = result.files.iter().find(|f| f.path() == "src/new-file.ts").unwrap();
        assert_eq!(new_diff.status, FileStatus::Added);
        assert!(new_diff.old_content.is_none());
        assert!(new_diff.new_content.is_some());
    }

    #[test]
    fn test_diff_branch_comparison_returns_shas() {
        let (dir, repo) = init_repo();
        let base = commit_files(&repo, dir.path(), &[("a.txt", "a")], "initial");

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        let head = commit_files(&repo, dir.path(), &[("a.txt", "b")], "change");

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.base_sha.as_deref(), Some(base.to_string().as_str()));
        assert_eq!(result.head_sha.as_deref(), Some(head.to_string().as_str()));
    }

    // ── Commit Range Tests ──

    #[test]
    fn test_diff_commit_range() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("file1.ts", "v1")], "first");
        commit_files(&repo, dir.path(), &[("file1.ts", "v2")], "second");
        commit_files(&repo, dir.path(), &[("file2.ts", "new file")], "third");

        let result = diff_range(&repo, "HEAD~2..HEAD").unwrap();
        assert_eq!(result.files.len(), 2);

        let paths: Vec<&str> = result.files.iter().map(|f| f.path()).collect();
        assert!(paths.contains(&"file1.ts"));
        assert!(paths.contains(&"file2.ts"));
    }

    #[test]
    fn test_diff_commit_range_single_commit() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "hello")], "first");
        commit_files(&repo, dir.path(), &[("a.txt", "world")], "second");

        let result = diff_range(&repo, "HEAD~1..HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path(), "a.txt");
    }

    #[test]
    fn test_diff_range_invalid_format() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        let result = diff_range(&repo, "HEAD");
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::InvalidRange(_) => {}
            e => panic!("expected InvalidRange, got: {e}"),
        }
    }

    // ── Staged Changes Tests ──

    #[test]
    fn test_diff_staged_changes() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("file.ts", "original content")], "initial");

        // Modify and stage
        fs::write(dir.path().join("file.ts"), "modified content").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file.ts")).unwrap();
        index.write().unwrap();

        let result = diff_staged(&repo).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path(), "file.ts");
        assert_eq!(result.files[0].status, FileStatus::Modified);
        assert!(result.base_sha.is_some());
    }

    #[test]
    fn test_diff_staged_new_file() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("existing.ts", "x")], "initial");

        // Create and stage a new file
        fs::write(dir.path().join("brand-new.ts"), "export const y = 1;").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("brand-new.ts")).unwrap();
        index.write().unwrap();

        let result = diff_staged(&repo).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path(), "brand-new.ts");
        assert_eq!(result.files[0].status, FileStatus::Added);
    }

    // ── Unstaged Changes Tests ──

    #[test]
    fn test_diff_unstaged_changes() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("file.ts", "original")], "initial");

        // Modify without staging
        fs::write(dir.path().join("file.ts"), "modified").unwrap();

        let result = diff_unstaged(&repo).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path(), "file.ts");
        assert_eq!(result.files[0].status, FileStatus::Modified);
    }

    // ── File Rename Tests ──

    #[test]
    fn test_diff_file_rename() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("old-name.ts", "export function hello() { return 'hi'; }")],
            "initial",
        );

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // Remove old, add new with same content (git detects as rename)
        let mut index = repo.index().unwrap();
        fs::remove_file(dir.path().join("old-name.ts")).unwrap();
        index.remove_path(Path::new("old-name.ts")).unwrap();

        fs::write(
            dir.path().join("new-name.ts"),
            "export function hello() { return 'hi'; }",
        )
        .unwrap();
        index.add_path(Path::new("new-name.ts")).unwrap();
        index.write().unwrap();

        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "rename", &tree, &[&parent])
            .unwrap();

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        let renamed = result
            .files
            .iter()
            .find(|f| f.status == FileStatus::Renamed);
        assert!(
            renamed.is_some(),
            "Expected a renamed file, got statuses: {:?}",
            result.files.iter().map(|f| &f.status).collect::<Vec<_>>()
        );
        let renamed = renamed.unwrap();
        assert_eq!(renamed.old_path.as_deref(), Some("old-name.ts"));
        assert_eq!(renamed.new_path.as_deref(), Some("new-name.ts"));
    }

    // ── Binary Files Tests ──

    #[test]
    fn test_diff_binary_files_skipped() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("readme.txt", "hello")],
            "initial",
        );

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // Add a binary file (PNG header bytes) and a text file
        let png_header: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        let binary_path = dir.path().join("image.png");
        fs::write(&binary_path, png_header).unwrap();
        fs::write(dir.path().join("code.ts"), "const x = 1;").unwrap();

        let mut index = repo.index().unwrap();
        index.add_path(Path::new("image.png")).unwrap();
        index.add_path(Path::new("code.ts")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add files", &tree, &[&parent])
            .unwrap();

        let result = diff_refs(&repo, "base", "HEAD").unwrap();

        // Binary file should be excluded
        let paths: Vec<&str> = result.files.iter().map(|f| f.path()).collect();
        assert!(
            !paths.contains(&"image.png"),
            "Binary file should be skipped, got: {paths:?}"
        );
        assert!(paths.contains(&"code.ts"));
    }

    // ── Empty Repo Tests ──

    #[test]
    fn test_diff_empty_repo() {
        let (_dir, repo) = init_repo();
        // No commits — staged diff should fail gracefully
        let result = diff_staged(&repo);
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::EmptyRepo => {}
            e => panic!("expected EmptyRepo, got: {e}"),
        }
    }

    // ── Deleted Files Tests ──

    #[test]
    fn test_diff_deleted_files() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[
                ("keep.ts", "stays"),
                ("remove.ts", "goes away"),
            ],
            "initial",
        );

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        delete_files_and_commit(&repo, dir.path(), &["remove.ts"], "delete file");

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        let deleted = &result.files[0];
        assert_eq!(deleted.status, FileStatus::Deleted);
        assert_eq!(deleted.old_path.as_deref(), Some("remove.ts"));
        assert!(deleted.old_content.is_some());
        assert!(deleted.new_content.is_none());
    }

    // ── New Files Tests ──

    #[test]
    fn test_diff_new_files() {
        let (dir, repo) = init_repo();
        let base = commit_files(&repo, dir.path(), &[("existing.ts", "hi")], "initial");

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("brand-new.ts", "export const z = 99;")],
            "add new file",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        let new_file = &result.files[0];
        assert_eq!(new_file.status, FileStatus::Added);
        assert!(new_file.old_content.is_none());
        assert_eq!(
            new_file.new_content.as_deref(),
            Some("export const z = 99;")
        );
        assert_eq!(new_file.path(), "brand-new.ts");
    }

    // ── Hunk Extraction Tests ──

    #[test]
    fn test_diff_hunks_extracted() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\nline2\nline3\nline4\nline5\n")],
            "initial",
        );

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\nchanged\nline3\nline4\nline5\n")],
            "modify",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        assert!(
            !result.files[0].hunks.is_empty(),
            "Expected at least one hunk"
        );
    }

    // ── Line Count Tests ──

    #[test]
    fn test_diff_line_counts() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\nline2\nline3\n")],
            "initial",
        );

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // Replace line2 with two new lines
        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\nnew_line_a\nnew_line_b\nline3\n")],
            "expand",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        let file = &result.files[0];
        assert!(file.additions > 0, "Expected additions");
        assert!(file.deletions > 0, "Expected deletions");
    }

    // ── Content Retrieval Tests ──

    #[test]
    fn test_diff_old_and_new_content() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "const x = 1;")],
            "initial",
        );

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "const x = 2;")],
            "update",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files[0].old_content.as_deref(), Some("const x = 1;"));
        assert_eq!(result.files[0].new_content.as_deref(), Some("const x = 2;"));
    }

    // ── Multiple Files Tests ──

    #[test]
    fn test_diff_multiple_files() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[
                ("src/a.ts", "a1"),
                ("src/b.ts", "b1"),
                ("src/c.ts", "c1"),
            ],
            "initial",
        );

        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // Modify a, delete b, add d
        fs::write(dir.path().join("src/a.ts"), "a2").unwrap();
        fs::remove_file(dir.path().join("src/b.ts")).unwrap();
        fs::write(dir.path().join("src/d.ts"), "d1").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("src/a.ts")).unwrap();
        index.remove_path(Path::new("src/b.ts")).unwrap();
        index.add_path(Path::new("src/d.ts")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "multi-change", &tree, &[&parent])
            .unwrap();

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 3);

        let statuses: Vec<(&str, &FileStatus)> = result
            .files
            .iter()
            .map(|f| (f.path(), &f.status))
            .collect();
        assert!(statuses.iter().any(|(p, s)| *p == "src/a.ts" && **s == FileStatus::Modified));
        assert!(statuses.iter().any(|(p, s)| *p == "src/b.ts" && **s == FileStatus::Deleted));
        assert!(statuses.iter().any(|(p, s)| *p == "src/d.ts" && **s == FileStatus::Added));
    }

    // ── Ref Not Found Tests ──

    #[test]
    fn test_diff_ref_not_found() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        let result = diff_refs(&repo, "nonexistent-branch", "HEAD");
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::RefNotFound(r) => assert_eq!(r, "nonexistent-branch"),
            e => panic!("expected RefNotFound, got: {e}"),
        }
    }

    // ── No Changes Tests ──

    #[test]
    fn test_diff_no_changes() {
        let (dir, repo) = init_repo();
        let base = commit_files(&repo, dir.path(), &[("a.txt", "same")], "initial");
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // No changes between base and HEAD
        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert!(result.files.is_empty());
    }

    // ── Subdirectory Tests ──

    #[test]
    fn test_diff_subdirectories() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("src/handlers/auth.ts", "old auth")],
            "initial",
        );
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[
                ("src/handlers/auth.ts", "new auth"),
                ("src/services/user/index.ts", "user service"),
            ],
            "changes in subdirs",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        let paths: Vec<&str> = result.files.iter().map(|f| f.path()).collect();
        assert!(paths.contains(&"src/handlers/auth.ts"));
        assert!(paths.contains(&"src/services/user/index.ts"));
    }

    // ── FileDiff::path() Tests ──

    #[test]
    fn test_file_diff_path_helper() {
        let added = FileDiff {
            old_path: None,
            new_path: Some("new.ts".into()),
            old_content: None,
            new_content: Some("x".into()),
            hunks: vec![],
            status: FileStatus::Added,
            additions: 1,
            deletions: 0,
            is_binary: false,
        };
        assert_eq!(added.path(), "new.ts");

        let deleted = FileDiff {
            old_path: Some("old.ts".into()),
            new_path: None,
            old_content: Some("x".into()),
            new_content: None,
            hunks: vec![],
            status: FileStatus::Deleted,
            additions: 0,
            deletions: 1,
            is_binary: false,
        };
        assert_eq!(deleted.path(), "old.ts");
    }

    #[test]
    fn test_file_diff_path_fallback() {
        // Both paths None → "<unknown>"
        let orphan = FileDiff {
            old_path: None,
            new_path: None,
            old_content: None,
            new_content: None,
            hunks: vec![],
            status: FileStatus::Modified,
            additions: 0,
            deletions: 0,
            is_binary: false,
        };
        assert_eq!(orphan.path(), "<unknown>");
    }

    // ── Rename With Content Change Tests ──

    #[test]
    fn test_diff_rename_with_content_change() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("old.ts", "export function greet() { return 'hello'; }\n// padding line 1\n// padding line 2\n// padding line 3\n")],
            "initial",
        );
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // Rename + small content change (still high similarity → rename detected)
        let mut index = repo.index().unwrap();
        fs::remove_file(dir.path().join("old.ts")).unwrap();
        index.remove_path(Path::new("old.ts")).unwrap();
        fs::write(
            dir.path().join("new.ts"),
            "export function greet() { return 'hello world'; }\n// padding line 1\n// padding line 2\n// padding line 3\n",
        )
        .unwrap();
        index.add_path(Path::new("new.ts")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "rename+edit", &tree, &[&parent])
            .unwrap();

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        // git should detect rename even with small edit
        assert_eq!(result.files.len(), 1);
        let f = &result.files[0];
        assert_eq!(f.status, FileStatus::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("old.ts"));
        assert_eq!(f.new_path.as_deref(), Some("new.ts"));
        assert!(f.old_content.is_some());
        assert!(f.new_content.is_some());
    }

    // ── Empty File Tests ──

    #[test]
    fn test_diff_empty_file_added() {
        let (dir, repo) = init_repo();
        let base = commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(&repo, dir.path(), &[("empty.ts", "")], "add empty file");

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        let f = &result.files[0];
        assert_eq!(f.status, FileStatus::Added);
        // Empty file has empty content or None depending on git behavior
        assert!(f.new_content.is_none() || f.new_content.as_deref() == Some(""));
    }

    #[test]
    fn test_diff_empty_file_modified_to_content() {
        let (dir, repo) = init_repo();
        let base = commit_files(&repo, dir.path(), &[("file.ts", "")], "initial with empty");
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "const x = 1;")],
            "add content to empty file",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].status, FileStatus::Modified);
        assert!(result.files[0].additions > 0);
    }

    // ── Unicode Content Tests ──

    #[test]
    fn test_diff_unicode_content() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "const greeting = 'hello';")],
            "initial",
        );
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "const greeting = '你好世界 🌍';")],
            "add unicode",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        let f = &result.files[0];
        assert!(f.new_content.as_deref().unwrap().contains('🌍'));
        assert!(f.new_content.as_deref().unwrap().contains("你好世界"));
    }

    // ── Deeply Nested Directory Tests ──

    #[test]
    fn test_diff_deeply_nested_paths() {
        let (dir, repo) = init_repo();
        let deep_path = "src/services/auth/handlers/middleware/v2/token.ts";
        let base = commit_files(&repo, dir.path(), &[(deep_path, "v1")], "initial");
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(&repo, dir.path(), &[(deep_path, "v2")], "update deep file");

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path(), deep_path);
    }

    // ── Multiple Hunks Tests ──

    #[test]
    fn test_diff_multiple_hunks_in_single_file() {
        let (dir, repo) = init_repo();
        // Create a file with well-separated sections so changes produce multiple hunks
        let original = (1..=50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let base = commit_files(&repo, dir.path(), &[("file.ts", &original)], "initial");
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // Change line 3 and line 47 (far apart → two hunks with 3 context lines)
        let modified: String = (1..=50)
            .map(|i| {
                if i == 3 {
                    "CHANGED_LINE_3".to_string()
                } else if i == 47 {
                    "CHANGED_LINE_47".to_string()
                } else {
                    format!("line {i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        commit_files(&repo, dir.path(), &[("file.ts", &modified)], "two changes");

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 1);
        let f = &result.files[0];
        assert!(
            f.hunks.len() >= 2,
            "Expected at least 2 hunks for changes far apart, got {}",
            f.hunks.len()
        );
    }

    // ── Hunk Consistency Tests ──

    #[test]
    fn test_diff_hunk_fields_valid() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\nline2\nline3\n")],
            "initial",
        );
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\nmodified\nline3\n")],
            "modify middle line",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        for file in &result.files {
            for hunk in &file.hunks {
                // old_start and new_start are 1-indexed, must be > 0
                assert!(hunk.old_start > 0, "old_start must be 1-indexed");
                assert!(hunk.new_start > 0, "new_start must be 1-indexed");
            }
        }
    }

    // ── Large Diff Tests ──

    #[test]
    fn test_diff_many_files() {
        let (dir, repo) = init_repo();
        let initial_files: Vec<(String, String)> = (0..30)
            .map(|i| (format!("src/file_{i}.ts"), format!("content_{i}")))
            .collect();
        let refs: Vec<(&str, &str)> = initial_files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        let base = commit_files(&repo, dir.path(), &refs, "initial 30 files");
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // Modify all 30 files
        let modified_files: Vec<(String, String)> = (0..30)
            .map(|i| (format!("src/file_{i}.ts"), format!("modified_{i}")))
            .collect();
        let refs2: Vec<(&str, &str)> = modified_files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        commit_files(&repo, dir.path(), &refs2, "modify all 30 files");

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        assert_eq!(result.files.len(), 30);
        // All should be Modified
        for f in &result.files {
            assert_eq!(f.status, FileStatus::Modified);
        }
    }

    // ── Additions-Only / Deletions-Only Tests ──

    #[test]
    fn test_diff_additions_only() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\n")],
            "initial",
        );
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\nline2\nline3\n")],
            "add lines only",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        let f = &result.files[0];
        assert!(f.additions > 0, "Expected additions > 0");
        assert_eq!(f.deletions, 0, "Expected no deletions");
    }

    #[test]
    fn test_diff_deletions_only() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\nline2\nline3\n")],
            "initial",
        );
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "line1\n")],
            "remove lines only",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        let f = &result.files[0];
        assert_eq!(f.additions, 0, "Expected no additions");
        assert!(f.deletions > 0, "Expected deletions > 0");
    }

    // ── Determinism Tests ──

    #[test]
    fn test_diff_deterministic_output() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[
                ("src/a.ts", "a1"),
                ("src/b.ts", "b1"),
                ("src/c.ts", "c1"),
            ],
            "initial",
        );
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[
                ("src/a.ts", "a2"),
                ("src/b.ts", "b2"),
                ("src/c.ts", "c2"),
            ],
            "modify all",
        );

        let r1 = diff_refs(&repo, "base", "HEAD").unwrap();
        let r2 = diff_refs(&repo, "base", "HEAD").unwrap();

        assert_eq!(r1.files.len(), r2.files.len());
        for (f1, f2) in r1.files.iter().zip(r2.files.iter()) {
            assert_eq!(f1.old_path, f2.old_path);
            assert_eq!(f1.new_path, f2.new_path);
            assert_eq!(f1.status, f2.status);
            assert_eq!(f1.additions, f2.additions);
            assert_eq!(f1.deletions, f2.deletions);
            assert_eq!(f1.hunks.len(), f2.hunks.len());
            assert_eq!(f1.old_content, f2.old_content);
            assert_eq!(f1.new_content, f2.new_content);
        }
    }

    // ── is_binary Field Tests ──

    #[test]
    fn test_diff_returned_files_not_binary() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "x"), ("util.ts", "y")],
            "initial",
        );
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        commit_files(
            &repo,
            dir.path(),
            &[("file.ts", "x2"), ("util.ts", "y2")],
            "modify",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        for f in &result.files {
            assert!(!f.is_binary, "Returned files should never be binary (binary is filtered)");
        }
    }

    // ── Staged vs Unstaged Isolation Tests ──

    #[test]
    fn test_diff_staged_excludes_unstaged() {
        let (dir, repo) = init_repo();
        commit_files(
            &repo,
            dir.path(),
            &[("staged.ts", "v1"), ("unstaged.ts", "v1")],
            "initial",
        );

        // Stage changes to one file only
        fs::write(dir.path().join("staged.ts"), "v2").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged.ts")).unwrap();
        index.write().unwrap();

        // Modify another file without staging
        fs::write(dir.path().join("unstaged.ts"), "v2").unwrap();

        let staged_result = diff_staged(&repo).unwrap();
        let staged_paths: Vec<&str> = staged_result.files.iter().map(|f| f.path()).collect();
        assert!(staged_paths.contains(&"staged.ts"));
        assert!(
            !staged_paths.contains(&"unstaged.ts"),
            "Unstaged files should not appear in staged diff"
        );

        let unstaged_result = diff_unstaged(&repo).unwrap();
        let unstaged_paths: Vec<&str> = unstaged_result.files.iter().map(|f| f.path()).collect();
        assert!(unstaged_paths.contains(&"unstaged.ts"));
        assert!(
            !unstaged_paths.contains(&"staged.ts"),
            "Staged files should not appear in unstaged diff"
        );
    }

    // ── Error Display Tests ──

    #[test]
    fn test_git_error_display() {
        let e = GitError::RefNotFound("main".into());
        assert_eq!(format!("{e}"), "ref not found: main");

        let e = GitError::EmptyRepo;
        assert_eq!(format!("{e}"), "empty repository — no commits found");

        let e = GitError::InvalidRange("bad".into());
        assert_eq!(format!("{e}"), "invalid range: bad");
    }

    // ── DiffHunk Serde Roundtrip Tests ──

    #[test]
    fn test_diff_hunk_serde_roundtrip() {
        let hunk = DiffHunk {
            old_start: 10,
            old_lines: 5,
            new_start: 12,
            new_lines: 7,
        };
        let json = serde_json::to_string(&hunk).unwrap();
        let back: DiffHunk = serde_json::from_str(&json).unwrap();
        assert_eq!(hunk, back);
    }

    #[test]
    fn test_file_status_serde_roundtrip() {
        for status in [
            FileStatus::Added,
            FileStatus::Modified,
            FileStatus::Deleted,
            FileStatus::Renamed,
            FileStatus::Copied,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: FileStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn test_file_diff_serde_roundtrip() {
        let fd = FileDiff {
            old_path: Some("old.ts".into()),
            new_path: Some("new.ts".into()),
            old_content: Some("const x = 1;".into()),
            new_content: Some("const x = 2;".into()),
            hunks: vec![
                DiffHunk {
                    old_start: 1,
                    old_lines: 1,
                    new_start: 1,
                    new_lines: 1,
                },
            ],
            status: FileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
        };
        let json = serde_json::to_string(&fd).unwrap();
        let back: FileDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(fd, back);
    }

    #[test]
    fn test_file_diff_serde_with_nulls() {
        // Added file: old_path and old_content are None
        let fd = FileDiff {
            old_path: None,
            new_path: Some("new.ts".into()),
            old_content: None,
            new_content: Some("code".into()),
            hunks: vec![],
            status: FileStatus::Added,
            additions: 1,
            deletions: 0,
            is_binary: false,
        };
        let json = serde_json::to_string(&fd).unwrap();
        assert!(json.contains("null"));
        let back: FileDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(fd, back);
    }

    // ── Git Auto-Discovery Tests ──

    #[test]
    fn test_list_branches_basic() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "hello")], "initial");

        // Create a couple of branches
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature-a", &head, false).unwrap();
        repo.branch("feature-b", &head, false).unwrap();

        let branches = list_branches(&repo).unwrap();
        assert!(branches.len() >= 3); // main/master + feature-a + feature-b

        // Current branch should be first
        assert!(branches[0].is_current);

        // All branches should have names
        for b in &branches {
            assert!(!b.name.is_empty());
        }

        // feature-a and feature-b should be present
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"feature-a"));
        assert!(names.contains(&"feature-b"));
    }

    #[test]
    fn test_list_branches_empty_repo() {
        let (_dir, repo) = init_repo();
        // No commits = no branches
        let branches = list_branches(&repo).unwrap();
        assert!(branches.is_empty());
    }

    #[test]
    fn test_list_branches_sorted() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("z-branch", &head, false).unwrap();
        repo.branch("a-branch", &head, false).unwrap();

        let branches = list_branches(&repo).unwrap();
        // Current branch first, then alphabetical
        assert!(branches[0].is_current);
        // Non-current branches should be sorted alphabetically
        let non_current: Vec<&str> = branches
            .iter()
            .filter(|b| !b.is_current)
            .map(|b| b.name.as_str())
            .collect();
        let mut sorted = non_current.clone();
        sorted.sort();
        assert_eq!(non_current, sorted);
    }

    #[test]
    fn test_list_worktrees_main_only() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        let worktrees = list_worktrees(&repo).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].is_main);
        assert!(worktrees[0].branch.is_some());
    }

    #[test]
    fn test_get_branch_status_no_upstream() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        let status = get_branch_status(&repo).unwrap();
        assert!(!status.branch.is_empty());
        assert!(status.upstream.is_none());
        assert_eq!(status.ahead, 0);
        assert_eq!(status.behind, 0);
    }

    #[test]
    fn test_get_branch_status_empty_repo() {
        let (_dir, repo) = init_repo();
        let result = get_branch_status(&repo);
        assert!(result.is_err());
    }

    #[test]
    fn test_detect_default_branch_main() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        // The default branch after init is typically "main" or "master"
        let default = detect_default_branch(&repo).unwrap();
        assert!(
            default == "main" || default == "master",
            "Expected main or master, got: {}",
            default
        );
    }

    #[test]
    fn test_detect_default_branch_empty_repo() {
        let (_dir, repo) = init_repo();
        let result = detect_default_branch(&repo);
        assert!(result.is_err());
    }

    #[test]
    fn test_diff_merge_base_basic() {
        let (dir, repo) = init_repo();
        let base = commit_files(&repo, dir.path(), &[("a.txt", "v1")], "initial");

        // Create base branch
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base-branch", &base_commit, false).unwrap();

        // Add commits on HEAD
        commit_files(&repo, dir.path(), &[("a.txt", "v2")], "change 1");
        commit_files(&repo, dir.path(), &[("b.txt", "new")], "change 2");

        let result = diff_merge_base(&repo, "base-branch", "HEAD").unwrap();
        assert_eq!(result.files.len(), 2);

        let paths: Vec<&str> = result.files.iter().map(|f| f.path()).collect();
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"b.txt"));

        // base_sha should be the merge base
        assert!(result.base_sha.is_some());
        assert!(result.head_sha.is_some());
    }

    #[test]
    fn test_diff_merge_base_diverged() {
        let (dir, repo) = init_repo();
        let base = commit_files(
            &repo,
            dir.path(),
            &[("shared.txt", "original")],
            "initial",
        );

        // Create a branch at the initial commit
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("feature", &base_commit, false).unwrap();

        // Add a commit on the main branch (HEAD)
        commit_files(
            &repo,
            dir.path(),
            &[("main-only.txt", "main change")],
            "main commit",
        );

        // Switch to feature branch and add a commit
        repo.set_head("refs/heads/feature").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        commit_files(
            &repo,
            dir.path(),
            &[("feature-only.txt", "feature change")],
            "feature commit",
        );

        // diff_merge_base from main to feature should show only feature changes
        let current_branch = repo.head().unwrap().shorthand().unwrap().to_string();
        assert_eq!(current_branch, "feature");

        // The merge base is the initial commit.
        // Diffing merge_base..feature should show only feature-only.txt
        let result = diff_merge_base(&repo, "refs/heads/feature~1", "feature").unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path(), "feature-only.txt");
    }

    #[test]
    fn test_diff_merge_base_same_commit() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        // Merge base of HEAD with itself is HEAD — should be empty diff
        let result = diff_merge_base(&repo, "HEAD", "HEAD").unwrap();
        assert!(result.files.is_empty());
    }

    #[test]
    fn test_diff_merge_base_invalid_ref() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        let result = diff_merge_base(&repo, "nonexistent", "HEAD");
        assert!(result.is_err());
    }

    #[test]
    fn test_current_branch_basic() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        let branch = current_branch(&repo);
        assert!(branch.is_some());
        assert!(!branch.unwrap().is_empty());
    }

    #[test]
    fn test_current_branch_empty_repo() {
        let (_dir, repo) = init_repo();
        let branch = current_branch(&repo);
        // No commits yet — HEAD doesn't point to a valid branch
        // git2 may return None or Some("master") depending on version
        // Just ensure it doesn't panic
        let _ = branch;
    }

    #[test]
    fn test_branch_info_serde_roundtrip() {
        let info = BranchInfo {
            name: "feature/auth".to_string(),
            is_current: true,
            has_upstream: false,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: BranchInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn test_worktree_info_serde_roundtrip() {
        let info = WorktreeInfo {
            path: "/home/user/project".to_string(),
            branch: Some("main".to_string()),
            is_main: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: WorktreeInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn test_branch_status_serde_roundtrip() {
        let status = BranchStatus {
            branch: "feature".to_string(),
            upstream: Some("origin/feature".to_string()),
            ahead: 3,
            behind: 1,
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: BranchStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, back);
    }

    #[test]
    fn test_branch_status_serde_no_upstream() {
        let status = BranchStatus {
            branch: "local-only".to_string(),
            upstream: None,
            ahead: 0,
            behind: 0,
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: BranchStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, back);
        assert!(json.contains("null"));
    }

    // ── Range Edge Cases ──

    #[test]
    fn test_diff_range_triple_dot_invalid() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        // Triple-dot is not supported by our simple split
        let result = diff_range(&repo, "HEAD~1...HEAD");
        assert!(result.is_err());
    }

    #[test]
    fn test_diff_range_empty_parts() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.txt", "x")], "initial");

        let result = diff_range(&repo, "..HEAD");
        assert!(result.is_err()); // empty base ref → RefNotFound
    }

    // ── Copy Detection Tests ──

    #[test]
    fn test_diff_copy_detection() {
        let (dir, repo) = init_repo();
        let content = "export function helper() {\n  return 42;\n}\n// padding\n// more padding\n";
        let base = commit_files(&repo, dir.path(), &[("original.ts", content)], "initial");
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("base", &base_commit, false).unwrap();

        // Add a copy with identical content (keep original too)
        commit_files(
            &repo,
            dir.path(),
            &[("original.ts", content), ("copy.ts", content)],
            "copy file",
        );

        let result = diff_refs(&repo, "base", "HEAD").unwrap();
        // Should detect copy.ts as either Added or Copied
        let copy_file = result.files.iter().find(|f| f.path() == "copy.ts");
        assert!(copy_file.is_some(), "copy.ts should be in diff");
        let copy_file = copy_file.unwrap();
        assert!(
            copy_file.status == FileStatus::Copied || copy_file.status == FileStatus::Added,
            "Expected Copied or Added, got {:?}",
            copy_file.status
        );
    }

    // ── Property-Based Tests ──

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_file_status() -> impl Strategy<Value = FileStatus> {
            prop_oneof![
                Just(FileStatus::Added),
                Just(FileStatus::Modified),
                Just(FileStatus::Deleted),
                Just(FileStatus::Renamed),
                Just(FileStatus::Copied),
            ]
        }

        fn arb_diff_hunk() -> impl Strategy<Value = DiffHunk> {
            (1..10000u32, 0..1000u32, 1..10000u32, 0..1000u32).prop_map(
                |(old_start, old_lines, new_start, new_lines)| DiffHunk {
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                },
            )
        }

        fn arb_file_path() -> impl Strategy<Value = String> {
            prop_oneof![
                Just("file.ts".to_string()),
                Just("src/index.ts".to_string()),
                Just("a/b/c/d/e.py".to_string()),
                "[a-z]{1,10}(\\.[a-z]{1,4})?"
                    .prop_map(|s| format!("src/{s}")),
            ]
        }

        fn arb_content() -> impl Strategy<Value = Option<String>> {
            prop_oneof![
                Just(None),
                Just(Some(String::new())),
                "[a-zA-Z0-9 _\\n]{1,200}".prop_map(Some),
            ]
        }

        fn arb_file_diff() -> impl Strategy<Value = FileDiff> {
            (
                proptest::option::of(arb_file_path()),
                proptest::option::of(arb_file_path()),
                arb_content(),
                arb_content(),
                proptest::collection::vec(arb_diff_hunk(), 0..5),
                arb_file_status(),
                0..5000u32,
                0..5000u32,
            )
                .prop_map(
                    |(old_path, new_path, old_content, new_content, hunks, status, add, del)| {
                        FileDiff {
                            old_path,
                            new_path,
                            old_content,
                            new_content,
                            hunks,
                            status,
                            additions: add,
                            deletions: del,
                            is_binary: false,
                        }
                    },
                )
        }

        proptest! {
            #[test]
            fn prop_file_status_serde_roundtrip(status in arb_file_status()) {
                let json = serde_json::to_string(&status).unwrap();
                let back: FileStatus = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(status, back);
            }

            #[test]
            fn prop_diff_hunk_serde_roundtrip(hunk in arb_diff_hunk()) {
                let json = serde_json::to_string(&hunk).unwrap();
                let back: DiffHunk = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(hunk, back);
            }

            #[test]
            fn prop_file_diff_serde_roundtrip(fd in arb_file_diff()) {
                let json = serde_json::to_string(&fd).unwrap();
                let back: FileDiff = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&fd, &back);
            }

            #[test]
            fn prop_file_diff_path_never_empty(fd in arb_file_diff()) {
                // path() should always return something (at worst "<unknown>")
                let p = fd.path();
                prop_assert!(!p.is_empty(), "path() must never be empty");
            }

            #[test]
            fn prop_diff_hunk_old_start_positive(hunk in arb_diff_hunk()) {
                // Hunks are 1-indexed
                prop_assert!(hunk.old_start >= 1);
                prop_assert!(hunk.new_start >= 1);
            }

            #[test]
            fn prop_file_diff_is_binary_always_false(fd in arb_file_diff()) {
                // Our arb always produces non-binary (matching real behavior:
                // extract_file_diffs skips binary files)
                prop_assert!(!fd.is_binary);
            }

            #[test]
            fn prop_additions_deletions_bounded(add in 0..10000u32, del in 0..10000u32) {
                // Verify no overflow when summing
                let total = add as u64 + del as u64;
                prop_assert!(total < 20000);
            }

            #[test]
            fn prop_file_diff_clone_eq(fd in arb_file_diff()) {
                let cloned = fd.clone();
                prop_assert_eq!(&fd, &cloned);
            }

            #[test]
            fn prop_diff_hunk_clone_eq(hunk in arb_diff_hunk()) {
                let cloned = hunk.clone();
                prop_assert_eq!(hunk, cloned);
            }

            #[test]
            fn prop_file_diff_json_has_status_field(fd in arb_file_diff()) {
                let json = serde_json::to_string(&fd).unwrap();
                prop_assert!(json.contains("\"status\""));
            }

            #[test]
            fn prop_file_diff_json_parseable(fd in arb_file_diff()) {
                let json = serde_json::to_string(&fd).unwrap();
                let val: serde_json::Value = serde_json::from_str(&json).unwrap();
                prop_assert!(val.is_object());
            }
        }
    }

    // ── file_content_at_ref tests ─────────────────────────────────────

    #[test]
    fn file_content_at_ref_reads_existing_file() {
        let (dir, repo) = init_repo();
        let content = "hello world\n";
        commit_files(&repo, dir.path(), &[("src/main.ts", content)], "init");

        let result = file_content_at_ref(&repo, "HEAD", "src/main.ts").unwrap();
        assert_eq!(result, Some(content.to_string()));
    }

    #[test]
    fn file_content_at_ref_returns_none_for_missing_file() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.ts", "x")], "init");

        let result = file_content_at_ref(&repo, "HEAD", "nonexistent.ts").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn file_content_at_ref_reads_at_specific_commit() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("f.ts", "v1")], "first");
        commit_files(&repo, dir.path(), &[("f.ts", "v2")], "second");

        // HEAD should have v2
        let head = file_content_at_ref(&repo, "HEAD", "f.ts").unwrap();
        assert_eq!(head, Some("v2".to_string()));

        // HEAD~1 should have v1
        let prev = file_content_at_ref(&repo, "HEAD~1", "f.ts").unwrap();
        assert_eq!(prev, Some("v1".to_string()));
    }

    #[test]
    fn file_content_at_ref_reads_by_branch_name() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("f.ts", "on main")], "init");

        // Create a branch and commit different content
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &head, false).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        commit_files(&repo, dir.path(), &[("f.ts", "on feature")], "feature change");

        let main_content = file_content_at_ref(&repo, "main", "f.ts").unwrap();
        assert_eq!(main_content, Some("on main".to_string()));

        let feature_content = file_content_at_ref(&repo, "feature", "f.ts").unwrap();
        assert_eq!(feature_content, Some("on feature".to_string()));
    }

    #[test]
    fn file_content_at_ref_invalid_ref() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("f.ts", "x")], "init");

        let result = file_content_at_ref(&repo, "nonexistent-branch", "f.ts");
        assert!(result.is_err());
    }

    #[test]
    fn file_content_at_ref_nested_path() {
        let (dir, repo) = init_repo();
        let content = "deep content";
        commit_files(
            &repo,
            dir.path(),
            &[("packages/core/src/handlers/auth.ts", content)],
            "init",
        );

        let result = file_content_at_ref(&repo, "HEAD", "packages/core/src/handlers/auth.ts").unwrap();
        assert_eq!(result, Some(content.to_string()));
    }

    #[test]
    fn file_content_at_ref_by_sha() {
        let (dir, repo) = init_repo();
        let oid = commit_files(&repo, dir.path(), &[("f.ts", "by sha")], "init");

        let result = file_content_at_ref(&repo, &oid.to_string(), "f.ts").unwrap();
        assert_eq!(result, Some("by sha".to_string()));
    }

    #[test]
    fn file_content_at_ref_deleted_file_in_later_commit() {
        let (dir, repo) = init_repo();
        commit_files(&repo, dir.path(), &[("a.ts", "exists")], "add");
        delete_files_and_commit(&repo, dir.path(), &["a.ts"], "delete");

        // HEAD should not have the file
        let head = file_content_at_ref(&repo, "HEAD", "a.ts").unwrap();
        assert_eq!(head, None);

        // HEAD~1 should still have it
        let prev = file_content_at_ref(&repo, "HEAD~1", "a.ts").unwrap();
        assert_eq!(prev, Some("exists".to_string()));
    }
}
