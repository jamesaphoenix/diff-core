import { describe, it, expect } from "vitest";
import type {
  FlowGroup,
  FlowEdge,
  Pass1Response,
  Pass2Response,
  Pass2FileAnnotation,
} from "./types";

// We test the pure HTML generation logic that the webview panel uses,
// without depending on the vscode module.

// ── Test fixtures ───────────────────────────────────────────────────

function makeGroup(overrides: Partial<FlowGroup> = {}): FlowGroup {
  return {
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
    risk_score: 0.82,
    review_order: 1,
    ...overrides,
  };
}

// ── HTML escape tests ───────────────────────────────────────────────

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

describe("HTML escaping", () => {
  it("escapes ampersands", () => {
    expect(escapeHtml("a & b")).toBe("a &amp; b");
  });

  it("escapes angle brackets", () => {
    expect(escapeHtml("<script>alert('xss')</script>")).toBe(
      "&lt;script&gt;alert('xss')&lt;/script&gt;"
    );
  });

  it("escapes double quotes", () => {
    expect(escapeHtml('"hello"')).toBe("&quot;hello&quot;");
  });

  it("handles empty string", () => {
    expect(escapeHtml("")).toBe("");
  });

  it("handles string with no special chars", () => {
    expect(escapeHtml("hello world")).toBe("hello world");
  });

  it("handles multiple special chars", () => {
    expect(escapeHtml('a & b < c > d "e"')).toBe(
      "a &amp; b &lt; c &gt; d &quot;e&quot;"
    );
  });
});

// ── Risk badge tests ────────────────────────────────────────────────

function riskLevel(score: number): "high" | "medium" | "low" {
  if (score >= 0.7) return "high";
  if (score >= 0.4) return "medium";
  return "low";
}

describe("Risk badge", () => {
  it("high risk for score >= 0.7", () => {
    expect(riskLevel(0.82)).toBe("high");
    expect(riskLevel(0.7)).toBe("high");
    expect(riskLevel(1.0)).toBe("high");
  });

  it("medium risk for 0.4 <= score < 0.7", () => {
    expect(riskLevel(0.5)).toBe("medium");
    expect(riskLevel(0.4)).toBe("medium");
    expect(riskLevel(0.69)).toBe("medium");
  });

  it("low risk for score < 0.4", () => {
    expect(riskLevel(0.1)).toBe("low");
    expect(riskLevel(0.0)).toBe("low");
    expect(riskLevel(0.39)).toBe("low");
  });
});

// ── Mermaid graph generation tests ──────────────────────────────────

/** Escape a string for use inside a Mermaid node label (within double quotes). */
function escapeMermaidLabel(s: string): string {
  return s.replace(/"/g, "#quot;").replace(/</g, "#lt;").replace(/>/g, "#gt;");
}

function buildMermaid(group: FlowGroup): string {
  if (group.edges.length === 0) {
    return "graph TD\n  A[No edges]";
  }

  const lines = ["graph TD"];
  const nodeIds = new Map<string, string>();
  let counter = 0;

  function getNodeId(label: string): string {
    if (!nodeIds.has(label)) {
      nodeIds.set(label, `N${counter++}`);
    }
    return nodeIds.get(label)!;
  }

  for (const edge of group.edges) {
    const fromId = getNodeId(edge.from);
    const toId = getNodeId(edge.to);
    const fromLabel = escapeMermaidLabel(edge.from.split("::").pop() ?? edge.from);
    const toLabel = escapeMermaidLabel(edge.to.split("::").pop() ?? edge.to);
    lines.push(`  ${fromId}["${fromLabel}"] -->|${edge.edge_type}| ${toId}["${toLabel}"]`);
  }

  return lines.join("\n");
}

describe("Mermaid graph generation", () => {
  it("generates empty graph placeholder", () => {
    const group = makeGroup({ edges: [] });
    const mermaid = buildMermaid(group);
    expect(mermaid).toBe("graph TD\n  A[No edges]");
  });

  it("generates single edge graph", () => {
    const group = makeGroup();
    const mermaid = buildMermaid(group);
    expect(mermaid).toContain("graph TD");
    expect(mermaid).toContain("POST");
    expect(mermaid).toContain("createUser");
    expect(mermaid).toContain("Calls");
  });

  it("generates multi-edge graph", () => {
    const edges: FlowEdge[] = [
      { from: "a.ts::foo", to: "b.ts::bar", edge_type: "Calls" },
      { from: "b.ts::bar", to: "c.ts::baz", edge_type: "Imports" },
    ];
    const group = makeGroup({ edges });
    const mermaid = buildMermaid(group);
    expect(mermaid).toContain("foo");
    expect(mermaid).toContain("bar");
    expect(mermaid).toContain("baz");
    expect(mermaid.split("\n").length).toBe(3); // header + 2 edges
  });

  it("reuses node IDs for same symbol", () => {
    const edges: FlowEdge[] = [
      { from: "a.ts::foo", to: "b.ts::bar", edge_type: "Calls" },
      { from: "b.ts::bar", to: "c.ts::baz", edge_type: "Calls" },
    ];
    const group = makeGroup({ edges });
    const mermaid = buildMermaid(group);
    // b.ts::bar appears in both edges — should use same node ID
    const lines = mermaid.split("\n").slice(1);
    const barIds = lines
      .flatMap((l) => [...l.matchAll(/N(\d+)\["bar"\]/g)])
      .map((m) => m[1]);
    // The same ID should be used for "bar" in both edges
    expect(new Set(barIds).size).toBe(1);
  });

  it("extracts symbol name from qualified path", () => {
    const edges: FlowEdge[] = [
      {
        from: "src/deep/nested/file.ts::myFunction",
        to: "src/other/file.ts::otherFunction",
        edge_type: "Calls",
      },
    ];
    const group = makeGroup({ edges });
    const mermaid = buildMermaid(group);
    expect(mermaid).toContain("myFunction");
    expect(mermaid).toContain("otherFunction");
    expect(mermaid).not.toContain("src/deep/nested");
  });

  it("handles edges without :: separator", () => {
    const edges: FlowEdge[] = [
      { from: "module_a", to: "module_b", edge_type: "Imports" },
    ];
    const group = makeGroup({ edges });
    const mermaid = buildMermaid(group);
    expect(mermaid).toContain("module_a");
    expect(mermaid).toContain("module_b");
  });
});

// ── Pass1/Pass2 annotation structure tests ──────────────────────────

describe("Pass1 annotation structure", () => {
  it("has required fields", () => {
    const pass1: Pass1Response = {
      groups: [
        {
          id: "g1",
          name: "Test group",
          summary: "A test summary",
          review_order_rationale: "Review first because...",
          risk_flags: ["auth_change", "breaking_api"],
        },
      ],
      overall_summary: "Overall changes summary",
      suggested_review_order: ["g1"],
    };
    expect(pass1.groups).toHaveLength(1);
    expect(pass1.groups[0].risk_flags).toHaveLength(2);
    expect(pass1.overall_summary).toBeTruthy();
  });

  it("handles empty risk flags", () => {
    const pass1: Pass1Response = {
      groups: [
        {
          id: "g1",
          name: "Safe change",
          summary: "Minor refactor",
          review_order_rationale: "Low priority",
          risk_flags: [],
        },
      ],
      overall_summary: "Minor changes",
      suggested_review_order: ["g1"],
    };
    expect(pass1.groups[0].risk_flags).toHaveLength(0);
  });
});

describe("Pass2 annotation structure", () => {
  it("has required fields", () => {
    const pass2: Pass2Response = {
      group_id: "g1",
      flow_narrative: "Data flows from A to B to C",
      file_annotations: [
        {
          file: "src/handler.ts",
          role_in_flow: "Entrypoint",
          changes_summary: "Added validation",
          risks: ["Missing edge case"],
          suggestions: ["Add error handling"],
        },
      ],
      cross_cutting_concerns: ["Error handling inconsistent"],
    };
    expect(pass2.file_annotations).toHaveLength(1);
    expect(pass2.cross_cutting_concerns).toHaveLength(1);
  });

  it("handles empty annotations", () => {
    const pass2: Pass2Response = {
      group_id: "g1",
      flow_narrative: "Simple change",
      file_annotations: [],
      cross_cutting_concerns: [],
    };
    expect(pass2.file_annotations).toHaveLength(0);
  });

  it("handles file annotation with no risks or suggestions", () => {
    const annotation: Pass2FileAnnotation = {
      file: "src/config.ts",
      role_in_flow: "Config",
      changes_summary: "Updated defaults",
      risks: [],
      suggestions: [],
    };
    expect(annotation.risks).toHaveLength(0);
    expect(annotation.suggestions).toHaveLength(0);
  });
});

// ── Security: Mermaid label escaping tests ──────────────────────────

describe("Mermaid label escaping", () => {
  it("escapes double quotes in symbol names", () => {
    const label = escapeMermaidLabel('foo"bar');
    expect(label).toBe("foo#quot;bar");
    expect(label).not.toContain('"');
  });

  it("escapes < and > in symbol names", () => {
    const label = escapeMermaidLabel("Array<string>");
    expect(label).toBe("Array#lt;string#gt;");
    expect(label).not.toContain("<");
    expect(label).not.toContain(">");
  });

  it("preserves normal labels unchanged", () => {
    const label = escapeMermaidLabel("createUser");
    expect(label).toBe("createUser");
  });

  it("escapes labels in Mermaid graph output", () => {
    const edges: FlowEdge[] = [
      {
        from: 'src/file.ts::handle"quote',
        to: "src/other.ts::Array<T>",
        edge_type: "Calls",
      },
    ];
    const group = makeGroup({ edges });
    const mermaid = buildMermaid(group);
    expect(mermaid).toContain("#quot;");
    expect(mermaid).toContain("#lt;");
    expect(mermaid).toContain("#gt;");
    // Should NOT contain raw special chars inside labels
    expect(mermaid).not.toMatch(/\["[^"]*"[^"]*"\]/);
  });

  it("handles multiple special chars in one label", () => {
    const label = escapeMermaidLabel('<script>"alert"</script>');
    expect(label).not.toContain("<");
    expect(label).not.toContain(">");
    expect(label).not.toContain('"');
  });
});
