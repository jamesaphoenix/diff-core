//! LLM group metadata pass — fills in the review metadata carried on each group.
//!
//! Consumes the *final* groups, after any refinement ops have been applied, and
//! asks an LLM "how should I review this group" rather than "what changed". The
//! answer lands on the `FlowGroup` itself: type, risk band, impact scope, review
//! complexity, review focus, a one-line description, and the invariant a reviewer
//! should verify.
//!
//! `risk_score` is never written — it drives review ranking and stays
//! deterministic. See `specs/group-metadata.md`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::git::FileDiff;
use crate::types::{FlowGroup, MAX_REVIEW_FOCUS, MAX_SUMMARY_BULLETS};

use super::schema;
use super::schema::{
    GroupMetadata, MetadataFileInput, MetadataGroupDetail, MetadataGroupIndexEntry, MetadataRequest,
};
use super::{LlmError, LlmProvider};

/// Per-file cap on the changed-code excerpt sent to the model.
const MAX_FILE_EXCERPT_TOKENS: usize = 600;

/// Cap on metadata batches in flight. A large diff produces dozens of batches and
/// providers rate-limit per account, so the fan-out has to be bounded.
const MAX_CONCURRENT_BATCHES: usize = 3;

/// Run the metadata pass over `groups`, mutating them in place.
///
/// Batches are dispatched concurrently — at most `MAX_CONCURRENT_BATCHES` at a
/// time — and their results merged by group id, so completion order cannot reach
/// the output. Groups are left sorted by `review_order`.
///
/// A batch that fails is logged and skipped; the groups it covered keep whatever
/// the deterministic floor gave them.
pub async fn run_metadata_pass(
    provider: Arc<dyn LlmProvider>,
    groups: &mut [FlowGroup],
    diffs: &[FileDiff],
    batch_size: usize,
) -> Result<(), LlmError> {
    let requests = build_metadata_requests(groups, diffs, batch_size);
    if requests.is_empty() {
        return Ok(());
    }

    let permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_BATCHES));
    let handles: Vec<_> = requests
        .into_iter()
        .map(|request| {
            let provider = Arc::clone(&provider);
            let permits = Arc::clone(&permits);
            tokio::spawn(async move {
                let _permit = permits.acquire_owned().await;
                provider.describe_groups(&request).await
            })
        })
        .collect();

    let mut merged: HashMap<String, GroupMetadata> = HashMap::new();
    let mut succeeded = 0usize;
    let total = handles.len();

    for handle in handles {
        match handle.await {
            Ok(Ok(response)) => {
                succeeded += 1;
                for meta in response.groups {
                    merged.insert(meta.id.clone(), meta);
                }
            }
            Ok(Err(e)) => log::warn!(target: "metadata", "batch failed: {}", e),
            Err(e) => log::warn!(target: "metadata", "batch panicked: {}", e),
        }
    }

    if succeeded == 0 {
        return Err(LlmError::ParseResponse(format!(
            "all {} metadata batches failed",
            total
        )));
    }

    apply_metadata(groups, &merged);
    groups.sort_by_key(|g| g.review_order);
    Ok(())
}

/// Split `groups` into batches of at most `batch_size`, each carrying full detail
/// for its own groups plus a read-only index of every group in the analysis.
pub fn build_metadata_requests(
    groups: &[FlowGroup],
    diffs: &[FileDiff],
    batch_size: usize,
) -> Vec<MetadataRequest> {
    if groups.is_empty() {
        return Vec::new();
    }

    let excerpts = excerpts_by_path(diffs);

    let index: Vec<MetadataGroupIndexEntry> = groups
        .iter()
        .map(|g| MetadataGroupIndexEntry {
            id: g.id.clone(),
            name: g.name.clone(),
            files: g.files.iter().map(|f| f.path.clone()).collect(),
            risk_score: g.risk_score,
        })
        .collect();

    groups
        .chunks(batch_size.max(1))
        .map(|chunk| MetadataRequest {
            groups: chunk
                .iter()
                .map(|g| MetadataGroupDetail {
                    id: g.id.clone(),
                    name: g.name.clone(),
                    entrypoint: g
                        .entrypoint
                        .as_ref()
                        .map(|ep| format!("{}::{}", ep.file, ep.symbol)),
                    risk_score: g.risk_score,
                    files: g
                        .files
                        .iter()
                        .map(|f| MetadataFileInput {
                            path: f.path.clone(),
                            role: format!("{:?}", f.role),
                            diff: excerpts.get(f.path.as_str()).cloned().unwrap_or_default(),
                        })
                        .collect(),
                })
                .collect(),
            index: index.clone(),
        })
        .collect()
}

/// Write metadata onto the groups it names, clamping to the response constraints.
///
/// Unknown ids are ignored, and a field the model omitted leaves whatever the
/// deterministic floor produced in place.
pub fn apply_metadata(groups: &mut [FlowGroup], metadata: &HashMap<String, GroupMetadata>) {
    for group in groups.iter_mut() {
        let Some(meta) = metadata.get(&group.id) else {
            continue;
        };

        if meta.group_type.is_some() {
            group.group_type = meta.group_type;
        }
        if meta.risk.is_some() {
            group.risk = meta.risk;
        }
        if meta.impact.is_some() {
            group.impact = meta.impact;
        }
        if meta.complexity.is_some() {
            group.complexity = meta.complexity;
        }
        if let Some(ref description) = meta.description {
            if let Some(clamped) = clamp_line(description) {
                group.description = Some(clamped);
            }
        }
        if let Some(ref invariant) = meta.invariant {
            if let Some(clamped) = clamp_sentence(invariant) {
                group.invariant = Some(clamped);
            }
        }
        if !meta.review_focus.is_empty() {
            let mut focus = meta.review_focus.clone();
            focus.dedup();
            focus.truncate(MAX_REVIEW_FOCUS);
            group.review_focus = focus;
        }
        let summary: Vec<String> = meta
            .summary
            .iter()
            .filter_map(|bullet| clamp_line(strip_bullet_marker(bullet)))
            .take(MAX_SUMMARY_BULLETS)
            .collect();
        if !summary.is_empty() {
            group.summary = summary;
        }
    }
}

/// Drop a leading `-`, `*` or `\u{2022}` marker. Models add them despite the
/// schema saying not to, and the UI supplies its own.
fn strip_bullet_marker(text: &str) -> &str {
    text.trim_start()
        .trim_start_matches(['-', '*', '\u{2022}'])
        .trim_start()
}

/// Collapse to a single plain-text line. Returns `None` for empty input.
fn clamp_line(text: &str) -> Option<String> {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Collapse to a single plain-text sentence, keeping only the first.
fn clamp_sentence(text: &str) -> Option<String> {
    let line = clamp_line(text)?;
    let end = line
        .char_indices()
        .find(|(i, c)| {
            matches!(c, '.' | '!' | '?')
                && line[i + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(|next| next == ' ')
        })
        .map(|(i, c)| i + c.len_utf8());

    match end {
        Some(end) if end < line.len() => Some(line[..end].trim_end().to_string()),
        _ => Some(line),
    }
}

fn excerpts_by_path(diffs: &[FileDiff]) -> HashMap<&str, String> {
    diffs
        .iter()
        .map(|file| (file.path(), changed_code_excerpt(file)))
        .collect()
}

/// Render the changed regions of a file, hunk by hunk.
///
/// There is no unified-diff text in the pipeline — `git.rs` keeps hunk line ranges
/// plus whole-file contents — so this slices the post-change lines each hunk covers.
fn changed_code_excerpt(file: &FileDiff) -> String {
    if file.is_binary {
        return "(binary file)".to_string();
    }

    let use_new = file.new_content.is_some();
    let Some(content) = file.new_content.as_deref().or(file.old_content.as_deref()) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();

    let mut out = String::new();
    for hunk in &file.hunks {
        let (start, count) = if use_new {
            (hunk.new_start, hunk.new_lines)
        } else {
            (hunk.old_start, hunk.old_lines)
        };
        let from = (start.saturating_sub(1)) as usize;
        let to = (from + count as usize).min(lines.len());
        if from >= to {
            continue;
        }
        out.push_str(&format!("@@ line {} @@\n", start));
        for line in &lines[from..to] {
            out.push_str(line);
            out.push('\n');
        }
    }

    super::truncate_to_token_budget(&out, MAX_FILE_EXCERPT_TOKENS)
}

/// Build the system prompt for the group metadata pass.
pub fn metadata_system_prompt() -> String {
    format!(
        "You are a senior engineer triaging a code review. For each group you are given, \
         answer the question a reviewer actually has: HOW SHOULD I REVIEW THIS? Not what changed \
         line by line — what to watch for while reading it.\n\n\
         Field meanings:\n\
         - `group_type`: the kind of change this is.\n\
         - `risk`: how much damage a mistake here does. Judge the change, not its size.\n\
         - `impact`: how far the blast radius reaches. Use the group index to see whether other \
           groups touch the same surface; CrossCutting means the change reaches beyond this group's \
           own files.\n\
         - `complexity`: how much effort reading this group carefully will take.\n\
         - `review_focus`: at most {max_focus} concerns, most important first. Pick only concerns \
           actually at stake in this change. Fewer is better than wrong.\n\
         - `description`: ONE line of plain text. What this group changes.\n\
         - `invariant`: ONE sentence of plain text naming the property that must still hold after \
           this change — the thing a reviewer should actively try to break. \
           Good: 'Two workers must never successfully claim the same job.' \
           Bad: 'The code should work correctly.'\n\n\
         Rules:\n\
         - `description` and `invariant` are PLAIN TEXT. No markdown, no bullet points, no code \
           fences, no backticks, no bold.\n\
         - `summary` is a LIST of plain-text entries. Size it to the change: one entry when a \
           single sentence covers what the group achieves, more only when it genuinely does \
           several things, at most {max_summary}. Each entry is a bare sentence — no leading \
           '-' or '*', no markdown. Convey the essence, do not restate `description`.\n\
         - Never restate line counts, file counts, or anything else mechanically visible from the \
           file list — the reviewer can already see it.\n\
         - Never invent an invariant you cannot support from the code shown. If nothing meaningful \
           is at stake, state the narrow property that is.\n\
         - Judge each group on its own merits. Do not compute it from the other groups' answers.\n\n\
         {}",
        schema::metadata_schema_description(),
        max_focus = MAX_REVIEW_FOCUS,
        max_summary = MAX_SUMMARY_BULLETS,
    )
}

/// Build the user prompt for one metadata batch.
pub fn metadata_user_prompt(request: &MetadataRequest) -> String {
    let mut prompt = String::from("## Groups to describe\n");
    prompt.push_str(
        "Produce exactly one metadata entry for each group in this section, keyed by its id.\n",
    );

    for group in &request.groups {
        prompt.push_str(&format!(
            "\n### {} ({})\n- Entrypoint: {}\n- Risk score: {:.2}\n",
            group.name,
            group.id,
            group.entrypoint.as_deref().unwrap_or("none"),
            group.risk_score,
        ));
        for file in &group.files {
            prompt.push_str(&format!("\n#### {} (role: {})\n", file.path, file.role));
            if file.diff.is_empty() {
                prompt.push_str("(no changed content available)\n");
            } else {
                prompt.push_str(&format!("```\n{}\n```\n", file.diff));
            }
        }
    }

    prompt.push_str(
        "\n## All groups in this analysis (context only — do NOT describe these)\n\
         Use this list to judge how far each change reaches. It is read-only context.\n",
    );
    for entry in &request.index {
        prompt.push_str(&format!(
            "- {} ({}, risk {:.2}): {}\n",
            entry.name,
            entry.id,
            entry.risk_score,
            entry.files.join(", "),
        ));
    }

    prompt
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::git::{DiffHunk, FileStatus};
    use crate::types::{
        ChangeStats, FileChange, FileRole, GroupType, ImpactScope, ReviewComplexity, ReviewFocus,
        Risk,
    };

    fn make_group(id: &str, review_order: u32, paths: &[&str]) -> FlowGroup {
        FlowGroup {
            id: id.to_string(),
            name: format!("Group {}", id),
            files: paths
                .iter()
                .enumerate()
                .map(|(i, p)| FileChange {
                    path: p.to_string(),
                    flow_position: i as u32,
                    role: FileRole::Service,
                    changes: ChangeStats {
                        additions: 1,
                        deletions: 0,
                    },
                    symbols_changed: vec![],
                })
                .collect(),
            risk_score: 0.5,
            review_order,
            ..Default::default()
        }
    }

    fn make_diff(path: &str, content: &str) -> FileDiff {
        FileDiff {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            old_content: None,
            new_content: Some(content.to_string()),
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 0,
                new_start: 2,
                new_lines: 2,
            }],
            status: FileStatus::Modified,
            additions: 2,
            deletions: 0,
            is_binary: false,
        }
    }

    #[test]
    fn batches_cover_every_group_and_carry_the_full_index() {
        let groups: Vec<FlowGroup> = (0..7)
            .map(|i| make_group(&format!("g{}", i), i, &["src/a.ts"]))
            .collect();

        let requests = build_metadata_requests(&groups, &[], 3);

        assert_eq!(requests.len(), 3);
        let described: Vec<&str> = requests
            .iter()
            .flat_map(|r| r.groups.iter().map(|g| g.id.as_str()))
            .collect();
        assert_eq!(described.len(), 7);
        for request in &requests {
            assert_eq!(request.index.len(), 7, "every batch sees every group");
        }
    }

    #[test]
    fn batch_index_carries_no_diff_content() {
        let groups = vec![make_group("g0", 0, &["src/a.ts"])];
        let diffs = vec![make_diff("src/a.ts", "one\ntwo\nthree\n")];

        let requests = build_metadata_requests(&groups, &diffs, 20);
        let prompt = metadata_user_prompt(&requests[0]);

        assert!(requests[0].groups[0].files[0].diff.contains("two"));
        let index_section = prompt.split("All groups in this analysis").nth(1).unwrap();
        assert!(!index_section.contains("two"));
    }

    #[test]
    fn applying_metadata_is_independent_of_batch_completion_order() {
        let mut a = vec![
            make_group("g0", 2, &["src/a.ts"]),
            make_group("g1", 1, &["src/b.ts"]),
        ];
        let mut b = a.clone();

        let mut forward = HashMap::new();
        forward.insert(
            "g0".to_string(),
            GroupMetadata {
                id: "g0".to_string(),
                risk: Some(Risk::High),
                ..Default::default()
            },
        );
        forward.insert(
            "g1".to_string(),
            GroupMetadata {
                id: "g1".to_string(),
                risk: Some(Risk::Low),
                ..Default::default()
            },
        );

        apply_metadata(&mut a, &forward);
        a.sort_by_key(|g| g.review_order);
        apply_metadata(&mut b, &forward);
        b.sort_by_key(|g| g.review_order);

        assert_eq!(a, b);
        assert_eq!(a[0].id, "g1");
        assert_eq!(a[0].risk, Some(Risk::Low));
    }

    #[test]
    fn caps_are_reenforced_on_the_consuming_side() {
        let mut groups = vec![make_group("g0", 0, &["src/a.ts"])];
        let mut metadata = HashMap::new();
        metadata.insert(
            "g0".to_string(),
            GroupMetadata {
                id: "g0".to_string(),
                description: Some("first line\nsecond line\nthird".to_string()),
                invariant: Some(
                    "Two workers never claim the same job. Also the cache stays warm.".to_string(),
                ),
                review_focus: vec![
                    ReviewFocus::Concurrency,
                    ReviewFocus::DataIntegrity,
                    ReviewFocus::Security,
                    ReviewFocus::Performance,
                ],
                ..Default::default()
            },
        );

        apply_metadata(&mut groups, &metadata);

        assert_eq!(
            groups[0].description.as_deref(),
            Some("first line second line third")
        );
        assert_eq!(
            groups[0].invariant.as_deref(),
            Some("Two workers never claim the same job.")
        );
        assert_eq!(groups[0].review_focus.len(), MAX_REVIEW_FOCUS);
    }

    #[test]
    fn metadata_never_touches_risk_score_or_review_order() {
        let mut groups = vec![make_group("g0", 4, &["src/a.ts"])];
        let (score, order) = (groups[0].risk_score, groups[0].review_order);

        let mut metadata = HashMap::new();
        metadata.insert(
            "g0".to_string(),
            GroupMetadata {
                id: "g0".to_string(),
                risk: Some(Risk::Critical),
                impact: Some(ImpactScope::System),
                complexity: Some(ReviewComplexity::Complex),
                group_type: Some(GroupType::Fix),
                ..Default::default()
            },
        );

        apply_metadata(&mut groups, &metadata);

        assert_eq!(groups[0].risk_score, score);
        assert_eq!(groups[0].review_order, order);
        assert_eq!(groups[0].risk, Some(Risk::Critical));
    }

    #[test]
    fn omitted_fields_leave_the_deterministic_floor_alone() {
        let mut groups = vec![make_group("g0", 0, &["src/a.ts"])];
        groups[0].risk = Some(Risk::Medium);
        groups[0].group_type = Some(GroupType::Chore);

        let mut metadata = HashMap::new();
        metadata.insert(
            "g0".to_string(),
            GroupMetadata {
                id: "g0".to_string(),
                description: Some("Adds a retry.".to_string()),
                ..Default::default()
            },
        );

        apply_metadata(&mut groups, &metadata);

        assert_eq!(groups[0].risk, Some(Risk::Medium));
        assert_eq!(groups[0].group_type, Some(GroupType::Chore));
        assert_eq!(groups[0].description.as_deref(), Some("Adds a retry."));
    }

    #[test]
    fn unknown_group_ids_are_ignored() {
        let mut groups = vec![make_group("g0", 0, &["src/a.ts"])];
        let mut metadata = HashMap::new();
        metadata.insert(
            "hallucinated".to_string(),
            GroupMetadata {
                id: "hallucinated".to_string(),
                risk: Some(Risk::Critical),
                ..Default::default()
            },
        );

        apply_metadata(&mut groups, &metadata);

        assert_eq!(groups[0].risk, None);
    }

    #[test]
    fn empty_groups_produce_no_requests() {
        assert!(build_metadata_requests(&[], &[], 20).is_empty());
    }

    #[test]
    fn zero_batch_size_does_not_divide_by_zero() {
        let groups = vec![make_group("g0", 0, &["src/a.ts"])];
        assert_eq!(build_metadata_requests(&groups, &[], 0).len(), 1);
    }

    #[test]
    fn prompt_states_the_caps_and_the_plain_text_rule() {
        let prompt = metadata_system_prompt();
        assert!(prompt.contains("at most 3"));
        assert!(prompt.contains("PLAIN TEXT"));
        assert!(prompt.contains("ONE sentence"));
        assert!(prompt.contains("ONE line"));
    }

    /// A provider that answers every batch, after a delay that inverts completion
    /// order relative to dispatch order, recording the peak number of calls it was
    /// ever handling at once.
    #[derive(Default)]
    struct BatchingProvider {
        batches_seen: Arc<std::sync::Mutex<Vec<usize>>>,
        delay_ms: u64,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for BatchingProvider {
        fn name(&self) -> &str {
            "batching-mock"
        }
        fn model(&self) -> &str {
            "batching-mock-v1"
        }
        fn max_context_tokens(&self) -> usize {
            100_000
        }
        async fn annotate_overview(
            &self,
            _: &crate::llm::schema::Pass1Request,
        ) -> Result<crate::llm::schema::Pass1Response, LlmError> {
            unimplemented!()
        }
        async fn annotate_group(
            &self,
            _: &crate::llm::schema::Pass2Request,
        ) -> Result<crate::llm::schema::Pass2Response, LlmError> {
            unimplemented!()
        }
        async fn evaluate_quality(
            &self,
            _: &crate::llm::schema::JudgeRequest,
        ) -> Result<crate::llm::schema::JudgeResponse, LlmError> {
            unimplemented!()
        }
        async fn refine_groups(
            &self,
            _: &crate::llm::schema::RefinementRequest,
        ) -> Result<crate::llm::schema::RefinementResponse, LlmError> {
            unimplemented!()
        }
        async fn describe_groups(
            &self,
            request: &crate::llm::schema::MetadataRequest,
        ) -> Result<crate::llm::schema::MetadataResponse, LlmError> {
            let first: usize = request.groups[0]
                .id
                .trim_start_matches('g')
                .parse()
                .unwrap();
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(
                self.delay_ms * (10 - first as u64),
            ))
            .await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            if let Ok(mut seen) = self.batches_seen.lock() {
                seen.push(first);
            }
            Ok(crate::llm::schema::MetadataResponse {
                groups: request
                    .groups
                    .iter()
                    .map(|g| GroupMetadata {
                        id: g.id.clone(),
                        description: Some(format!("desc for {}", g.id)),
                        risk: Some(Risk::Medium),
                        ..Default::default()
                    })
                    .collect(),
            })
        }
    }

    #[tokio::test]
    async fn every_group_gets_metadata_across_concurrent_batches() {
        let mut groups: Vec<FlowGroup> = (0..9)
            .map(|i| make_group(&format!("g{}", i), 9 - i, &["src/a.ts"]))
            .collect();
        let provider = Arc::new(BatchingProvider {
            batches_seen: Arc::new(std::sync::Mutex::new(Vec::new())),
            delay_ms: 5,
            ..Default::default()
        });

        run_metadata_pass(provider.clone(), &mut groups, &[], 3)
            .await
            .unwrap();

        assert_eq!(groups.len(), 9);
        for group in &groups {
            assert!(
                group.description.is_some(),
                "{} missing metadata",
                group.id
            );
        }

        let orders: Vec<u32> = groups.iter().map(|g| g.review_order).collect();
        let mut sorted = orders.clone();
        sorted.sort_unstable();
        assert_eq!(orders, sorted, "output must be sorted by review_order");

        let seen = provider.batches_seen.lock().unwrap().clone();
        assert_eq!(seen, vec![6, 3, 0], "batches completed out of dispatch order");
    }

    #[tokio::test]
    async fn batch_fan_out_stays_within_the_concurrency_cap() {
        let mut groups: Vec<FlowGroup> = (0..9)
            .map(|i| make_group(&format!("g{}", i), i, &["src/a.ts"]))
            .collect();
        let provider = Arc::new(BatchingProvider {
            batches_seen: Arc::new(std::sync::Mutex::new(Vec::new())),
            delay_ms: 2,
            ..Default::default()
        });

        run_metadata_pass(provider.clone(), &mut groups, &[], 1)
            .await
            .unwrap();

        let peak = provider.max_in_flight.load(Ordering::SeqCst);
        assert!(
            peak <= MAX_CONCURRENT_BATCHES,
            "{} batches in flight at once, cap is {}",
            peak,
            MAX_CONCURRENT_BATCHES
        );
        assert!(peak > 1, "batches must still overlap");
        assert!(groups.iter().all(|g| g.description.is_some()));
    }

    struct FailingProvider;

    #[async_trait::async_trait]
    impl LlmProvider for FailingProvider {
        fn name(&self) -> &str {
            "failing-mock"
        }
        fn model(&self) -> &str {
            "failing-mock-v1"
        }
        fn max_context_tokens(&self) -> usize {
            100_000
        }
        async fn annotate_overview(
            &self,
            _: &crate::llm::schema::Pass1Request,
        ) -> Result<crate::llm::schema::Pass1Response, LlmError> {
            unimplemented!()
        }
        async fn annotate_group(
            &self,
            _: &crate::llm::schema::Pass2Request,
        ) -> Result<crate::llm::schema::Pass2Response, LlmError> {
            unimplemented!()
        }
        async fn evaluate_quality(
            &self,
            _: &crate::llm::schema::JudgeRequest,
        ) -> Result<crate::llm::schema::JudgeResponse, LlmError> {
            unimplemented!()
        }
        async fn refine_groups(
            &self,
            _: &crate::llm::schema::RefinementRequest,
        ) -> Result<crate::llm::schema::RefinementResponse, LlmError> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn a_provider_that_cannot_describe_leaves_groups_untouched() {
        let mut groups = vec![make_group("g0", 0, &["src/a.ts"])];
        groups[0].risk = Some(Risk::Medium);

        let err = run_metadata_pass(Arc::new(FailingProvider), &mut groups, &[], 20)
            .await
            .unwrap_err();

        assert!(matches!(err, LlmError::ParseResponse(_)));
        assert_eq!(groups[0].risk, Some(Risk::Medium));
        assert_eq!(groups[0].description, None);
    }
}
