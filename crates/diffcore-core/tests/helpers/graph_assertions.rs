//! Custom assertion helpers for graph structures and analysis output.
//!
//! Provides ergonomic assertion functions for common patterns in integration tests,
//! reducing boilerplate and producing clear error messages.

use diffcore_core::types::AnalysisOutput;

/// Assert that every changed file is accounted for — either in a flow group or infrastructure.
pub fn assert_all_files_accounted(output: &AnalysisOutput) {
    let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra: usize = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.len())
        .unwrap_or(0);
    assert_eq!(
        total_grouped + infra,
        output.summary.total_files_changed as usize,
        "File accounting: grouped({}) + infra({}) != total({})",
        total_grouped,
        infra,
        output.summary.total_files_changed,
    );
}

/// Assert that all risk scores are in the valid [0.0, 1.0] range
/// and all review orders are >= 1.
pub fn assert_valid_scores(output: &AnalysisOutput) {
    for group in &output.groups {
        assert!(
            group.risk_score >= 0.0 && group.risk_score <= 1.0,
            "Group '{}' risk_score {} out of bounds [0, 1]",
            group.name,
            group.risk_score,
        );
        assert!(
            group.review_order >= 1,
            "Group '{}' review_order {} < 1",
            group.name,
            group.review_order,
        );
    }
}

/// Assert that a specific language was detected in the analysis output.
pub fn assert_language_detected(output: &AnalysisOutput, language: &str) {
    assert!(
        output
            .summary
            .languages_detected
            .contains(&language.to_string()),
        "Expected language '{}' to be detected, got: {:?}",
        language,
        output.summary.languages_detected,
    );
}

/// Assert that a file path substring appears in at least one flow group.
pub fn assert_file_in_some_group(output: &AnalysisOutput, path_contains: &str) {
    let found = output
        .groups
        .iter()
        .any(|g| g.files.iter().any(|f| f.path.contains(path_contains)));
    let in_infra = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.iter().any(|f| f.contains(path_contains)))
        .unwrap_or(false);
    assert!(
        found || in_infra,
        "File matching '{}' not found in any group or infrastructure",
        path_contains,
    );
}

/// Assert that the JSON output roundtrips cleanly (serialize → deserialize → serialize is stable).
pub fn assert_json_roundtrip(output: &AnalysisOutput) {
    let json1 = diffcore_core::output::to_json(output).unwrap();
    let parsed: AnalysisOutput = serde_json::from_str(&json1).unwrap();
    let json2 = diffcore_core::output::to_json(&parsed).unwrap();
    assert_eq!(json1, json2, "JSON roundtrip not stable");
}

/// Assert that the output JSON is valid and contains all required top-level fields.
pub fn assert_valid_json_schema(output: &AnalysisOutput) {
    let json_str = diffcore_core::output::to_json(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(v["version"], "1.0.0");
    assert!(v["diff_source"].is_object(), "missing diff_source");
    assert!(v["summary"].is_object(), "missing summary");
    assert!(v["groups"].is_array(), "missing groups");
    assert!(
        v["summary"]["total_files_changed"].is_number(),
        "missing total_files_changed"
    );
    assert!(
        v["summary"]["total_groups"].is_number(),
        "missing total_groups"
    );
    assert!(
        v["summary"]["languages_detected"].is_array(),
        "missing languages_detected"
    );
    assert!(
        v["summary"]["frameworks_detected"].is_array(),
        "missing frameworks_detected"
    );

    // Each group has required fields
    if let Some(groups) = v["groups"].as_array() {
        for g in groups {
            assert!(g["id"].is_string(), "group should have id");
            assert!(g["name"].is_string(), "group should have name");
            assert!(g["files"].is_array(), "group should have files array");
            assert!(g["edges"].is_array(), "group should have edges array");
            assert!(g["risk_score"].is_number(), "group should have risk_score");
            assert!(
                g["review_order"].is_number(),
                "group should have review_order"
            );

            // Each file has required fields
            for f in g["files"].as_array().unwrap() {
                assert!(f["path"].is_string(), "file should have path");
                assert!(
                    f["flow_position"].is_number(),
                    "file should have flow_position"
                );
                assert!(f["role"].is_string(), "file should have role");
                assert!(
                    f["changes"]["additions"].is_number(),
                    "file should have additions"
                );
                assert!(
                    f["changes"]["deletions"].is_number(),
                    "file should have deletions"
                );
                assert!(
                    f["symbols_changed"].is_array(),
                    "file should have symbols_changed"
                );
            }
        }
    }
}

/// Assert that Mermaid diagrams are valid for all groups.
pub fn assert_valid_mermaid(output: &AnalysisOutput) {
    for group in &output.groups {
        let mermaid = diffcore_core::output::generate_mermaid(group);
        assert!(
            mermaid.starts_with("graph TD"),
            "Group '{}' Mermaid should start with 'graph TD', got: {}",
            group.name,
            &mermaid[..mermaid.len().min(50)],
        );
        assert!(!mermaid.is_empty(), "Mermaid diagram should not be empty");
    }
}
