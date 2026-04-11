# Diffcore TODOs

## Next: GitHub Integration

Replace GitHub's PR review tab entirely — do the full review in Diffcore, push results back.

### Phase 1: GitHub Login + PR Fetching
- GitHub OAuth login flow in Tauri app
- `diffcore review <pr-url>` — fetch PR diff directly from GitHub API
- Store GitHub token securely (keychain / 1Password)

### Phase 2: Bidirectional Comment Sync
- Push review comments from Diffcore back to GitHub as a PR review
- Pull existing GitHub PR comments into Diffcore's review UI
- Map Diffcore flow groups to GitHub's per-file comment model
- Approve/request-changes/comment review actions from Diffcore

### Phase 2.5: GitHub Comments API — Bidirectional Sync for Repos
Tag each comment with its origin: **`online`** (created on GitHub) vs **`diffcore`** (created in Diffcore).
- Fetch all PR review comments via GitHub Comments API (`GET /repos/{owner}/{repo}/pulls/{pull_number}/comments`)
- Tag incoming comments as `online`, tag Diffcore-authored comments as `diffcore`
- On sync: pull new `online` comments into Diffcore, push new `diffcore` comments to GitHub
- Conflict resolution: `online` comments are read-only in Diffcore; `diffcore` comments are editable in both but GitHub edits create a new `online` version
- Display origin tag in Diffcore UI (badge/icon distinguishing online vs diffcore)
- Support filtering/sorting by origin tag in the review panel
- Webhook listener (or polling fallback) to detect new `online` comments in real-time

### Phase 3: Structured Agent Feedback Loop
- Reject a group with structured feedback
- Generate a prompt for the agent to fix the rejected group
- Track review iterations (attempt #1 → #2 → #3)

### Future: Prompt Alignment
- Ingest the agent prompt/task/issue that produced the PR
- Score diff against intent: "here's what was asked, here's what was done, here's what's missing"
- Flag unnecessary/unrelated changes that don't trace back to the task
