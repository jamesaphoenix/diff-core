use git2::{Delta, DiffOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
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
        let delta = diff.deltas().nth(delta_idx).unwrap();

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
}
