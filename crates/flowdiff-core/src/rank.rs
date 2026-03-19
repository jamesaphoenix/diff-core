use crate::types::{GroupRankInput, RankWeights, RankedGroup};

/// Compute the composite ranking score for a single group.
///
/// score(group) = w₁·risk + w₂·centrality + w₃·surface_area + w₄·uncertainty
///
/// All input components and the output are clamped to [0.0, 1.0].
pub fn composite_score(input: &GroupRankInput, weights: &RankWeights) -> f64 {
    let raw = weights.risk * input.risk.clamp(0.0, 1.0)
        + weights.centrality * input.centrality.clamp(0.0, 1.0)
        + weights.surface_area * input.surface_area.clamp(0.0, 1.0)
        + weights.uncertainty * input.uncertainty.clamp(0.0, 1.0);

    let weight_sum = weights.risk + weights.centrality + weights.surface_area + weights.uncertainty;

    if weight_sum == 0.0 {
        return 0.0;
    }

    // Normalize by weight sum to keep score in [0.0, 1.0]
    (raw / weight_sum).clamp(0.0, 1.0)
}

/// Rank a set of flow groups by their composite score.
///
/// Returns groups ordered by descending score (highest priority first).
/// Ties are broken by group_id (lexicographic) for determinism.
pub fn rank_groups(inputs: &[GroupRankInput], weights: &RankWeights) -> Vec<RankedGroup> {
    let mut scored: Vec<(String, f64)> = inputs
        .iter()
        .map(|input| (input.group_id.clone(), composite_score(input, weights)))
        .collect();

    // Sort by score descending, then by group_id ascending for deterministic tie-breaking
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    scored
        .into_iter()
        .enumerate()
        .map(|(i, (group_id, score))| RankedGroup {
            group_id,
            composite_score: score,
            review_order: (i + 1) as u32,
        })
        .collect()
}

/// Compute a risk score from file-level risk indicators.
///
/// Each indicator contributes to the risk score:
/// - Schema change: +0.3
/// - API change: +0.25
/// - Auth change: +0.35
/// - DB migration: +0.3
///
/// Clamped to [0.0, 1.0].
pub fn compute_risk_score(
    has_schema_change: bool,
    has_api_change: bool,
    has_auth_change: bool,
    has_db_migration: bool,
) -> f64 {
    let mut score: f64 = 0.0;
    if has_schema_change {
        score += 0.3;
    }
    if has_api_change {
        score += 0.25;
    }
    if has_auth_change {
        score += 0.35;
    }
    if has_db_migration {
        score += 0.3;
    }
    score.clamp(0.0, 1.0)
}

/// Compute a surface area score from change volume.
///
/// Uses a logarithmic scale: score = log2(1 + total_changed_lines) / log2(1 + max_lines)
/// where max_lines is the normalizing constant (default 10000).
pub fn compute_surface_area(additions: u32, deletions: u32, max_lines: u32) -> f64 {
    let total = (additions + deletions) as f64;
    let max = max_lines.max(1) as f64;
    let score = (1.0 + total).ln() / (1.0 + max).ln();
    score.clamp(0.0, 1.0)
}

/// Detect if a file path indicates a risk pattern.
pub fn is_risk_path(path: &str) -> RiskPathResult {
    let lower = path.to_lowercase();
    RiskPathResult {
        is_schema: lower.contains("schema")
            || lower.contains("migration")
            || lower.contains("prisma/schema")
            || lower.ends_with(".sql"),
        is_auth: lower.contains("auth")
            || lower.contains("security")
            || lower.contains("permission")
            || lower.contains("rbac")
            || lower.contains("acl"),
        is_api: lower.contains("api/")
            || lower.contains("routes/")
            || lower.contains("handlers/")
            || lower.contains("endpoints/"),
        is_test: lower.contains(".test.")
            || lower.contains(".spec.")
            || lower.contains("__tests__")
            || lower.starts_with("test/")
            || lower.starts_with("tests/"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RiskPathResult {
    pub is_schema: bool,
    pub is_auth: bool,
    pub is_api: bool,
    pub is_test: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(id: &str, risk: f64, centrality: f64, surface: f64, uncertainty: f64) -> GroupRankInput {
        GroupRankInput {
            group_id: id.to_string(),
            risk,
            centrality,
            surface_area: surface,
            uncertainty,
        }
    }

    // ── Unit Tests ──

    #[test]
    fn test_composite_score_default_weights() {
        let input = make_input("g1", 1.0, 1.0, 1.0, 1.0);
        let weights = RankWeights::default();
        let score = composite_score(&input, &weights);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_composite_score_all_zero() {
        let input = make_input("g1", 0.0, 0.0, 0.0, 0.0);
        let weights = RankWeights::default();
        let score = composite_score(&input, &weights);
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_composite_score_risk_only() {
        let input = make_input("g1", 1.0, 0.0, 0.0, 0.0);
        let weights = RankWeights::default();
        let score = composite_score(&input, &weights);
        // score = 0.35 * 1.0 / 1.0 = 0.35
        assert!((score - 0.35).abs() < f64::EPSILON);
    }

    #[test]
    fn test_composite_score_custom_weights() {
        let input = make_input("g1", 0.5, 0.5, 0.5, 0.5);
        let weights = RankWeights {
            risk: 0.5,
            centrality: 0.5,
            surface_area: 0.0,
            uncertainty: 0.0,
        };
        let score = composite_score(&input, &weights);
        // score = (0.5*0.5 + 0.5*0.5) / 1.0 = 0.5
        assert!((score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_composite_score_zero_weights() {
        let input = make_input("g1", 1.0, 1.0, 1.0, 1.0);
        let weights = RankWeights {
            risk: 0.0,
            centrality: 0.0,
            surface_area: 0.0,
            uncertainty: 0.0,
        };
        let score = composite_score(&input, &weights);
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_composite_score_clamps_inputs() {
        let input = make_input("g1", 2.0, -1.0, 1.5, 0.5);
        let weights = RankWeights::default();
        let score = composite_score(&input, &weights);
        // After clamping: risk=1.0, centrality=0.0, surface=1.0, uncertainty=0.5
        // score = (0.35*1 + 0.25*0 + 0.20*1 + 0.20*0.5) / 1.0 = 0.65
        assert!((score - 0.65).abs() < f64::EPSILON);
    }

    #[test]
    fn test_risk_scoring_schema_change() {
        let score = compute_risk_score(true, false, false, false);
        assert!((score - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_risk_scoring_auth() {
        let score = compute_risk_score(false, false, true, false);
        assert!((score - 0.35).abs() < f64::EPSILON);
    }

    #[test]
    fn test_risk_scoring_test_only() {
        let score = compute_risk_score(false, false, false, false);
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_risk_scoring_all_flags() {
        let score = compute_risk_score(true, true, true, true);
        // 0.3 + 0.25 + 0.35 + 0.3 = 1.2, clamped to 1.0
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_centrality_hub_node() {
        // Hub node with high centrality should produce high score
        let input = make_input("hub", 0.0, 0.9, 0.0, 0.0);
        let weights = RankWeights::default();
        let score = composite_score(&input, &weights);
        assert!(score > 0.2);
    }

    #[test]
    fn test_centrality_leaf_node() {
        let input = make_input("leaf", 0.0, 0.1, 0.0, 0.0);
        let weights = RankWeights::default();
        let score = composite_score(&input, &weights);
        assert!(score < 0.1);
    }

    #[test]
    fn test_surface_area_zero_changes() {
        let score = compute_surface_area(0, 0, 10000);
        assert!((score - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_surface_area_large_changes() {
        let score = compute_surface_area(5000, 5000, 10000);
        assert!(score > 0.9);
    }

    #[test]
    fn test_surface_area_moderate_changes() {
        let score = compute_surface_area(100, 50, 10000);
        assert!(score > 0.0);
        assert!(score < 1.0);
    }

    #[test]
    fn test_ranking_single_group() {
        let inputs = vec![make_input("g1", 0.5, 0.5, 0.5, 0.5)];
        let ranked = rank_groups(&inputs, &RankWeights::default());
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].review_order, 1);
        assert!(ranked[0].composite_score > 0.0);
        assert!(ranked[0].composite_score <= 1.0);
    }

    #[test]
    fn test_ranking_order() {
        let inputs = vec![
            make_input("low", 0.1, 0.1, 0.1, 0.1),
            make_input("high", 0.9, 0.9, 0.9, 0.9),
            make_input("mid", 0.5, 0.5, 0.5, 0.5),
        ];
        let ranked = rank_groups(&inputs, &RankWeights::default());
        assert_eq!(ranked[0].group_id, "high");
        assert_eq!(ranked[1].group_id, "mid");
        assert_eq!(ranked[2].group_id, "low");
        assert_eq!(ranked[0].review_order, 1);
        assert_eq!(ranked[1].review_order, 2);
        assert_eq!(ranked[2].review_order, 3);
    }

    #[test]
    fn test_ranking_stability() {
        let inputs = vec![
            make_input("a", 0.7, 0.3, 0.5, 0.2),
            make_input("b", 0.2, 0.8, 0.1, 0.9),
            make_input("c", 0.5, 0.5, 0.5, 0.5),
        ];
        let weights = RankWeights::default();
        let run1 = rank_groups(&inputs, &weights);
        let run2 = rank_groups(&inputs, &weights);
        assert_eq!(run1, run2);
    }

    #[test]
    fn test_ranking_empty_input() {
        let ranked = rank_groups(&[], &RankWeights::default());
        assert!(ranked.is_empty());
    }

    #[test]
    fn test_ranking_tie_breaking() {
        // Same scores, different IDs — should be ordered by ID
        let inputs = vec![
            make_input("b_group", 0.5, 0.5, 0.5, 0.5),
            make_input("a_group", 0.5, 0.5, 0.5, 0.5),
        ];
        let ranked = rank_groups(&inputs, &RankWeights::default());
        assert_eq!(ranked[0].group_id, "a_group");
        assert_eq!(ranked[1].group_id, "b_group");
    }

    #[test]
    fn test_is_risk_path_schema() {
        let result = is_risk_path("prisma/schema.prisma");
        assert!(result.is_schema);
    }

    #[test]
    fn test_is_risk_path_migration() {
        let result = is_risk_path("db/migrations/001_create_users.sql");
        assert!(result.is_schema);
    }

    #[test]
    fn test_is_risk_path_auth() {
        let result = is_risk_path("src/middleware/auth.ts");
        assert!(result.is_auth);
    }

    #[test]
    fn test_is_risk_path_api() {
        let result = is_risk_path("src/api/users/route.ts");
        assert!(result.is_api);
    }

    #[test]
    fn test_is_risk_path_test() {
        let result = is_risk_path("src/services/user.test.ts");
        assert!(result.is_test);
        assert!(!result.is_schema);
        assert!(!result.is_auth);
    }

    #[test]
    fn test_is_risk_path_normal_file() {
        let result = is_risk_path("src/utils/format.ts");
        assert!(!result.is_schema);
        assert!(!result.is_auth);
        assert!(!result.is_api);
        assert!(!result.is_test);
    }

    // ── Property-Based Tests ──

    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        // Strategy for generating valid score components [0.0, 1.0]
        fn score_component() -> impl Strategy<Value = f64> {
            0.0f64..=1.0f64
        }

        // Strategy for generating group rank inputs
        fn arb_group_input() -> impl Strategy<Value = GroupRankInput> {
            (
                "[a-z]{1,10}",
                score_component(),
                score_component(),
                score_component(),
                score_component(),
            )
                .prop_map(|(id, risk, centrality, surface_area, uncertainty)| GroupRankInput {
                    group_id: id,
                    risk,
                    centrality,
                    surface_area,
                    uncertainty,
                })
        }

        // Strategy for generating positive weights
        fn arb_weights() -> impl Strategy<Value = RankWeights> {
            (0.01f64..=1.0, 0.01f64..=1.0, 0.01f64..=1.0, 0.01f64..=1.0).prop_map(
                |(r, c, s, u)| RankWeights {
                    risk: r,
                    centrality: c,
                    surface_area: s,
                    uncertainty: u,
                },
            )
        }

        proptest! {
            /// Ranking scores are always in [0.0, 1.0]
            #[test]
            fn prop_score_in_bounds(input in arb_group_input(), weights in arb_weights()) {
                let score = composite_score(&input, &weights);
                prop_assert!(score >= 0.0, "Score {} is negative", score);
                prop_assert!(score <= 1.0, "Score {} exceeds 1.0", score);
            }

            /// Every input group appears in exactly one position in the output
            #[test]
            fn prop_every_group_appears_once(
                inputs in proptest::collection::vec(arb_group_input(), 1..20),
                weights in arb_weights()
            ) {
                // Deduplicate IDs for this test
                let mut seen_ids = std::collections::HashSet::new();
                let unique_inputs: Vec<_> = inputs.into_iter()
                    .filter(|i| seen_ids.insert(i.group_id.clone()))
                    .collect();

                let ranked = rank_groups(&unique_inputs, &weights);
                prop_assert_eq!(ranked.len(), unique_inputs.len());

                let mut ranked_ids: Vec<_> = ranked.iter().map(|r| &r.group_id).collect();
                ranked_ids.sort();
                let mut input_ids: Vec<_> = unique_inputs.iter().map(|i| &i.group_id).collect();
                input_ids.sort();
                prop_assert_eq!(ranked_ids, input_ids);
            }

            /// Ranking is a total order (review_order is 1..=N with no gaps)
            #[test]
            fn prop_ranking_is_total_order(
                inputs in proptest::collection::vec(arb_group_input(), 1..20),
                weights in arb_weights()
            ) {
                let mut seen_ids = std::collections::HashSet::new();
                let unique_inputs: Vec<_> = inputs.into_iter()
                    .filter(|i| seen_ids.insert(i.group_id.clone()))
                    .collect();
                let n = unique_inputs.len();

                let ranked = rank_groups(&unique_inputs, &weights);
                let mut orders: Vec<u32> = ranked.iter().map(|r| r.review_order).collect();
                orders.sort();
                let expected: Vec<u32> = (1..=(n as u32)).collect();
                prop_assert_eq!(orders, expected);
            }

            /// Empty diff produces empty groups
            #[test]
            fn prop_empty_input_empty_output(_weights in arb_weights()) {
                let ranked = rank_groups(&[], &_weights);
                prop_assert!(ranked.is_empty());
            }

            /// Single group always gets review_order = 1
            #[test]
            fn prop_single_group_order_one(input in arb_group_input(), weights in arb_weights()) {
                let ranked = rank_groups(&[input], &weights);
                prop_assert_eq!(ranked.len(), 1);
                prop_assert_eq!(ranked[0].review_order, 1);
            }

            /// Determinism: same input always produces same output
            #[test]
            fn prop_deterministic(
                inputs in proptest::collection::vec(arb_group_input(), 1..10),
                weights in arb_weights()
            ) {
                let mut seen_ids = std::collections::HashSet::new();
                let unique_inputs: Vec<_> = inputs.into_iter()
                    .filter(|i| seen_ids.insert(i.group_id.clone()))
                    .collect();

                let run1 = rank_groups(&unique_inputs, &weights);
                let run2 = rank_groups(&unique_inputs, &weights);
                prop_assert_eq!(run1, run2);
            }

            /// Higher risk input ≥ lower risk input (all else equal)
            #[test]
            fn prop_higher_risk_higher_score(
                low_risk in 0.0f64..=0.49,
                high_risk in 0.51f64..=1.0,
                centrality in score_component(),
                surface in score_component(),
                uncertainty in score_component(),
            ) {
                let weights = RankWeights::default();
                let low = make_input("low", low_risk, centrality, surface, uncertainty);
                let high = make_input("high", high_risk, centrality, surface, uncertainty);
                let low_score = composite_score(&low, &weights);
                let high_score = composite_score(&high, &weights);
                prop_assert!(high_score >= low_score,
                    "High risk score {} should be >= low risk score {}", high_score, low_score);
            }

            /// Risk score from indicators is in [0.0, 1.0]
            #[test]
            fn prop_risk_score_bounds(
                schema in proptest::bool::ANY,
                api in proptest::bool::ANY,
                auth in proptest::bool::ANY,
                migration in proptest::bool::ANY,
            ) {
                let score = compute_risk_score(schema, api, auth, migration);
                prop_assert!(score >= 0.0);
                prop_assert!(score <= 1.0);
            }

            /// Surface area score is in [0.0, 1.0]
            #[test]
            fn prop_surface_area_bounds(
                additions in 0u32..100000,
                deletions in 0u32..100000,
                max_lines in 1u32..100000,
            ) {
                let score = compute_surface_area(additions, deletions, max_lines);
                prop_assert!(score >= 0.0, "Surface area {} is negative", score);
                prop_assert!(score <= 1.0, "Surface area {} exceeds 1.0", score);
            }

            /// More changes → higher or equal surface area
            #[test]
            fn prop_more_changes_higher_surface(
                base_add in 0u32..1000,
                base_del in 0u32..1000,
                extra in 1u32..1000,
            ) {
                let max_lines = 10000;
                let low = compute_surface_area(base_add, base_del, max_lines);
                let high = compute_surface_area(base_add + extra, base_del, max_lines);
                prop_assert!(high >= low,
                    "More additions ({}) should give >= surface area ({} vs {})",
                    base_add + extra, high, low);
            }

            /// Inputs beyond [0,1] are clamped, score still valid
            #[test]
            fn prop_out_of_range_inputs_still_valid(
                risk in -10.0f64..10.0,
                centrality in -10.0f64..10.0,
                surface in -10.0f64..10.0,
                uncertainty in -10.0f64..10.0,
            ) {
                let input = make_input("test", risk, centrality, surface, uncertainty);
                let weights = RankWeights::default();
                let score = composite_score(&input, &weights);
                prop_assert!(score >= 0.0);
                prop_assert!(score <= 1.0);
            }
        }
    }
}
