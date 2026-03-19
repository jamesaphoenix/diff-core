import * as vscode from "vscode";
import { execFile } from "child_process";
import { promisify } from "util";
import type { AnalysisOutput } from "./types";

const execFileAsync = promisify(execFile);

export interface RunOptions {
  repoPath: string;
  base?: string;
  head?: string;
  range?: string;
  staged?: boolean;
  unstaged?: boolean;
  annotate?: boolean;
  refine?: boolean;
}

export interface RunResult {
  output: AnalysisOutput;
  stderr: string;
}

/**
 * Resolves the path to the flowdiff CLI binary.
 * Checks user config first, then falls back to PATH lookup.
 */
export function resolveBinaryPath(): string {
  const config = vscode.workspace.getConfiguration("flowdiff");
  const configPath = config.get<string>("binaryPath", "");
  return configPath || "flowdiff";
}

/**
 * Builds CLI arguments from run options.
 */
export function buildArgs(options: RunOptions): string[] {
  const args = ["analyze", "--repo", options.repoPath];

  if (options.range) {
    args.push("--range", options.range);
  } else if (options.staged) {
    args.push("--staged");
  } else if (options.unstaged) {
    args.push("--unstaged");
  } else {
    const base = options.base ?? "main";
    args.push("--base", base);
    if (options.head) {
      args.push("--head", options.head);
    }
  }

  if (options.annotate) {
    args.push("--annotate");
  }

  if (options.refine) {
    args.push("--refine");
  }

  return args;
}

/**
 * Parses raw JSON stdout from the flowdiff CLI into a typed AnalysisOutput.
 * Throws if the JSON is invalid or doesn't match the expected schema.
 */
export function parseOutput(stdout: string): AnalysisOutput {
  const trimmed = stdout.trim();
  if (!trimmed) {
    throw new Error("flowdiff produced empty output");
  }

  const parsed = JSON.parse(trimmed) as AnalysisOutput;

  // Basic schema validation
  if (!parsed.version || !parsed.diff_source || !parsed.summary || !Array.isArray(parsed.groups)) {
    throw new Error(
      "flowdiff output missing required fields (version, diff_source, summary, groups)"
    );
  }

  return parsed;
}

/**
 * Runs the flowdiff CLI and returns parsed analysis output.
 */
export async function runFlowdiff(options: RunOptions): Promise<RunResult> {
  const binaryPath = resolveBinaryPath();
  const args = buildArgs(options);

  try {
    const { stdout, stderr } = await execFileAsync(binaryPath, args, {
      maxBuffer: 50 * 1024 * 1024, // 50 MB for large diffs
      timeout: 120_000, // 2 minute timeout
    });

    const output = parseOutput(stdout);
    return { output, stderr };
  } catch (error: unknown) {
    if (error instanceof Error) {
      const execError = error as Error & { code?: string; stderr?: string };

      if (execError.code === "ENOENT") {
        throw new Error(
          `flowdiff binary not found at "${binaryPath}". ` +
            'Install flowdiff or set "flowdiff.binaryPath" in settings.'
        );
      }

      if (execError.stderr) {
        throw new Error(`flowdiff failed: ${execError.stderr}`);
      }

      throw new Error(`flowdiff failed: ${error.message}`);
    }
    throw error;
  }
}
