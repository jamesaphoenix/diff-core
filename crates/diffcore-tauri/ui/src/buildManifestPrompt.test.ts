import { describe, it, expect } from "vitest";
import { buildManifestPrompt } from "./buildManifestPrompt";

describe("buildManifestPrompt", () => {
  const defaultOpts = {
    manifestPath: "/repo/.diffcore/groups.json",
    repoPath: "/repo",
    groupCount: 5,
    fileCount: 20,
    infraCount: 3,
  };

  it("includes the manifest file path", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain("/repo/.diffcore/groups.json");
  });

  it("includes group count", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain("5 flow groups");
  });

  it("includes file count", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain("20 changed files");
  });

  it("includes ungrouped count", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain("3 infrastructure/ungrouped files");
  });

  it("includes CLI install instructions", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain("cargo install");
    expect(prompt).toContain("diffcore --version");
  });

  it("includes manifest JSON format", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain('"version": "1.0.0"');
    expect(prompt).toContain('"review_order"');
    expect(prompt).toContain('"unassigned_files"');
  });

  it("includes guidelines", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain("Merge");
    expect(prompt).toContain("Promote");
    expect(prompt).toContain("Split");
    expect(prompt).toContain("Rename");
  });

  it("includes CLI validation command with repo path", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain("diffcore import-groups -i /repo/.diffcore/groups.json --repo /repo");
  });

  it("shows divide-and-conquer for large PRs (>10 groups)", () => {
    const prompt = buildManifestPrompt({ ...defaultOpts, groupCount: 15 });
    expect(prompt).toContain("This PR has 15 groups");
    expect(prompt).toContain("divide-and-conquer");
    expect(prompt).toContain("sub-agents");
  });

  it("shows generic large PR advice for small PRs", () => {
    const prompt = buildManifestPrompt({ ...defaultOpts, groupCount: 3 });
    expect(prompt).not.toContain("This PR has 3 groups");
    expect(prompt).toContain("If the PR is large");
  });

  it("uses repoPath in the validation command", () => {
    const prompt = buildManifestPrompt({
      ...defaultOpts,
      repoPath: "/custom/path",
    });
    expect(prompt).toContain("--repo /custom/path");
  });

  it("handles zero counts", () => {
    const prompt = buildManifestPrompt({
      ...defaultOpts,
      groupCount: 0,
      fileCount: 0,
      infraCount: 0,
    });
    expect(prompt).toContain("0 flow groups");
    expect(prompt).toContain("0 changed files");
    expect(prompt).toContain("0 infrastructure/ungrouped files");
  });

  it("is non-empty for any valid input", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt.length).toBeGreaterThan(100);
  });

  it("includes live watching note", () => {
    const prompt = buildManifestPrompt(defaultOpts);
    expect(prompt).toContain("watching this file for live changes");
  });
});
