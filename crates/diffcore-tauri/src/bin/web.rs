//! diffcore-web: headless server hosting the diffcore UI in a browser.
//!
//! Binds 127.0.0.1 by default; there is no auth. For remote use, reach it
//! through an SSH tunnel or a reverse proxy that provides auth.

use std::path::PathBuf;
use std::process::ExitCode;

use diffcore_tauri::web_server::{serve, WebOptions};

const USAGE: &str = "usage: diffcore-web [--host 127.0.0.1] [--port 4400] [--ui-dir <dist>] [--repo <path>] [--allowed-host <domain>]...
Requests must carry a Host/Origin naming localhost, the bind host, or an
--allowed-host value; when binding non-loopback, pass each public hostname
or IP clients will use via --allowed-host.";

fn parse_args() -> Result<WebOptions, String> {
    let mut opts = WebOptions {
        host: "127.0.0.1".to_string(),
        port: 4400,
        ui_dir: default_ui_dir(),
        default_repo: None,
        allowed_hosts: Vec::new(),
    };
    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let mut value = |name: &str| argv.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--host" => opts.host = value("--host")?,
            "--port" => {
                opts.port = value("--port")?
                    .parse()
                    .map_err(|e| format!("invalid --port: {e}"))?
            }
            "--ui-dir" => opts.ui_dir = PathBuf::from(value("--ui-dir")?),
            "--repo" => opts.default_repo = Some(value("--repo")?),
            "--allowed-host" => opts.allowed_hosts.push(value("--allowed-host")?),
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag: {other}\n{USAGE}")),
        }
    }
    if !opts.ui_dir.join("index.html").is_file() {
        return Err(format!(
            "UI build not found at {}. Run `npm --prefix crates/diffcore-tauri/ui run build` or pass --ui-dir",
            opts.ui_dir.display()
        ));
    }
    Ok(opts)
}

/// Built UI in the nix store layout (`../share/diffcore/ui`) or the in-repo dist.
fn default_ui_dir() -> PathBuf {
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
    {
        let store_ui = exe_dir.join("../share/diffcore/ui");
        if store_ui.join("index.html").is_file() {
            return store_ui;
        }
    }
    PathBuf::from("crates/diffcore-tauri/ui/dist")
}

#[tokio::main]
async fn main() -> ExitCode {
    diffcore_core::logging::init(None);
    let opts = match parse_args() {
        Ok(opts) => opts,
        Err(msg) => {
            log::error!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(msg) = serve(opts).await {
        log::error!("{msg}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
