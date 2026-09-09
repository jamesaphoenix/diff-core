//! Parse pull-request / merge-request URLs and resolve them to local git refs.
//!
//! The desktop app's repository field and the CLI's `--repo` flag both accept a
//! PR/MR URL in place of a filesystem path. Resolution clones (or reuses) a
//! cached checkout under `~/.diffcore/cache/repos` and fetches the provider's
//! pull-request ref namespace, so no API token is needed for public repos.
//!
//! See `docs/pr-url-providers.md` for the provider table — which forges are
//! supported, their URL shapes and ref namespaces, and which ones publish no
//! PR refs at all. Keep that document and `Provider::ref_glob` in step.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};
use url::Url;

/// How long a lock file may sit before it is assumed to belong to a process
/// that died mid-clone. Longer than a slow monorepo clone, short enough that a
/// Ctrl-C during one does not wedge the cache for the rest of the day.
const LOCK_STALE: Duration = Duration::from_secs(1800);

#[derive(Debug, thiserror::Error)]
pub enum PrUrlError {
    #[error(
        "{0} does not publish pull-request refs over git — clone the repository and pick the source/target branches manually"
    )]
    NoGitRefs(&'static str),
    #[error("{0} #{1} not found on {2} (private repo? try `git ls-remote {2}`)")]
    NotFound(&'static str, u64, String),
    #[error(
        "{0} #{1} resolves to an empty diff: it was fast-forward merged (leaving no merge commit to recover the base branch from) or contains no commits. Open the repository directly and pick the branches by hand."
    )]
    EmptyDiff(&'static str, u64),
    #[error(
        "another diffcore process is using this cached clone. If none is running, delete {0}"
    )]
    CacheBusy(String),
    #[error("`git {0}` failed: {1}")]
    Git(String, String),
    #[error("cannot locate a cache directory — set DIFFCORE_REPO_CACHE_DIR or HOME")]
    NoCacheDir,
    #[error("io error running git: {0}")]
    Io(#[from] std::io::Error),
}

/// Forge families that share a pull-request URL shape and ref namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// github.com and GitHub Enterprise Server.
    GitHub,
    /// gitlab.com and self-managed GitLab (CE/EE).
    GitLab,
    /// Gitea, Forgejo, Codeberg, Gogs, Gitee — same `/pulls/{n}` shape.
    Gitea,
    /// Pagure (pagure.io, Fedora/CentOS infrastructure).
    Pagure,
    /// Bitbucket Data Center / Server (on-prem).
    BitbucketServer,
    /// Azure DevOps Services and Azure DevOps Server / TFS.
    AzureDevOps,
    /// Gerrit changes (the review unit, equivalent to a PR).
    Gerrit,
    /// bitbucket.org (Cloud) — no PR refs over git.
    BitbucketCloud,
    /// SourceForge / Apache Allura — MR heads live in a downstream fork,
    /// reachable only through the Allura REST API.
    SourceForge,
    /// Launchpad merge proposals — no PR refs over git.
    Launchpad,
    /// AWS CodeCommit — no PR refs; SigV4-signed API only.
    CodeCommit,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::GitHub => "GitHub",
            Provider::GitLab => "GitLab",
            Provider::Gitea => "Gitea/Forgejo",
            Provider::Pagure => "Pagure",
            Provider::BitbucketServer => "Bitbucket Data Center",
            Provider::BitbucketCloud => "Bitbucket Cloud",
            Provider::AzureDevOps => "Azure DevOps",
            Provider::Gerrit => "Gerrit",
            Provider::SourceForge => "SourceForge",
            Provider::Launchpad => "Launchpad",
            Provider::CodeCommit => "AWS CodeCommit",
        }
    }

    /// What the provider calls a change request, for user-facing messages.
    pub fn unit(self) -> &'static str {
        match self {
            Provider::GitLab | Provider::SourceForge => "merge request",
            Provider::Gerrit => "change",
            Provider::Launchpad => "merge proposal",
            _ => "pull request",
        }
    }
}

/// A parsed pull/merge request URL.
#[derive(Debug, Clone)]
pub struct PrUrl {
    pub provider: Provider,
    /// Host, including port when non-default (e.g. `gitea.internal:3000`).
    pub host: String,
    /// Namespace/owner path (`rust-lang`, `gitlab-org/security`, `myorg/myproject`).
    /// Empty for forges that allow top-level projects (Pagure, root-mounted Gerrit).
    pub owner: String,
    pub repo: String,
    /// PR / MR / change number.
    pub number: u64,
    /// Gerrit patchset, when pinned in the URL.
    pub patchset: Option<u64>,
    /// HTTPS clone URL for the repository.
    pub clone_url: String,
}

/// Parse a PR/MR URL. Returns `None` for anything that is not an http(s) URL
/// matching a known forge layout — callers treat that as a filesystem path.
pub fn parse(input: &str) -> Option<PrUrl> {
    let raw = input.trim().trim_end_matches('/');
    let lower = raw.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    let mut url = Url::parse(raw).ok()?;
    url.set_query(None);
    url.set_fragment(None);

    let scheme = url.scheme().to_ascii_lowercase();
    let mut host = url.host_str()?.to_ascii_lowercase();
    if let Some(port) = url.port() {
        host = format!("{host}:{port}");
    }
    let segs: Vec<String> = url
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if segs.is_empty() {
        return None;
    }

    // api.github.com/repos/{owner}/{repo}/pulls/{n} — rewrite to the web shape.
    let (host, segs) =
        if host == "api.github.com" && segs.first().map(String::as_str) == Some("repos") {
            ("github.com".to_string(), segs[1..].to_vec())
        } else {
            (host, segs)
        };

    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    let base = format!("{scheme}://{host}");

    // Azure DevOps: .../{org}/{project}/_git/{repo}/pullrequest/{n}
    if let Some(i) = s.iter().position(|x| *x == "_git") {
        let repo = s.get(i + 1)?;
        if s.get(i + 2).copied() != Some("pullrequest") {
            return None;
        }
        let number = parse_number(s.get(i + 3)?)?;
        let owner = s[..i].join("/");
        if owner.is_empty() {
            return None;
        }
        return Some(PrUrl {
            provider: Provider::AzureDevOps,
            clone_url: format!("{base}/{owner}/_git/{repo}"),
            host,
            owner,
            repo: (*repo).to_string(),
            number,
            patchset: None,
        });
    }

    // Bitbucket Data Center: [ctx]/projects/{KEY}/repos/{repo}/pull-requests/{n}
    if let Some(i) = s.iter().position(|x| *x == "projects") {
        if s.get(i + 2).copied() == Some("repos") && s.get(i + 4).copied() == Some("pull-requests")
        {
            let key = s.get(i + 1)?;
            let repo = s.get(i + 3)?;
            let number = parse_number(s.get(i + 5)?)?;
            let ctx = s[..i].join("/");
            let prefix = if ctx.is_empty() {
                base.clone()
            } else {
                format!("{base}/{ctx}")
            };
            return Some(PrUrl {
                provider: Provider::BitbucketServer,
                clone_url: format!("{prefix}/scm/{key}/{repo}.git"),
                host,
                owner: (*key).to_string(),
                repo: (*repo).to_string(),
                number,
                patchset: None,
            });
        }
    }

    // Gerrit: [ctx]/c/{project...}/+/{change}[/{patchset}]. Most self-hosted
    // instances mount the UI under /r/, so anchor on `+` and walk back to `c`
    // rather than requiring `c` to be the first segment.
    if let Some(plus) = s.iter().position(|x| *x == "+") {
        if let Some(c) = s[..plus].iter().rposition(|x| *x == "c") {
            let project = s[c + 1..plus].join("/");
            if !project.is_empty() {
                let number = parse_number(s.get(plus + 1)?)?;
                let patchset = s.get(plus + 2).and_then(|p| parse_number(p));
                let repo = project.rsplit('/').next().unwrap_or(&project).to_string();
                let owner = project
                    .rsplit_once('/')
                    .map(|(o, _)| o.to_string())
                    .unwrap_or_default();
                // The context path is part of the git URL too — Wikimedia
                // serves the repo at /r/mediawiki/core, not /mediawiki/core.
                let ctx = s[..c].join("/");
                let prefix = if ctx.is_empty() {
                    base.clone()
                } else {
                    format!("{base}/{ctx}")
                };
                return Some(PrUrl {
                    provider: Provider::Gerrit,
                    clone_url: format!("{prefix}/{project}"),
                    host,
                    owner,
                    repo,
                    number,
                    patchset,
                });
            }
        }
    }

    // SourceForge / Allura: /p/{project}/{repo}/merge-requests/{n}
    if s.first().copied() == Some("p") && s.get(3).copied() == Some("merge-requests") {
        let project = s.get(1)?;
        let repo = s.get(2)?;
        let number = parse_number(s.get(4)?)?;
        return Some(PrUrl {
            provider: Provider::SourceForge,
            clone_url: format!("https://git.code.sf.net/p/{project}/{repo}"),
            host,
            owner: (*project).to_string(),
            repo: (*repo).to_string(),
            number,
            patchset: None,
        });
    }

    // Launchpad: /~{user}/{project}/+git/{repo}/+merge/{n}
    if let Some(i) = s.iter().position(|x| *x == "+merge") {
        let number = parse_number(s.get(i + 1)?)?;
        let repo = s.get(i.checked_sub(1)?)?;
        return Some(PrUrl {
            provider: Provider::Launchpad,
            clone_url: format!("https://git.launchpad.net/{}", s[..i].join("/")),
            host,
            owner: s[..i - 1].join("/"),
            repo: (*repo).to_string(),
            number,
            patchset: None,
        });
    }

    // Pagure: /{repo}/pull-request/{n} or /fork/{user}/{repo}/pull-request/{n}.
    // Projects can live at the path root, so it cannot use the generic branch
    // below (which requires a namespace segment).
    if let Some((i, number)) = find_marker(&s, "pull-request") {
        let repo = s.get(i.checked_sub(1)?)?;
        let owner = s[..i - 1].join("/");
        let path = s[..i].join("/");
        return Some(PrUrl {
            provider: Provider::Pagure,
            clone_url: format!("{base}/{path}.git"),
            host,
            owner,
            repo: (*repo).to_string(),
            number,
            patchset: None,
        });
    }

    // AWS CodeCommit console: /codesuite/codecommit/repositories/{repo}/pull-requests/{n}.
    // Checked before the generic scan, which would otherwise see `pull-requests`
    // and blame Bitbucket Cloud.
    if let Some(i) = s.iter().position(|x| *x == "codecommit") {
        if let Some((_, number)) = find_marker(&s, "pull-requests") {
            let repo = s.get(i + 2).copied().unwrap_or("");
            return Some(PrUrl {
                provider: Provider::CodeCommit,
                clone_url: format!("{base}/{}", s.join("/")),
                host,
                owner: String::new(),
                repo: repo.to_string(),
                number,
                patchset: None,
            });
        }
    }

    // Remaining forges share `{namespace...}/{repo}/<marker>/{n}`.
    let (provider, i, number) = [
        (Provider::BitbucketCloud, "pull-requests"),
        (Provider::GitLab, "merge_requests"),
        (Provider::Gitea, "pulls"),
        (Provider::GitHub, "pull"),
    ]
    .into_iter()
    .find_map(|(p, m)| find_marker(&s, m).map(|(i, n)| (p, i, n)))?;

    // `api.github.com/repos/{o}/{r}/pulls/{n}` shares Gitea's `pulls` marker.
    let provider = match (provider, host.as_str()) {
        (Provider::Gitea, "github.com") => Provider::GitHub,
        (p, _) => p,
    };

    // GitLab inserts a `/-/` separator between the project path and the route.
    let path_end = if i > 0 && s[i - 1] == "-" { i - 1 } else { i };
    if path_end < 2 {
        return None;
    }
    let repo = s[path_end - 1];
    let owner = s[..path_end - 1].join("/");

    Some(PrUrl {
        provider,
        clone_url: format!("{base}/{owner}/{repo}.git"),
        host,
        owner,
        repo: repo.to_string(),
        number,
        patchset: None,
    })
}

/// Locate a route marker followed by a PR number.
///
/// Scans from the right and requires the next segment to parse, so a repository
/// or namespace literally named `pull` / `merge_requests` does not shadow the
/// real route later in the path.
fn find_marker(segs: &[&str], marker: &str) -> Option<(usize, u64)> {
    let i = segs.iter().rposition(|x| *x == marker)?;
    Some((i, parse_number(segs.get(i + 1)?)?))
}

/// Strip a `.diff` / `.patch` suffix and parse the remainder as a number.
fn parse_number(seg: &str) -> Option<u64> {
    let core = seg
        .strip_suffix(".diff")
        .or_else(|| seg.strip_suffix(".patch"))
        .unwrap_or(seg);
    if core.is_empty() || !core.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    core.parse().ok()
}

impl PrUrl {
    /// `git ls-remote` glob covering every ref the provider publishes for this PR.
    ///
    /// `None` means the provider exposes no PR refs over git at all.
    pub fn ref_glob(&self) -> Option<String> {
        let n = self.number;
        Some(match self.provider {
            Provider::GitHub | Provider::Gitea | Provider::Pagure | Provider::AzureDevOps => {
                format!("refs/pull/{n}/*")
            }
            Provider::GitLab => format!("refs/merge-requests/{n}/*"),
            Provider::BitbucketServer => format!("refs/pull-requests/{n}/*"),
            // refs/changes/{last two digits, zero padded}/{change}/{patchset}
            Provider::Gerrit => format!("refs/changes/{:02}/{n}/*", n % 100),
            Provider::BitbucketCloud
            | Provider::SourceForge
            | Provider::Launchpad
            | Provider::CodeCommit => return None,
        })
    }

    /// Pick the head ref (the PR's tip) and, when published, the merge ref
    /// (whose first parent is the target branch at merge-preview time).
    ///
    /// Azure DevOps publishes only `refs/pull/{n}/merge`, so a missing head ref
    /// is not fatal — `resolve` derives the head from the merge commit's second
    /// parent instead.
    fn pick_refs(&self, refs: &[String]) -> Option<(Option<String>, Option<String>)> {
        if self.provider == Provider::Gerrit {
            // Highest patchset wins unless the URL pinned one.
            let want = self.patchset.map(|p| format!("/{p}"));
            let head = match &want {
                Some(suffix) => refs.iter().find(|r| r.ends_with(suffix))?.clone(),
                None => refs
                    .iter()
                    .max_by_key(|r| {
                        r.rsplit('/')
                            .next()
                            .and_then(|p| p.parse::<u64>().ok())
                            .unwrap_or(0)
                    })?
                    .clone(),
            };
            return Some((Some(head), None));
        }
        // Gitee spells the merge ref `/MERGE`; match the tail case-insensitively.
        let tail_is = |r: &String, want: &str| {
            r.rsplit('/').next().is_some_and(|t| t.eq_ignore_ascii_case(want))
        };
        let head = refs
            .iter()
            .find(|r| tail_is(r, "head") || tail_is(r, "from"))
            .cloned();
        let merge = refs.iter().find(|r| tail_is(r, "merge")).cloned();
        if head.is_none() && merge.is_none() {
            return None;
        }
        Some((head, merge))
    }

    /// Directory this repo is cached in.
    ///
    /// The trailing hash keeps distinct remotes apart: slugging turns `/` into
    /// `-`, so `gitlab.com/a/b/repo` and `gitlab.com/a-b/repo` would otherwise
    /// share a checkout and silently serve each other's diffs.
    fn cache_dir(&self, root: &Path) -> Result<PathBuf, PrUrlError> {
        let digest = hex::encode(Sha256::digest(self.clone_url.as_bytes()));
        Ok(root.join(slug(&self.host)).join(format!(
            "{}-{}",
            slug(&self.repo),
            &digest[..8]
        )))
    }
}

/// Filesystem-safe path component. Rejects `.` and `..` outright so a hostile
/// host or repo name cannot walk out of the cache root.
fn slug(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if out.chars().all(|c| c == '.') {
        "-".repeat(out.len().max(1))
    } else {
        out
    }
}

/// A PR URL resolved to a usable local repository + revision pair.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedPr {
    /// Local working tree to analyze.
    pub path: String,
    /// Merge base (fork point) of the PR against its target branch, so
    /// `base..head` matches the provider's "Files changed" view.
    pub base: String,
    /// Local branch `pr-{n}` at the PR's tip. HEAD in the checkout stays
    /// detached so re-resolving the same PR can update it.
    pub head: String,
}

/// Held for the duration of a `resolve` so two processes cannot clone, fetch
/// and `checkout --force` the same cached working tree concurrently.
struct CacheLock(PathBuf);

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn lock_cache(dir: &Path) -> Result<CacheLock, PrUrlError> {
    let path = PathBuf::from(format!("{}.lock", dir.display()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(_) => return Ok(CacheLock(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let age = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|m| m.elapsed().ok());
                if !age.is_some_and(|age| age > LOCK_STALE) {
                    break;
                }
                // Claim the stale lock by renaming it somewhere unique: rename
                // is atomic, so of two processes racing to reap, only the one
                // that moves *this* file proceeds. Unlinking by path instead
                // would let the loser delete the winner's fresh lock.
                let claimed = PathBuf::from(format!("{}.{}", path.display(), std::process::id()));
                if std::fs::rename(&path, &claimed).is_ok() {
                    let _ = std::fs::remove_file(&claimed);
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(PrUrlError::CacheBusy(path.display().to_string()))
}

/// Clone (or reuse) the repository behind `pr` and fetch its PR refs.
///
/// Uses the `git` CLI rather than git2 because libgit2 is built here without
/// network transports, and because shelling out inherits the user's existing
/// credential helpers and SSH agent for private repositories.
///
/// There is deliberately no wall-clock timeout: a first clone of a large
/// monorepo legitimately takes minutes, and killing it would be worse than
/// waiting. Interactive credential prompts are suppressed instead, so the
/// failure mode for a private repo is a fast error rather than a hang.
pub fn resolve(pr: &PrUrl) -> Result<ResolvedPr, PrUrlError> {
    let root = std::env::var_os("DIFFCORE_REPO_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| crate::paths::cache_dir().map(|dir| dir.join("repos")))
        .ok_or(PrUrlError::NoCacheDir)?;
    resolve_in(pr, &root)
}

/// SHA of the PR's tip on the remote, via a single `ls-remote` — no clone, no
/// checkout. Providers that publish only a merge ref (Azure DevOps) report that
/// ref's SHA instead; it moves whenever either side of the PR does, which is
/// all a change watcher needs.
pub fn remote_head_sha(pr: &PrUrl) -> Result<String, PrUrlError> {
    let glob = pr
        .ref_glob()
        .ok_or(PrUrlError::NoGitRefs(pr.provider.name()))?;
    let listing = git(Path::new("."), &["ls-remote", &pr.clone_url, &glob])?;
    let pairs: Vec<(&str, &str)> = listing
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(sha, r)| (sha.trim(), r.trim())))
        .collect();
    let refs: Vec<String> = pairs.iter().map(|(_, r)| r.to_string()).collect();
    let not_found =
        || PrUrlError::NotFound(pr.provider.unit(), pr.number, pr.clone_url.clone());
    let (head_ref, merge_ref) = pr.pick_refs(&refs).ok_or_else(not_found)?;
    let want = head_ref.or(merge_ref).ok_or_else(not_found)?;
    pairs
        .iter()
        .find(|(_, r)| *r == want)
        .map(|(sha, _)| sha.to_string())
        .ok_or_else(not_found)
}

/// `resolve`, against an explicit cache root. Tests use this so they never have
/// to mutate the process environment, which races every other thread's getenv.
pub(crate) fn resolve_in(pr: &PrUrl, root: &Path) -> Result<ResolvedPr, PrUrlError> {
    let glob = pr
        .ref_glob()
        .ok_or(PrUrlError::NoGitRefs(pr.provider.name()))?;

    let listing = git(Path::new("."), &["ls-remote", &pr.clone_url, &glob])?;
    let refs: Vec<String> = listing
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(_, r)| r.trim().to_string()))
        .collect();
    let (head_ref, merge_ref) = pr.pick_refs(&refs).ok_or_else(|| {
        PrUrlError::NotFound(pr.provider.unit(), pr.number, pr.clone_url.clone())
    })?;

    let dir = pr.cache_dir(root)?;
    let _lock = lock_cache(&dir)?;

    if dir.join(".git").exists() {
        // The directory is keyed by a hash of the clone URL, so this should
        // already match; realign it anyway rather than fetch from the wrong
        // remote if the scheme or a credential prefix changed.
        git(&dir, &["remote", "set-url", "origin", &pr.clone_url])?;
    } else {
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Full clone: libgit2 (which does the actual diffing) has no partial-clone
        // promisor support, so a `--filter=blob:none` clone fails on missing blobs.
        // The cost is paid once per repository and then cached.
        git(
            Path::new("."),
            &["clone", &pr.clone_url, &dir.to_string_lossy()],
        )?;
    }

    let local_head = format!("refs/heads/pr-{}", pr.number);
    let local_merge = format!("refs/diffcore/pr-{}/merge", pr.number);
    // `+refs/heads/*` keeps origin/* current on a reused clone. Without it the
    // default branch is frozen at first-clone time and every merge base computed
    // from it drifts further out of date as the cache ages.
    let mut args = vec![
        "fetch".to_string(),
        "--force".to_string(),
        "--prune".to_string(),
        "origin".to_string(),
        "+refs/heads/*:refs/remotes/origin/*".to_string(),
    ];
    if let Some(h) = &head_ref {
        args.push(format!("+{h}:{local_head}"));
    }
    if let Some(m) = &merge_ref {
        args.push(format!("+{m}:{local_merge}"));
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    git(&dir, &argv)?;
    // `--prune` deletes the old branch on an upstream rename but leaves
    // refs/remotes/origin/HEAD symrefed at it, so default_branch would return a
    // dangling ref and every base computed from it would fail.
    git(&dir, &["remote", "set-head", "origin", "--auto"])?;

    // Azure DevOps publishes only the merge ref; its second parent is the PR tip.
    if head_ref.is_none() {
        let tip = git(&dir, &["rev-parse", &format!("{local_merge}^2")])?;
        git(&dir, &["update-ref", &local_head, tip.trim()])?;
    }
    git(&dir, &["checkout", "--force", "--detach", &local_head])?;

    // Target-branch tip: the merge ref's first parent when the provider publishes
    // one, otherwise reconstructed from the default branch.
    let target = match merge_ref {
        Some(_) => format!("{local_merge}^1"),
        None => target_from_default_branch(&dir, &local_head)?,
    };
    // Report the fork point, not the target tip, so a plain two-dot `base..head`
    // diff equals what the provider shows under "Files changed". Abbreviated so
    // the UI's branch selector shows something readable.
    let fork_point = git(&dir, &["merge-base", &target, &local_head])?;
    let fork_point = fork_point.trim();

    // A fast-forward merged PR leaves no merge commit, so the fork point
    // collapses onto the head and the diff would come back empty. Say so
    // instead of presenting "0 files changed" as a successful review.
    let head_sha = git(&dir, &["rev-parse", &local_head])?;
    if fork_point == head_sha.trim() {
        return Err(PrUrlError::EmptyDiff(pr.provider.unit(), pr.number));
    }

    let base = git(&dir, &["rev-parse", "--short", fork_point])?
        .trim()
        .to_string();

    Ok(ResolvedPr {
        path: dir.to_string_lossy().into_owned(),
        base,
        head: format!("pr-{}", pr.number),
    })
}

/// Best guess at the PR's target-branch tip when no merge ref is published.
///
/// Providers drop the merge ref once a PR lands, and by then the head is an
/// ancestor of the default branch, so `merge-base(default, head)` collapses to
/// `head` and the diff comes out empty. Recover the pre-merge tip from the merge
/// commit that landed the PR.
///
/// Squash- and rebase-merges are fine: they rewrite the commit, so the head is
/// not an ancestor and the plain merge base is already the fork point. Only
/// fast-forward merges are unrecoverable, and `resolve` rejects those explicitly.
fn target_from_default_branch(dir: &Path, head: &str) -> Result<String, PrUrlError> {
    let default = default_branch(dir)?;
    let already_merged = git(dir, &["merge-base", "--is-ancestor", head, &default]).is_ok();
    if already_merged {
        let range = format!("{head}..{default}");
        let merges = git(dir, &["rev-list", "--ancestry-path", "--merges", &range])?;
        // Oldest merge on the ancestry path is the one that landed this PR.
        if let Some(landing) = merges.split_whitespace().next_back() {
            if let Ok(parent) = git(dir, &["rev-parse", &format!("{landing}^1")]) {
                return Ok(parent.trim().to_string());
            }
        }
    }
    Ok(default)
}

/// Resolve the remote's default branch, for providers without a merge ref.
fn default_branch(dir: &Path) -> Result<String, PrUrlError> {
    if let Ok(r) = git(dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        let r = r.trim();
        if !r.is_empty() {
            return Ok(r.to_string());
        }
    }
    Ok("origin/HEAD".to_string())
}

/// Run git, failing loudly.
///
/// All interactive credential paths are closed off: `GIT_TERMINAL_PROMPT`
/// covers the terminal, the askpass variables cover GUI helpers a user may have
/// configured globally, and `BatchMode` covers SSH. Without these a desktop
/// build can sit forever behind an invisible password dialog.
fn git(dir: &Path, args: &[&str]) -> Result<String, PrUrlError> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo")
        .env("SSH_ASKPASS", "echo")
        .env("SSH_ASKPASS_REQUIRE", "never");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(PrUrlError::Git(
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
mod tests {
    use super::*;

    fn p(u: &str) -> PrUrl {
        parse(u).unwrap_or_else(|| panic!("expected {u} to parse"))
    }

    // ── parsing ──

    #[rustfmt::skip]
    #[test]
    fn parses_every_supported_provider() {
        // (url, provider, host, owner, repo, number, clone_url)
        let cases: &[(&str, Provider, &str, &str, &str, u64, &str)] = &[
            ("https://github.com/rust-lang/rust/pull/12345", Provider::GitHub, "github.com", "rust-lang", "rust", 12345, "https://github.com/rust-lang/rust.git"),
            ("https://github.com/rust-lang/rust/pull/12345/files", Provider::GitHub, "github.com", "rust-lang", "rust", 12345, "https://github.com/rust-lang/rust.git"),
            ("https://github.com/rust-lang/rust/pull/12345.diff", Provider::GitHub, "github.com", "rust-lang", "rust", 12345, "https://github.com/rust-lang/rust.git"),
            ("HTTPS://GitHub.com/rust-lang/rust/pull/12345", Provider::GitHub, "github.com", "rust-lang", "rust", 12345, "https://github.com/rust-lang/rust.git"),
            ("https://api.github.com/repos/rust-lang/rust/pulls/12345", Provider::GitHub, "github.com", "rust-lang", "rust", 12345, "https://github.com/rust-lang/rust.git"),
            ("https://ghe.corp.internal/platform/api/pull/7", Provider::GitHub, "ghe.corp.internal", "platform", "api", 7, "https://ghe.corp.internal/platform/api.git"),
            ("https://gitlab.com/gitlab-org/gitlab/-/merge_requests/999", Provider::GitLab, "gitlab.com", "gitlab-org", "gitlab", 999, "https://gitlab.com/gitlab-org/gitlab.git"),
            ("https://gitlab.com/a/b/c/repo/-/merge_requests/4", Provider::GitLab, "gitlab.com", "a/b/c", "repo", 4, "https://gitlab.com/a/b/c/repo.git"),
            ("https://gitlab.example.com/team/repo/merge_requests/4", Provider::GitLab, "gitlab.example.com", "team", "repo", 4, "https://gitlab.example.com/team/repo.git"),
            ("https://codeberg.org/forgejo/forgejo/pulls/321", Provider::Gitea, "codeberg.org", "forgejo", "forgejo", 321, "https://codeberg.org/forgejo/forgejo.git"),
            ("http://gitea.internal:3000/ops/infra/pulls/8", Provider::Gitea, "gitea.internal:3000", "ops", "infra", 8, "http://gitea.internal:3000/ops/infra.git"),
            ("https://gitee.com/openharmony/docs/pulls/55", Provider::Gitea, "gitee.com", "openharmony", "docs", 55, "https://gitee.com/openharmony/docs.git"),
            ("https://pagure.io/pagure/pull-request/5", Provider::Pagure, "pagure.io", "", "pagure", 5, "https://pagure.io/pagure.git"),
            ("https://pagure.io/fork/user/pagure/pull-request/5", Provider::Pagure, "pagure.io", "fork/user", "pagure", 5, "https://pagure.io/fork/user/pagure.git"),
            ("https://bb.corp.com/projects/PLAT/repos/api/pull-requests/17/overview", Provider::BitbucketServer, "bb.corp.com", "PLAT", "api", 17, "https://bb.corp.com/scm/PLAT/api.git"),
            ("https://corp.com/bitbucket/projects/PLAT/repos/api/pull-requests/17", Provider::BitbucketServer, "corp.com", "PLAT", "api", 17, "https://corp.com/bitbucket/scm/PLAT/api.git"),
            ("https://dev.azure.com/contoso/Payments/_git/gateway/pullrequest/88", Provider::AzureDevOps, "dev.azure.com", "contoso/Payments", "gateway", 88, "https://dev.azure.com/contoso/Payments/_git/gateway"),
            ("https://contoso.visualstudio.com/Payments/_git/gateway/pullrequest/88", Provider::AzureDevOps, "contoso.visualstudio.com", "Payments", "gateway", 88, "https://contoso.visualstudio.com/Payments/_git/gateway"),
            ("https://gerrit.googlesource.com/c/gerrit/+/400123", Provider::Gerrit, "gerrit.googlesource.com", "", "gerrit", 400123, "https://gerrit.googlesource.com/gerrit"),
            ("https://bitbucket.org/atlassian/stash/pull-requests/42", Provider::BitbucketCloud, "bitbucket.org", "atlassian", "stash", 42, "https://bitbucket.org/atlassian/stash.git"),
            ("https://sourceforge.net/p/mingw/mingw-org-wsl/merge-requests/3", Provider::SourceForge, "sourceforge.net", "mingw", "mingw-org-wsl", 3, "https://git.code.sf.net/p/mingw/mingw-org-wsl"),
            ("https://code.launchpad.net/~user/proj/+git/repo/+merge/456", Provider::Launchpad, "code.launchpad.net", "~user/proj/+git", "repo", 456, "https://git.launchpad.net/~user/proj/+git/repo"),
        ];
        for (url, provider, host, owner, repo, number, clone_url) in cases {
            let g = p(url);
            assert_eq!(
                (g.provider, g.host.as_str(), g.owner.as_str(), g.repo.as_str(), g.number, g.clone_url.as_str()),
                (*provider, *host, *owner, *repo, *number, *clone_url),
                "parsing {url}"
            );
        }
    }

    #[test]
    fn parses_gerrit_under_a_context_path() {
        // Most self-hosted Gerrit mounts the UI under /r/; only googlesource
        // and opendev serve it from the root.
        let wm = p("https://gerrit.wikimedia.org/r/c/mediawiki/core/+/1234");
        assert_eq!(wm.repo, "core");
        assert_eq!(wm.owner, "mediawiki");
        assert_eq!(wm.number, 1234);
        // Verified live: /r/ is part of the git URL, not just the web UI.
        assert_eq!(wm.clone_url, "https://gerrit.wikimedia.org/r/mediawiki/core");
    }

    #[test]
    fn gerrit_url_pins_patchset_and_shards_ref() {
        let pinned = p("https://gerrit.example.org/c/platform/build/+/1234/7");
        assert_eq!(pinned.patchset, Some(7));
        // Gerrit sharding: last two digits of the change number, zero padded.
        assert_eq!(pinned.ref_glob().as_deref(), Some("refs/changes/34/1234/*"));
        assert_eq!(
            p("https://gerrit.example.org/c/proj/+/7").ref_glob().as_deref(),
            Some("refs/changes/07/7/*")
        );
    }

    #[test]
    fn providers_without_pr_refs_have_no_glob() {
        // These must fail with NoGitRefs, not a misleading "not found".
        for url in [
            "https://bitbucket.org/w/r/pull-requests/5",
            "https://sourceforge.net/p/proj/repo/merge-requests/3",
            "https://code.launchpad.net/~u/p/+git/r/+merge/6",
        ] {
            assert_eq!(p(url).ref_glob(), None, "{url}");
        }
        assert_eq!(
            p("https://github.com/o/r/pull/1").ref_glob().as_deref(),
            Some("refs/pull/1/*")
        );
        assert_eq!(
            p("https://gitlab.com/o/r/-/merge_requests/2").ref_glob().as_deref(),
            Some("refs/merge-requests/2/*")
        );
        assert_eq!(
            p("https://bb.corp.com/projects/P/repos/r/pull-requests/3").ref_glob().as_deref(),
            Some("refs/pull-requests/3/*")
        );
    }

    #[test]
    fn a_repo_named_like_a_route_does_not_shadow_the_real_route() {
        // Scanning from the left would match the `pull-requests` namespace
        // segment and mis-parse this as a Bitbucket Cloud URL.
        let g = p("https://gitlab.com/group/pull-requests/-/merge_requests/12");
        assert_eq!(g.provider, Provider::GitLab);
        assert_eq!(g.repo, "pull-requests");
        assert_eq!(g.number, 12);
        let h = p("https://github.com/acme/pull/pull/9");
        assert_eq!((h.provider, h.repo.as_str(), h.number), (Provider::GitHub, "pull", 9));
    }

    #[test]
    fn rejects_non_pr_input() {
        for input in [
            "/home/me/projects/repo",
            "~/code/repo",
            "C:\\code\\repo",
            "git@github.com:o/r.git",
            "https://github.com/rust-lang/rust",
            "https://github.com/rust-lang/rust/issues/123",
            "https://github.com/rust-lang/rust/pull/notanumber",
            "https://gitlab.com/o/r/-/issues/5",
            "https://example.com",
            "not a url at all",
            "",
        ] {
            assert!(parse(input).is_none(), "{input} should not parse as a PR URL");
        }
    }

    // ── resolution against a local origin ──

    /// Minimal upstream repo: `main` with one commit, plus a `feature` branch
    /// forked from it. Tests then land `feature` however they like and plant
    /// whichever provider refs they want to simulate.
    struct Origin {
        dir: PathBuf,
    }

    impl Origin {
        fn new(root: &Path, name: &str) -> Origin {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let o = Origin { dir };
            o.run(&["init", "-q", "-b", "main"]);
            o.run(&["config", "user.email", "test@diffcore.invalid"]);
            o.run(&["config", "user.name", "diffcore test"]);
            o.commit("shared.txt", "base\n", "base");
            o
        }

        fn run(&self, args: &[&str]) -> String {
            match git(&self.dir, args) {
                Ok(out) => out.trim().to_string(),
                Err(e) => panic!("git {args:?} in {:?}: {e}", self.dir),
            }
        }

        fn commit(&self, file: &str, body: &str, msg: &str) -> String {
            std::fs::write(self.dir.join(file), body).unwrap();
            self.run(&["add", "-A"]);
            self.run(&["commit", "-qm", msg]);
            self.run(&["rev-parse", "HEAD"])
        }

        fn url(&self) -> String {
            self.dir.to_string_lossy().into_owned()
        }

        /// A PR branched off the current `main`, published at `refs/pull/{n}/head`.
        fn open_pr(&self, n: u64, file: &str) -> String {
            self.run(&["checkout", "-q", "-B", &format!("pr{n}"), "main"]);
            let tip = self.commit(file, "pr work\n", "pr work");
            self.run(&["checkout", "-q", "main"]);
            self.run(&["update-ref", &format!("refs/pull/{n}/head"), &tip]);
            tip
        }
    }

    /// A GitHub-shaped `PrUrl` pointed at a local origin.
    fn local_pr(origin: &Origin, n: u64) -> PrUrl {
        PrUrl {
            provider: Provider::GitHub,
            host: "example.test".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            number: n,
            patchset: None,
            clone_url: origin.url(),
        }
    }

    /// Gives each test its own origin root and cache root. Deliberately does not
    /// touch DIFFCORE_REPO_CACHE_DIR: `set_var` mutates the whole process while
    /// 1700 other tests are calling getenv, which is a data race today and a
    /// hard error in edition 2024.
    fn with_cache<T>(f: impl FnOnce(&Path, &Path) -> T) -> T {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        f(tmp.path(), &cache)
    }

    fn changed_files(dir: &Path, base: &str, head: &str) -> String {
        match git(dir, &["diff", "--name-only", &format!("{base}..{head}")]) {
            Ok(o) => o.trim().to_string(),
            Err(e) => panic!("diff: {e}"),
        }
    }

    #[test]
    fn remote_head_sha_tracks_the_pr_tip_without_cloning() {
        with_cache(|root, _cache| {
            let o = Origin::new(root, "origin");
            let tip = o.open_pr(1, "feature.txt");
            let pr = local_pr(&o, 1);
            assert_eq!(remote_head_sha(&pr).unwrap(), tip);

            // New commits land on the PR branch and the provider republishes
            // the head ref — the watcher must see the new SHA.
            o.run(&["checkout", "-q", "pr1"]);
            let new_tip = o.commit("feature.txt", "more work\n", "more work");
            o.run(&["checkout", "-q", "main"]);
            o.run(&["update-ref", "refs/pull/1/head", &new_tip]);
            assert_eq!(remote_head_sha(&pr).unwrap(), new_tip);
        });
    }

    #[test]
    fn resolve_uses_the_merge_ref_parent_as_the_target_tip() {
        with_cache(|root, cache| {
            let o = Origin::new(root, "origin");
            let fork = o.run(&["rev-parse", "HEAD"]);
            let tip = o.open_pr(1, "feature.txt");
            // Target branch moves on after the fork, so the tip and the fork
            // point differ — a two-dot diff from the tip would be wrong.
            let main_tip = o.commit("unrelated.txt", "moved on\n", "main moves on");
            let merge = o.run(&[
                "commit-tree", "-p", &main_tip, "-p", &tip, "-m", "merge preview",
                &format!("{main_tip}^{{tree}}"),
            ]);
            o.run(&["update-ref", "refs/pull/1/merge", &merge]);

            let pr = local_pr(&o, 1);
            let r = resolve_in(&pr, cache).unwrap();
            let again = resolve_in(&pr, cache).unwrap();
            assert_eq!(r.path, again.path, "second resolve must reuse the clone");

            let dir = PathBuf::from(&r.path);
            assert_eq!(git(&dir, &["rev-parse", &r.base]).unwrap().trim(), fork);
            assert_eq!(git(&dir, &["rev-parse", &r.head]).unwrap().trim(), tip);
            assert_eq!(changed_files(&dir, &r.base, &r.head), "feature.txt");
        });
    }

    #[test]
    fn resolve_recovers_the_base_of_a_pr_that_already_landed() {
        with_cache(|root, cache| {
            let o = Origin::new(root, "origin");
            let fork = o.run(&["rev-parse", "HEAD"]);
            let tip = o.open_pr(1, "feature.txt");
            // Landed with a merge commit, and the provider dropped the merge
            // ref — the common shape for a closed GitHub PR.
            o.run(&["merge", "-q", "--no-ff", &tip, "-m", "landed"]);
            o.commit("after.txt", "later\n", "unrelated later work");

            let r = resolve_in(&local_pr(&o, 1), cache).unwrap();
            let dir = PathBuf::from(&r.path);
            assert_eq!(git(&dir, &["rev-parse", &r.base]).unwrap().trim(), fork);
            assert_eq!(changed_files(&dir, &r.base, &r.head), "feature.txt");
        });
    }

    #[test]
    fn resolve_handles_a_squash_merged_pr() {
        with_cache(|root, cache| {
            let o = Origin::new(root, "origin");
            let tip = o.open_pr(1, "feature.txt");
            // Squash rewrites the SHA, so the head is not an ancestor of main
            // and the plain merge base is already the fork point.
            o.run(&["merge", "-q", "--squash", &tip]);
            o.run(&["commit", "-qm", "squashed"]);

            let r = resolve_in(&local_pr(&o, 1), cache).unwrap();
            let dir = PathBuf::from(&r.path);
            assert_eq!(changed_files(&dir, &r.base, &r.head), "feature.txt");
        });
    }

    #[test]
    fn resolve_rejects_a_fast_forward_merged_pr_instead_of_returning_nothing() {
        with_cache(|root, cache| {
            let o = Origin::new(root, "origin");
            let tip = o.open_pr(1, "feature.txt");
            // Fast-forward leaves no merge commit and no new SHA, so the base
            // is unrecoverable from git alone.
            o.run(&["merge", "-q", "--ff-only", &tip]);

            match resolve_in(&local_pr(&o, 1), cache) {
                Err(PrUrlError::EmptyDiff(_, 1)) => {}
                other => panic!("expected EmptyDiff, got {other:?}"),
            }
        });
    }

    #[test]
    fn resolve_refreshes_the_default_branch_on_a_cached_clone() {
        with_cache(|root, cache| {
            let o = Origin::new(root, "origin");
            o.open_pr(1, "first.txt");
            // Populate the cache while main is still at the first commit.
            resolve_in(&local_pr(&o, 1), cache).unwrap();

            // Upstream moves on, and a later PR forks from the new tip.
            let new_base = o.commit("mainline.txt", "new mainline\n", "main advances");
            let tip2 = o.open_pr(2, "second.txt");

            // Without re-fetching refs/heads/*, origin/HEAD is frozen at the
            // original clone and the merge base lands before "main advances",
            // pulling mainline.txt into the PR's diff.
            let r = resolve_in(&local_pr(&o, 2), cache).unwrap();
            let dir = PathBuf::from(&r.path);
            assert_eq!(git(&dir, &["rev-parse", &r.base]).unwrap().trim(), new_base);
            assert_eq!(git(&dir, &["rev-parse", &r.head]).unwrap().trim(), tip2);
            assert_eq!(changed_files(&dir, &r.base, &r.head), "second.txt");
        });
    }

    #[test]
    fn resolve_derives_the_head_from_a_merge_only_ref() {
        with_cache(|root, cache| {
            let o = Origin::new(root, "origin");
            let fork = o.run(&["rev-parse", "HEAD"]);
            let tip = o.open_pr(1, "feature.txt");
            // Azure DevOps publishes only refs/pull/{n}/merge.
            o.run(&["update-ref", "-d", "refs/pull/1/head"]);
            let main_tip = o.run(&["rev-parse", "main"]);
            let merge = o.run(&[
                "commit-tree", "-p", &main_tip, "-p", &tip, "-m", "merge preview",
                &format!("{main_tip}^{{tree}}"),
            ]);
            o.run(&["update-ref", "refs/pull/1/merge", &merge]);

            let r = resolve_in(&local_pr(&o, 1), cache).unwrap();
            let dir = PathBuf::from(&r.path);
            assert_eq!(git(&dir, &["rev-parse", &r.head]).unwrap().trim(), tip);
            assert_eq!(git(&dir, &["rev-parse", &r.base]).unwrap().trim(), fork);
        });
    }

    #[test]
    fn distinct_remotes_never_share_a_cache_directory() {
        with_cache(|root, cache| {
            // Slugging turns `/` into `-`, so these two owners collide on the
            // filesystem; the clone-URL hash is what keeps them apart.
            let a = Origin::new(root, "a");
            a.open_pr(1, "from-a.txt");
            let b = Origin::new(root, "b");
            b.open_pr(1, "from-b.txt");

            let mut pr_a = local_pr(&a, 1);
            pr_a.owner = "x/y".to_string();
            let mut pr_b = local_pr(&b, 1);
            pr_b.owner = "x-y".to_string();

            let ra = resolve_in(&pr_a, cache).unwrap();
            let rb = resolve_in(&pr_b, cache).unwrap();
            assert_ne!(ra.path, rb.path);
            assert_eq!(changed_files(Path::new(&ra.path), &ra.base, &ra.head), "from-a.txt");
            assert_eq!(changed_files(Path::new(&rb.path), &rb.base, &rb.head), "from-b.txt");
        });
    }

    #[test]
    fn resolve_survives_an_upstream_default_branch_rename() {
        with_cache(|root, cache| {
            let o = Origin::new(root, "origin");
            o.open_pr(1, "first.txt");
            resolve_in(&local_pr(&o, 1), cache).unwrap();

            // Upstream renames its default branch. --prune deletes the old
            // remote-tracking branch but leaves origin/HEAD symrefed at it, so
            // without `remote set-head` the default branch is a dangling ref and
            // every base computed from it fails.
            o.run(&["branch", "-m", "main", "trunk"]);
            let new_base = o.run(&["rev-parse", "trunk"]);
            o.run(&["checkout", "-q", "-B", "pr2", "trunk"]);
            let tip = o.commit("second.txt", "pr work\n", "pr work");
            o.run(&["checkout", "-q", "trunk"]);
            o.run(&["update-ref", "refs/pull/2/head", &tip]);

            let r = resolve_in(&local_pr(&o, 2), cache).unwrap();
            let dir = PathBuf::from(&r.path);
            assert_eq!(git(&dir, &["rev-parse", &r.base]).unwrap().trim(), new_base);
            assert_eq!(changed_files(&dir, &r.base, &r.head), "second.txt");
        });
    }

    #[test]
    fn a_hostile_host_or_repo_name_cannot_escape_the_cache_root() {
        with_cache(|root, cache| {
            let o = Origin::new(root, "origin");
            o.open_pr(1, "feature.txt");
            o.commit("later.txt", "later\n", "main moves on");

            let mut pr = local_pr(&o, 1);
            pr.host = "../../etc".to_string();
            pr.owner = "..".to_string();
            pr.repo = "..".to_string();

            let r = resolve_in(&pr, cache).unwrap();
            let path = PathBuf::from(&r.path);
            assert!(path.starts_with(cache), "{path:?} escaped {cache:?}");
            assert!(!path.components().any(|c| c == std::path::Component::ParentDir));
        });
    }
}
