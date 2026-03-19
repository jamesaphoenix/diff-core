//! Entrypoint detection module.
//!
//! Automatically detects entry points into the application by analyzing
//! file paths, AST-extracted symbols, imports, call sites, and exports.

use crate::ast::{ImportInfo, Language, ParsedFile};
use crate::types::{Entrypoint, EntrypointType};

/// Detect all entrypoints across a set of parsed files.
pub fn detect_entrypoints(files: &[ParsedFile]) -> Vec<Entrypoint> {
    let mut entrypoints = Vec::new();
    for file in files {
        detect_file_entrypoints(file, &mut entrypoints);
    }
    // Deduplicate by (file, symbol) pair
    entrypoints.sort_by(|a, b| (&a.file, &a.symbol).cmp(&(&b.file, &b.symbol)));
    entrypoints.dedup_by(|a, b| a.file == b.file && a.symbol == b.symbol);
    entrypoints
}

fn detect_file_entrypoints(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    detect_test_file(file, out);
    detect_http_routes(file, out);
    detect_cli_commands(file, out);
    detect_queue_consumers(file, out);
    detect_cron_jobs(file, out);
    detect_react_pages(file, out);
    detect_event_handlers(file, out);
}

// ---------------------------------------------------------------------------
// Test file detection
// ---------------------------------------------------------------------------

fn detect_test_file(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    if is_test_path(&file.path) {
        // Find test functions/describes as symbols, or use the file itself
        let test_symbols: Vec<&str> = file
            .definitions
            .iter()
            .filter(|d| is_test_symbol_name(&d.name))
            .map(|d| d.name.as_str())
            .collect();

        if test_symbols.is_empty() {
            out.push(Entrypoint {
                file: file.path.clone(),
                symbol: file_stem(&file.path),
                entrypoint_type: EntrypointType::TestFile,
            });
        } else {
            for sym in test_symbols {
                out.push(Entrypoint {
                    file: file.path.clone(),
                    symbol: sym.to_string(),
                    entrypoint_type: EntrypointType::TestFile,
                });
            }
        }
    }
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    // File name patterns
    lower.contains(".test.") || lower.contains(".spec.") || lower.contains("_test.")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.ts")
        || lower.ends_with("_test.js")
        // Directory patterns
        || lower.contains("__tests__/")
        || lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.starts_with("tests/")
        || lower.starts_with("test/")
        // Python test convention
        || path.split('/').last().map_or(false, |f| f.starts_with("test_"))
        // Rust test files (though not currently parsed)
        || lower.contains("_tests.rs")
}

fn is_test_symbol_name(name: &str) -> bool {
    name.starts_with("test_")
        || name.starts_with("it_")
        || name == "describe"
        || name == "it"
        || name == "test"
}

// ---------------------------------------------------------------------------
// HTTP route detection
// ---------------------------------------------------------------------------

fn detect_http_routes(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    match file.language {
        Language::TypeScript | Language::JavaScript => detect_http_routes_js(file, out),
        Language::Python => detect_http_routes_python(file, out),
        Language::Unknown => {}
    }
}

/// Detect Express/Fastify-style route registrations: app.get(), router.post(), etc.
/// Also detect Next.js/file-based routing patterns.
fn detect_http_routes_js(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    let http_methods = ["get", "post", "put", "delete", "patch", "options", "head", "all"];
    let router_objects = ["app", "router", "server"];

    // Check for Next.js App Router conventions (route.ts exporting HTTP methods)
    if is_nextjs_route_file(&file.path) {
        for export in &file.exports {
            let upper = export.name.to_uppercase();
            if http_methods.contains(&upper.to_lowercase().as_str()) || upper == "GET" || upper == "POST"
                || upper == "PUT" || upper == "DELETE" || upper == "PATCH"
                || upper == "OPTIONS" || upper == "HEAD"
            {
                out.push(Entrypoint {
                    file: file.path.clone(),
                    symbol: export.name.clone(),
                    entrypoint_type: EntrypointType::HttpRoute,
                });
            }
        }
    }

    // Check for Next.js Pages Router conventions (default export from pages/)
    if is_nextjs_pages_file(&file.path) {
        for export in &file.exports {
            if export.is_default {
                out.push(Entrypoint {
                    file: file.path.clone(),
                    symbol: export.name.clone(),
                    entrypoint_type: EntrypointType::HttpRoute,
                });
                break;
            }
        }
    }

    // Check call sites for router.get/post/... patterns
    for call in &file.call_sites {
        if let Some((obj, method)) = call.callee.split_once('.') {
            if router_objects.contains(&obj) && http_methods.contains(&method) {
                let symbol = call
                    .containing_function
                    .clone()
                    .unwrap_or_else(|| call.callee.clone());
                out.push(Entrypoint {
                    file: file.path.clone(),
                    symbol,
                    entrypoint_type: EntrypointType::HttpRoute,
                });
            }
        }
    }
}

fn is_nextjs_route_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    // Next.js App Router: app/**/route.ts
    (lower.contains("/app/") || lower.starts_with("app/"))
        && path
            .split('/')
            .last()
            .map_or(false, |f| f.starts_with("route."))
}

fn is_nextjs_pages_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    // Next.js Pages Router: pages/**/*.tsx (excluding _app, _document, _error, api/)
    let in_pages = lower.contains("/pages/") || lower.starts_with("pages/");
    if !in_pages {
        return false;
    }
    let filename = path.split('/').last().unwrap_or("");
    !filename.starts_with('_') && !lower.contains("/api/")
}

/// Detect Flask/FastAPI/Django route decorators in Python.
fn detect_http_routes_python(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    let route_decorators = [
        "app.route",
        "app.get",
        "app.post",
        "app.put",
        "app.delete",
        "app.patch",
        "router.route",
        "router.get",
        "router.post",
        "router.put",
        "router.delete",
        "router.patch",
        "api_view",
    ];

    // Check call sites for decorator-style route registrations
    for call in &file.call_sites {
        if route_decorators.iter().any(|d| call.callee == *d) {
            if let Some(ref func) = call.containing_function {
                out.push(Entrypoint {
                    file: file.path.clone(),
                    symbol: func.clone(),
                    entrypoint_type: EntrypointType::HttpRoute,
                });
            }
        }
    }

    // Also check if file path suggests it's a views/routes module
    let is_route_module = file.path.contains("/views/")
        || file.path.contains("/routes/")
        || file.path.contains("/endpoints/")
        || file.path.ends_with("views.py")
        || file.path.ends_with("routes.py")
        || file.path.ends_with("endpoints.py");

    if is_route_module {
        // Functions in route modules that import from web frameworks are likely handlers
        let has_web_framework_import = file.imports.iter().any(|i| is_web_framework_import(i));
        if has_web_framework_import {
            for def in &file.definitions {
                if def.kind == crate::types::SymbolKind::Function
                    && !def.name.starts_with('_')
                    && def.name != "__init__"
                {
                    out.push(Entrypoint {
                        file: file.path.clone(),
                        symbol: def.name.clone(),
                        entrypoint_type: EntrypointType::HttpRoute,
                    });
                }
            }
        }
    }
}

fn is_web_framework_import(imp: &ImportInfo) -> bool {
    let src = &imp.source;
    src == "flask"
        || src == "fastapi"
        || src.starts_with("django.")
        || src == "django"
        || src == "starlette"
        || src.starts_with("starlette.")
        || src == "sanic"
        || src == "aiohttp"
        || src.starts_with("aiohttp.")
}

// ---------------------------------------------------------------------------
// CLI command detection
// ---------------------------------------------------------------------------

fn detect_cli_commands(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    // Detect main() functions
    let has_main = file.definitions.iter().any(|d| d.name == "main");

    if has_main {
        // Python: if __name__ == '__main__' pattern (detected via main def + path convention)
        // JS/TS: main() function in entry-like files
        let is_cli_path = is_cli_file_path(&file.path);
        if is_cli_path || file.language == Language::Python {
            out.push(Entrypoint {
                file: file.path.clone(),
                symbol: "main".to_string(),
                entrypoint_type: EntrypointType::CliCommand,
            });
        }
    }

    // Check for argument parser imports (Python)
    if file.language == Language::Python {
        let has_argparse = file
            .imports
            .iter()
            .any(|i| i.source == "argparse" || i.source == "click" || i.source == "typer");
        if has_argparse {
            // Functions decorated with @click.command or @app.command are CLI entrypoints
            for call in &file.call_sites {
                if call.callee == "click.command"
                    || call.callee == "click.group"
                    || call.callee == "app.command"
                    || call.callee == "typer.command"
                {
                    if let Some(ref func) = call.containing_function {
                        out.push(Entrypoint {
                            file: file.path.clone(),
                            symbol: func.clone(),
                            entrypoint_type: EntrypointType::CliCommand,
                        });
                    }
                }
            }
        }
    }

    // JS/TS: check for commander/yargs imports
    if matches!(file.language, Language::TypeScript | Language::JavaScript) {
        let has_cli_framework = file.imports.iter().any(|i| {
            i.source == "commander"
                || i.source == "yargs"
                || i.source == "meow"
                || i.source == "cac"
        });
        if has_cli_framework {
            out.push(Entrypoint {
                file: file.path.clone(),
                symbol: file_stem(&file.path),
                entrypoint_type: EntrypointType::CliCommand,
            });
        }
    }

    // Check for bin-like file paths
    if is_bin_path(&file.path) && !has_main {
        out.push(Entrypoint {
            file: file.path.clone(),
            symbol: file_stem(&file.path),
            entrypoint_type: EntrypointType::CliCommand,
        });
    }
}

fn is_cli_file_path(path: &str) -> bool {
    path.contains("/cli/")
        || path.contains("/cmd/")
        || path.contains("/bin/")
        || path.ends_with("/main.ts")
        || path.ends_with("/main.js")
        || path.ends_with("/cli.ts")
        || path.ends_with("/cli.js")
        || path.ends_with("/cli.py")
}

fn is_bin_path(path: &str) -> bool {
    path.contains("/bin/") || path.starts_with("bin/")
}

// ---------------------------------------------------------------------------
// Queue consumer detection
// ---------------------------------------------------------------------------

fn detect_queue_consumers(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    let consumer_patterns = [
        "subscribe",
        "consume",
        "onMessage",
        "on_message",
        "process",
        "handle_message",
        "handleMessage",
    ];

    let queue_imports = [
        "amqplib",
        "bull",
        "bullmq",
        "bee-queue",
        "sqs-consumer",
        "kafkajs",
        "celery",
        "kombu",
        "pika",
        "aio_pika",
        "rq",
    ];

    let has_queue_import = file.imports.iter().any(|i| {
        queue_imports.iter().any(|q| i.source == *q || i.source.starts_with(&format!("{q}/")))
    });

    if !has_queue_import {
        return;
    }

    // Look for consumer registration call sites
    for call in &file.call_sites {
        let callee_lower = call.callee.to_lowercase();
        if consumer_patterns
            .iter()
            .any(|p| callee_lower.contains(&p.to_lowercase()))
        {
            let symbol = call
                .containing_function
                .clone()
                .unwrap_or_else(|| call.callee.clone());
            out.push(Entrypoint {
                file: file.path.clone(),
                symbol,
                entrypoint_type: EntrypointType::QueueConsumer,
            });
        }
    }

    // Check for worker/consumer file path patterns
    if is_worker_path(&file.path) {
        for def in &file.definitions {
            if def.kind == crate::types::SymbolKind::Function
                && (def.name.contains("process")
                    || def.name.contains("handle")
                    || def.name.contains("consume")
                    || def.name == "run"
                    || def.name == "execute")
            {
                out.push(Entrypoint {
                    file: file.path.clone(),
                    symbol: def.name.clone(),
                    entrypoint_type: EntrypointType::QueueConsumer,
                });
            }
        }
    }
}

fn is_worker_path(path: &str) -> bool {
    path.contains("/workers/")
        || path.contains("/jobs/")
        || path.contains("/consumers/")
        || path.contains("/tasks/")
        || path.ends_with("_worker.py")
        || path.ends_with("_worker.ts")
        || path.ends_with("_worker.js")
        || path.ends_with("Worker.ts")
        || path.ends_with("Worker.js")
}

// ---------------------------------------------------------------------------
// Cron job detection
// ---------------------------------------------------------------------------

fn detect_cron_jobs(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    let cron_imports = [
        "node-cron",
        "cron",
        "node-schedule",
        "agenda",
        "apscheduler",
        "schedule",
        "celery",
        "celery.schedules",
    ];

    let has_cron_import = file.imports.iter().any(|i| {
        cron_imports.iter().any(|c| i.source == *c || i.source.starts_with(&format!("{c}.")))
    });

    if !has_cron_import && !is_cron_path(&file.path) {
        return;
    }

    let cron_call_patterns = ["schedule", "cron", "every", "interval", "addJob", "add_job"];

    for call in &file.call_sites {
        if cron_call_patterns
            .iter()
            .any(|p| call.callee.contains(p))
        {
            let symbol = call
                .containing_function
                .clone()
                .unwrap_or_else(|| call.callee.clone());
            out.push(Entrypoint {
                file: file.path.clone(),
                symbol,
                entrypoint_type: EntrypointType::CronJob,
            });
        }
    }
}

fn is_cron_path(path: &str) -> bool {
    path.contains("/cron/")
        || path.contains("/scheduler/")
        || path.contains("/scheduled/")
        || path.ends_with("_cron.py")
        || path.ends_with("_scheduler.py")
}

// ---------------------------------------------------------------------------
// React page detection
// ---------------------------------------------------------------------------

fn detect_react_pages(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    if !matches!(file.language, Language::TypeScript | Language::JavaScript) {
        return;
    }

    // Already handled by HTTP route detection for Next.js route files
    if is_nextjs_route_file(&file.path) {
        return;
    }

    // React page conventions: default export from pages/ or app/ directories
    let is_page = is_react_page_path(&file.path);
    if !is_page {
        return;
    }

    for export in &file.exports {
        if export.is_default {
            out.push(Entrypoint {
                file: file.path.clone(),
                symbol: export.name.clone(),
                entrypoint_type: EntrypointType::ReactPage,
            });
            return;
        }
    }

    // Also check for page.tsx (Next.js App Router page component)
    if file
        .path
        .split('/')
        .last()
        .map_or(false, |f| f.starts_with("page."))
    {
        for export in &file.exports {
            if export.is_default {
                out.push(Entrypoint {
                    file: file.path.clone(),
                    symbol: export.name.clone(),
                    entrypoint_type: EntrypointType::ReactPage,
                });
                return;
            }
        }
    }
}

fn is_react_page_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    // Next.js App Router page.tsx
    let filename = path.split('/').last().unwrap_or("");
    if filename.starts_with("page.") {
        return true;
    }
    // Pages directory (excluding API routes and internals)
    let in_pages = lower.contains("/pages/") || lower.starts_with("pages/");
    if in_pages {
        return !lower.contains("/api/") && !filename.starts_with('_');
    }
    false
}

// ---------------------------------------------------------------------------
// Event handler detection
// ---------------------------------------------------------------------------

fn detect_event_handlers(file: &ParsedFile, out: &mut Vec<Entrypoint>) {
    let listener_patterns = [
        "addEventListener",
        "on",
        "addListener",
        "once",
        "subscribe",
    ];

    let event_imports = [
        "events",
        "eventemitter3",
        "mitt",
        "rxjs",
        "socket.io",
        "ws",
    ];

    let has_event_import = file.imports.iter().any(|i| {
        event_imports.iter().any(|e| i.source == *e || i.source.starts_with(&format!("{e}/")))
    });

    if !has_event_import {
        return;
    }

    for call in &file.call_sites {
        // Match patterns like emitter.on(), socket.addEventListener(), etc.
        let callee_parts: Vec<&str> = call.callee.rsplitn(2, '.').collect();
        if callee_parts.len() == 2 {
            let method = callee_parts[0];
            if listener_patterns.contains(&method) {
                let symbol = call
                    .containing_function
                    .clone()
                    .unwrap_or_else(|| call.callee.clone());
                out.push(Entrypoint {
                    file: file.path.clone(),
                    symbol,
                    entrypoint_type: EntrypointType::EventHandler,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn file_stem(path: &str) -> String {
    path.split('/')
        .last()
        .unwrap_or(path)
        .split('.')
        .next()
        .unwrap_or(path)
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{CallSite, Definition, ExportInfo, ImportInfo, ImportedName};
    use crate::types::SymbolKind;

    fn make_file(path: &str, lang: Language) -> ParsedFile {
        ParsedFile {
            path: path.to_string(),
            language: lang,
            definitions: vec![],
            imports: vec![],
            exports: vec![],
            call_sites: vec![],
        }
    }

    fn make_def(name: &str, kind: SymbolKind) -> Definition {
        Definition {
            name: name.to_string(),
            kind,
            start_line: 1,
            end_line: 5,
        }
    }

    fn make_import(source: &str) -> ImportInfo {
        ImportInfo {
            source: source.to_string(),
            names: vec![],
            is_default: false,
            is_namespace: false,
            line: 1,
        }
    }

    fn make_import_with_names(source: &str, names: Vec<&str>) -> ImportInfo {
        ImportInfo {
            source: source.to_string(),
            names: names
                .into_iter()
                .map(|n| ImportedName {
                    name: n.to_string(),
                    alias: None,
                })
                .collect(),
            is_default: false,
            is_namespace: false,
            line: 1,
        }
    }

    fn make_export(name: &str, is_default: bool) -> ExportInfo {
        ExportInfo {
            name: name.to_string(),
            is_default,
            is_reexport: false,
            source: None,
            line: 1,
        }
    }

    fn make_call(callee: &str, containing: Option<&str>) -> CallSite {
        CallSite {
            callee: callee.to_string(),
            line: 1,
            containing_function: containing.map(|s| s.to_string()),
        }
    }

    // ========================================================================
    // Test file detection
    // ========================================================================

    #[test]
    fn test_detect_test_file_by_path_dot_test() {
        let file = make_file("src/utils.test.ts", Language::TypeScript);
        let result = detect_entrypoints(&[file]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entrypoint_type, EntrypointType::TestFile);
        assert_eq!(result[0].symbol, "utils");
    }

    #[test]
    fn test_detect_test_file_by_path_dot_spec() {
        let file = make_file("src/utils.spec.js", Language::JavaScript);
        let result = detect_entrypoints(&[file]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entrypoint_type, EntrypointType::TestFile);
    }

    #[test]
    fn test_detect_test_file_python_prefix() {
        let file = make_file("tests/test_utils.py", Language::Python);
        let result = detect_entrypoints(&[file]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entrypoint_type, EntrypointType::TestFile);
    }

    #[test]
    fn test_detect_test_file_tests_directory() {
        let file = make_file("__tests__/App.test.tsx", Language::TypeScript);
        let result = detect_entrypoints(&[file]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entrypoint_type, EntrypointType::TestFile);
    }

    #[test]
    fn test_detect_test_file_with_test_functions() {
        let mut file = make_file("tests/test_auth.py", Language::Python);
        file.definitions = vec![
            make_def("test_login", SymbolKind::Function),
            make_def("test_logout", SymbolKind::Function),
            make_def("helper_setup", SymbolKind::Function),
        ];
        let result = detect_entrypoints(&[file]);
        // Should detect test_login and test_logout but not helper_setup
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|e| e.symbol == "test_login"));
        assert!(result.iter().any(|e| e.symbol == "test_logout"));
    }

    #[test]
    fn test_non_test_file_not_detected() {
        let file = make_file("src/utils.ts", Language::TypeScript);
        let result = detect_entrypoints(&[file]);
        assert!(result.is_empty());
    }

    // ========================================================================
    // HTTP route detection — JS/TS
    // ========================================================================

    #[test]
    fn test_detect_express_route() {
        let mut file = make_file("src/routes/users.ts", Language::TypeScript);
        file.call_sites = vec![
            make_call("app.get", Some("setupRoutes")),
            make_call("app.post", Some("setupRoutes")),
        ];
        let result = detect_entrypoints(&[file]);
        assert!(!result.is_empty());
        assert!(result
            .iter()
            .all(|e| e.entrypoint_type == EntrypointType::HttpRoute));
    }

    #[test]
    fn test_detect_router_route() {
        let mut file = make_file("src/routes/api.ts", Language::TypeScript);
        file.call_sites = vec![make_call("router.get", Some("getUsers"))];
        let result = detect_entrypoints(&[file]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].symbol, "getUsers");
        assert_eq!(result[0].entrypoint_type, EntrypointType::HttpRoute);
    }

    #[test]
    fn test_detect_nextjs_app_router_route() {
        let mut file = make_file("src/app/api/users/route.ts", Language::TypeScript);
        file.exports = vec![
            make_export("GET", false),
            make_export("POST", false),
        ];
        let result = detect_entrypoints(&[file]);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|e| e.symbol == "GET"));
        assert!(result.iter().any(|e| e.symbol == "POST"));
        assert!(result
            .iter()
            .all(|e| e.entrypoint_type == EntrypointType::HttpRoute));
    }

    #[test]
    fn test_detect_nextjs_pages_router() {
        let mut file = make_file("pages/about.tsx", Language::TypeScript);
        file.exports = vec![make_export("AboutPage", true)];
        let result = detect_entrypoints(&[file]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].symbol, "AboutPage");
        assert_eq!(result[0].entrypoint_type, EntrypointType::HttpRoute);
    }

    #[test]
    fn test_nextjs_pages_skip_internal_files() {
        let mut file = make_file("pages/_app.tsx", Language::TypeScript);
        file.exports = vec![make_export("App", true)];
        let result = detect_entrypoints(&[file]);
        // _app.tsx should NOT be detected as a page route
        assert!(result
            .iter()
            .all(|e| e.entrypoint_type != EntrypointType::HttpRoute));
    }

    #[test]
    fn test_non_route_call_not_detected() {
        let mut file = make_file("src/utils.ts", Language::TypeScript);
        file.call_sites = vec![make_call("console.log", Some("debug"))];
        let result = detect_entrypoints(&[file]);
        assert!(result.is_empty());
    }

    // ========================================================================
    // HTTP route detection — Python
    // ========================================================================

    #[test]
    fn test_detect_flask_route() {
        let mut file = make_file("src/routes.py", Language::Python);
        file.imports = vec![make_import_with_names("flask", vec!["Flask"])];
        file.call_sites = vec![make_call("app.route", Some("list_users"))];
        let result = detect_entrypoints(&[file]);
        assert!(result.iter().any(|e| e.symbol == "list_users"
            && e.entrypoint_type == EntrypointType::HttpRoute));
    }

    #[test]
    fn test_detect_fastapi_route() {
        let mut file = make_file("src/routes.py", Language::Python);
        file.imports = vec![make_import_with_names("fastapi", vec!["FastAPI"])];
        file.call_sites = vec![make_call("app.get", Some("get_users"))];
        let result = detect_entrypoints(&[file]);
        assert!(result.iter().any(|e| e.symbol == "get_users"
            && e.entrypoint_type == EntrypointType::HttpRoute));
    }

    #[test]
    fn test_detect_python_views_module() {
        let mut file = make_file("myapp/views.py", Language::Python);
        file.imports = vec![make_import("django.http")];
        file.definitions = vec![
            make_def("index", SymbolKind::Function),
            make_def("detail", SymbolKind::Function),
            make_def("__init__", SymbolKind::Function),
            make_def("_helper", SymbolKind::Function),
        ];
        let result = detect_entrypoints(&[file]);
        // Should detect index and detail, but not __init__ or _helper
        let http_routes: Vec<_> = result
            .iter()
            .filter(|e| e.entrypoint_type == EntrypointType::HttpRoute)
            .collect();
        assert_eq!(http_routes.len(), 2);
        assert!(http_routes.iter().any(|e| e.symbol == "index"));
        assert!(http_routes.iter().any(|e| e.symbol == "detail"));
    }

    // ========================================================================
    // CLI command detection
    // ========================================================================

    #[test]
    fn test_detect_python_main() {
        let mut file = make_file("src/cli.py", Language::Python);
        file.definitions = vec![make_def("main", SymbolKind::Function)];
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .any(|e| e.symbol == "main" && e.entrypoint_type == EntrypointType::CliCommand));
    }

    #[test]
    fn test_detect_ts_main_in_cli_path() {
        let mut file = make_file("src/cli/main.ts", Language::TypeScript);
        file.definitions = vec![make_def("main", SymbolKind::Function)];
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .any(|e| e.symbol == "main" && e.entrypoint_type == EntrypointType::CliCommand));
    }

    #[test]
    fn test_detect_commander_cli() {
        let mut file = make_file("src/cli.ts", Language::TypeScript);
        file.imports = vec![make_import("commander")];
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .any(|e| e.entrypoint_type == EntrypointType::CliCommand));
    }

    #[test]
    fn test_detect_click_cli() {
        let mut file = make_file("src/main.py", Language::Python);
        file.imports = vec![make_import("click")];
        file.definitions = vec![make_def("main", SymbolKind::Function)];
        file.call_sites = vec![make_call("click.command", Some("main"))];
        let result = detect_entrypoints(&[file]);
        let cli_entries: Vec<_> = result
            .iter()
            .filter(|e| e.entrypoint_type == EntrypointType::CliCommand)
            .collect();
        assert!(!cli_entries.is_empty());
    }

    #[test]
    fn test_detect_bin_path_as_cli() {
        let file = make_file("bin/run.js", Language::JavaScript);
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .any(|e| e.entrypoint_type == EntrypointType::CliCommand));
    }

    #[test]
    fn test_main_in_non_cli_path_not_cli_for_ts() {
        // A main() in a random TS file shouldn't be CLI
        let mut file = make_file("src/components/Widget.ts", Language::TypeScript);
        file.definitions = vec![make_def("main", SymbolKind::Function)];
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .all(|e| e.entrypoint_type != EntrypointType::CliCommand));
    }

    // ========================================================================
    // Queue consumer detection
    // ========================================================================

    #[test]
    fn test_detect_bull_queue_consumer() {
        let mut file = make_file("src/workers/email.ts", Language::TypeScript);
        file.imports = vec![make_import("bullmq")];
        file.call_sites = vec![make_call("queue.process", Some("processEmail"))];
        let result = detect_entrypoints(&[file]);
        assert!(result.iter().any(|e| e.entrypoint_type == EntrypointType::QueueConsumer));
    }

    #[test]
    fn test_detect_celery_consumer() {
        let mut file = make_file("src/tasks/send_email.py", Language::Python);
        file.imports = vec![make_import("celery")];
        file.definitions = vec![make_def("process_email", SymbolKind::Function)];
        // Worker path + celery import → queue consumer for process-like functions
        let result = detect_entrypoints(&[file]);
        assert!(result.iter().any(|e| e.entrypoint_type == EntrypointType::QueueConsumer));
    }

    #[test]
    fn test_no_queue_without_import() {
        let mut file = make_file("src/workers/email.ts", Language::TypeScript);
        file.call_sites = vec![make_call("queue.process", Some("processEmail"))];
        // No queue import → no detection
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .all(|e| e.entrypoint_type != EntrypointType::QueueConsumer));
    }

    // ========================================================================
    // Cron job detection
    // ========================================================================

    #[test]
    fn test_detect_node_cron() {
        let mut file = make_file("src/cron/cleanup.ts", Language::TypeScript);
        file.imports = vec![make_import("node-cron")];
        file.call_sites = vec![make_call("cron.schedule", Some("scheduleCleanup"))];
        let result = detect_entrypoints(&[file]);
        assert!(result.iter().any(|e| e.entrypoint_type == EntrypointType::CronJob));
    }

    #[test]
    fn test_detect_apscheduler() {
        let mut file = make_file("src/scheduler/jobs.py", Language::Python);
        file.imports = vec![make_import("apscheduler")];
        file.call_sites = vec![make_call("scheduler.add_job", Some("daily_report"))];
        let result = detect_entrypoints(&[file]);
        assert!(result.iter().any(|e| e.entrypoint_type == EntrypointType::CronJob));
    }

    // ========================================================================
    // React page detection
    // ========================================================================

    #[test]
    fn test_detect_nextjs_page_tsx() {
        let mut file = make_file("src/app/dashboard/page.tsx", Language::TypeScript);
        file.exports = vec![make_export("DashboardPage", true)];
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .any(|e| e.symbol == "DashboardPage"
                && e.entrypoint_type == EntrypointType::ReactPage));
    }

    #[test]
    fn test_detect_pages_dir_page() {
        let mut file = make_file("pages/dashboard.tsx", Language::TypeScript);
        file.exports = vec![make_export("Dashboard", true)];
        let result = detect_entrypoints(&[file]);
        // Should be detected as either HttpRoute (from pages router detection) or ReactPage
        assert!(!result.is_empty());
    }

    #[test]
    fn test_python_file_not_react_page() {
        let mut file = make_file("pages/admin.py", Language::Python);
        file.exports = vec![];
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .all(|e| e.entrypoint_type != EntrypointType::ReactPage));
    }

    // ========================================================================
    // Event handler detection
    // ========================================================================

    #[test]
    fn test_detect_socket_event_handler() {
        let mut file = make_file("src/socket/handler.ts", Language::TypeScript);
        file.imports = vec![make_import("socket.io")];
        file.call_sites = vec![make_call("socket.on", Some("handleConnection"))];
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .any(|e| e.entrypoint_type == EntrypointType::EventHandler));
    }

    #[test]
    fn test_detect_eventemitter_handler() {
        let mut file = make_file("src/events/listener.ts", Language::TypeScript);
        file.imports = vec![make_import("events")];
        file.call_sites = vec![make_call("emitter.addListener", Some("onUserCreated"))];
        let result = detect_entrypoints(&[file]);
        assert!(result.iter().any(|e| e.entrypoint_type == EntrypointType::EventHandler));
    }

    #[test]
    fn test_no_event_handler_without_import() {
        let mut file = make_file("src/events/listener.ts", Language::TypeScript);
        file.call_sites = vec![make_call("emitter.on", Some("handler"))];
        // No event import → no detection
        let result = detect_entrypoints(&[file]);
        assert!(result
            .iter()
            .all(|e| e.entrypoint_type != EntrypointType::EventHandler));
    }

    // ========================================================================
    // Multi-entrypoint and deduplication
    // ========================================================================

    #[test]
    fn test_multiple_files_multiple_entrypoints() {
        let mut route_file = make_file("src/routes/users.ts", Language::TypeScript);
        route_file.call_sites = vec![
            make_call("router.get", Some("getUsers")),
            make_call("router.post", Some("createUser")),
        ];

        let test_file = make_file("src/routes/users.test.ts", Language::TypeScript);

        let mut cli_file = make_file("src/cli/main.ts", Language::TypeScript);
        cli_file.definitions = vec![make_def("main", SymbolKind::Function)];

        let result = detect_entrypoints(&[route_file, test_file, cli_file]);

        let types: Vec<_> = result.iter().map(|e| &e.entrypoint_type).collect();
        assert!(types.contains(&&EntrypointType::HttpRoute));
        assert!(types.contains(&&EntrypointType::TestFile));
        assert!(types.contains(&&EntrypointType::CliCommand));
    }

    #[test]
    fn test_deduplication() {
        // A file that could trigger the same entrypoint via multiple detection paths
        let mut file = make_file("src/app/api/users/route.ts", Language::TypeScript);
        file.exports = vec![make_export("GET", false)];

        let result = detect_entrypoints(&[file]);
        // Should not have duplicates
        let get_entries: Vec<_> = result
            .iter()
            .filter(|e| e.symbol == "GET" && e.file == "src/app/api/users/route.ts")
            .collect();
        assert_eq!(get_entries.len(), 1);
    }

    #[test]
    fn test_empty_input() {
        let result = detect_entrypoints(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_no_entrypoints_in_plain_utility() {
        let mut file = make_file("src/utils/format.ts", Language::TypeScript);
        file.definitions = vec![
            make_def("formatDate", SymbolKind::Function),
            make_def("formatCurrency", SymbolKind::Function),
        ];
        file.imports = vec![make_import("date-fns")];
        let result = detect_entrypoints(&[file]);
        assert!(result.is_empty());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn test_unknown_language_no_entrypoints() {
        let file = make_file("main.go", Language::Unknown);
        let result = detect_entrypoints(&[file]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_file_stem_extraction() {
        assert_eq!(file_stem("src/utils/format.ts"), "format");
        assert_eq!(file_stem("main.py"), "main");
        assert_eq!(file_stem("Makefile"), "Makefile");
    }

    // ========================================================================
    // Path detection helpers
    // ========================================================================

    #[test]
    fn test_is_test_path_variants() {
        assert!(is_test_path("src/utils.test.ts"));
        assert!(is_test_path("src/utils.spec.js"));
        assert!(is_test_path("__tests__/App.test.tsx"));
        assert!(is_test_path("tests/test_utils.py"));
        assert!(is_test_path("test/integration.py"));
        assert!(is_test_path("src/auth_test.py"));
        assert!(!is_test_path("src/utils.ts"));
        assert!(!is_test_path("src/testing-utils.ts"));
    }

    #[test]
    fn test_is_nextjs_route_file() {
        assert!(is_nextjs_route_file("src/app/api/users/route.ts"));
        assert!(is_nextjs_route_file("app/api/route.ts"));
        assert!(!is_nextjs_route_file("src/app/api/users/page.ts"));
        assert!(!is_nextjs_route_file("src/routes/users.ts"));
    }

    #[test]
    fn test_is_worker_path() {
        assert!(is_worker_path("src/workers/email.ts"));
        assert!(is_worker_path("src/jobs/cleanup.py"));
        assert!(is_worker_path("src/email_worker.ts"));
        assert!(!is_worker_path("src/services/email.ts"));
    }
}
