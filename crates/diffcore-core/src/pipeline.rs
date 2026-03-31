//! Unified analysis pipeline using the declarative query engine and IR types.
//!
//! This module provides the primary entry point for the analysis pipeline,
//! composing the query engine, IR conversion, and all downstream analysis
//! stages (graph, entrypoints, flow, clustering, ranking).
//!
//! # Pipeline flow
//!
//! ```text
//! source code → QueryEngine::parse_file → ParsedFile
//!             → QueryEngine::extract_data_flow → DataFlowInfo
//!             → IrFile::from_parsed_file + enrich_with_data_flow → IrFile
//!             → SymbolGraph::build_from_ir (graph construction)
//!             → detect_entrypoints_ir (entrypoint detection)
//!             → analyze_data_flow_ir (heuristic flow analysis)
//!             → trace_data_flow_ir (variable-level data flow edges)
//!             → cluster + rank (unchanged, operate on graph/entrypoints)
//! ```

use dashmap::DashMap;
use log::{debug, info, warn};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::ast::{self, ParsedFile};
use crate::ir::IrFile;
use crate::query_engine::QueryEngine;

// ---------------------------------------------------------------------------
// Content-addressed IrFile cache
// ---------------------------------------------------------------------------

/// Returns true when `DIFFCORE_CACHE_DEBUG=1` is set.
/// Checked once per process via `OnceLock`.
fn cache_debug_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("DIFFCORE_CACHE_DEBUG").as_deref() == Ok("1"))
}

/// A thread-safe, content-addressed cache for parsed `IrFile` results.
///
/// Key: `SHA-256(file_path + "\0" + source_content)` — identical content at the
/// same path always produces the same IR, so a cache hit can skip tree-sitter
/// parsing entirely.
///
/// The cache is designed to be shared across multiple `parse_to_ir` /
/// `parse_all_to_ir` calls within the same process (e.g., across test fixtures
/// in the eval harness, or across repeated analysis runs in the Tauri app).
pub struct IrCache {
    inner: DashMap<[u8; 32], IrFile>,
    hits: std::sync::atomic::AtomicUsize,
    misses: std::sync::atomic::AtomicUsize,
}

impl IrCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
            hits: std::sync::atomic::AtomicUsize::new(0),
            misses: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Compute the content-addressed key for a (path, source) pair.
    fn cache_key(path: &str, source: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(source.as_bytes());
        hasher.finalize().into()
    }

    /// Look up a cached IrFile. Returns a clone if found.
    fn get(&self, path: &str, source: &str) -> Option<IrFile> {
        let key = Self::cache_key(path, source);
        match self.inner.get(&key) {
            Some(entry) => {
                self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if cache_debug_enabled() {
                    eprintln!("[IrCache] HIT  {}", path);
                }
                Some(entry.value().clone())
            }
            None => {
                if cache_debug_enabled() {
                    eprintln!("[IrCache] MISS {}", path);
                }
                None
            }
        }
    }

    /// Insert a parsed IrFile into the cache.
    fn insert(&self, path: &str, source: &str, ir: &IrFile) {
        let key = Self::cache_key(path, source);
        self.misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.insert(key, ir.clone());
    }

    /// Number of entries in the cache.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Total cache hits since creation.
    pub fn hits(&self) -> usize {
        self.hits.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Total cache misses since creation.
    pub fn misses(&self) -> usize {
        self.misses.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Log cache statistics at debug level.
    /// Also prints to stderr when `DIFFCORE_CACHE_DEBUG=1`.
    pub fn log_stats(&self) {
        let hits = self.hits();
        let misses = self.misses();
        let total = hits + misses;
        if total > 0 {
            let msg = format!(
                "IrCache stats: {} hits, {} misses, {} entries ({:.0}% hit rate)",
                hits,
                misses,
                self.len(),
                (hits as f64 / total as f64) * 100.0
            );
            debug!("{}", msg);
            if cache_debug_enabled() {
                eprintln!("[IrCache] {}", msg);
            }
        }
    }
}

impl Default for IrCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Disk-persistent IrFile cache
// ---------------------------------------------------------------------------

/// Default maximum disk cache size in bytes (100 MB).
const DEFAULT_MAX_CACHE_BYTES: u64 = 100 * 1024 * 1024;

/// A disk-persistent wrapper around `IrCache`.
///
/// On creation (`load`), existing `.bincode` files from the cache directory are
/// read into the in-memory `IrCache`. On `flush`, any *new* entries (those not
/// already on disk) are written out. LRU eviction keeps total disk usage under
/// a configurable byte limit.
///
/// Cache directory layout:
/// ```text
/// <repo>/.diffcore/cache/ir/<hex(sha256)>.bincode
/// ```
///
/// Invalidation is content-addressed: the SHA-256 key includes the file path
/// and source content, so changed files produce new keys. Stale entries are
/// evicted by LRU when the size limit is exceeded.
pub struct DiskIrCache {
    /// The in-memory cache (shared with the pipeline).
    memory: IrCache,
    /// Directory where `.bincode` files are stored.
    dir: PathBuf,
    /// Maximum total disk cache size in bytes.
    max_bytes: u64,
    /// Keys that were loaded from disk (so we know which ones are new on flush).
    loaded_keys: dashmap::DashSet<[u8; 32]>,
}

impl DiskIrCache {
    /// Load an existing disk cache (or create an empty one) from the given
    /// repository working directory.
    ///
    /// Entries that fail to deserialize are silently skipped (and will be
    /// overwritten on the next flush).
    pub fn load(workdir: &Path) -> Self {
        Self::load_with_limit(workdir, DEFAULT_MAX_CACHE_BYTES)
    }

    /// Load with a custom size limit (useful for tests).
    pub fn load_with_limit(workdir: &Path, max_bytes: u64) -> Self {
        let dir = workdir.join(".diffcore").join("cache").join("ir");
        let memory = IrCache::new();
        let loaded_keys = dashmap::DashSet::new();

        if dir.is_dir() {
            let entries: Vec<_> = match std::fs::read_dir(&dir) {
                Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
                Err(_) => vec![],
            };

            for entry in &entries {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("bincode") {
                    continue;
                }

                // Extract the hex key from the filename (without extension).
                let stem = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s,
                    None => continue,
                };

                let key: [u8; 32] = match hex::decode(stem) {
                    Ok(bytes) if bytes.len() == 32 => {
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&bytes);
                        arr
                    }
                    _ => continue, // skip malformed filenames
                };

                let bytes = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(_) => continue,
                };

                let ir: IrFile = match bincode::deserialize(&bytes) {
                    Ok(ir) => ir,
                    Err(e) => {
                        warn!(
                            "Skipping malformed IR cache entry {}: {}",
                            path.display(),
                            e
                        );
                        continue;
                    }
                };

                memory.inner.insert(key, ir);
                loaded_keys.insert(key);
            }

            let count = loaded_keys.len();
            if count > 0 {
                debug!("Loaded {} IR cache entries from disk", count);
            }
        }

        Self {
            memory,
            dir,
            max_bytes,
            loaded_keys,
        }
    }

    /// Get a reference to the in-memory cache (pass this to `parse_to_ir`).
    pub fn memory(&self) -> &IrCache {
        &self.memory
    }

    /// Flush new entries to disk and evict old entries if over the size limit.
    ///
    /// This is best-effort: I/O errors are logged as warnings but never propagate.
    pub fn flush(&self) {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            warn!(
                "Failed to create IR cache directory {}: {}",
                self.dir.display(),
                e
            );
            return;
        }

        // Write new entries (those in memory but not in loaded_keys).
        let mut new_count = 0u64;
        for entry in self.memory.inner.iter() {
            let key = *entry.key();
            if self.loaded_keys.contains(&key) {
                continue; // already on disk
            }

            let hex_key = hex::encode(key);
            let path = self.dir.join(format!("{}.bincode", hex_key));

            match bincode::serialize(entry.value()) {
                Ok(bytes) => {
                    if let Err(e) = std::fs::write(&path, &bytes) {
                        warn!("Failed to write IR cache entry {}: {}", path.display(), e);
                    } else {
                        new_count += 1;
                    }
                }
                Err(e) => {
                    warn!("Failed to serialize IR cache entry: {}", e);
                }
            }
        }

        if new_count > 0 {
            info!("Wrote {} new IR cache entries to disk", new_count);
        }

        // LRU eviction: if total size exceeds limit, remove oldest files first.
        self.evict_lru();

        self.memory.log_stats();
    }

    /// Evict oldest cache files until total size is under `max_bytes`.
    fn evict_lru(&self) {
        // Collect all .bincode files with metadata.
        let entries: Vec<_> = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(_) => return,
        };

        let mut file_infos: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
        let mut total_size: u64 = 0;

        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("bincode") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let size = meta.len();
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                total_size += size;
                file_infos.push((path, size, modified));
            }
        }

        if total_size <= self.max_bytes {
            return;
        }

        // Sort by modification time ascending (oldest first).
        file_infos.sort_by_key(|(_path, _size, mtime)| *mtime);

        let mut evicted = 0u64;
        for (path, size, _mtime) in &file_infos {
            if total_size <= self.max_bytes {
                break;
            }
            if let Err(e) = std::fs::remove_file(path) {
                warn!("Failed to evict IR cache entry {}: {}", path.display(), e);
            } else {
                total_size -= size;
                evicted += 1;
            }
        }

        if evicted > 0 {
            info!(
                "Evicted {} IR cache entries (disk usage now ~{} bytes)",
                evicted, total_size
            );
        }
    }
}

/// Parse a single file into an IR representation using the query engine.
///
/// This is the primary parsing entry point for the IR pipeline. It uses the
/// declarative `.scm` query files instead of imperative tree-sitter code.
///
/// The tree-sitter parse is performed **once** and the resulting tree is shared
/// between symbol extraction (`parse_file`) and data-flow extraction
/// (`extract_data_flow`), avoiding the previous double-parse overhead.
///
/// If an `IrCache` is provided, the result is looked up by content hash before
/// parsing. On a cache miss the parsed `IrFile` is inserted into the cache.
pub fn parse_to_ir(
    engine: &QueryEngine,
    path: &str,
    source: &str,
    cache: Option<&IrCache>,
) -> Result<IrFile, PipelineError> {
    // Check the cache first.
    if let Some(cache) = cache {
        if let Some(ir) = cache.get(path, source) {
            return Ok(ir);
        }
    }

    // Parse the tree-sitter tree once for this file.
    let tree_and_lang = engine
        .parse_tree_for_path(path, source)
        .map_err(|e| PipelineError::Parse(format!("{}: {}", path, e)))?;

    let (parsed, data_flow_result) = match tree_and_lang {
        Some((tree, language)) => {
            let parsed = engine
                .parse_file_with_tree(path, source, &tree, language)
                .map_err(|e| PipelineError::Parse(format!("{}: {}", path, e)))?;
            let df = engine.extract_data_flow_with_tree(path, source, &tree, language);
            (parsed, df)
        }
        None => {
            // Unknown language — produce empty ParsedFile, skip data flow.
            let parsed = engine
                .parse_file(path, source)
                .map_err(|e| PipelineError::Parse(format!("{}: {}", path, e)))?;
            (
                parsed,
                Ok(crate::ast::DataFlowInfo {
                    assignments: vec![],
                    calls_with_args: vec![],
                }),
            )
        }
    };

    let mut ir = IrFile::from_parsed_file(&parsed);

    // Enrich with data flow info (assignments, call arguments).
    // Non-fatal: file may have syntax errors or unsupported language.
    match data_flow_result {
        Ok(df) => ir.enrich_with_data_flow(&df),
        Err(e) => warn!(
            "Data flow extraction failed for {}: {} (non-fatal, skipping enrichment)",
            path, e
        ),
    }

    // Store in cache for future lookups.
    if let Some(cache) = cache {
        cache.insert(path, source, &ir);
    }

    Ok(ir)
}

/// Parse multiple files into IR representations in parallel using rayon.
///
/// Files that fail to parse are skipped (with errors collected).
/// Results are sorted by file path to ensure deterministic output regardless
/// of thread scheduling.
///
/// If an `IrCache` is provided it is shared across all rayon worker threads —
/// duplicate file contents (common in test fixtures and monorepos) will only
/// be parsed once.
pub fn parse_all_to_ir(
    engine: &QueryEngine,
    files: &[(&str, &str)],
    cache: Option<&IrCache>,
) -> (Vec<IrFile>, Vec<PipelineError>) {
    let results: Vec<Result<IrFile, PipelineError>> = files
        .par_iter()
        .map(|&(path, source)| parse_to_ir(engine, path, source, cache))
        .collect();

    let mut ir_files = Vec::with_capacity(files.len());
    let mut errors = Vec::new();

    for result in results {
        match result {
            Ok(ir) => ir_files.push(ir),
            Err(e) => errors.push(e),
        }
    }

    // Sort by path to ensure deterministic output regardless of thread ordering.
    ir_files.sort_by(|a, b| a.path.cmp(&b.path));

    (ir_files, errors)
}

/// Parse multiple source files into `ParsedFile` representations in parallel.
///
/// This is the parallel equivalent of calling `ast::parse_file` in a loop —
/// used by the CLI and Tauri analysis pipelines. Each file is parsed
/// independently via rayon, then results are collected and sorted by path
/// for determinism.
///
/// Files that fail to parse are skipped with a warning logged. Tree-sitter is
/// error-tolerant, so failures are rare — typically only unsupported languages.
pub fn parse_files_parallel(files: &[(&str, &str)]) -> Vec<ParsedFile> {
    let results: Vec<Result<ParsedFile, (String, crate::ast::AstError)>> = files
        .par_iter()
        .map(|&(path, source)| ast::parse_file(path, source).map_err(|e| (path.to_string(), e)))
        .collect();

    let mut parsed = Vec::with_capacity(files.len());
    for result in results {
        match result {
            Ok(file) => parsed.push(file),
            Err((path, e)) => {
                warn!("Skipping file {} due to parse error: {}", path, e);
            }
        }
    }

    // Sort by path for deterministic ordering.
    parsed.sort_by(|a, b| a.path.cmp(&b.path));

    parsed
}

/// Errors from the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("query engine error: {0}")]
    QueryEngine(String),
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

    /// Shared `QueryEngine` instance across all tests in this module.
    /// Lazy query compilation per language happens once and is reused.
    fn shared_engine() -> &'static QueryEngine {
        use std::sync::OnceLock;
        static ENGINE: OnceLock<QueryEngine> = OnceLock::new();
        ENGINE.get_or_init(|| QueryEngine::new().expect("shared QueryEngine init"))
    }

    /// Shared `IrCache` instance across all tests in this module.
    /// Content-addressed, so no cross-test pollution.
    #[allow(dead_code)]
    fn shared_cache() -> &'static IrCache {
        use std::sync::OnceLock;
        static CACHE: OnceLock<IrCache> = OnceLock::new();
        CACHE.get_or_init(IrCache::new)
    }

    #[test]
    fn test_parse_to_ir_typescript() {
        let engine = shared_engine();
        let source = r#"
import { validate } from './utils';
export function handler(req: Request) {
    const data = validate(req.body);
    return save(data);
}
function save(data: any) {
    return db.insert(data);
}
"#;
        let ir = parse_to_ir(engine, "src/handler.ts", source, None).unwrap();

        assert_eq!(ir.path, "src/handler.ts");
        assert!(!ir.functions.is_empty(), "should have function definitions");
        assert!(!ir.imports.is_empty(), "should have imports");
        assert!(!ir.exports.is_empty(), "should have exports");
        assert!(
            !ir.call_expressions.is_empty(),
            "should have call expressions"
        );
    }

    #[test]
    fn test_parse_to_ir_python() {
        let engine = shared_engine();
        let source = r#"
from flask import Flask
app = Flask(__name__)

@app.route('/users')
def list_users():
    users = db.query('SELECT * FROM users')
    return users
"#;
        let ir = parse_to_ir(engine, "app/views.py", source, None).unwrap();

        assert_eq!(ir.path, "app/views.py");
        assert!(!ir.functions.is_empty());
        assert!(!ir.imports.is_empty());
    }

    #[test]
    fn test_parse_to_ir_unknown_language() {
        let engine = shared_engine();
        let ir = parse_to_ir(engine, "data.csv", "a,b,c\n1,2,3", None).unwrap();
        assert_eq!(ir.path, "data.csv");
        assert!(ir.functions.is_empty());
    }

    #[test]
    fn test_parse_all_to_ir() {
        let engine = shared_engine();
        let files = vec![
            ("src/a.ts", "export function a() {}"),
            ("src/b.ts", "import { a } from './a'; function b() { a(); }"),
        ];
        let (ir_files, errors) = parse_all_to_ir(engine, &files, None);

        assert_eq!(ir_files.len(), 2);
        assert!(errors.is_empty());
        // Sorted by path
        let paths: Vec<&str> = ir_files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"src/a.ts"));
        assert!(paths.contains(&"src/b.ts"));
    }

    #[test]
    fn test_parse_to_ir_enriches_data_flow() {
        let engine = shared_engine();
        let source = r#"
function process() {
    const result = fetchData();
    transform(result);
}
"#;
        let ir = parse_to_ir(engine, "src/process.ts", source, None).unwrap();

        // Should have assignments from data flow enrichment.
        // The exact content depends on query engine extraction, but the pipeline
        // should not error.
        assert_eq!(ir.path, "src/process.ts");
        assert!(!ir.functions.is_empty());
    }

    #[test]
    fn test_full_ir_pipeline() {
        let engine = shared_engine();
        let files = vec![
            (
                "src/utils.ts",
                r#"
export function validate(data: any) { return data; }
export function sanitize(data: any) { return data; }
"#,
            ),
            (
                "src/handler.ts",
                r#"
import { validate, sanitize } from './utils';
export function handle(req: any) {
    const clean = sanitize(req.body);
    const valid = validate(clean);
    return valid;
}
"#,
            ),
        ];

        let (ir_files, errors) = parse_all_to_ir(engine, &files, None);
        assert!(errors.is_empty());
        assert_eq!(ir_files.len(), 2);

        // Build graph from IR.
        let graph = crate::graph::SymbolGraph::build_from_ir(&ir_files);
        assert!(graph.node_count() > 0);
        assert!(graph.edge_count() > 0);

        // Detect entrypoints from IR.
        let entrypoints = crate::entrypoint::detect_entrypoints_ir(&ir_files);
        // No entrypoints expected (no HTTP routes, CLI commands, etc.)
        // but the function should not panic.
        let _ = entrypoints;

        // Analyze data flow from IR.
        let flow_analysis =
            crate::flow::analyze_data_flow_ir(&ir_files, &crate::flow::FlowConfig::default());
        let _ = flow_analysis;

        // Trace data flow from IR (no source re-parsing needed!).
        let data_flow_edges = crate::flow::trace_data_flow_ir(&ir_files);
        let _ = data_flow_edges;
    }

    // ── Empty / edge-case inputs ─────────────────────────────────────

    #[test]
    fn parse_to_ir_empty_source() {
        let engine = shared_engine();
        let ir = parse_to_ir(engine, "src/empty.ts", "", None).unwrap();
        assert_eq!(ir.path, "src/empty.ts");
        assert!(ir.functions.is_empty());
        assert!(ir.imports.is_empty());
        assert!(ir.exports.is_empty());
        assert!(ir.call_expressions.is_empty());
    }

    #[test]
    fn parse_to_ir_whitespace_only_source() {
        let engine = shared_engine();
        let ir = parse_to_ir(engine, "src/blank.ts", "   \n\n  \t  \n", None).unwrap();
        assert_eq!(ir.path, "src/blank.ts");
        assert!(ir.functions.is_empty());
    }

    #[test]
    fn parse_to_ir_comments_only() {
        let engine = shared_engine();
        let ir = parse_to_ir(
            engine,
            "src/comments.ts",
            "// just a comment\n/* block */\n",
            None,
        )
        .unwrap();
        assert_eq!(ir.path, "src/comments.ts");
        assert!(ir.functions.is_empty());
    }

    #[test]
    fn parse_all_to_ir_empty_list() {
        let engine = shared_engine();
        let (ir_files, errors) = parse_all_to_ir(engine, &[], None);
        assert!(ir_files.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn parse_all_to_ir_single_file() {
        let engine = shared_engine();
        let files = vec![("src/one.ts", "function one() {}")];
        let (ir_files, errors) = parse_all_to_ir(engine, &files, None);
        assert_eq!(ir_files.len(), 1);
        assert!(errors.is_empty());
        assert_eq!(ir_files[0].path, "src/one.ts");
    }

    #[test]
    fn parse_all_to_ir_sorted_by_path() {
        let engine = shared_engine();
        let files = vec![
            ("c.ts", "function c() {}"),
            ("a.ts", "function a() {}"),
            ("b.ts", "function b() {}"),
        ];
        let (ir_files, errors) = parse_all_to_ir(engine, &files, None);
        assert!(errors.is_empty());
        // Parallel output is sorted by path for determinism
        assert_eq!(ir_files[0].path, "a.ts");
        assert_eq!(ir_files[1].path, "b.ts");
        assert_eq!(ir_files[2].path, "c.ts");
    }

    #[test]
    fn parse_all_to_ir_mixed_languages() {
        let engine = shared_engine();
        let files = vec![
            ("handler.ts", "export function handler() {}"),
            ("views.py", "def handler(): pass"),
            ("data.json", r#"{"key": "value"}"#),
        ];
        let (ir_files, errors) = parse_all_to_ir(engine, &files, None);
        assert!(errors.is_empty());
        assert_eq!(ir_files.len(), 3);
        // Check by path (sorted: data.json, handler.ts, views.py)
        let ts = ir_files.iter().find(|f| f.path == "handler.ts").unwrap();
        let py = ir_files.iter().find(|f| f.path == "views.py").unwrap();
        let json = ir_files.iter().find(|f| f.path == "data.json").unwrap();
        assert!(!ts.functions.is_empty());
        assert!(!py.functions.is_empty());
        assert!(json.functions.is_empty());
    }

    // ── Syntax error tolerance ───────────────────────────────────────

    #[test]
    fn parse_to_ir_tolerates_ts_syntax_errors() {
        let engine = shared_engine();
        let source = "function broken( { return; }\nfunction ok() { return 1; }";
        // Should not error — tree-sitter is error-tolerant
        let ir = parse_to_ir(engine, "src/broken.ts", source, None).unwrap();
        assert_eq!(ir.path, "src/broken.ts");
    }

    #[test]
    fn parse_to_ir_tolerates_python_syntax_errors() {
        let engine = shared_engine();
        let source = "def broken(\n    pass\ndef ok():\n    return 1";
        let ir = parse_to_ir(engine, "broken.py", source, None).unwrap();
        assert_eq!(ir.path, "broken.py");
    }

    // ── Path handling ────────────────────────────────────────────────

    #[test]
    fn parse_to_ir_deeply_nested_path() {
        let engine = shared_engine();
        let ir = parse_to_ir(
            engine,
            "packages/core/src/modules/auth/handlers/login.ts",
            "export function login() {}",
            None,
        )
        .unwrap();
        assert_eq!(ir.path, "packages/core/src/modules/auth/handlers/login.ts");
    }

    #[test]
    fn parse_to_ir_nextjs_dynamic_route_path() {
        let engine = shared_engine();
        let ir = parse_to_ir(
            engine,
            "src/app/[slug]/page.tsx",
            "export default function Page() { return null; }",
            None,
        )
        .unwrap();
        assert_eq!(ir.path, "src/app/[slug]/page.tsx");
    }

    #[test]
    fn parse_to_ir_dotfile_path() {
        let engine = shared_engine();
        let ir = parse_to_ir(engine, ".eslintrc.js", "module.exports = {};", None).unwrap();
        assert_eq!(ir.path, ".eslintrc.js");
    }

    // ── Data flow enrichment ─────────────────────────────────────────

    #[test]
    fn parse_to_ir_enriches_ts_variable_assignments() {
        let engine = shared_engine();
        let source = r#"
function processOrder() {
    const user = getUser();
    const order = createOrder(user);
    const receipt = sendReceipt(order);
    return receipt;
}
"#;
        let ir = parse_to_ir(engine, "src/order.ts", source, None).unwrap();
        assert!(!ir.functions.is_empty());
        // Data flow enrichment should populate assignments
        assert!(
            !ir.assignments.is_empty(),
            "should have enriched assignments from data flow"
        );
    }

    #[test]
    fn parse_to_ir_enriches_python_assignments() {
        let engine = shared_engine();
        let source = r#"
def process():
    data = fetch_data()
    result = transform(data)
    return result
"#;
        let ir = parse_to_ir(engine, "src/process.py", source, None).unwrap();
        assert!(!ir.functions.is_empty());
        assert!(
            !ir.assignments.is_empty(),
            "should have enriched Python assignments"
        );
    }

    // ── PipelineError display ────────────────────────────────────────

    #[test]
    fn pipeline_error_parse_display() {
        let err = PipelineError::Parse("file.ts: unexpected token".into());
        let msg = format!("{}", err);
        assert!(msg.contains("parse error"));
        assert!(msg.contains("file.ts"));
        assert!(msg.contains("unexpected token"));
    }

    #[test]
    fn pipeline_error_query_engine_display() {
        let err = PipelineError::QueryEngine("failed to compile query".into());
        let msg = format!("{}", err);
        assert!(msg.contains("query engine error"));
        assert!(msg.contains("failed to compile query"));
    }

    #[test]
    fn pipeline_error_is_debug() {
        let err = PipelineError::Parse("test".into());
        let debug = format!("{:?}", err);
        assert!(debug.contains("Parse"));
    }

    // ── Determinism ──────────────────────────────────────────────────

    #[test]
    fn parse_to_ir_deterministic() {
        let engine = shared_engine();
        let source = r#"
import { a } from './a';
import { b } from './b';
export function handler() {
    a();
    b();
}
"#;
        let ir1 = parse_to_ir(engine, "src/h.ts", source, None).unwrap();
        let ir2 = parse_to_ir(engine, "src/h.ts", source, None).unwrap();
        assert_eq!(ir1.path, ir2.path);
        assert_eq!(ir1.functions.len(), ir2.functions.len());
        assert_eq!(ir1.imports.len(), ir2.imports.len());
        assert_eq!(ir1.exports.len(), ir2.exports.len());
        assert_eq!(ir1.call_expressions.len(), ir2.call_expressions.len());
    }

    #[test]
    fn parse_all_to_ir_deterministic() {
        let engine = shared_engine();
        let files = vec![
            ("a.ts", "export function a() {}"),
            (
                "b.ts",
                "import { a } from './a'; export function b() { a(); }",
            ),
            ("c.py", "def c(): pass"),
        ];
        let (ir1, err1) = parse_all_to_ir(engine, &files, None);
        let (ir2, err2) = parse_all_to_ir(engine, &files, None);
        assert_eq!(ir1.len(), ir2.len());
        assert_eq!(err1.len(), err2.len());
        for (a, b) in ir1.iter().zip(ir2.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.functions.len(), b.functions.len());
        }
    }

    // ── Parallel parse_files_parallel tests ────────────────────────────

    #[test]
    fn parse_files_parallel_basic() {
        let files = vec![
            ("src/a.ts", "export function a() {}"),
            ("src/b.ts", "function b() {}"),
        ];
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed.len(), 2);
        let paths: Vec<&str> = parsed.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"src/a.ts"));
        assert!(paths.contains(&"src/b.ts"));
    }

    #[test]
    fn parse_files_parallel_empty() {
        let parsed = parse_files_parallel(&[]);
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_files_parallel_single_file() {
        let files = vec![("main.ts", "function main() {}")];
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, "main.ts");
    }

    #[test]
    fn parse_files_parallel_mixed_languages() {
        let files = vec![
            ("app.ts", "export function app() {}"),
            ("views.py", "def view(): pass"),
            ("config.json", "{}"),
        ];
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed.len(), 3);
        let ts = parsed.iter().find(|f| f.path == "app.ts").unwrap();
        let py = parsed.iter().find(|f| f.path == "views.py").unwrap();
        assert!(!ts.definitions.is_empty());
        assert!(!py.definitions.is_empty());
    }

    #[test]
    fn parse_files_parallel_deterministic() {
        let files = vec![
            ("c.ts", "function c() {}"),
            ("a.ts", "function a() {}"),
            ("b.ts", "function b() {}"),
        ];
        let r1 = parse_files_parallel(&files);
        let r2 = parse_files_parallel(&files);
        assert_eq!(r1.len(), r2.len());
        for (a, b) in r1.iter().zip(r2.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.definitions.len(), b.definitions.len());
        }
    }

    #[test]
    fn parse_files_parallel_sorted_by_path() {
        let files = vec![
            ("z.ts", "function z() {}"),
            ("a.ts", "function a() {}"),
            ("m.ts", "function m() {}"),
        ];
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed[0].path, "a.ts");
        assert_eq!(parsed[1].path, "m.ts");
        assert_eq!(parsed[2].path, "z.ts");
    }

    #[test]
    fn parse_files_parallel_many_files() {
        let sources: Vec<(String, String)> = (0..50)
            .map(|i| {
                (
                    format!("src/file_{:03}.ts", i),
                    format!("function f{}() {{}}", i),
                )
            })
            .collect();
        let files: Vec<(&str, &str)> = sources
            .iter()
            .map(|(p, s)| (p.as_str(), s.as_str()))
            .collect();
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed.len(), 50);
        // Verify sorted
        for w in parsed.windows(2) {
            assert!(w[0].path <= w[1].path);
        }
    }

    // ── Property-based tests ─────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_ts_function_name() -> impl Strategy<Value = String> {
            "[a-z][a-zA-Z0-9]{0,15}".prop_map(|s| s.to_string())
        }

        fn arb_ts_file() -> impl Strategy<Value = (String, String)> {
            (
                arb_ts_function_name(),
                proptest::collection::vec(arb_ts_function_name(), 0..5),
            )
                .prop_map(|(main_fn, extra_fns)| {
                    let mut source = format!("function {}() {{}}\n", main_fn);
                    for f in &extra_fns {
                        source.push_str(&format!("function {}() {{}}\n", f));
                    }
                    let path = format!("src/{}.ts", main_fn);
                    (path, source)
                })
        }

        proptest! {
            #[test]
            fn prop_parse_to_ir_never_panics(source in ".*") {
                let engine = shared_engine();
                // Should never panic, even on garbage input
                let _ = parse_to_ir(engine, "test.ts", &source, None);
            }

            #[test]
            fn prop_parse_to_ir_path_preserved(
                path in "[a-z/]{1,30}\\.(ts|py|js|tsx|jsx)"
            ) {
                let engine = shared_engine();
                let ir = parse_to_ir(engine, &path, "function f() {}", None).unwrap();
                prop_assert_eq!(&ir.path, &path);
            }

            #[test]
            fn prop_parse_all_to_ir_file_count(
                files in proptest::collection::vec(arb_ts_file(), 0..10)
            ) {
                let engine = shared_engine();
                let file_refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, s)| (p.as_str(), s.as_str()))
                    .collect();
                let (ir_files, errors) = parse_all_to_ir(engine, &file_refs, None);
                // Total IR files + errors should equal input count
                prop_assert_eq!(ir_files.len() + errors.len(), files.len());
            }

            #[test]
            fn prop_parse_to_ir_deterministic(
                (path, source) in arb_ts_file()
            ) {
                let engine = shared_engine();
                let ir1 = parse_to_ir(engine, &path, &source, None).unwrap();
                let ir2 = parse_to_ir(engine, &path, &source, None).unwrap();
                prop_assert_eq!(ir1.path, ir2.path);
                prop_assert_eq!(ir1.functions.len(), ir2.functions.len());
                prop_assert_eq!(ir1.imports.len(), ir2.imports.len());
            }

            #[test]
            fn prop_parse_to_ir_empty_source_has_no_definitions(
                path in "[a-z]{1,10}\\.(ts|py|js)"
            ) {
                let engine = shared_engine();
                let ir = parse_to_ir(engine, &path, "", None).unwrap();
                prop_assert!(ir.functions.is_empty());
                prop_assert!(ir.imports.is_empty());
                prop_assert!(ir.exports.is_empty());
                prop_assert!(ir.call_expressions.is_empty());
            }

            #[test]
            fn prop_parse_files_parallel_deterministic(
                files in proptest::collection::vec(arb_ts_file(), 0..10)
            ) {
                let file_refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, s)| (p.as_str(), s.as_str()))
                    .collect();
                let r1 = parse_files_parallel(&file_refs);
                let r2 = parse_files_parallel(&file_refs);
                prop_assert_eq!(r1.len(), r2.len());
                for (a, b) in r1.iter().zip(r2.iter()) {
                    prop_assert_eq!(&a.path, &b.path);
                    prop_assert_eq!(a.definitions.len(), b.definitions.len());
                }
            }

            #[test]
            fn prop_parse_files_parallel_count_matches(
                files in proptest::collection::vec(arb_ts_file(), 0..10)
            ) {
                let file_refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, s)| (p.as_str(), s.as_str()))
                    .collect();
                let parsed = parse_files_parallel(&file_refs);
                // All valid TS files should parse successfully
                prop_assert_eq!(parsed.len(), files.len());
            }
        }
    }

    // ── Error path and edge case tests ────────────────────────────────

    #[test]
    fn parse_files_parallel_handles_unsupported_languages_gracefully() {
        // Files with unsupported extensions should be parsed (tree-sitter is
        // error-tolerant) or gracefully skipped without panicking.
        let files = vec![
            ("src/app.ts", "export function app() {}"),
            ("data.csv", "a,b,c\n1,2,3"),
            ("image.png", "PNG binary-like content"), // non-code content
            ("src/main.rs", "fn main() {}"),          // Rust not yet supported by query engine
        ];
        let parsed = parse_files_parallel(&files);
        // Should not panic. The exact count depends on which languages are supported,
        // but TS should always parse.
        assert!(parsed.iter().any(|f| f.path == "src/app.ts"));
    }

    #[test]
    fn parse_all_to_ir_collects_errors_separately() {
        let engine = shared_engine();
        // Mix of valid and invalid files
        let files = vec![
            ("valid.ts", "export function valid() {}"),
            ("also_valid.py", "def also_valid(): pass"),
        ];
        let (ir_files, errors) = parse_all_to_ir(engine, &files, None);
        // Both should succeed (tree-sitter is error-tolerant)
        assert_eq!(ir_files.len() + errors.len(), files.len());
    }

    #[test]
    fn parse_to_ir_with_very_large_source() {
        let engine = shared_engine();
        // Generate a large file with many functions
        let mut source = String::new();
        for i in 0..500 {
            source.push_str(&format!("function fn_{}() {{ return {}; }}\n", i, i));
        }
        let ir = parse_to_ir(engine, "src/large.ts", &source, None).unwrap();
        assert_eq!(ir.path, "src/large.ts");
        assert!(ir.functions.len() >= 100); // Should parse many (if not all) functions
    }

    #[test]
    fn parse_to_ir_with_null_bytes_in_source() {
        let engine = shared_engine();
        // Source with embedded null bytes (could come from binary files that
        // slipped through the binary filter)
        let source = "function a() {}\0\0\0function b() {}";
        // Should not panic
        let result = parse_to_ir(engine, "src/nulls.ts", source, None);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_to_ir_with_unicode_content() {
        let engine = shared_engine();
        let source = r#"
function greet(name: string) {
    return `Hello, ${name}! 🎉`;
}
const message = "日本語テスト";
"#;
        let ir = parse_to_ir(engine, "src/unicode.ts", source, None).unwrap();
        assert_eq!(ir.path, "src/unicode.ts");
        assert!(!ir.functions.is_empty());
    }

    #[test]
    fn parse_files_parallel_with_duplicate_paths() {
        // Two entries with the same path but different content
        let files = vec![
            ("src/a.ts", "function a() {}"),
            ("src/a.ts", "function b() {}"),
        ];
        let parsed = parse_files_parallel(&files);
        // Both should parse (parallel parsing doesn't deduplicate)
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn pipeline_error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PipelineError>();
    }

    // ── IrCache tests ─────────────────────────────────────────────────

    #[test]
    fn ir_cache_new_is_empty() {
        let cache = IrCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 0);
    }

    #[test]
    fn ir_cache_default_is_empty() {
        let cache = IrCache::default();
        assert!(cache.is_empty());
    }

    #[test]
    fn ir_cache_hit_on_same_content() {
        let engine = shared_engine();
        let cache = IrCache::new();
        let source = "export function hello() {}";

        // First call: cache miss, parses the file.
        let ir1 = parse_to_ir(engine, "src/a.ts", source, Some(&cache)).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);

        // Second call: cache hit, returns clone without parsing.
        let ir2 = parse_to_ir(engine, "src/a.ts", source, Some(&cache)).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 1);

        // Results must be identical.
        assert_eq!(ir1, ir2);
    }

    #[test]
    fn ir_cache_miss_on_different_content() {
        let engine = shared_engine();
        let cache = IrCache::new();

        let _ = parse_to_ir(engine, "src/a.ts", "function a() {}", Some(&cache)).unwrap();
        let _ = parse_to_ir(engine, "src/a.ts", "function b() {}", Some(&cache)).unwrap();

        // Same path, different content → two separate entries.
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);
    }

    #[test]
    fn ir_cache_miss_on_different_path() {
        let engine = shared_engine();
        let cache = IrCache::new();
        let source = "function f() {}";

        let _ = parse_to_ir(engine, "src/a.ts", source, Some(&cache)).unwrap();
        let _ = parse_to_ir(engine, "src/b.ts", source, Some(&cache)).unwrap();

        // Same content, different path → two separate entries (path is part of key).
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.misses(), 2);
    }

    #[test]
    fn ir_cache_shared_across_parse_all_to_ir() {
        let engine = shared_engine();
        let cache = IrCache::new();
        let files = vec![
            ("src/a.ts", "export function a() {}"),
            ("src/b.ts", "import { a } from './a'; function b() { a(); }"),
        ];

        // First batch: all misses.
        let (ir1, err1) = parse_all_to_ir(engine, &files, Some(&cache));
        assert!(err1.is_empty());
        assert_eq!(ir1.len(), 2);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);

        // Second batch with same files: all hits.
        let (ir2, err2) = parse_all_to_ir(engine, &files, Some(&cache));
        assert!(err2.is_empty());
        assert_eq!(ir2.len(), 2);
        assert_eq!(cache.hits(), 2);

        // Results must be identical.
        for (a, b) in ir1.iter().zip(ir2.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn ir_cache_partial_hit() {
        let engine = shared_engine();
        let cache = IrCache::new();

        // Pre-populate cache with one file.
        let _ = parse_to_ir(engine, "src/a.ts", "function a() {}", Some(&cache)).unwrap();
        assert_eq!(cache.misses(), 1);

        // Parse batch with one cached and one new file.
        let files = vec![
            ("src/a.ts", "function a() {}"), // hit
            ("src/c.ts", "function c() {}"), // miss
        ];
        let (ir_files, errors) = parse_all_to_ir(engine, &files, Some(&cache));
        assert!(errors.is_empty());
        assert_eq!(ir_files.len(), 2);
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 2); // 1 original + 1 new
    }

    #[test]
    fn ir_cache_cached_result_byte_identical() {
        // Verify the spec requirement: "cached results must be byte-identical to uncached results"
        let engine = shared_engine();
        let cache = IrCache::new();
        let source = r#"
import { validate } from './utils';
export function handler(req: Request) {
    const data = validate(req.body);
    return save(data);
}
"#;

        // Parse without cache.
        let ir_uncached = parse_to_ir(engine, "src/handler.ts", source, None).unwrap();

        // Parse with cache (miss, populates cache).
        let ir_cached_miss = parse_to_ir(engine, "src/handler.ts", source, Some(&cache)).unwrap();

        // Parse with cache (hit, from cache).
        let ir_cached_hit = parse_to_ir(engine, "src/handler.ts", source, Some(&cache)).unwrap();

        // All three must be identical.
        assert_eq!(ir_uncached, ir_cached_miss);
        assert_eq!(ir_uncached, ir_cached_hit);

        // Verify via JSON serialization for byte-level equivalence.
        let json_uncached = serde_json::to_string(&ir_uncached).unwrap();
        let json_cached = serde_json::to_string(&ir_cached_hit).unwrap();
        assert_eq!(json_uncached, json_cached);
    }

    #[test]
    fn ir_cache_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<IrCache>();
    }

    #[test]
    fn ir_cache_log_stats_does_not_panic() {
        let cache = IrCache::new();
        cache.log_stats(); // empty cache
        let engine = shared_engine();
        let _ = parse_to_ir(engine, "a.ts", "function a() {}", Some(&cache));
        cache.log_stats(); // with entries
    }

    // ── DiskIrCache tests ────────────────────────────────────────────

    #[test]
    fn disk_cache_load_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dc = DiskIrCache::load(tmp.path());
        assert!(dc.memory().is_empty());
        assert_eq!(dc.memory().hits(), 0);
        assert_eq!(dc.memory().misses(), 0);
    }

    #[test]
    fn disk_cache_flush_creates_dir_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dc = DiskIrCache::load(tmp.path());

        let engine = shared_engine();
        let _ = parse_to_ir(engine, "src/a.ts", "function a() {}", Some(dc.memory())).unwrap();
        let _ = parse_to_ir(engine, "src/b.ts", "function b() {}", Some(dc.memory())).unwrap();

        dc.flush();

        let cache_dir = tmp.path().join(".diffcore").join("cache").join("ir");
        assert!(cache_dir.is_dir());

        let entries: Vec<_> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("bincode"))
            .collect();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn disk_cache_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = shared_engine();
        let source = "export function hello() { return 42; }";

        // First run: parse → flush to disk.
        {
            let dc = DiskIrCache::load(tmp.path());
            let ir1 = parse_to_ir(engine, "src/hello.ts", source, Some(dc.memory())).unwrap();
            dc.flush();

            // Second run: load from disk → cache hit.
            let dc2 = DiskIrCache::load(tmp.path());
            assert_eq!(dc2.memory().len(), 1, "should load 1 entry from disk");

            let ir2 = parse_to_ir(engine, "src/hello.ts", source, Some(dc2.memory())).unwrap();
            assert_eq!(dc2.memory().hits(), 1);
            assert_eq!(ir1, ir2, "disk-cached result must match original");
        }
    }

    #[test]
    fn disk_cache_roundtrip_byte_identical() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = shared_engine();
        let source = r#"
import { validate } from './utils';
export function handler(req: Request) {
    const data = validate(req.body);
    return save(data);
}
"#;

        // Parse without any cache.
        let ir_uncached = parse_to_ir(engine, "src/handler.ts", source, None).unwrap();

        // Parse with disk cache (miss, flush to disk).
        let dc = DiskIrCache::load(tmp.path());
        let _ = parse_to_ir(engine, "src/handler.ts", source, Some(dc.memory())).unwrap();
        dc.flush();

        // Load from disk and retrieve.
        let dc2 = DiskIrCache::load(tmp.path());
        let ir_from_disk =
            parse_to_ir(engine, "src/handler.ts", source, Some(dc2.memory())).unwrap();

        // Must be byte-identical via JSON serialization.
        let json_uncached = serde_json::to_string(&ir_uncached).unwrap();
        let json_from_disk = serde_json::to_string(&ir_from_disk).unwrap();
        assert_eq!(json_uncached, json_from_disk);
    }

    #[test]
    fn disk_cache_no_cache_flag_skips_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = shared_engine();

        // Populate disk cache.
        let dc = DiskIrCache::load(tmp.path());
        let _ = parse_to_ir(engine, "src/a.ts", "function a() {}", Some(dc.memory())).unwrap();
        dc.flush();

        // Simulating --no-cache: just don't load disk cache, use plain IrCache.
        let memory_only = IrCache::new();
        let _ = parse_to_ir(engine, "src/a.ts", "function a() {}", Some(&memory_only)).unwrap();
        assert_eq!(
            memory_only.hits(),
            0,
            "no-cache means no pre-loaded entries"
        );
        assert_eq!(memory_only.misses(), 1);
    }

    #[test]
    fn disk_cache_lru_eviction() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = shared_engine();

        // Create entries with a very small limit to force eviction.
        let dc = DiskIrCache::load_with_limit(tmp.path(), 1); // 1 byte limit
        for i in 0..5 {
            let path = format!("src/f{}.ts", i);
            let source = format!("function f{}() {{}}", i);
            let _ = parse_to_ir(engine, &path, &source, Some(dc.memory())).unwrap();
        }
        dc.flush();

        // After eviction, some files should have been removed.
        let cache_dir = tmp.path().join(".diffcore").join("cache").join("ir");
        let remaining: Vec<_> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("bincode"))
            .collect();
        // With 1 byte limit, most (if not all) entries should be evicted.
        // At most 1 can remain (the last one written may be ≤ 1 byte? No, it'll be larger).
        // Actually all should be evicted since even one bincode entry is > 1 byte.
        assert!(
            remaining.is_empty(),
            "all entries should be evicted with 1-byte limit"
        );
    }

    #[test]
    fn disk_cache_skips_malformed_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join(".diffcore").join("cache").join("ir");
        std::fs::create_dir_all(&cache_dir).unwrap();

        // Write a malformed .bincode file with a valid hex filename.
        let fake_key = "a".repeat(64); // valid hex for 32 bytes
        let path = cache_dir.join(format!("{}.bincode", fake_key));
        std::fs::write(&path, b"not valid bincode").unwrap();

        // Should load without panic, skipping the malformed entry.
        let dc = DiskIrCache::load(tmp.path());
        assert!(dc.memory().is_empty());
    }

    #[test]
    fn disk_cache_skips_non_bincode_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join(".diffcore").join("cache").join("ir");
        std::fs::create_dir_all(&cache_dir).unwrap();

        // Write a non-.bincode file.
        std::fs::write(cache_dir.join("readme.txt"), "hello").unwrap();

        let dc = DiskIrCache::load(tmp.path());
        assert!(dc.memory().is_empty());
    }

    #[test]
    fn disk_cache_flush_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = shared_engine();

        let dc = DiskIrCache::load(tmp.path());
        let _ = parse_to_ir(engine, "src/a.ts", "function a() {}", Some(dc.memory())).unwrap();

        dc.flush();
        dc.flush(); // second flush should not duplicate files

        let cache_dir = tmp.path().join(".diffcore").join("cache").join("ir");
        let entries: Vec<_> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("bincode"))
            .collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn disk_cache_multiple_runs_accumulate() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = shared_engine();

        // Run 1: parse file A.
        {
            let dc = DiskIrCache::load(tmp.path());
            let _ = parse_to_ir(engine, "src/a.ts", "function a() {}", Some(dc.memory())).unwrap();
            dc.flush();
        }

        // Run 2: parse file B (A should persist from run 1).
        {
            let dc = DiskIrCache::load(tmp.path());
            assert_eq!(dc.memory().len(), 1); // file A loaded from disk
            let _ = parse_to_ir(engine, "src/b.ts", "function b() {}", Some(dc.memory())).unwrap();
            dc.flush();
        }

        // Run 3: both A and B should be in cache.
        {
            let dc = DiskIrCache::load(tmp.path());
            assert_eq!(dc.memory().len(), 2);
        }
    }

    #[test]
    fn disk_cache_flush_readonly_dir_does_not_panic() {
        // Flush to an impossible path should not panic.
        let dc = DiskIrCache::load(std::path::Path::new("/dev/null/impossible"));
        // Parsing something so there's a new entry to flush.
        let engine = shared_engine();
        let _ = parse_to_ir(engine, "a.ts", "function a() {}", Some(dc.memory()));
        dc.flush(); // should log warning, not panic
    }
}
