# Web server mode (`diffcore-web`)

Status: v1 implemented (2026-08-21)

Run the Tauri app's UI as a browser-hosted web app so diffs can be reviewed on a
remote server without a desktop environment.

## Architecture

The existing pieces already do most of the work: the UI runs in plain browsers
(`IS_TAURI` detection + mock demo mode), LLM activity streaming is already an
axum SSE server (`activity_stream.rs`, zero tauri imports), and all commands are
plain serde-typed functions.

New pieces:

1. **`State` shim** (`crates/diffcore-tauri/src/state_shim.rs`): a `Deref`
   newtype over `&AppState`, cfg-swapped for `tauri::State` when the `desktop`
   feature is off. Command signatures change from `tauri::State<'_, AppState>`
   to a bare `State<'_, AppState>` resolved by a conditional `use`. No
   per-command splitting.
2. **Feature gating** (`crates/diffcore-tauri/Cargo.toml`): `desktop` (default)
   enables tauri + plugins; `web` enables axum static serving. Both binaries
   carry `required-features`. `build.rs` only calls `tauri_build::build()` when
   the desktop feature is enabled. The web binary links no webkit/gtk.
3. **Process-global background runtime** (`runtime.rs`): background jobs and
   `block_on` go through one shared tokio runtime instead of
   `tauri::async_runtime`, so command logic works identically under tauri and
   axum.
4. **`diffcore-web` binary** (`src/bin/web.rs` + `src/web_server.rs`): axum
   server with
   - `POST /api/invoke/{cmd}`: JSON body = command args (camelCase keys,
     mirroring tauri IPC 1:1), dispatched by a hand-written match; sync commands
     run under `spawn_blocking`. Unknown/desktop-only commands → 501.
   - `GET /api/health`: `{ "ok": true, "default_repo": <--repo flag> }`; the
     UI probes this to enter web mode.
   - the existing SSE routes merged in (same origin; `activity_stream_base_url`
     is set to `""` so `stream_url` comes out relative).
   - static file serving of the built UI (`--ui-dir`, default
     `crates/diffcore-tauri/ui/dist`).
5. **Frontend third mode** (`ui/src`): transport priority is Tauri IPC → web
   API (same-origin `/api/health` probe at bootstrap) → mock demo. Branches
   that mean "real backend vs mock" use `HAS_BACKEND`; branches that are truly
   desktop-only (updater, file-watch events, editor opening) stay on
   `IS_TAURI`.

## v1 cuts (deliberate)

- **No auth.** Binds `127.0.0.1` by default; `--host` to override. Remote
  access via SSH tunnel or a reverse proxy with auth. Do not expose bare.
  Requests are still guarded against DNS rebinding and CSRF: Host/Origin
  headers must name an allowed host (`localhost`, the bind host, or
  `--allowed-host` values, e.g. a reverse-proxy domain), and invoke bodies
  must be well-formed `application/json`.
- **Single-user server.** One shared `AppState`: concurrent clients analyzing
  different repos/refs overwrite each other's `last_analysis`/refinement
  state. Fine for the personal-tunnel posture; multi-user needs per-session
  state.
- No git-HEAD/manifest watch push on web (tauri events; UI already tolerates
  the failed `watch_git_head` invoke). Manual refresh covers it.
- Updater and open-in-editor stay desktop-only.

## Usage

```
# nix (the package ships all three modes; UI assets are bundled)
nix run .#web -- --repo /srv/checkouts/myrepo --port 4400
nix run .#desktop        # tauri app
nix run .#cli -- --help  # diffcore CLI

# plain cargo
cargo build --release --bin diffcore-web --no-default-features --features web
npm --prefix crates/diffcore-tauri/ui run build
diffcore-web --repo /srv/checkouts/myrepo --port 4400

ssh -L 4400:localhost:4400 server   # then open http://localhost:4400
```

When binding non-loopback (`--host 0.0.0.0`) or fronting with a reverse proxy,
pass each hostname/IP clients will use via `--allowed-host`. The Host/Origin
guard rejects anything else with 403.

## Testing

Rust integration test spins the router and exercises `/api/health` plus an
invoke round-trip. Both feature configurations must compile; the full workspace
test suite and the UI build stay green.
