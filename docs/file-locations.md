## File Locations

Diffcore follows the [XDG Base Directory spec](https://specifications.freedesktop.org/basedir-spec/latest/) for everything it stores per user. Project state stays in the repo.

| Purpose | Location |
| --- | --- |
| Global config (LLM settings, API key) | `$XDG_CONFIG_HOME/diffcore/config.toml` |
| UI preferences (theme, panel layout) | `$XDG_CONFIG_HOME/diffcore/ui.toml` |
| Review comments | `$XDG_DATA_HOME/diffcore/comments/` |
| Refinement + embedding caches | `$XDG_CACHE_HOME/diffcore/` |
| Desktop log | `$XDG_STATE_HOME/diffcore/desktop.log` |
| Project config | `<repo>/.diffcore.toml` |
| IR cache, comments, eval history | `<repo>/.diffcore/` |

`DIFFCORE_CONFIG_HOME`, `DIFFCORE_DATA_HOME`, `DIFFCORE_CACHE_HOME` and `DIFFCORE_STATE_HOME` override each root. `DIFFCORE_LOG_FILE` overrides the log path for any binary.

On unix the XDG variables are honoured as-is, including on macOS — `~/.config/diffcore` rather than `~/Library/Application Support`, matching git and most CLI tooling. On Windows the roots fall back to `%USERPROFILE%`.

### Why UI preferences are a separate file

`ui.toml` is written on every theme toggle and panel resize. `config.toml` holds your API key in plaintext and is written rarely. Sharing one file would mean each save is a read-modify-write of the credential — with the desktop app and `diffcore-web` both running, a theme change could drop a key the other instance had just written.

Keeping them apart removes that class of bug instead of mitigating it. `config.toml` is written atomically (temp file + rename) and created `0600`.

### Migrating from pre-XDG installs

Older versions kept everything under `~/.diffcore/`. Two things fall back to the old location when the new one is absent:

- the global config (`~/.diffcore/config.toml`)
- review comments (`~/.diffcore/cache/comments/`)

Writes always go to the new paths; nothing is moved or deleted, so a downgrade still works. Once the new config exists you can delete `~/.diffcore/config.toml` — it holds your API key in plaintext and is no longer read. Caches and logs are regenerated rather than migrated, so the old ones are just dead weight.
