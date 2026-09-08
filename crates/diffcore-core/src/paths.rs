//! XDG base directory resolution for diffcore's per-user state.
//!
//! Config, cache and state each get their own root, following the XDG Base
//! Directory spec on every unix (macOS included — this matches git/nvim
//! convention rather than `~/Library/Application Support`). `USERPROFILE` is
//! the last resort so Windows resolves to something instead of nothing.
//!
//! Repo-local `<repo>/.diffcore/` is project state and is not resolved here.

use std::path::PathBuf;

/// Per-user config root: `$DIFFCORE_CONFIG_HOME`, `$XDG_CONFIG_HOME/diffcore`,
/// or `$HOME/.config/diffcore`.
pub fn config_dir() -> Option<PathBuf> {
    resolve("DIFFCORE_CONFIG_HOME", "XDG_CONFIG_HOME", &[".config"])
}

/// Per-user cache root: `$DIFFCORE_CACHE_HOME`, `$XDG_CACHE_HOME/diffcore`,
/// or `$HOME/.cache/diffcore`.
pub fn cache_dir() -> Option<PathBuf> {
    resolve("DIFFCORE_CACHE_HOME", "XDG_CACHE_HOME", &[".cache"])
}

/// Per-user data root: `$DIFFCORE_DATA_HOME`, `$XDG_DATA_HOME/diffcore`,
/// or `$HOME/.local/share/diffcore`.
///
/// For state the user authored and would be upset to lose. Unlike
/// [`cache_dir`], which the XDG spec declares safe for anything to delete.
pub fn data_dir() -> Option<PathBuf> {
    resolve("DIFFCORE_DATA_HOME", "XDG_DATA_HOME", &[".local", "share"])
}

/// Per-user state root: `$DIFFCORE_STATE_HOME`, `$XDG_STATE_HOME/diffcore`,
/// or `$HOME/.local/state/diffcore`.
pub fn state_dir() -> Option<PathBuf> {
    resolve(
        "DIFFCORE_STATE_HOME",
        "XDG_STATE_HOME",
        &[".local", "state"],
    )
}

/// Pre-XDG location (`$HOME/.diffcore`), kept as a read-only fallback so
/// existing installs keep their config. Never written to.
pub fn legacy_dir() -> Option<PathBuf> {
    home().map(|home| home.join(".diffcore"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn resolve(override_var: &str, xdg_var: &str, home_suffix: &[&str]) -> Option<PathBuf> {
    resolve_from(
        std::env::var_os(override_var).map(PathBuf::from),
        std::env::var_os(xdg_var).map(PathBuf::from),
        home(),
        home_suffix,
    )
}

/// Pure core of [`resolve`], with the environment passed in.
///
/// The XDG spec says a relative `$XDG_*_HOME` is invalid and must be ignored.
fn resolve_from(
    override_home: Option<PathBuf>,
    xdg_home: Option<PathBuf>,
    home: Option<PathBuf>,
    home_suffix: &[&str],
) -> Option<PathBuf> {
    if let Some(path) = override_home.filter(|path| !path.as_os_str().is_empty()) {
        return Some(path);
    }
    if let Some(path) = xdg_home.filter(|path| path.is_absolute()) {
        return Some(path.join("diffcore"));
    }
    let mut path = home?;
    for segment in home_suffix {
        path.push(segment);
    }
    path.push("diffcore");
    Some(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::Path;

    fn config(
        override_home: Option<&str>,
        xdg: Option<&str>,
        home: Option<&str>,
    ) -> Option<PathBuf> {
        resolve_from(
            override_home.map(PathBuf::from),
            xdg.map(PathBuf::from),
            home.map(PathBuf::from),
            &[".config"],
        )
    }

    #[test]
    fn override_wins_over_everything() {
        let got = config(Some("/opt/dc"), Some("/xdg"), Some("/home/u")).unwrap();
        assert_eq!(got, Path::new("/opt/dc"));
    }

    #[test]
    fn xdg_home_gets_diffcore_suffix() {
        let got = config(None, Some("/xdg"), Some("/home/u")).unwrap();
        assert_eq!(got, Path::new("/xdg/diffcore"));
    }

    #[test]
    fn relative_xdg_home_is_ignored() {
        let got = config(None, Some("relative/xdg"), Some("/home/u")).unwrap();
        assert_eq!(got, Path::new("/home/u/.config/diffcore"));
    }

    #[test]
    fn empty_override_falls_through() {
        let got = config(Some(""), None, Some("/home/u")).unwrap();
        assert_eq!(got, Path::new("/home/u/.config/diffcore"));
    }

    #[test]
    fn no_home_resolves_to_nothing() {
        assert!(config(None, None, None).is_none());
    }

    #[test]
    fn state_suffix_is_nested() {
        let got = resolve_from(
            None,
            None,
            Some(PathBuf::from("/home/u")),
            &[".local", "state"],
        )
        .unwrap();
        assert_eq!(got, Path::new("/home/u/.local/state/diffcore"));
    }
}
