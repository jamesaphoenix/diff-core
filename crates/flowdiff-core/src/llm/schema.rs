//! Structured output schemas for LLM annotation passes.
//!
//! These types define the JSON schemas sent to LLM providers (via structured outputs)
//! and the response types we deserialize back. See spec §5.2 for details.

use serde::{Deserialize, Serialize};

// ── Pass 1: Overview ──

/// Pass 1 request context sent to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pass1Request {
    /// Full diff summary text.
    pub diff_summary: String,
    /// Deterministic flow groups (serialized from analysis).
    pub flow_groups: Vec<Pass1GroupInput>,
    /// Graph structure description.
    pub graph_summary: String,
}

/// A flow group as presented to the LLM in Pass 1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pass1GroupInput {
    pub id: String,
    pub name: String,
    pub entrypoint: Option<String>,
    pub files: Vec<String>,
    pub risk_score: f64,
    pub edge_summary: String,
}

/// Pass 1 structured output: overview annotation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pass1Response {
    /// Per-group annotations.
    pub groups: Vec<Pass1GroupAnnotation>,
    /// Overall summary of the entire diff.
    pub overall_summary: String,
    /// Suggested review order (group IDs).
    pub suggested_review_order: Vec<String>,
}

/// Per-group annotation from Pass 1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pass1GroupAnnotation {
    pub id: String,
    /// Human-readable name (may differ from deterministic name).
    pub name: String,
    /// Narrative summary of what this group does.
    pub summary: String,
    /// Why the LLM suggests this review order position.
    pub review_order_rationale: String,
    /// Risk flags identified by the LLM.
    pub risk_flags: Vec<String>,
}

// ── Pass 2: Deep Analysis ──

/// Pass 2 request context for a single group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pass2Request {
    /// The group being analyzed.
    pub group_id: String,
    pub group_name: String,
    /// Full file contents + diffs for each file in the group.
    pub files: Vec<Pass2FileInput>,
    /// Graph context (edges, related symbols).
    pub graph_context: String,
}

/// A file as presented to the LLM in Pass 2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pass2FileInput {
    pub path: String,
    /// The unified diff for this file.
    pub diff: String,
    /// Full new content (for context).
    pub new_content: Option<String>,
    /// Role inferred by deterministic analysis.
    pub role: String,
}

/// Pass 2 structured output: deep analysis of one group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pass2Response {
    pub group_id: String,
    /// Narrative of how data flows through this group's changes.
    pub flow_narrative: String,
    /// Per-file annotations.
    pub file_annotations: Vec<Pass2FileAnnotation>,
    /// Cross-cutting concerns spanning multiple files.
    pub cross_cutting_concerns: Vec<String>,
}

/// Per-file annotation from Pass 2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pass2FileAnnotation {
    pub file: String,
    /// The file's role in the data flow.
    pub role_in_flow: String,
    /// Summary of what changed in this file.
    pub changes_summary: String,
    /// Risks identified by the LLM.
    pub risks: Vec<String>,
    /// Suggestions for improvement.
    pub suggestions: Vec<String>,
}

// ── Combined Annotations ──

/// Combined annotations attached to the analysis output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Annotations {
    /// Pass 1 overview (if run).
    pub overview: Option<Pass1Response>,
    /// Pass 2 deep analyses keyed by group_id (populated on-demand).
    pub deep_analyses: Vec<Pass2Response>,
}

// ── LLM-as-Judge Evaluation ──

/// Request context for the LLM-as-judge evaluator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgeRequest {
    /// The full analysis output JSON (serialized AnalysisOutput).
    pub analysis_json: String,
    /// Source files from the fixture codebase (path → content).
    pub source_files: Vec<JudgeSourceFile>,
    /// The diff being analyzed (unified diff format).
    pub diff_text: String,
    /// Fixture name for context.
    pub fixture_name: String,
}

/// A source file provided to the judge for context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgeSourceFile {
    pub path: String,
    pub content: String,
}

/// LLM-as-judge structured response with per-criterion scores.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgeResponse {
    /// Per-criterion scores (1-5 scale).
    pub criteria: Vec<JudgeCriterionScore>,
    /// Overall score (1.0-5.0), average of criteria scores.
    pub overall_score: f64,
    /// Explanations for any scores below 3.
    pub failure_explanations: Vec<String>,
    /// Notable strengths of the analysis.
    pub strengths: Vec<String>,
}

/// A single criterion score from the LLM judge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgeCriterionScore {
    /// Criterion name (e.g., "group_coherence", "review_ordering").
    pub criterion: String,
    /// Score from 1 (poor) to 5 (excellent).
    pub score: u8,
    /// Brief explanation of the score.
    pub explanation: String,
}

// ── JSON Schema Generation ──

/// Generate the JSON schema description for Pass 1 structured output.
/// Used in the system prompt to instruct the LLM.
pub fn pass1_schema_description() -> &'static str {
    r#"Respond with a JSON object matching this exact schema:
{
  "groups": [
    {
      "id": "string (group ID from input)",
      "name": "string (human-readable name for this change group)",
      "summary": "string (1-3 sentence summary of what this group changes)",
      "review_order_rationale": "string (why review this group at this position)",
      "risk_flags": ["string (risk flag, e.g. 'auth_change', 'breaking_api', 'schema_change')"]
    }
  ],
  "overall_summary": "string (1-3 sentence overall summary of the entire diff)",
  "suggested_review_order": ["string (group IDs in suggested review order)"]
}"#
}

/// Generate the JSON schema description for the LLM-as-judge evaluator.
pub fn judge_schema_description() -> &'static str {
    r#"Respond with a JSON object matching this exact schema:
{
  "criteria": [
    {
      "criterion": "string (one of: group_coherence, review_ordering, entrypoint_identification, risk_reasonableness, mermaid_accuracy)",
      "score": "integer (1-5, where 1=poor, 2=below average, 3=acceptable, 4=good, 5=excellent)",
      "explanation": "string (brief explanation for the score)"
    }
  ],
  "overall_score": "number (average of all criteria scores, 1.0-5.0)",
  "failure_explanations": ["string (explanation for any criterion scoring below 3)"],
  "strengths": ["string (notable strengths of the analysis)"]
}

You MUST include exactly these 5 criteria in the 'criteria' array:
1. group_coherence: Are files that participate in the same logical data flow grouped together? Are unrelated files separated?
2. review_ordering: Is the suggested review order logical? Are high-risk, foundational changes reviewed first?
3. entrypoint_identification: Are HTTP routes, CLI commands, queue consumers, and other entrypoints correctly identified?
4. risk_reasonableness: Are risk scores sensible? Do auth/schema changes score higher than utility changes?
5. mermaid_accuracy: Does the Mermaid graph accurately represent the data flow between files in each group?"#
}

/// Generate the JSON schema description for Pass 2 structured output.
pub fn pass2_schema_description() -> &'static str {
    r#"Respond with a JSON object matching this exact schema:
{
  "group_id": "string (the group ID being analyzed)",
  "flow_narrative": "string (narrative of how data flows through the changes)",
  "file_annotations": [
    {
      "file": "string (file path)",
      "role_in_flow": "string (this file's role in the data flow)",
      "changes_summary": "string (what changed in this file)",
      "risks": ["string (identified risks)"],
      "suggestions": ["string (improvement suggestions)"]
    }
  ],
  "cross_cutting_concerns": ["string (concerns spanning multiple files)"]
}"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pass1_response_roundtrip() {
        let response = Pass1Response {
            groups: vec![Pass1GroupAnnotation {
                id: "group_1".to_string(),
                name: "User authentication token refresh".to_string(),
                summary: "Changes the token refresh flow to use rotating refresh tokens"
                    .to_string(),
                review_order_rationale:
                    "Review first — changes auth contract that downstream groups depend on"
                        .to_string(),
                risk_flags: vec!["auth_change".to_string(), "breaking_api".to_string()],
            }],
            overall_summary: "Implements rotating refresh tokens and updates downstream consumers"
                .to_string(),
            suggested_review_order: vec!["group_1".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Pass1Response = serde_json::from_str(&json).unwrap();
        assert_eq!(response, deserialized);
    }

    #[test]
    fn test_pass2_response_roundtrip() {
        let response = Pass2Response {
            group_id: "group_1".to_string(),
            flow_narrative: "Data enters at POST /auth/refresh, validated by middleware".to_string(),
            file_annotations: vec![Pass2FileAnnotation {
                file: "src/handlers/auth.rs".to_string(),
                role_in_flow: "Entrypoint — receives refresh token request".to_string(),
                changes_summary: "Added rotation logic for refresh tokens".to_string(),
                risks: vec!["Token invalidation race condition".to_string()],
                suggestions: vec!["Consider adding a mutex on token rotation".to_string()],
            }],
            cross_cutting_concerns: vec![
                "Error handling path doesn't cover token expiry".to_string()
            ],
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Pass2Response = serde_json::from_str(&json).unwrap();
        assert_eq!(response, deserialized);
    }

    #[test]
    fn test_annotations_combined() {
        let annotations = Annotations {
            overview: Some(Pass1Response {
                groups: vec![],
                overall_summary: "test".to_string(),
                suggested_review_order: vec![],
            }),
            deep_analyses: vec![Pass2Response {
                group_id: "g1".to_string(),
                flow_narrative: "test".to_string(),
                file_annotations: vec![],
                cross_cutting_concerns: vec![],
            }],
        };
        let json = serde_json::to_string(&annotations).unwrap();
        let deserialized: Annotations = serde_json::from_str(&json).unwrap();
        assert_eq!(annotations, deserialized);
    }

    #[test]
    fn test_pass1_request_serialization() {
        let request = Pass1Request {
            diff_summary: "47 files changed".to_string(),
            flow_groups: vec![Pass1GroupInput {
                id: "g1".to_string(),
                name: "POST /api/users".to_string(),
                entrypoint: Some("src/routes/users.ts::POST".to_string()),
                files: vec!["src/routes/users.ts".to_string()],
                risk_score: 0.82,
                edge_summary: "users.ts calls user-service.ts".to_string(),
            }],
            graph_summary: "3 nodes, 2 edges".to_string(),
        };
        let json = serde_json::to_string_pretty(&request).unwrap();
        assert!(json.contains("POST /api/users"));
        assert!(json.contains("0.82"));
    }

    #[test]
    fn test_pass2_request_serialization() {
        let request = Pass2Request {
            group_id: "g1".to_string(),
            group_name: "User creation flow".to_string(),
            files: vec![Pass2FileInput {
                path: "src/route.ts".to_string(),
                diff: "+  const user = await createUser(data);".to_string(),
                new_content: Some("full file content".to_string()),
                role: "Entrypoint".to_string(),
            }],
            graph_context: "route.ts -> service.ts -> repo.ts".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Pass2Request = serde_json::from_str(&json).unwrap();
        assert_eq!(request, deserialized);
    }

    #[test]
    fn test_schema_descriptions_not_empty() {
        assert!(!pass1_schema_description().is_empty());
        assert!(!pass2_schema_description().is_empty());
        // Should contain JSON structure markers
        assert!(pass1_schema_description().contains("groups"));
        assert!(pass1_schema_description().contains("overall_summary"));
        assert!(pass2_schema_description().contains("group_id"));
        assert!(pass2_schema_description().contains("file_annotations"));
    }

    #[test]
    fn test_empty_pass1_response() {
        let response = Pass1Response {
            groups: vec![],
            overall_summary: String::new(),
            suggested_review_order: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Pass1Response = serde_json::from_str(&json).unwrap();
        assert_eq!(response, deserialized);
        assert!(deserialized.groups.is_empty());
    }

    #[test]
    fn test_empty_pass2_response() {
        let response = Pass2Response {
            group_id: "g1".to_string(),
            flow_narrative: String::new(),
            file_annotations: vec![],
            cross_cutting_concerns: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Pass2Response = serde_json::from_str(&json).unwrap();
        assert_eq!(response, deserialized);
    }

    #[test]
    fn test_pass1_multiple_groups() {
        let response = Pass1Response {
            groups: vec![
                Pass1GroupAnnotation {
                    id: "g1".to_string(),
                    name: "Auth flow".to_string(),
                    summary: "Changes auth".to_string(),
                    review_order_rationale: "Review first".to_string(),
                    risk_flags: vec!["auth_change".to_string()],
                },
                Pass1GroupAnnotation {
                    id: "g2".to_string(),
                    name: "DB migration".to_string(),
                    summary: "Schema update".to_string(),
                    review_order_rationale: "Review second".to_string(),
                    risk_flags: vec!["schema_change".to_string(), "breaking_api".to_string()],
                },
            ],
            overall_summary: "Auth + DB changes".to_string(),
            suggested_review_order: vec!["g1".to_string(), "g2".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["groups"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["suggested_review_order"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_pass2_multiple_files() {
        let response = Pass2Response {
            group_id: "g1".to_string(),
            flow_narrative: "Complex flow".to_string(),
            file_annotations: vec![
                Pass2FileAnnotation {
                    file: "a.ts".to_string(),
                    role_in_flow: "Entrypoint".to_string(),
                    changes_summary: "Added handler".to_string(),
                    risks: vec![],
                    suggestions: vec![],
                },
                Pass2FileAnnotation {
                    file: "b.ts".to_string(),
                    role_in_flow: "Service".to_string(),
                    changes_summary: "Updated logic".to_string(),
                    risks: vec!["Potential null".to_string()],
                    suggestions: vec!["Add null check".to_string()],
                },
            ],
            cross_cutting_concerns: vec!["Error handling".to_string()],
        };
        assert_eq!(response.file_annotations.len(), 2);
        assert_eq!(response.file_annotations[1].risks.len(), 1);
    }

    // ── Judge Schema Tests ──

    #[test]
    fn test_judge_request_roundtrip() {
        let request = JudgeRequest {
            analysis_json: r#"{"version":"1.0.0"}"#.to_string(),
            source_files: vec![JudgeSourceFile {
                path: "src/route.ts".to_string(),
                content: "export function handler() {}".to_string(),
            }],
            diff_text: "+ new line".to_string(),
            fixture_name: "test fixture".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: JudgeRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(request, deserialized);
    }

    #[test]
    fn test_judge_response_roundtrip() {
        let response = JudgeResponse {
            criteria: vec![
                JudgeCriterionScore {
                    criterion: "group_coherence".to_string(),
                    score: 4,
                    explanation: "Files are well grouped".to_string(),
                },
                JudgeCriterionScore {
                    criterion: "review_ordering".to_string(),
                    score: 3,
                    explanation: "Ordering is acceptable".to_string(),
                },
            ],
            overall_score: 3.5,
            failure_explanations: vec![],
            strengths: vec!["Good entrypoint detection".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: JudgeResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(response, deserialized);
    }

    #[test]
    fn test_judge_response_with_failures() {
        let response = JudgeResponse {
            criteria: vec![JudgeCriterionScore {
                criterion: "mermaid_accuracy".to_string(),
                score: 2,
                explanation: "Graph missing edges".to_string(),
            }],
            overall_score: 2.0,
            failure_explanations: vec!["Mermaid graph is incomplete".to_string()],
            strengths: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["overall_score"], 2.0);
        assert_eq!(parsed["failure_explanations"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_judge_criterion_score_bounds() {
        let scores = vec![1u8, 2, 3, 4, 5];
        for s in scores {
            let criterion = JudgeCriterionScore {
                criterion: "test".to_string(),
                score: s,
                explanation: "test".to_string(),
            };
            let json = serde_json::to_string(&criterion).unwrap();
            let deser: JudgeCriterionScore = serde_json::from_str(&json).unwrap();
            assert_eq!(deser.score, s);
        }
    }

    #[test]
    fn test_judge_schema_description_not_empty() {
        let desc = judge_schema_description();
        assert!(!desc.is_empty());
        assert!(desc.contains("criteria"));
        assert!(desc.contains("group_coherence"));
        assert!(desc.contains("review_ordering"));
        assert!(desc.contains("entrypoint_identification"));
        assert!(desc.contains("risk_reasonableness"));
        assert!(desc.contains("mermaid_accuracy"));
        assert!(desc.contains("overall_score"));
    }

    #[test]
    fn test_judge_source_file_roundtrip() {
        let file = JudgeSourceFile {
            path: "src/service.ts".to_string(),
            content: "export class Service {}".to_string(),
        };
        let json = serde_json::to_string(&file).unwrap();
        let deser: JudgeSourceFile = serde_json::from_str(&json).unwrap();
        assert_eq!(file, deser);
    }

    #[test]
    fn test_judge_request_multiple_source_files() {
        let request = JudgeRequest {
            analysis_json: "{}".to_string(),
            source_files: vec![
                JudgeSourceFile {
                    path: "a.ts".to_string(),
                    content: "// a".to_string(),
                },
                JudgeSourceFile {
                    path: "b.ts".to_string(),
                    content: "// b".to_string(),
                },
                JudgeSourceFile {
                    path: "c.py".to_string(),
                    content: "# c".to_string(),
                },
            ],
            diff_text: "diff".to_string(),
            fixture_name: "multi-file".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["source_files"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_judge_response_all_criteria() {
        let criteria_names = vec![
            "group_coherence",
            "review_ordering",
            "entrypoint_identification",
            "risk_reasonableness",
            "mermaid_accuracy",
        ];
        let criteria: Vec<JudgeCriterionScore> = criteria_names
            .iter()
            .map(|name| JudgeCriterionScore {
                criterion: name.to_string(),
                score: 4,
                explanation: format!("{} is good", name),
            })
            .collect();
        let overall = criteria.iter().map(|c| c.score as f64).sum::<f64>() / criteria.len() as f64;
        let response = JudgeResponse {
            criteria,
            overall_score: overall,
            failure_explanations: vec![],
            strengths: vec!["Complete analysis".to_string()],
        };
        assert_eq!(response.criteria.len(), 5);
        assert!((response.overall_score - 4.0).abs() < f64::EPSILON);
    }
}
