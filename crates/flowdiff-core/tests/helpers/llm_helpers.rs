//! LLM test utilities — env loading, live test gating, sample request fixtures.
//!
//! Shared across `llm_live.rs`, `vcr_integration.rs`, and `llm_judge.rs`.

use flowdiff_core::llm::schema::{Pass1GroupInput, Pass1Request, Pass2FileInput, Pass2Request};

/// Check if live LLM tests should run.
///
/// Returns `true` when `FLOWDIFF_RUN_LIVE_LLM_TESTS` is set to `"1"` or `"true"`.
pub fn should_run_live() -> bool {
    std::env::var("FLOWDIFF_RUN_LIVE_LLM_TESTS")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

/// Load .env file from the known path if it exists.
///
/// Only sets variables that are not already in the environment
/// (does not override explicit env vars).
pub fn load_env() {
    // Load from FLOWDIFF_ENV_FILE or fall back to .env in the repo root
    let env_path = std::env::var("FLOWDIFF_ENV_FILE").unwrap_or_else(|_| {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); // crates/flowdiff-core -> crates
        p.pop(); // crates -> repo root
        p.push(".env");
        p.to_string_lossy().to_string()
    });
    if let Ok(contents) = std::fs::read_to_string(&env_path) {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                if std::env::var(key).is_err() {
                    std::env::set_var(key, value);
                }
            }
        }
    }
}

/// Build a sample Pass 1 request for testing.
pub fn sample_pass1_request() -> Pass1Request {
    Pass1Request {
        diff_summary: "12 files changed across 3 modules. Changes include a new user registration \
            endpoint, updated validation logic, and a database migration for the users table."
            .to_string(),
        flow_groups: vec![
            Pass1GroupInput {
                id: "group_1".to_string(),
                name: "POST /api/users registration flow".to_string(),
                entrypoint: Some("src/routes/users.ts::POST".to_string()),
                files: vec![
                    "src/routes/users.ts".to_string(),
                    "src/services/user-service.ts".to_string(),
                    "src/repositories/user-repo.ts".to_string(),
                ],
                risk_score: 0.78,
                edge_summary: "users.ts calls user-service.ts, user-service.ts calls user-repo.ts"
                    .to_string(),
            },
            Pass1GroupInput {
                id: "group_2".to_string(),
                name: "User validation utilities".to_string(),
                entrypoint: None,
                files: vec![
                    "src/utils/validation.ts".to_string(),
                    "src/types/user.ts".to_string(),
                ],
                risk_score: 0.35,
                edge_summary: "validation.ts imports types from user.ts".to_string(),
            },
        ],
        graph_summary: "5 nodes, 4 edges. Primary flow: route → service → repo. \
            Shared utility: validation used by both route and service."
            .to_string(),
    }
}

/// Build a sample Pass 2 request for testing.
pub fn sample_pass2_request() -> Pass2Request {
    Pass2Request {
        group_id: "group_1".to_string(),
        group_name: "POST /api/users registration flow".to_string(),
        files: vec![
            Pass2FileInput {
                path: "src/routes/users.ts".to_string(),
                diff: r#"+ import { createUser } from '../services/user-service';
+ import { validateUserInput } from '../utils/validation';
+
+ export async function POST(req: Request) {
+   const body = await req.json();
+   const validated = validateUserInput(body);
+   const user = await createUser(validated);
+   return Response.json(user, { status: 201 });
+ }"#
                    .to_string(),
                new_content: None,
                role: "Entrypoint".to_string(),
            },
            Pass2FileInput {
                path: "src/services/user-service.ts".to_string(),
                diff: r#"+ import { UserRepository } from '../repositories/user-repo';
+
+ export async function createUser(data: UserInput): Promise<User> {
+   const existing = await UserRepository.findByEmail(data.email);
+   if (existing) throw new Error('User already exists');
+   return UserRepository.insert(data);
+ }"#
                    .to_string(),
                new_content: None,
                role: "Service".to_string(),
            },
        ],
        graph_context: "route.ts -> user-service.ts -> user-repo.ts (calls chain)".to_string(),
    }
}
