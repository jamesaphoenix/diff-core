import { describe, it, expect } from "vitest";
import type {
  AnalysisOutput,
  FlowGroup,
  FileChange,
  InfrastructureGroup,
} from "./types";

// We can't import vscode in unit tests, so we test the data logic
// by verifying the types and data flow patterns used by the tree view.

// ── Test fixtures ───────────────────────────────────────────────────

function makeFileChange(overrides: Partial<FileChange> = {}): FileChange {
  return {
    path: "src/handler.ts",
    flow_position: 0,
    role: "Entrypoint",
    changes: { additions: 10, deletions: 5 },
    symbols_changed: ["handleRequest"],
    ...overrides,
  };
}

function makeGroup(overrides: Partial<FlowGroup> = {}): FlowGroup {
  return {
    id: "group_1",
    name: "POST /api/users",
    entrypoint: {
      file: "src/routes/users.ts",
      symbol: "POST",
      entrypoint_type: "HttpRoute",
    },
    files: [makeFileChange()],
    edges: [],
    risk_score: 0.7,
    review_order: 1,
    ...overrides,
  };
}

function makeAnalysis(overrides: Partial<AnalysisOutput> = {}): AnalysisOutput {
  return {
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
      frameworks_detected: [],
    },
    groups: [makeGroup()],
    infrastructure_group: null,
    annotations: null,
    ...overrides,
  };
}

// ── Data logic tests (no vscode dependency) ─────────────────────────

describe("FlowGroup sorting", () => {
  it("sorts groups by review_order ascending", () => {
    const groups = [
      makeGroup({ id: "g3", review_order: 3, risk_score: 0.1 }),
      makeGroup({ id: "g1", review_order: 1, risk_score: 0.9 }),
      makeGroup({ id: "g2", review_order: 2, risk_score: 0.5 }),
    ];
    const sorted = groups.slice().sort((a, b) => a.review_order - b.review_order);
    expect(sorted.map((g) => g.id)).toEqual(["g1", "g2", "g3"]);
  });

  it("handles single group", () => {
    const groups = [makeGroup()];
    const sorted = groups.slice().sort((a, b) => a.review_order - b.review_order);
    expect(sorted).toHaveLength(1);
  });

  it("handles empty groups", () => {
    const groups: FlowGroup[] = [];
    const sorted = groups.slice().sort((a, b) => a.review_order - b.review_order);
    expect(sorted).toHaveLength(0);
  });
});

describe("File sorting within groups", () => {
  it("sorts files by flow_position ascending", () => {
    const files = [
      makeFileChange({ path: "repo.ts", flow_position: 2, role: "Repository" }),
      makeFileChange({ path: "handler.ts", flow_position: 0, role: "Entrypoint" }),
      makeFileChange({ path: "service.ts", flow_position: 1, role: "Service" }),
    ];
    const sorted = files.slice().sort((a, b) => a.flow_position - b.flow_position);
    expect(sorted.map((f) => f.path)).toEqual(["handler.ts", "service.ts", "repo.ts"]);
  });

  it("handles empty files", () => {
    const group = makeGroup({ files: [] });
    expect(group.files).toHaveLength(0);
  });
});

describe("Risk classification", () => {
  it("classifies high risk (>= 0.7)", () => {
    const group = makeGroup({ risk_score: 0.8 });
    const level = group.risk_score >= 0.7 ? "high" : group.risk_score >= 0.4 ? "medium" : "low";
    expect(level).toBe("high");
  });

  it("classifies medium risk (0.4 - 0.7)", () => {
    const group = makeGroup({ risk_score: 0.5 });
    const level = group.risk_score >= 0.7 ? "high" : group.risk_score >= 0.4 ? "medium" : "low";
    expect(level).toBe("medium");
  });

  it("classifies low risk (< 0.4)", () => {
    const group = makeGroup({ risk_score: 0.2 });
    const level = group.risk_score >= 0.7 ? "high" : group.risk_score >= 0.4 ? "medium" : "low";
    expect(level).toBe("low");
  });

  it("boundary: exactly 0.7 is high", () => {
    const group = makeGroup({ risk_score: 0.7 });
    const level = group.risk_score >= 0.7 ? "high" : group.risk_score >= 0.4 ? "medium" : "low";
    expect(level).toBe("high");
  });

  it("boundary: exactly 0.4 is medium", () => {
    const group = makeGroup({ risk_score: 0.4 });
    const level = group.risk_score >= 0.7 ? "high" : group.risk_score >= 0.4 ? "medium" : "low";
    expect(level).toBe("medium");
  });
});

describe("Short path generation", () => {
  it("extracts last two segments", () => {
    const file = makeFileChange({ path: "src/routes/users.ts" });
    const shortPath = file.path.split("/").slice(-2).join("/");
    expect(shortPath).toBe("routes/users.ts");
  });

  it("handles single segment path", () => {
    const file = makeFileChange({ path: "file.ts" });
    const shortPath = file.path.split("/").slice(-2).join("/");
    expect(shortPath).toBe("file.ts");
  });

  it("handles deeply nested path", () => {
    const file = makeFileChange({ path: "a/b/c/d/e.ts" });
    const shortPath = file.path.split("/").slice(-2).join("/");
    expect(shortPath).toBe("d/e.ts");
  });
});

describe("Infrastructure group", () => {
  it("handles null infrastructure group", () => {
    const analysis = makeAnalysis({ infrastructure_group: null });
    expect(analysis.infrastructure_group).toBeNull();
  });

  it("handles empty infrastructure files", () => {
    const infra: InfrastructureGroup = { files: [], reason: "test" };
    expect(infra.files).toHaveLength(0);
  });

  it("lists infrastructure files", () => {
    const infra: InfrastructureGroup = {
      files: ["tsconfig.json", "package.json", ".eslintrc"],
      reason: "Not reachable from any detected entrypoint",
    };
    expect(infra.files).toHaveLength(3);
  });
});

describe("Entrypoint display", () => {
  it("renders entrypoint info", () => {
    const group = makeGroup();
    expect(group.entrypoint).not.toBeNull();
    expect(group.entrypoint!.symbol).toBe("POST");
    expect(group.entrypoint!.entrypoint_type).toBe("HttpRoute");
  });

  it("handles null entrypoint", () => {
    const group = makeGroup({ entrypoint: null });
    expect(group.entrypoint).toBeNull();
  });
});

describe("Change stats display", () => {
  it("formats additions and deletions", () => {
    const file = makeFileChange({ changes: { additions: 25, deletions: 10 } });
    const display = `+${file.changes.additions} -${file.changes.deletions}`;
    expect(display).toBe("+25 -10");
  });

  it("handles zero changes", () => {
    const file = makeFileChange({ changes: { additions: 0, deletions: 0 } });
    const display = `+${file.changes.additions} -${file.changes.deletions}`;
    expect(display).toBe("+0 -0");
  });
});

describe("Navigation state", () => {
  it("navigates through groups sequentially", () => {
    const groups = [
      makeGroup({ id: "g1", review_order: 1 }),
      makeGroup({ id: "g2", review_order: 2 }),
      makeGroup({ id: "g3", review_order: 3 }),
    ];
    const sorted = groups.slice().sort((a, b) => a.review_order - b.review_order);

    let groupIndex = 0;

    // Next group
    groupIndex++;
    expect(sorted[groupIndex].id).toBe("g2");

    // Next group
    groupIndex++;
    expect(sorted[groupIndex].id).toBe("g3");

    // Can't go beyond last
    expect(groupIndex).toBe(sorted.length - 1);
  });

  it("navigates through files within a group", () => {
    const files = [
      makeFileChange({ path: "route.ts", flow_position: 0 }),
      makeFileChange({ path: "service.ts", flow_position: 1 }),
      makeFileChange({ path: "repo.ts", flow_position: 2 }),
    ];
    const sorted = files.slice().sort((a, b) => a.flow_position - b.flow_position);

    let fileIndex = 0;
    expect(sorted[fileIndex].path).toBe("route.ts");

    fileIndex++;
    expect(sorted[fileIndex].path).toBe("service.ts");

    fileIndex++;
    expect(sorted[fileIndex].path).toBe("repo.ts");
  });

  it("resets file index when changing groups", () => {
    let groupIndex = 0;
    let fileIndex = 2; // mid-way through files

    // Change group
    groupIndex++;
    fileIndex = 0; // reset
    expect(fileIndex).toBe(0);
  });
});
