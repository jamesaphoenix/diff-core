/**
 * Build an agent prompt for iterative manifest refinement.
 * Extracted as a pure function for testability.
 */
export function buildManifestPrompt(opts: {
  manifestPath: string;
  repoPath: string;
  groupCount: number;
  fileCount: number;
  infraCount: number;
}): string {
  const { manifestPath, repoPath, groupCount, fileCount, infraCount } = opts;

  return `# Diffcore Groups Manifest Refinement

You are refining the flow groupings for a PR diff analysis. The groupings have been exported to a JSON manifest that the Diffcore desktop app is watching for live changes.

## Setup

\`\`\`bash
# Install the Diffcore CLI (if not already installed)
cargo install --git https://github.com/jamesaphoenix/diff-core.git diffcore-cli

# Verify installation
diffcore --version
\`\`\`

## Current State

- **Manifest file**: \`${manifestPath}\`
- **Groups**: ${groupCount} flow groups
- **Files**: ${fileCount} changed files
- **Ungrouped**: ${infraCount} infrastructure/ungrouped files
- **The Diffcore desktop app is watching this file for live changes**

## Your Task

Edit \`${manifestPath}\` to improve the groupings. The desktop app will update in real-time as you save.

### Manifest Format

\`\`\`json
{
  "version": "1.0.0",
  "groups": [
    {
      "name": "descriptive name of the change",
      "files": ["path/to/file.ts", "path/to/other.ts"],
      "review_order": 1,
      "description": "optional context for reviewers"
    }
  ],
  "unassigned_files": ["files/not/in/any/group.ts"]
}
\`\`\`

### Guidelines

1. **Read the manifest** first: \`cat ${manifestPath}\`
2. **Merge** scattered single-file groups that belong to the same domain/feature
3. **Promote** ungrouped files into groups where they logically belong (schemas with their services, configs with their features)
4. **Split** groups that contain unrelated changes
5. **Rename** groups to be descriptive: "media asset upload pipeline" not "page test flow"
6. **Order** review by dependency direction: schemas/types -> data layer -> services -> API routes -> UI/tests
7. **Validate** after each edit by checking the desktop app updates correctly
8. Leave truly infrastructure files (package.json, CI configs, lockfiles) as unassigned

### Workflow

\`\`\`bash
# Read current manifest
cat ${manifestPath}

# Edit with your changes (use Write tool or Edit tool)
# The desktop app updates automatically on save

# You can also use the CLI to validate:
diffcore import-groups -i ${manifestPath} --repo ${repoPath || "."}
\`\`\`

### Divide and Conquer (for ${groupCount > 10 ? "this large PR" : "large PRs"})

${groupCount > 10 ? `This PR has ${groupCount} groups — use a divide-and-conquer strategy:
1. First identify the major domains/features being changed
2. Group files by domain, then by layer within each domain
3. Merge scattered single-file groups that belong to the same domain
4. Consider using sub-agents to handle each domain independently` : "If the PR is large (10+ groups), identify major domains first, then group by domain and layer."}
`;
}
