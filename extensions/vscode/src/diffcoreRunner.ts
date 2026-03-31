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
 * Resolves the path to the diffcore CLI binary.
 * Checks user config first, then falls back to PATH lookup.
 */
export function resolveBinaryPath(): string {
  const config = vscode.workspace.getConfiguration("diffcore");
  const configPath = config.get<string>("binaryPath", "");
  return configPath || "diffcore";
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
 * Parses raw JSON stdout from the diffcore CLI into a typed AnalysisOutput.
 * Throws if the JSON is invalid or doesn't match the expected schema.
 */
export function parseOutput(stdout: string): AnalysisOutput {
  const trimmed = stdout.trim();
  if (!trimmed) {
    throw new Error("diffcore produced empty output");
  }

  const parsed = JSON.parse(trimmed) as AnalysisOutput;

  // Basic schema validation
  if (!parsed.version || !parsed.diff_source || !parsed.summary || !Array.isArray(parsed.groups)) {
    throw new Error(
      "diffcore output missing required fields (version, diff_source, summary, groups)"
    );
  }

  return parsed;
}

/**
 * Runs the diffcore CLI and returns parsed analysis output.
 */
export async function runDiffcore(options: RunOptions): Promise<RunResult> {
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
      const execError = error as Error & {
        code?: string;
        stderr?: string;
        killed?: boolean;
        signal?: string;
      };

      if (execError.code === "ENOENT") {
        throw new Error(
          `diffcore binary not found at "${binaryPath}". ` +
            'Install diffcore or set "diffcore.binaryPath" in settings.'
        );
      }

      if (execError.code === "EACCES") {
        throw new Error(
          `diffcore binary at "${binaryPath}" is not executable. ` +
            "Check file permissions (chmod +x)."
        );
      }

      if (execError.killed || execError.signal === "SIGTERM") {
        throw new Error(
          "diffcore timed out after 2 minutes. " +
            "Try analyzing a smaller diff range or check if the repository is very large."
        );
      }

      if (execError.stderr) {
        throw new Error(`diffcore failed: ${execError.stderr}`);
      }

      throw new Error(`diffcore failed: ${error.message}`);
    }
    throw error;
  }
}
