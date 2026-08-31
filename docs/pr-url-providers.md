# Pull/Merge Request URLs

The repository field in the desktop app (and `diffcore analyze --repo`) accepts a
pull-request URL in place of a filesystem path. Diffcore clones the repository under
`~/.diffcore/cache/repos` (override with `DIFFCORE_REPO_CACHE_DIR`), fetches the
provider's PR ref namespace, and resolves the URL to a `base..head` pair matching
what the provider shows under "Files changed". A PR URL overrides `--base`/`--head`.

Clones are full and never evicted, so the cache grows without bound — a few
monorepo PRs can cost tens of gigabytes. Point `DIFFCORE_REPO_CACHE_DIR` somewhere
you are willing to delete, and keep it private: clones inherit the ambient umask,
so a world-readable directory yields world-readable clones of private repos.

No API token is involved. Cloning shells out to the `git` CLI, so private repositories
work through your existing credential helper or SSH agent.

Interactive prompts are suppressed (`GIT_TERMINAL_PROMPT`, `GIT_ASKPASS`,
`SSH_ASKPASS`, SSH `BatchMode`), but that is not a guarantee against hanging: a
configured `credential.helper` runs before any of them and may block on a locked
keyring or a biometric prompt. There is deliberately no wall-clock timeout, because
a first clone of a large monorepo legitimately takes minutes. If a resolve appears
stuck on a private repository, check whether your credential helper is waiting for
input.

Implementation: [`crates/diffcore-core/src/pr_url.rs`](../crates/diffcore-core/src/pr_url.rs).

## Supported

Every provider below publishes its pull requests as git refs, so diffcore can resolve
them with `git ls-remote` + `git fetch` alone.

| Provider | URL shape | Head ref | Merge ref |
|---|---|---|---|
| GitHub.com | `github.com/{owner}/{repo}/pull/{n}` | `refs/pull/{n}/head` | `refs/pull/{n}/merge` |
| GitHub Enterprise Server | `{host}/{owner}/{repo}/pull/{n}` | `refs/pull/{n}/head` | `refs/pull/{n}/merge` |
| GitHub REST | `api.github.com/repos/{owner}/{repo}/pulls/{n}` | `refs/pull/{n}/head` | `refs/pull/{n}/merge` |
| GitLab.com | `gitlab.com/{ns…}/{repo}/-/merge_requests/{n}` | `refs/merge-requests/{n}/head` | `refs/merge-requests/{n}/merge` |
| GitLab self-managed (CE/EE) | `{host}/{ns…}/{repo}/-/merge_requests/{n}` | `refs/merge-requests/{n}/head` | `refs/merge-requests/{n}/merge` |
| Gitea / Forgejo / Codeberg / Gitee / Gogs | `{host}/{owner}/{repo}/pulls/{n}` | `refs/pull/{n}/head` | — |
| Bitbucket Data Center / Server | `{host}/projects/{KEY}/repos/{repo}/pull-requests/{n}` | `refs/pull-requests/{n}/from` | `refs/pull-requests/{n}/merge` |
| Azure DevOps (Services, `{org}.visualstudio.com`, Server/TFS) | `{host}/{org}/{project}/_git/{repo}/pullrequest/{n}` | *see note* | `refs/pull/{n}/merge` |
| Pagure | `{host}/{repo}/pull-request/{n}`, `/fork/{user}/{repo}/pull-request/{n}` | `refs/pull/{n}/head` | — |
| Gerrit | `{host}[/{ctx}]/c/{project}/+/{change}[/{patchset}]` | `refs/changes/{nn}/{change}/{ps}` | — |

Notes:

- **Shape, not host.** Detection keys off the URL layout, so self-hosted GitHub
  Enterprise, GitLab, Gitea, Forgejo, Gogs and Bitbucket DC instances work on any
  hostname, port, or context path (`corp.com/bitbucket/projects/…`).
- **Sub-routes and suffixes** are accepted: `/pull/123/files`, `/pull/123/commits`,
  `/pull/123.diff`, `/pull/123.patch`, `/-/merge_requests/9/diffs`, query strings,
  and fragments.
- **GitLab subgroups** nest arbitrarily (`a/b/c/repo/-/merge_requests/4`), and the
  pre-11.0 URL shape without `/-/` still parses.
- **Gerrit** shards its refs by the last two digits of the change number, zero
  padded (change `1234` → `refs/changes/34/1234/{patchset}`). A patchset pinned in
  the URL wins; otherwise the newest patchset is used. The UI is commonly mounted
  under a context path (`gerrit.wikimedia.org/r/c/…`), which is handled.
- **Bitbucket DC** names the head ref `from`, not `head`.
- **Azure DevOps is unverified.** It refuses anonymous `ls-remote` even on public
  projects, so it needs configured credentials and we could not observe its ref
  layout. Published guidance consistently shows `refs/pull/{n}/merge` and not
  `/head`, so the code treats a missing head ref as normal and derives the head from
  the merge commit's second parent (base from its first). Treat this row as
  inferred, not confirmed.

## Not supported

### Recognised, but unsupported

Diffcore parses these and fails with a message naming the provider, rather than a
generic parse error — they publish no pull-request refs over git.

| Provider | URL shape | Why |
|---|---|---|
| Bitbucket Cloud | `bitbucket.org/{workspace}/{repo}/pull-requests/{n}` | No PR ref namespace; requires the 2.0 REST API |
| AWS CodeCommit | `console.aws.amazon.com/codesuite/codecommit/repositories/{repo}/pull-requests/{n}` | No PR refs; SigV4-signed API only. Closed to new customers since 2024 |
| SourceForge (Allura) | `sourceforge.net/p/{project}/{repo}/merge-requests/{n}` | Verified: `ls-remote` lists no `refs/merge-requests/*`. Allura keeps the MR head in the submitter's fork, reachable only via its REST API |
| Launchpad | `code.launchpad.net/~{user}/{proj}/+git/{repo}/+merge/{n}` | Merge proposals live outside the git repo |

### Not recognised

Listed for completeness. These fall through to being treated as a filesystem path,
so the error names the path rather than the provider.

| Provider | URL shape | Why |
|---|---|---|
| Phabricator / Phorge | `{host}/D{id}` | Differential revisions are staged as `refs/tags/phabricator/diff/{id}` only when a staging repo is configured |
| SourceHut | `lists.sr.ht/…` | Patch-series over email; no PR object |
| Radicle | `rad:{id}` | Not an http(s) URL; patches live in the peer-to-peer layer |
| Google Cloud Source Repositories | — | No pull-request concept |
| Gerrit (legacy) | `{host}/r/#/c/{n}/` | The change number lives in the URL fragment, which is stripped before parsing |

## How the base revision is chosen

Diffcore reports the *fork point*, so a plain `base..head` diff equals the provider's
"Files changed" view:

1. If the provider publishes a merge ref, its first parent is the target-branch tip.
2. Otherwise the remote's default branch is used — wrong only for open PRs that target
   a non-default branch on a provider with no merge ref.
3. If the PR already landed, `merge-base(default, head)` collapses to `head` and the
   diff would be empty, so the pre-merge tip is recovered from the merge commit that
   landed the PR.
4. The reported base is then `merge-base(target, head)`.

Squash- and rebase-merges are fine: both rewrite the commit, so the PR head is not an
ancestor of the default branch and step 2's plain merge base is already the fork point.

Known gap: **fast-forward merges** (GitLab's "Fast-forward merge" method, Gitea's
"Rebase then fast-forward", any manually fast-forwarded branch) leave no merge commit
*and* no rewritten SHA, so the base cannot be recovered from git alone. Rather than
present a successful review of zero files, `resolve` fails with an explicit error.
Fixing it properly needs a provider API call.

Cached clones live under the cache root keyed by a hash of the clone URL, and each
resolve re-fetches `refs/heads/*` so the default branch used in step 2 does not go
stale as the cache ages. A lock file serialises concurrent resolves of the same
repository, since they share one working tree.
