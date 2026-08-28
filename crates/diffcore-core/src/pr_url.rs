//! Parse pull-request / merge-request URLs and resolve them to local git refs.
//!
//! The desktop app's repository field and the CLI's `--repo` flag both accept a
//! PR/MR URL in place of a filesystem path. Resolution clones (or reuses) a
//! cached mirror under `~/.diffcore/cache/repos/` and fetches the provider's
//! pull-request ref namespace, so no API token is needed for public repos.
//!
//! Provider ref namespaces (see `docs/pr-url-providers.md` for the full table):
//!
//! | Provider                | web URL                                              | git refs                        |
//! |-------------------------|------------------------------------------------------|---------------------------------|
//! | GitHub / GHE            | `/{owner}/{repo}/pull/{n}`                           | `refs/pull/{n}/{head,merge}`    |
//! | GitLab (SaaS + self)    | `/{ns...}/{repo}/-/merge_requests/{n}`               | `refs/merge-requests/{n}/{head,merge}` |
//! | Gitea/Forgejo/Codeberg/Gitea-likes | `/{owner}/{repo}/pulls/{n}`               | `refs/pull/{n}/head`            |
//! | Bitbucket Data Center   | `/projects/{KEY}/repos/{repo}/pull-requests/{n}`     | `refs/pull-requests/{n}/{from,merge}` |
//! | Azure DevOps            | `/{org}/{project}/_git/{repo}/pullrequest/{n}`       | `refs/pull/{n}/{merge,head}`    |
//! | Gerrit                  | `/c/{project}/+/{change}[/{patchset}]`               | `refs/changes/{nn}/{change}/{ps}` |
//! | SourceForge (Allura)    | `/p/{project}/{repo}/merge-requests/{n}`             | `refs/merge-requests/{n}/head`  |
//! | Bitbucket Cloud         | `/{workspace}/{repo}/pull-requests/{n}`              | none — API only                 |
//! | Launchpad               | `/~{user}/{proj}/+git/{repo}/+merge/{n}`             | none — API only                 |

use std::path::{Path, PathBuf};
use std::process::Command;

use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum PrUrlError {
    #[error("not a recognised pull/merge request URL: {0}")]
    Unrecognised(String),
    #[error(
        "{0} does not publish pull-request refs over git — clone the repository and pick the source/target branches manually"
    )]
    NoGitRefs(&'static str),
    #[error("{0} #{1} not found on {2} (private repo? try `git ls-remote {2}`)")]
    NotFound(&'static str, u64, String),
    #[error("`git {0}` failed: {1}")]
    Git(String, String),
    #[error("cannot locate a cache directory — set DIFFCORE_REPO_CACHE_DIR or HOME")]
    NoCacheDir,
    #[error("io error running git: {0}")]
    Io(#[from] std::io::Error),
}

/// Forge families that share a pull-request URL shape and ref namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Provider {
    /// github.com and GitHub Enterprise Server.
    #[serde(rename = "github")]
    GitHub,
    /// gitlab.com and self-managed GitLab (CE/EE).
    #[serde(rename = "gitlab")]
    GitLab,
    /// Gitea, Forgejo, Codeberg, Gogs, Gitee — same `/pulls/{n}` shape.
    #[serde(rename = "gitea")]
    Gitea,
    /// Bitbucket Data Center / Server (on-prem).
    #[serde(rename = "bitbucket-server")]
    BitbucketServer,
    /// bitbucket.org (Cloud) — no PR refs over git.
    #[serde(rename = "bitbucket-cloud")]
    BitbucketCloud,
    /// Azure DevOps Services and Azure DevOps Server / TFS.
    #[serde(rename = "azure-devops")]
    AzureDevOps,
    /// Gerrit changes (the review unit, equivalent to a PR).
    #[serde(rename = "gerrit")]
    Gerrit,
    /// SourceForge / Apache Allura merge requests.
    #[serde(rename = "sourceforge")]
    SourceForge,
    /// Launchpad merge proposals — no PR refs over git.
    #[serde(rename = "launchpad")]
    Launchpad,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::GitHub => "GitHub",
            Provider::GitLab => "GitLab",
            Provider::Gitea => "Gitea/Forgejo",
            Provider::BitbucketServer => "Bitbucket Data Center",
            Provider::BitbucketCloud => "Bitbucket Cloud",
            Provider::AzureDevOps => "Azure DevOps",
            Provider::Gerrit => "Gerrit",
            Provider::SourceForge => "SourceForge",
            Provider::Launchpad => "Launchpad",
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrUrl {
    pub provider: Provider,
    /// Host, including port when non-default (e.g. `gitea.internal:3000`).
    pub host: String,
    /// Namespace/owner path (`rust-lang`, `gitlab-org/security`, `myorg/myproject`).
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
    if !(raw.starts_with("http://") || raw.starts_with("https://")) {
        return None;
    }
    let mut url = Url::parse(raw).ok()?;
    url.set_query(None);
    url.set_fragment(None);

    let scheme = url.scheme().to_string();
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
    let (host, segs) = if host == "api.github.com" && segs.first().map(String::as_str) == Some("repos")
    {
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
        if s.get(i + 2).copied() == Some("repos")
            && s.get(i + 4).copied() == Some("pull-requests")
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

    // Gerrit: /c/{project...}/+/{change}[/{patchset}]
    if s.first().copied() == Some("c") {
        if let Some(i) = s.iter().position(|x| *x == "+") {
            let project = s[1..i].join("/");
            let number = parse_number(s.get(i + 1)?)?;
            let patchset = s.get(i + 2).and_then(|p| parse_number(p));
            let repo = project.rsplit('/').next().unwrap_or(&project).to_string();
            let owner = project
                .rsplit_once('/')
                .map(|(o, _)| o.to_string())
                .unwrap_or_default();
            return Some(PrUrl {
                provider: Provider::Gerrit,
                clone_url: format!("{base}/{project}"),
                host,
                owner,
                repo,
                number,
                patchset,
            });
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
        let owner = s[..i - 1].join("/");
        return Some(PrUrl {
            provider: Provider::Launchpad,
            clone_url: format!("https://git.launchpad.net/{}", s[..i].join("/")),
            host,
            owner,
            repo: (*repo).to_string(),
            number,
            patchset: None,
        });
    }

    // Remaining forges all share `{namespace...}/{repo}/<marker>/{n}`.
    let (provider, marker) = [
        (Provider::BitbucketCloud, "pull-requests"),
        (Provider::GitLab, "merge_requests"),
        (Provider::Gitea, "pulls"),
        (Provider::GitHub, "pull"),
    ]
    .into_iter()
    .find(|(_, m)| s.contains(m))?;

    // `api.github.com/repos/{o}/{r}/pulls/{n}` shares Gitea's `pulls` marker.
    let provider = match (provider, host.as_str()) {
        (Provider::Gitea, "github.com") => Provider::GitHub,
        (p, _) => p,
    };

    let i = s.iter().position(|x| *x == marker)?;
    let number = parse_number(s.get(i + 1)?)?;
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

/// Strip a `.diff` / `.patch` suffix and parse the remainder as a number.
fn parse_number(seg: &str) -> Option<u64> {
    seg.trim_end_matches(".diff")
        .trim_end_matches(".patch")
        .parse()
        .ok()
}

impl PrUrl {
    /// `git ls-remote` glob covering every ref the provider publishes for this PR.
    ///
    /// `None` means the provider exposes no PR refs over git at all.
    pub fn ref_glob(&self) -> Option<String> {
        let n = self.number;
        Some(match self.provider {
            Provider::GitHub | Provider::Gitea | Provider::AzureDevOps => {
                format!("refs/pull/{n}/*")
            }
            Provider::GitLab => format!("refs/merge-requests/{n}/*"),
            Provider::BitbucketServer => format!("refs/pull-requests/{n}/*"),
            Provider::SourceForge => format!("refs/merge-requests/{n}/*"),
            // refs/changes/{last two digits, zero padded}/{change}/{patchset}
            Provider::Gerrit => format!("refs/changes/{:02}/{n}/*", n % 100),
            Provider::BitbucketCloud | Provider::Launchpad => return None,
        })
    }

    /// Pick the head ref (the PR's tip) and, when published, the merge ref
    /// (whose first parent is the target branch at merge-preview time).
    fn pick_refs(&self, refs: &[String]) -> Option<(String, Option<String>)> {
        if self.provider == Provider::Gerrit {
            // Highest patchset wins unless the URL pinned one.
            let want = self.patchset.map(|p| format!("/{p}"));
            let head = match &want {
                Some(suffix) => refs.iter().find(|r| r.ends_with(suffix))?.clone(),
                None => refs
                    .iter()
                    .max_by_key(|r| {
                        r.rsplit('/').next().and_then(|p| p.parse::<u64>().ok()).unwrap_or(0)
                    })?
                    .clone(),
            };
            return Some((head, None));
        }
        let head = refs
            .iter()
            .find(|r| r.ends_with("/head") || r.ends_with("/from"))?
            .clone();
        let merge = refs.iter().find(|r| r.ends_with("/merge")).cloned();
        Some((head, merge))
    }

    /// Directory this repo is cached in.
    fn cache_dir(&self) -> Result<PathBuf, PrUrlError> {
        let root = std::env::var_os("DIFFCORE_REPO_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".diffcore").join("cache").join("repos"))
            })
            .ok_or(PrUrlError::NoCacheDir)?;
        Ok(root
            .join(slug(&self.host))
            .join(slug(&self.owner))
            .join(slug(&self.repo)))
    }
}

/// Filesystem-safe path component.
fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

/// A PR URL resolved to a usable local repository + revision pair.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedPr {
    /// Local working tree to analyze.
    pub path: String,
    /// Base revision — the merge base (fork point) of the PR against its target
    /// branch, so `base..head` matches the provider's "Files changed" view.
    pub base: String,
    /// Head revision — a local branch `pr-{n}` at the PR's tip commit. HEAD in
    /// the checkout stays detached, so re-resolving the same PR can update it.
    pub head: String,
    pub provider: Provider,
    pub number: u64,
}

/// Clone (or reuse) the repository behind `pr` and fetch its PR refs.
///
/// Uses the `git` CLI rather than git2 because libgit2 is built here without
/// network transports, and because shelling out inherits the user's existing
/// credential helpers and SSH agent for private repositories.
pub fn resolve(pr: &PrUrl) -> Result<ResolvedPr, PrUrlError> {
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

    let dir = pr.cache_dir()?;
    if !dir.join(".git").exists() {
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Full clone: libgit2 (which does the actual diffing) has no partial-clone
        // promisor support, so a `--filter=blob:none` clone fails on missing blobs.
        // The cost is paid once per repository and then cached.
        git(Path::new("."), &["clone", &pr.clone_url, &lossy(&dir)])?;
    }

    let local_head = format!("refs/heads/pr-{}", pr.number);
    let local_merge = format!("refs/diffcore/pr-{}/merge", pr.number);
    let mut args = vec![
        "fetch".to_string(),
        "--force".to_string(),
        "origin".to_string(),
        format!("+{head_ref}:{local_head}"),
    ];
    if let Some(m) = &merge_ref {
        args.push(format!("+{m}:{local_merge}"));
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    git(&dir, &argv)?;
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
    let base = git(&dir, &["rev-parse", "--short", fork_point.trim()])?
        .trim()
        .to_string();

    Ok(ResolvedPr {
        path: lossy(&dir),
        base,
        head: format!("pr-{}", pr.number),
        provider: pr.provider,
        number: pr.number,
    })
}

/// Best guess at the PR's target-branch tip when no merge ref is published.
///
/// Providers drop the merge ref once a PR lands, and by then the head is an
/// ancestor of the default branch, so `merge-base(default, head)` collapses to
/// `head` and the diff comes out empty. Recover the pre-merge tip from the merge
/// commit that landed the PR.
///
/// ponytail: squash- and rebase-merged PRs leave no merge commit, so their base
/// is unrecoverable from git alone and the diff still comes out empty — needs a
/// provider API call to fix.
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

fn lossy(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Run git, failing loudly. `GIT_TERMINAL_PROMPT=0` keeps a private repo from
/// hanging on an interactive credential prompt with no terminal attached.
fn git(dir: &Path, args: &[&str]) -> Result<String, PrUrlError> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if !out.status.success() {
        return Err(PrUrlError::Git(
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `resolve` and `cache_dir` both read DIFFCORE_REPO_CACHE_DIR, which is
    /// process-global — serialize the tests that set it.
    static CACHE_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn run(dir: &Path, args: &[&str]) -> String {
        match git(dir, args) {
            Ok(out) => out.trim().to_string(),
            Err(e) => unreachable!("git {args:?} in {dir:?}: {e}"),
        }
    }

    fn write(dir: &Path, name: &str, body: &str) {
        if let Err(e) = std::fs::write(dir.join(name), body) {
            unreachable!("write {name}: {e}");
        }
    }

    fn p(u: &str) -> PrUrl {
        match parse(u) {
            Some(pr) => pr,
            None => unreachable!("expected {u} to parse"),
        }
    }

    #[test]
    fn parses_every_supported_provider() {
        let cases: &[(&str, Provider, &str, &str, u64, &str)] = &[
            (
                "https://github.com/rust-lang/rust/pull/12345",
                Provider::GitHub,
                "rust-lang",
                "rust",
                12345,
                "https://github.com/rust-lang/rust.git",
            ),
            // Sub-routes and patch suffixes are common copy-paste shapes.
            (
                "https://github.com/rust-lang/rust/pull/12345/files",
                Provider::GitHub,
                "rust-lang",
                "rust",
                12345,
                "https://github.com/rust-lang/rust.git",
            ),
            (
                "https://github.com/rust-lang/rust/pull/12345.diff",
                Provider::GitHub,
                "rust-lang",
                "rust",
                12345,
                "https://github.com/rust-lang/rust.git",
            ),
            (
                "https://api.github.com/repos/rust-lang/rust/pulls/12345",
                Provider::GitHub,
                "rust-lang",
                "rust",
                12345,
                "https://github.com/rust-lang/rust.git",
            ),
            (
                "https://ghe.corp.internal/platform/api/pull/7",
                Provider::GitHub,
                "platform",
                "api",
                7,
                "https://ghe.corp.internal/platform/api.git",
            ),
            (
                "https://gitlab.com/gitlab-org/gitlab/-/merge_requests/999",
                Provider::GitLab,
                "gitlab-org",
                "gitlab",
                999,
                "https://gitlab.com/gitlab-org/gitlab.git",
            ),
            // Nested subgroups, and the pre-11.0 URL shape without `/-/`.
            (
                "https://gitlab.com/a/b/c/repo/-/merge_requests/4",
                Provider::GitLab,
                "a/b/c",
                "repo",
                4,
                "https://gitlab.com/a/b/c/repo.git",
            ),
            (
                "https://gitlab.example.com/team/repo/merge_requests/4",
                Provider::GitLab,
                "team",
                "repo",
                4,
                "https://gitlab.example.com/team/repo.git",
            ),
            (
                "https://codeberg.org/forgejo/forgejo/pulls/321",
                Provider::Gitea,
                "forgejo",
                "forgejo",
                321,
                "https://codeberg.org/forgejo/forgejo.git",
            ),
            (
                "http://gitea.internal:3000/ops/infra/pulls/8",
                Provider::Gitea,
                "ops",
                "infra",
                8,
                "http://gitea.internal:3000/ops/infra.git",
            ),
            (
                "https://gitee.com/openharmony/docs/pulls/55",
                Provider::Gitea,
                "openharmony",
                "docs",
                55,
                "https://gitee.com/openharmony/docs.git",
            ),
            (
                "https://bitbucket.org/atlassian/stash/pull-requests/42",
                Provider::BitbucketCloud,
                "atlassian",
                "stash",
                42,
                "https://bitbucket.org/atlassian/stash.git",
            ),
            (
                "https://bb.corp.com/projects/PLAT/repos/api/pull-requests/17/overview",
                Provider::BitbucketServer,
                "PLAT",
                "api",
                17,
                "https://bb.corp.com/scm/PLAT/api.git",
            ),
            // Bitbucket DC behind a context path.
            (
                "https://corp.com/bitbucket/projects/PLAT/repos/api/pull-requests/17",
                Provider::BitbucketServer,
                "PLAT",
                "api",
                17,
                "https://corp.com/bitbucket/scm/PLAT/api.git",
            ),
            (
                "https://dev.azure.com/contoso/Payments/_git/gateway/pullrequest/88",
                Provider::AzureDevOps,
                "contoso/Payments",
                "gateway",
                88,
                "https://dev.azure.com/contoso/Payments/_git/gateway",
            ),
            (
                "https://contoso.visualstudio.com/Payments/_git/gateway/pullrequest/88",
                Provider::AzureDevOps,
                "Payments",
                "gateway",
                88,
                "https://contoso.visualstudio.com/Payments/_git/gateway",
            ),
            (
                "https://gerrit.googlesource.com/c/gerrit/+/400123",
                Provider::Gerrit,
                "",
                "gerrit",
                400123,
                "https://gerrit.googlesource.com/gerrit",
            ),
            (
                "https://sourceforge.net/p/mingw/mingw-org-wsl/merge-requests/3",
                Provider::SourceForge,
                "mingw",
                "mingw-org-wsl",
                3,
                "https://git.code.sf.net/p/mingw/mingw-org-wsl",
            ),
            (
                "https://code.launchpad.net/~user/proj/+git/repo/+merge/456",
                Provider::Launchpad,
                "~user/proj/+git",
                "repo",
                456,
                "https://git.launchpad.net/~user/proj/+git/repo",
            ),
        ];

        for (url, provider, owner, repo, number, clone_url) in cases {
            let got = p(url);
            assert_eq!(
                (got.provider, got.owner.as_str(), got.repo.as_str(), got.number, got.clone_url.as_str()),
                (*provider, *owner, *repo, *number, *clone_url),
                "parsing {url}"
            );
        }
    }

    #[test]
    fn gerrit_url_pins_patchset_and_shards_ref() {
        let pinned = p("https://gerrit.example.org/c/platform/build/+/1234/7");
        assert_eq!(pinned.patchset, Some(7));
        assert_eq!(pinned.repo, "build");
        assert_eq!(pinned.owner, "platform");
        // Gerrit sharding: last two digits of the change number, zero padded.
        assert_eq!(pinned.ref_glob().as_deref(), Some("refs/changes/34/1234/*"));
        let single_digit = p("https://gerrit.example.org/c/proj/+/7");
        assert_eq!(single_digit.ref_glob().as_deref(), Some("refs/changes/07/7/*"));
    }

    #[test]
    fn ref_globs_match_provider_namespaces() {
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
        assert_eq!(
            p("https://dev.azure.com/o/p/_git/r/pullrequest/4").ref_glob().as_deref(),
            Some("refs/pull/4/*")
        );
        // No git-visible refs — callers must surface a "use branches" error.
        assert_eq!(p("https://bitbucket.org/w/r/pull-requests/5").ref_glob(), None);
        assert_eq!(
            p("https://code.launchpad.net/~u/p/+git/r/+merge/6").ref_glob(),
            None
        );
    }

    #[test]
    fn picks_head_and_merge_refs() {
        let gh = p("https://github.com/o/r/pull/1");
        let refs = vec!["refs/pull/1/head".to_string(), "refs/pull/1/merge".to_string()];
        assert_eq!(
            gh.pick_refs(&refs),
            Some(("refs/pull/1/head".into(), Some("refs/pull/1/merge".into())))
        );
        // Conflicted PRs have no merge ref; head alone must still resolve.
        assert_eq!(
            gh.pick_refs(&["refs/pull/1/head".to_string()]),
            Some(("refs/pull/1/head".into(), None))
        );
        assert_eq!(gh.pick_refs(&[]), None);

        // Bitbucket DC names the head ref `from`.
        let bb = p("https://bb.corp.com/projects/P/repos/r/pull-requests/3");
        assert_eq!(
            bb.pick_refs(&["refs/pull-requests/3/from".to_string()]),
            Some(("refs/pull-requests/3/from".into(), None))
        );

        // Gerrit: newest patchset unless the URL pinned one.
        let g = p("https://gerrit.example.org/c/proj/+/1234");
        let ps: Vec<String> = ["1", "2", "10"]
            .iter()
            .map(|n| format!("refs/changes/34/1234/{n}"))
            .collect();
        assert_eq!(g.pick_refs(&ps), Some(("refs/changes/34/1234/10".into(), None)));
        let pinned = p("https://gerrit.example.org/c/proj/+/1234/2");
        assert_eq!(pinned.pick_refs(&ps), Some(("refs/changes/34/1234/2".into(), None)));
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
            "https://gitlab.com/o/r/-/issues/5",
            "https://example.com",
            "not a url at all",
            "",
        ] {
            assert!(parse(input).is_none(), "{input} should not parse as a PR URL");
        }
    }

    #[test]
    fn cache_dir_is_sandboxed_and_filesystem_safe() {
        let _guard = CACHE_ENV.lock();
        let tmp = tempfile::tempdir().unwrap_or_else(|e| unreachable!("tempdir: {e}"));
        std::env::set_var("DIFFCORE_REPO_CACHE_DIR", tmp.path());
        let dir = p("http://gitea.internal:3000/a/b/c/pulls/1")
            .cache_dir()
            .unwrap_or_else(|e| unreachable!("cache_dir: {e}"));
        std::env::remove_var("DIFFCORE_REPO_CACHE_DIR");
        assert!(dir.starts_with(tmp.path()));
        assert_eq!(
            dir.strip_prefix(tmp.path()).ok(),
            Some(Path::new("gitea.internal-3000/a-b/c"))
        );
    }

    /// End-to-end `resolve` against a local origin publishing GitHub-shaped PR
    /// refs. Exercises clone, fetch, merge-ref parent lookup and fork-point
    /// resolution without touching the network.
    #[test]
    fn resolve_clones_fetches_and_reports_the_fork_point() {
        let _guard = CACHE_ENV.lock();
        let tmp = tempfile::tempdir().unwrap_or_else(|e| unreachable!("tempdir: {e}"));
        let origin = tmp.path().join("origin");
        if let Err(e) = std::fs::create_dir_all(&origin) {
            unreachable!("mkdir origin: {e}");
        }

        run(&origin, &["init", "-q", "-b", "main"]);
        run(&origin, &["config", "user.email", "test@diffcore.invalid"]);
        run(&origin, &["config", "user.name", "diffcore test"]);
        write(&origin, "shared.txt", "base\n");
        run(&origin, &["add", "-A"]);
        run(&origin, &["commit", "-qm", "base"]);
        let fork_point = run(&origin, &["rev-parse", "HEAD"]);

        run(&origin, &["checkout", "-q", "-b", "feature"]);
        write(&origin, "feature.txt", "pr work\n");
        run(&origin, &["add", "-A"]);
        run(&origin, &["commit", "-qm", "pr work"]);
        let pr_head = run(&origin, &["rev-parse", "HEAD"]);

        // The target branch moves on after the fork, so the target tip and the
        // fork point differ — a two-dot diff from the tip would be wrong.
        run(&origin, &["checkout", "-q", "main"]);
        write(&origin, "unrelated.txt", "moved on\n");
        run(&origin, &["add", "-A"]);
        run(&origin, &["commit", "-qm", "main moves on"]);
        let main_tip = run(&origin, &["rev-parse", "HEAD"]);

        // Provider-published refs: head plus a merge preview whose first parent
        // is the target branch tip.
        let merge = run(
            &origin,
            &[
                "commit-tree",
                "-p",
                &main_tip,
                "-p",
                &pr_head,
                "-m",
                "merge preview",
                &format!("{main_tip}^{{tree}}"),
            ],
        );
        run(&origin, &["update-ref", "refs/pull/1/head", &pr_head]);
        run(&origin, &["update-ref", "refs/pull/1/merge", &merge]);

        std::env::set_var("DIFFCORE_REPO_CACHE_DIR", tmp.path().join("cache"));
        let pr = PrUrl {
            provider: Provider::GitHub,
            host: "example.test".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            number: 1,
            patchset: None,
            clone_url: origin.to_string_lossy().into_owned(),
        };
        let resolved = resolve(&pr).unwrap_or_else(|e| unreachable!("resolve: {e}"));
        // Second call must reuse the existing clone rather than re-cloning.
        let again = resolve(&pr).unwrap_or_else(|e| unreachable!("re-resolve: {e}"));
        std::env::remove_var("DIFFCORE_REPO_CACHE_DIR");

        assert_eq!(resolved.path, again.path);
        let dir = PathBuf::from(&resolved.path);
        assert_eq!(
            run(&dir, &["rev-parse", &resolved.base]),
            fork_point,
            "base must be the fork point"
        );
        assert_ne!(run(&dir, &["rev-parse", &resolved.base]), main_tip);
        assert_eq!(run(&dir, &["rev-parse", &resolved.head]), pr_head);
        // What the provider shows under "Files changed": the PR's file only.
        assert_eq!(
            run(
                &dir,
                &[
                    "diff",
                    "--name-only",
                    &format!("{}..{}", resolved.base, resolved.head)
                ]
            ),
            "feature.txt"
        );
    }
}
