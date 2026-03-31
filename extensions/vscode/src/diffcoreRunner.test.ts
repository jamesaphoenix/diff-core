import { describe, it, expect } from "vitest";
import { buildArgs, parseOutput } from "./diffcoreRunner";

// Note: resolveBinaryPath and runDiffcore depend on vscode module,
// so we test the pure functions (buildArgs, parseOutput) directly.

describe("buildArgs", () => {
  it("builds args for branch comparison with defaults", () => {
    const args = buildArgs({ repoPath: "/repo" });
    expect(args).toEqual(["analyze", "--repo", "/repo", "--base", "main"]);
  });

  it("builds args for branch comparison with custom base", () => {
    const args = buildArgs({ repoPath: "/repo", base: "develop" });
    expect(args).toEqual(["analyze", "--repo", "/repo", "--base", "develop"]);
  });

  it("builds args for branch comparison with head", () => {
    const args = buildArgs({ repoPath: "/repo", base: "main", head: "feature" });
    expect(args).toEqual([
      "analyze",
      "--repo",
      "/repo",
      "--base",
      "main",
      "--head",
      "feature",
    ]);
  });

  it("builds args for commit range", () => {
    const args = buildArgs({ repoPath: "/repo", range: "HEAD~5..HEAD" });
    expect(args).toEqual(["analyze", "--repo", "/repo", "--range", "HEAD~5..HEAD"]);
  });

  it("builds args for staged changes", () => {
    const args = buildArgs({ repoPath: "/repo", staged: true });
    expect(args).toEqual(["analyze", "--repo", "/repo", "--staged"]);
  });

  it("builds args for unstaged changes", () => {
    const args = buildArgs({ repoPath: "/repo", unstaged: true });
    expect(args).toEqual(["analyze", "--repo", "/repo", "--unstaged"]);
  });

  it("range takes precedence over staged", () => {
    const args = buildArgs({ repoPath: "/repo", range: "a..b", staged: true });
    expect(args).toEqual(["analyze", "--repo", "/repo", "--range", "a..b"]);
  });

  it("staged takes precedence over unstaged", () => {
    const args = buildArgs({ repoPath: "/repo", staged: true, unstaged: true });
    expect(args).toEqual(["analyze", "--repo", "/repo", "--staged"]);
  });

  it("includes --annotate flag", () => {
    const args = buildArgs({ repoPath: "/repo", annotate: true });
    expect(args).toContain("--annotate");
  });

  it("includes --refine flag", () => {
    const args = buildArgs({ repoPath: "/repo", refine: true });
    expect(args).toContain("--refine");
  });

  it("includes both --annotate and --refine", () => {
    const args = buildArgs({ repoPath: "/repo", annotate: true, refine: true });
    expect(args).toContain("--annotate");
    expect(args).toContain("--refine");
  });

  it("handles repo path with spaces", () => {
    const args = buildArgs({ repoPath: "/my repo/path" });
    expect(args).toContain("/my repo/path");
  });
});

describe("parseOutput", () => {
  const validOutput = JSON.stringify({
    version: "1.0.0",
    diff_source: {
      diff_type: "BranchComparison",
      base: "main",
      head: "feature",
      base_sha: "abc",
      head_sha: "def",
    },
    summary: {
      total_files_changed: 5,
      total_groups: 2,
      languages_detected: ["typescript"],
      frameworks_detected: ["express"],
    },
    groups: [
      {
        id: "group_1",
        name: "POST /api/users",
        entrypoint: {
          file: "src/routes/users.ts",
          symbol: "POST",
          entrypoint_type: "HttpRoute",
        },
        files: [
          {
            path: "src/routes/users.ts",
            flow_position: 0,
            role: "Entrypoint",
            changes: { additions: 10, deletions: 5 },
            symbols_changed: ["POST"],
          },
        ],
        edges: [
          {
            from: "src/routes/users.ts::POST",
            to: "src/services/user.ts::createUser",
            edge_type: "Calls",
          },
        ],
        risk_score: 0.7,
        review_order: 1,
      },
    ],
    infrastructure_group: {
      files: ["tsconfig.json"],
      reason: "Not reachable",
    },
    annotations: null,
  });

  it("parses valid JSON output", () => {
    const result = parseOutput(validOutput);
    expect(result.version).toBe("1.0.0");
    expect(result.groups).toHaveLength(1);
    expect(result.groups[0].id).toBe("group_1");
    expect(result.groups[0].risk_score).toBe(0.7);
  });

  it("handles whitespace-padded output", () => {
    const result = parseOutput(`  \n${validOutput}\n  `);
    expect(result.version).toBe("1.0.0");
  });

  it("throws on empty output", () => {
    expect(() => parseOutput("")).toThrow("empty output");
  });

  it("throws on whitespace-only output", () => {
    expect(() => parseOutput("  \n  ")).toThrow("empty output");
  });

  it("throws on invalid JSON", () => {
    expect(() => parseOutput("not json")).toThrow();
  });

  it("throws on missing required fields", () => {
    expect(() => parseOutput(JSON.stringify({ version: "1.0.0" }))).toThrow(
      "missing required fields"
    );
  });

  it("throws on missing groups array", () => {
    expect(() =>
      parseOutput(
        JSON.stringify({
          version: "1.0.0",
          diff_source: { diff_type: "Staged" },
          summary: { total_files_changed: 0 },
        })
      )
    ).toThrow("missing required fields");
  });

  it("parses output with annotations", () => {
    const withAnnotations = JSON.parse(validOutput);
    withAnnotations.annotations = {
      groups: [{ id: "group_1", name: "test", summary: "s", review_order_rationale: "r", risk_flags: [] }],
      overall_summary: "summary",
      suggested_review_order: ["group_1"],
    };
    const result = parseOutput(JSON.stringify(withAnnotations));
    expect(result.annotations).not.toBeNull();
  });

  it("parses output with null infrastructure_group", () => {
    const noInfra = JSON.parse(validOutput);
    noInfra.infrastructure_group = null;
    const result = parseOutput(JSON.stringify(noInfra));
    expect(result.infrastructure_group).toBeNull();
  });

  it("parses output with empty groups", () => {
    const empty = JSON.parse(validOutput);
    empty.groups = [];
    const result = parseOutput(JSON.stringify(empty));
    expect(result.groups).toHaveLength(0);
  });

  it("preserves all FlowGroup fields", () => {
    const result = parseOutput(validOutput);
    const group = result.groups[0];
    expect(group.name).toBe("POST /api/users");
    expect(group.entrypoint?.symbol).toBe("POST");
    expect(group.entrypoint?.entrypoint_type).toBe("HttpRoute");
    expect(group.files[0].role).toBe("Entrypoint");
    expect(group.files[0].changes.additions).toBe(10);
    expect(group.edges[0].edge_type).toBe("Calls");
    expect(group.review_order).toBe(1);
  });

  it("preserves all DiffSource fields", () => {
    const result = parseOutput(validOutput);
    expect(result.diff_source.diff_type).toBe("BranchComparison");
    expect(result.diff_source.base).toBe("main");
    expect(result.diff_source.head).toBe("feature");
    expect(result.diff_source.base_sha).toBe("abc");
    expect(result.diff_source.head_sha).toBe("def");
  });

  it("preserves AnalysisSummary fields", () => {
    const result = parseOutput(validOutput);
    expect(result.summary.total_files_changed).toBe(5);
    expect(result.summary.total_groups).toBe(2);
    expect(result.summary.languages_detected).toEqual(["typescript"]);
    expect(result.summary.frameworks_detected).toEqual(["express"]);
  });
});
