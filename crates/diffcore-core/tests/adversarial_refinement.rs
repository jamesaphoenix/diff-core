#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
//! Adversarial refinement comparison tests.
//!
//! Runs LLM refinement on adversarial edge-case fixtures and scores both
//! deterministic (v1) and refined (v2) groupings using the eval suite.
//! Compares scores to measure whether LLM refinement improves, maintains,
//! or degrades analysis quality on pathological inputs.
//!
//! Gated behind `DIFFCORE_RUN_LIVE_LLM_TESTS=1`. Uses VCR caching in Auto
//! mode — first run hits the real LLM API, subsequent runs replay from cache.
//!
//! Run with:
//!   DIFFCORE_RUN_LIVE_LLM_TESTS=1 cargo test --test adversarial_refinement -- --nocapture

mod helpers;

use std::path::PathBuf;

use diffcore_core::ast;
use diffcore_core::cluster;
use diffcore_core::entrypoint;
use diffcore_core::eval::scoring::{
    score_output, EvalBaseline, EvalScores, ExpectedEntrypoint, ExpectedGroup,
    RiskOrderingConstraint,
};
use diffcore_core::flow::{self, FlowConfig};
use diffcore_core::graph::SymbolGraph;
use diffcore_core::llm::anthropic::AnthropicProvider;
use diffcore_core::llm::refinement::{apply_refinement, build_refinement_request, has_refinements};
use diffcore_core::llm::vcr::{VcrMode, VcrProvider};
use diffcore_core::llm::LlmProvider;
use diffcore_core::output::{self, build_analysis_output};
use diffcore_core::rank::{self, compute_risk_score, compute_surface_area};
use diffcore_core::types::{AnalysisOutput, EntrypointType, GroupRankInput, RankWeights};
use helpers::llm_helpers::{load_env, should_run_live};
use helpers::repo_builder::RepoBuilder;

// ═══════════════════════════════════════════════════════════════════════════
// Helper: run full pipeline on a repo
// ═══════════════════════════════════════════════════════════════════════════

fn run_full_pipeline(rb: &RepoBuilder, base: &str, head: &str) -> AnalysisOutput {
    let repo = git2::Repository::open(rb.path()).expect("failed to open repo");
    let diff_result = diffcore_core::git::diff_refs(&repo, base, head).expect("diff_refs failed");

    let mut parsed_files = Vec::new();
    for file_diff in &diff_result.files {
        if let Some(ref content) = file_diff.new_content {
            let path = file_diff.path();
            if let Ok(parsed) = ast::parse_file(path, content) {
                parsed_files.push(parsed);
            }
        }
    }

    let mut graph = SymbolGraph::build(&parsed_files);
    let entrypoints = entrypoint::detect_entrypoints(&parsed_files);
    let flow_analysis = flow::analyze_data_flow(&parsed_files, &FlowConfig::default());
    flow::enrich_graph(&mut graph, &flow_analysis);

    let changed_files: Vec<String> = diff_result
        .files
        .iter()
        .map(|f| f.path().to_string())
        .collect();
    let cluster_result = cluster::cluster_files(&graph, &entrypoints, &changed_files);

    let weights = RankWeights::default();
    let rank_inputs: Vec<GroupRankInput> = cluster_result
        .groups
        .iter()
        .map(|group| {
            let risk_flags = output::compute_group_risk_flags(
                &group
                    .files
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>(),
            );
            let total_add: u32 = group.files.iter().map(|f| f.changes.additions).sum();
            let total_del: u32 = group.files.iter().map(|f| f.changes.deletions).sum();

            GroupRankInput {
                group_id: group.id.clone(),
                risk: compute_risk_score(
                    risk_flags.has_schema_change,
                    risk_flags.has_api_change,
                    risk_flags.has_auth_change,
                    false,
                ),
                centrality: 0.5,
                surface_area: compute_surface_area(total_add, total_del, 1000),
                uncertainty: if risk_flags.has_test_only { 0.1 } else { 0.5 },
            }
        })
        .collect();

    let ranked = rank::rank_groups(&rank_inputs, &weights);

    let diff_source = output::diff_source_branch(
        base,
        head,
        diff_result.base_sha.as_deref(),
        diff_result.head_sha.as_deref(),
    );

    build_analysis_output(
        &diff_result,
        diff_source,
        &parsed_files,
        &cluster_result,
        &ranked,
    )
}

/// Build a refined AnalysisOutput by swapping groups and infrastructure.
fn apply_refinement_to_output(
    original: &AnalysisOutput,
    provider: &dyn LlmProvider,
    _vcr_cache_dir: &std::path::Path,
) -> (AnalysisOutput, bool) {
    let analysis_json = serde_json::to_string_pretty(original).unwrap();
    let diff_summary = format!(
        "{} files changed, {} groups",
        original.summary.total_files_changed, original.summary.total_groups
    );

    let request = build_refinement_request(&original.groups, &analysis_json, &diff_summary);

    // Use tokio runtime to call async refine_groups
    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(provider.refine_groups(&request));

    match response {
        Ok(ref refinement) if has_refinements(refinement) => {
            match apply_refinement(
                &original.groups,
                original.infrastructure_group.as_ref(),
                refinement,
            ) {
                Ok((refined_groups, refined_infra)) => {
                    let mut refined = original.clone();
                    refined.groups = refined_groups;
                    refined.infrastructure_group = refined_infra;
                    refined.summary.total_groups = refined.groups.len() as u32;
                    (refined, true)
                }
                Err(e) => {
                    eprintln!("  Refinement validation failed: {}", e);
                    (original.clone(), false)
                }
            }
        }
        Ok(_) => {
            // No refinements suggested
            (original.clone(), false)
        }
        Err(e) => {
            eprintln!("  LLM refinement call failed: {}", e);
            (original.clone(), false)
        }
    }
}

/// VCR cache directory for adversarial refinement tests.
fn vcr_cache_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("vcr_adversarial_refinement")
}

/// Create a VCR-wrapped Anthropic provider.
fn create_vcr_provider() -> Box<dyn LlmProvider> {
    load_env();
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let inner = AnthropicProvider::new(api_key, "claude-sonnet-4-6".to_string());
    let vcr = VcrProvider::new(Box::new(inner), vcr_cache_dir(), VcrMode::Auto);
    Box::new(vcr)
}

/// Score comparison result for one fixture.
#[derive(Debug)]
struct ComparisonResult {
    fixture_name: String,
    deterministic_scores: EvalScores,
    refined_scores: EvalScores,
    was_refined: bool,
    delta_overall: f64,
}

fn print_comparison(results: &[ComparisonResult]) {
    eprintln!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    eprintln!("║          Adversarial Refinement Comparison Report                    ║");
    eprintln!("╠═══════════════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║ {:30} │ {:8} │ {:8} │ {:8} │ {:7} ║",
        "Fixture", "Det.", "Refined", "Delta", "Changed"
    );
    eprintln!("╠═══════════════════════════════════════════════════════════════════════╣");

    let mut total_det = 0.0;
    let mut total_ref = 0.0;

    for r in results {
        eprintln!(
            "║ {:30} │ {:8.4} │ {:8.4} │ {:+8.4} │ {:7} ║",
            r.fixture_name,
            r.deterministic_scores.overall,
            r.refined_scores.overall,
            r.delta_overall,
            if r.was_refined { "yes" } else { "no" }
        );
        total_det += r.deterministic_scores.overall;
        total_ref += r.refined_scores.overall;
    }

    let n = results.len() as f64;
    let avg_det = total_det / n;
    let avg_ref = total_ref / n;
    eprintln!("╠═══════════════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║ {:30} │ {:8.4} │ {:8.4} │ {:+8.4} │         ║",
        "AVERAGE",
        avg_det,
        avg_ref,
        avg_ref - avg_det
    );
    eprintln!("╚═══════════════════════════════════════════════════════════════════════╝");

    // Per-criterion breakdown
    eprintln!("\n  Per-criterion breakdown (average across all fixtures):");
    let n = results.len() as f64;
    let avg = |f: fn(&EvalScores) -> f64| -> (f64, f64) {
        let det: f64 = results
            .iter()
            .map(|r| f(&r.deterministic_scores))
            .sum::<f64>()
            / n;
        let ref_: f64 = results.iter().map(|r| f(&r.refined_scores)).sum::<f64>() / n;
        (det, ref_)
    };

    fn coherence(s: &EvalScores) -> f64 {
        s.group_coherence
    }
    fn entrypoint(s: &EvalScores) -> f64 {
        s.entrypoint_accuracy
    }
    fn ordering(s: &EvalScores) -> f64 {
        s.review_ordering
    }
    fn risk(s: &EvalScores) -> f64 {
        s.risk_reasonableness
    }
    fn language(s: &EvalScores) -> f64 {
        s.language_detection
    }
    fn accounting(s: &EvalScores) -> f64 {
        s.file_accounting
    }

    let criteria: &[(&str, fn(&EvalScores) -> f64)] = &[
        ("Group coherence  (25%)", coherence),
        ("Entrypoint acc   (20%)", entrypoint),
        ("Review ordering  (15%)", ordering),
        ("Risk reasonable  (15%)", risk),
        ("Language detect  (15%)", language),
        ("File accounting  (10%)", accounting),
    ];

    for (name, f) in criteria {
        let (det, ref_) = avg(*f);
        eprintln!(
            "    {} : det={:.4}  ref={:.4}  delta={:+.4}",
            name,
            det,
            ref_,
            ref_ - det
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Adversarial Fixture Builders + Baselines
// ═══════════════════════════════════════════════════════════════════════════

fn build_circular_imports() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/circular");
    rb.checkout("feature/circular");

    // Cycle: a → b → c → d → e → a, plus an entrypoint route
    rb.write_file(
        "src/routes/handler.ts",
        r#"
import express from 'express';
import { a } from '../modules/a';
const router = express.Router();
router.get('/cycle', (req, res) => { res.json(a()); });
export default router;
"#,
    );
    rb.write_file(
        "src/modules/a.ts",
        "import { b } from './b';\nexport function a() { return b(); }\n",
    );
    rb.write_file(
        "src/modules/b.ts",
        "import { c } from './c';\nexport function b() { return c(); }\n",
    );
    rb.write_file(
        "src/modules/c.ts",
        "import { d } from './d';\nexport function c() { return d(); }\n",
    );
    rb.write_file(
        "src/modules/d.ts",
        "import { e } from './e';\nexport function d() { return e(); }\n",
    );
    rb.write_file(
        "src/modules/e.ts",
        "import { a } from './a';\nexport function e() { return a(); }\n",
    );
    rb.commit("circular imports with entrypoint");

    let baseline = EvalBaseline {
        name: "Circular Imports".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 6,
        expected_file_count: 6,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/handler".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![ExpectedGroup {
            label: "Cycle chain from route".to_string(),
            must_contain: vec!["routes/handler".to_string(), "modules/a".to_string()],
            must_not_contain: vec![],
        }],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

fn build_diamond_dependency() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/diamond");
    rb.checkout("feature/diamond");

    // Diamond: route → serviceA + serviceB → sharedRepo
    rb.write_file(
        "src/routes/orders.ts",
        r#"
import express from 'express';
import { processPayment } from '../services/paymentService';
import { updateInventory } from '../services/inventoryService';
const router = express.Router();
router.post('/orders', (req, res) => {
    processPayment(req.body);
    updateInventory(req.body);
    res.json({ ok: true });
});
export default router;
"#,
    );
    rb.write_file(
        "src/services/paymentService.ts",
        r#"
import { findProduct } from '../repositories/productRepo';
export function processPayment(data: any) {
    const product = findProduct(data.productId);
    return { charged: product.price };
}
"#,
    );
    rb.write_file(
        "src/services/inventoryService.ts",
        r#"
import { findProduct } from '../repositories/productRepo';
export function updateInventory(data: any) {
    const product = findProduct(data.productId);
    return { remaining: product.stock - 1 };
}
"#,
    );
    rb.write_file(
        "src/repositories/productRepo.ts",
        r#"
export function findProduct(id: string) {
    return { id, price: 9.99, stock: 100 };
}
"#,
    );
    rb.commit("diamond dependency pattern");

    let baseline = EvalBaseline {
        name: "Diamond Dependency".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 4,
        expected_file_count: 4,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/orders".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![ExpectedGroup {
            label: "Order flow with shared repo".to_string(),
            must_contain: vec![
                "routes/orders".to_string(),
                "repositories/productRepo".to_string(),
            ],
            must_not_contain: vec![],
        }],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

fn build_barrel_file_explosion() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/barrel");
    rb.checkout("feature/barrel");

    // Barrel index.ts re-exports many modules; route imports from barrel
    rb.write_file(
        "src/routes/api.ts",
        r#"
import express from 'express';
import { userService, orderService, authService } from '../services';
const router = express.Router();
router.get('/users', (req, res) => { res.json(userService.list()); });
router.get('/orders', (req, res) => { res.json(orderService.list()); });
export default router;
"#,
    );

    // Barrel file
    let mut barrel_exports = String::new();
    for svc in &[
        "userService",
        "orderService",
        "authService",
        "emailService",
        "logService",
        "cacheService",
        "configService",
        "metricsService",
    ] {
        barrel_exports.push_str(&format!(
            "export {{ default as {} }} from './{}';\n",
            svc, svc
        ));
    }
    rb.write_file("src/services/index.ts", &barrel_exports);

    // Service implementations
    for svc in &[
        "userService",
        "orderService",
        "authService",
        "emailService",
        "logService",
        "cacheService",
        "configService",
        "metricsService",
    ] {
        rb.write_file(
            &format!("src/services/{}.ts", svc),
            &format!(
                "export default {{ list: () => [], get: (id: string) => null }};\nexport const name = '{}';\n",
                svc
            ),
        );
    }
    rb.commit("barrel file re-exporting many services");

    let baseline = EvalBaseline {
        name: "Barrel File Explosion".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 10,
        expected_file_count: 10, // route + barrel + 8 services
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/api".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![ExpectedGroup {
            label: "API route flow".to_string(),
            must_contain: vec!["routes/api".to_string()],
            must_not_contain: vec![],
        }],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

fn build_hub_and_spoke() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/hub-spoke");
    rb.checkout("feature/hub-spoke");

    // Hub: shared/utils.ts imported by many files
    rb.write_file(
        "src/shared/utils.ts",
        "export function formatDate(d: Date) { return d.toISOString(); }\n\
         export function log(msg: string) { console.log(msg); }\n",
    );

    // Entrypoint routes
    rb.write_file(
        "src/routes/users.ts",
        r#"
import express from 'express';
import { formatDate } from '../shared/utils';
const router = express.Router();
router.get('/users', (req, res) => { res.json({ time: formatDate(new Date()) }); });
export default router;
"#,
    );
    rb.write_file(
        "src/routes/orders.ts",
        r#"
import express from 'express';
import { log } from '../shared/utils';
const router = express.Router();
router.post('/orders', (req, res) => { log('order'); res.json({ ok: true }); });
export default router;
"#,
    );

    // Spoke files all importing utils
    for i in 1..=8 {
        rb.write_file(
            &format!("src/services/svc{}.ts", i),
            &format!(
                "import {{ log }} from '../shared/utils';\nexport function svc{}() {{ log('svc{}'); return {{}}; }}\n",
                i, i
            ),
        );
    }
    rb.commit("hub-and-spoke: shared utils imported by 10+ files");

    let baseline = EvalBaseline {
        name: "Hub and Spoke".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 11,
        expected_file_count: 11, // utils + 2 routes + 8 services
        expected_entrypoints: vec![
            ExpectedEntrypoint {
                file_contains: "routes/users".to_string(),
                ep_type: EntrypointType::HttpRoute,
            },
            ExpectedEntrypoint {
                file_contains: "routes/orders".to_string(),
                ep_type: EntrypointType::HttpRoute,
            },
        ],
        expected_groups: vec![
            ExpectedGroup {
                label: "Users route".to_string(),
                must_contain: vec!["routes/users".to_string()],
                must_not_contain: vec!["routes/orders".to_string()],
            },
            ExpectedGroup {
                label: "Orders route".to_string(),
                must_contain: vec!["routes/orders".to_string()],
                must_not_contain: vec!["routes/users".to_string()],
            },
        ],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

fn build_orphan_clusters() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/orphans");
    rb.checkout("feature/orphans");

    // Connected cluster with no entrypoint
    rb.write_file(
        "src/utils/parser.ts",
        "import { tokenize } from './tokenizer';\nexport function parse(input: string) { return tokenize(input); }\n",
    );
    rb.write_file(
        "src/utils/tokenizer.ts",
        "export function tokenize(input: string) { return input.split(' '); }\n",
    );
    rb.write_file(
        "src/utils/formatter.ts",
        "import { parse } from './parser';\nexport function format(input: string) { return parse(input).join(', '); }\n",
    );

    // Separate connected cluster (also no entrypoint)
    rb.write_file(
        "src/lib/logger.ts",
        "import { rotate } from './logRotation';\nexport function log(msg: string) { rotate(); console.log(msg); }\n",
    );
    rb.write_file(
        "src/lib/logRotation.ts",
        "export function rotate() { /* rotate logs */ }\n",
    );

    // One file with an actual entrypoint
    rb.write_file(
        "src/routes/health.ts",
        r#"
import express from 'express';
const router = express.Router();
router.get('/health', (req, res) => { res.json({ status: 'ok' }); });
export default router;
"#,
    );
    rb.commit("orphan clusters + health route");

    let baseline = EvalBaseline {
        name: "Orphan Clusters".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 6,
        expected_file_count: 6,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/health".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![ExpectedGroup {
            label: "Health route".to_string(),
            must_contain: vec!["routes/health".to_string()],
            must_not_contain: vec!["utils/parser".to_string(), "lib/logger".to_string()],
        }],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

fn build_cross_language() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/cross-lang");
    rb.checkout("feature/cross-lang");

    // TypeScript frontend
    rb.write_file(
        "frontend/src/pages/Dashboard.tsx",
        r#"
import { DataChart } from '../components/DataChart';
import { fetchMetrics } from '../api/metricsApi';
export default function Dashboard() {
    return <DataChart data={fetchMetrics()} />;
}
"#,
    );
    rb.write_file(
        "frontend/src/components/DataChart.tsx",
        r#"
export function DataChart({ data }: { data: any }) {
    return <div>{JSON.stringify(data)}</div>;
}
"#,
    );
    rb.write_file(
        "frontend/src/api/metricsApi.ts",
        r#"
export async function fetchMetrics() {
    const res = await fetch('/api/metrics');
    return res.json();
}
"#,
    );

    // Python backend
    rb.write_file(
        "backend/app/routes/metrics.py",
        r#"
from fastapi import APIRouter
from app.services.metrics_service import get_metrics

router = APIRouter()

@router.get("/api/metrics")
async def metrics():
    return get_metrics()
"#,
    );
    rb.write_file(
        "backend/app/services/metrics_service.py",
        r#"
from app.repositories.metrics_repo import query_metrics

def get_metrics():
    raw = query_metrics()
    return {"total": len(raw), "items": raw}
"#,
    );
    rb.write_file(
        "backend/app/repositories/metrics_repo.py",
        r#"
def query_metrics():
    return [{"name": "requests", "value": 42}]
"#,
    );
    rb.commit("cross-language frontend + backend");

    let baseline = EvalBaseline {
        name: "Cross-Language".to_string(),
        expected_languages: vec!["typescript".to_string(), "python".to_string()],
        min_groups: 1,
        max_groups: 6,
        expected_file_count: 6,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/metrics".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![
            ExpectedGroup {
                label: "Frontend dashboard".to_string(),
                must_contain: vec!["Dashboard".to_string(), "DataChart".to_string()],
                must_not_contain: vec!["backend".to_string()],
            },
            ExpectedGroup {
                label: "Backend metrics API".to_string(),
                must_contain: vec![
                    "routes/metrics".to_string(),
                    "services/metrics_service".to_string(),
                ],
                must_not_contain: vec!["frontend".to_string()],
            },
        ],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

fn build_reexport_chains() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/reexport");
    rb.checkout("feature/reexport");

    // Re-export chain: route → facade → adapter → core
    rb.write_file(
        "src/routes/data.ts",
        r#"
import express from 'express';
import { getData } from '../facade';
const router = express.Router();
router.get('/data', (req, res) => { res.json(getData()); });
export default router;
"#,
    );
    rb.write_file(
        "src/facade/index.ts",
        "export { getData } from '../adapter';\nexport { transformData } from '../adapter';\n",
    );
    rb.write_file(
        "src/adapter/index.ts",
        "export { getData, transformData } from '../core/dataService';\n",
    );
    rb.write_file(
        "src/core/dataService.ts",
        r#"
export function getData() { return { items: [] }; }
export function transformData(data: any) { return data; }
"#,
    );
    rb.commit("re-export chain through facade and adapter");

    let baseline = EvalBaseline {
        name: "Re-export Chains".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 4,
        expected_file_count: 4,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/data".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![ExpectedGroup {
            label: "Data route through re-exports".to_string(),
            must_contain: vec!["routes/data".to_string(), "core/dataService".to_string()],
            must_not_contain: vec![],
        }],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

fn build_deeply_nested_transitive() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/deep-chain");
    rb.checkout("feature/deep-chain");

    // 10-level import chain starting from an entrypoint
    rb.write_file(
        "src/routes/entry.ts",
        r#"
import express from 'express';
import { level1 } from '../chain/level1';
const router = express.Router();
router.get('/deep', (req, res) => { res.json(level1()); });
export default router;
"#,
    );

    for i in 1..=9 {
        let next = i + 1;
        rb.write_file(
            &format!("src/chain/level{}.ts", i),
            &format!(
                "import {{ level{} }} from './level{}';\nexport function level{}() {{ return level{}(); }}\n",
                next, next, i, next
            ),
        );
    }
    rb.write_file(
        "src/chain/level10.ts",
        "export function level10() { return { depth: 10 }; }\n",
    );
    rb.commit("10-level deep transitive import chain");

    let baseline = EvalBaseline {
        name: "Deeply Nested Transitive".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 11,
        expected_file_count: 11, // route + 10 levels
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/entry".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![ExpectedGroup {
            label: "Deep chain from entrypoint".to_string(),
            must_contain: vec!["routes/entry".to_string()],
            must_not_contain: vec![],
        }],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

// ═══════════════════════════════════════════════════════════════════════════
// All fixtures + names
// ═══════════════════════════════════════════════════════════════════════════

type FixtureBuilder = fn() -> (RepoBuilder, EvalBaseline);

const ADVERSARIAL_FIXTURES: &[(&str, FixtureBuilder)] = &[
    ("circular-imports", build_circular_imports),
    ("diamond-dependency", build_diamond_dependency),
    ("barrel-file-explosion", build_barrel_file_explosion),
    ("hub-and-spoke", build_hub_and_spoke),
    ("orphan-clusters", build_orphan_clusters),
    ("cross-language", build_cross_language),
    ("reexport-chains", build_reexport_chains),
    ("deeply-nested-transitive", build_deeply_nested_transitive),
];

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

/// Run LLM refinement on all adversarial fixtures, score deterministic vs refined,
/// and print a comparison report.
///
/// This is the main integration test for Phase 10.7's remaining task.
#[test]
fn adversarial_refinement_comparison() {
    if !should_run_live() {
        eprintln!("Skipping adversarial refinement test (set DIFFCORE_RUN_LIVE_LLM_TESTS=1)");
        return;
    }

    let provider = create_vcr_provider();
    let mut results: Vec<ComparisonResult> = Vec::new();

    for &(name, builder) in ADVERSARIAL_FIXTURES {
        eprintln!("\n── Fixture: {} ──", name);

        let (rb, baseline) = builder();
        // Find the actual feature branch
        let actual_branch = {
            let repo = git2::Repository::open(rb.path()).unwrap();
            let branches = repo.branches(Some(git2::BranchType::Local)).unwrap();
            let mut found = None;
            for branch in branches {
                let (b, _) = branch.unwrap();
                let bname = b.name().unwrap().unwrap().to_string();
                if bname != "main" {
                    found = Some(bname);
                    break;
                }
            }
            found.expect("Should have a feature branch")
        };

        eprintln!("  Branch: {}", actual_branch);

        // 1. Run deterministic pipeline
        let det_output = run_full_pipeline(&rb, "main", &actual_branch);
        let det_scores = score_output(&det_output, &baseline);
        eprintln!(
            "  Deterministic: overall={:.4}, groups={}, files={}",
            det_scores.overall,
            det_output.groups.len(),
            det_output.summary.total_files_changed
        );

        // 2. Run LLM refinement
        let (ref_output, was_refined) =
            apply_refinement_to_output(&det_output, provider.as_ref(), &vcr_cache_dir());
        let ref_scores = score_output(&ref_output, &baseline);
        eprintln!(
            "  Refined:       overall={:.4}, groups={}, files={}, changed={}",
            ref_scores.overall,
            ref_output.groups.len(),
            ref_output.summary.total_files_changed,
            was_refined
        );

        let delta = ref_scores.overall - det_scores.overall;
        eprintln!("  Delta:         {:+.4}", delta);

        results.push(ComparisonResult {
            fixture_name: name.to_string(),
            deterministic_scores: det_scores,
            refined_scores: ref_scores,
            was_refined,
            delta_overall: delta,
        });
    }

    // Print comparison report
    print_comparison(&results);

    // Assertions: all scores must be in valid range
    let mut degraded_count = 0;
    for r in &results {
        assert!(
            r.deterministic_scores.overall >= 0.0 && r.deterministic_scores.overall <= 1.0,
            "Deterministic score out of range for {}: {:.4}",
            r.fixture_name,
            r.deterministic_scores.overall
        );
        assert!(
            r.refined_scores.overall >= 0.0 && r.refined_scores.overall <= 1.0,
            "Refined score out of range for {}: {:.4}",
            r.fixture_name,
            r.refined_scores.overall
        );

        // Track per-fixture degradation (warn, don't fail — LLM outputs are
        // non-deterministic and adversarial fixtures are intentionally pathological)
        if r.delta_overall < -0.20 {
            degraded_count += 1;
            eprintln!(
                "  WARNING: {} degraded by {:.4} (det={:.4}, ref={:.4})",
                r.fixture_name,
                r.delta_overall,
                r.deterministic_scores.overall,
                r.refined_scores.overall
            );
        }
    }

    // Aggregate: average refined score should not be catastrophically worse
    let avg_det: f64 = results
        .iter()
        .map(|r| r.deterministic_scores.overall)
        .sum::<f64>()
        / results.len() as f64;
    let avg_ref: f64 = results
        .iter()
        .map(|r| r.refined_scores.overall)
        .sum::<f64>()
        / results.len() as f64;

    eprintln!(
        "\n  Degraded fixtures: {}/{}",
        degraded_count,
        results.len()
    );

    // At most half the fixtures should degrade significantly
    assert!(
        degraded_count <= results.len() / 2,
        "Too many fixtures degraded by refinement: {}/{}",
        degraded_count,
        results.len()
    );

    // Average refined score should not drop by more than 0.15 from deterministic
    assert!(
        avg_ref >= avg_det - 0.15,
        "Average refined score ({:.4}) too much lower than deterministic ({:.4})",
        avg_ref,
        avg_det
    );

    eprintln!("  All adversarial refinement assertions passed.");
}

/// Individual fixture tests — deterministic only (always runs, no LLM needed).
/// Verifies the adversarial fixtures produce valid pipeline output.
#[test]
fn adversarial_fixtures_deterministic_valid() {
    for &(name, builder) in ADVERSARIAL_FIXTURES {
        let (rb, baseline) = builder();
        let actual_branch = {
            let repo = git2::Repository::open(rb.path()).unwrap();
            let branches = repo.branches(Some(git2::BranchType::Local)).unwrap();
            let mut found = None;
            for branch in branches {
                let (b, _) = branch.unwrap();
                let bname = b.name().unwrap().unwrap().to_string();
                if bname != "main" {
                    found = Some(bname);
                    break;
                }
            }
            found.expect("Should have a feature branch")
        };

        let output = run_full_pipeline(&rb, "main", &actual_branch);
        let scores = score_output(&output, &baseline);

        // Basic validity
        assert!(
            output.summary.total_files_changed > 0,
            "{}: should have changed files",
            name
        );
        assert!(
            scores.overall >= 0.0 && scores.overall <= 1.0,
            "{}: score out of range: {:.4}",
            name,
            scores.overall
        );

        // JSON roundtrip
        let json = serde_json::to_string(&output).unwrap();
        let parsed: AnalysisOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.summary.total_files_changed, output.summary.total_files_changed,
            "{}: JSON roundtrip changed file count",
            name
        );
    }
}

/// Each adversarial fixture should detect at least the expected languages.
#[test]
fn adversarial_fixtures_language_detection() {
    for &(name, builder) in ADVERSARIAL_FIXTURES {
        let (rb, baseline) = builder();
        let actual_branch = {
            let repo = git2::Repository::open(rb.path()).unwrap();
            let branches = repo.branches(Some(git2::BranchType::Local)).unwrap();
            let mut found = None;
            for branch in branches {
                let (b, _) = branch.unwrap();
                let bname = b.name().unwrap().unwrap().to_string();
                if bname != "main" {
                    found = Some(bname);
                    break;
                }
            }
            found.expect("Should have a feature branch")
        };

        let output = run_full_pipeline(&rb, "main", &actual_branch);
        let lang_score = diffcore_core::eval::scoring::score_language_detection(&output, &baseline);
        assert!(
            lang_score >= 0.5,
            "{}: language detection too low ({:.2}), expected: {:?}, got: {:?}",
            name,
            lang_score,
            baseline.expected_languages,
            output.summary.languages_detected
        );
    }
}

/// Verify deterministic analysis is identical across two runs for each fixture.
#[test]
fn adversarial_fixtures_deterministic_consistency() {
    for &(name, builder) in ADVERSARIAL_FIXTURES {
        let (rb, _baseline) = builder();
        let actual_branch = {
            let repo = git2::Repository::open(rb.path()).unwrap();
            let branches = repo.branches(Some(git2::BranchType::Local)).unwrap();
            let mut found = None;
            for branch in branches {
                let (b, _) = branch.unwrap();
                let bname = b.name().unwrap().unwrap().to_string();
                if bname != "main" {
                    found = Some(bname);
                    break;
                }
            }
            found.expect("Should have a feature branch")
        };

        let output1 = run_full_pipeline(&rb, "main", &actual_branch);
        let output2 = run_full_pipeline(&rb, "main", &actual_branch);

        let json1 = serde_json::to_string(&output1).unwrap();
        let json2 = serde_json::to_string(&output2).unwrap();
        assert_eq!(
            json1, json2,
            "{}: deterministic pipeline should produce identical output across runs",
            name
        );
    }
}
