#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INTERVAL_SECONDS="${1:-120}"
PROMPT_FILE="${2:-$ROOT_DIR/.claude/commands/research.md}"
LOG_DIR="${LOG_DIR:-/tmp/codex-research-loop-logs}"
CODEX_BIN="${CODEX_BIN:-codex}"
CORPUS_DIR="${CORPUS_DIR:-$HOME/Desktop/projects/just-understanding-data/diffcore-eval-corpus}"
PLAN_DIR="${PLAN_DIR:-$HOME/.codex/plans}"
RUN_ONCE="${RUN_ONCE:-0}"
DANGEROUS="${DANGEROUS:-0}"
KEEP_REMOTE_MCP="${KEEP_REMOTE_MCP:-0}"

if ! [[ "$INTERVAL_SECONDS" =~ ^[0-9]+$ ]] || [[ "$INTERVAL_SECONDS" -lt 1 ]]; then
  echo "error: interval must be a positive integer number of seconds" >&2
  exit 1
fi

if ! command -v "$CODEX_BIN" >/dev/null 2>&1; then
  echo "error: '$CODEX_BIN' is not installed or not on PATH" >&2
  exit 1
fi

if [[ ! -f "$PROMPT_FILE" ]]; then
  echo "error: prompt file not found: $PROMPT_FILE" >&2
  exit 1
fi

mkdir -p "$LOG_DIR" "$PLAN_DIR"

build_codex_args() {
  codex_args=(exec -C "$ROOT_DIR" --color always --add-dir "$PLAN_DIR")

  if [[ "$KEEP_REMOTE_MCP" != "1" ]]; then
    codex_args+=(
      -c "mcp_servers.openaiDeveloperDocs.enabled=false"
      -c "mcp_servers.auggie.enabled=false"
      -c "mcp_servers.linear.enabled=false"
      -c "mcp_servers.context7.enabled=false"
    )
  fi

  if [[ -d "$CORPUS_DIR" ]]; then
    codex_args+=(--add-dir "$CORPUS_DIR")
  fi

  if [[ "$DANGEROUS" == "1" ]]; then
    codex_args+=(--dangerously-bypass-approvals-and-sandbox)
  else
    codex_args+=(--full-auto)
  fi

  if [[ -n "${CODEX_MODEL:-}" ]]; then
    codex_args+=(-m "$CODEX_MODEL")
  fi

  if [[ -n "${CODEX_PROFILE:-}" ]]; then
    codex_args+=(-p "$CODEX_PROFILE")
  fi
}

render_prompt() {
  cat <<EOF
Execute exactly one autonomous research-loop iteration for this repository.

Treat the attached instruction file as the canonical task spec. If it mentions
Claude Code, slash commands like /research, or an Agent tool, map those to the
closest Codex equivalent and continue.

Do one run, make the changes you judge appropriate, then exit cleanly.

EOF
  cat "$PROMPT_FILE"
}

iteration=0

while true; do
  iteration=$((iteration + 1))
  timestamp="$(date '+%Y-%m-%d-%H%M%S')"
  log_file="$LOG_DIR/run-$timestamp.log"
  last_message_file="$LOG_DIR/run-$timestamp.last.txt"
  codex_args=()
  build_codex_args
  ln -sfn "$log_file" "$LOG_DIR/latest.log"
  ln -sfn "$last_message_file" "$LOG_DIR/latest.last.txt"

  echo
  echo "============================================================"
  echo "Codex research loop #$iteration at $(date '+%Y-%m-%d %H:%M:%S')"
  echo "Repo:   $ROOT_DIR"
  echo "Prompt: $PROMPT_FILE"
  echo "Log:    $log_file"
  echo "============================================================"

  set +e
  render_prompt | "$CODEX_BIN" "${codex_args[@]}" -o "$last_message_file" - 2>&1 | tee "$log_file"
  exit_code=${PIPESTATUS[1]}
  set -e

  if [[ $exit_code -eq 0 ]]; then
    echo "run #$iteration finished successfully"
  else
    echo "run #$iteration failed with exit code $exit_code"
  fi

  if [[ "$RUN_ONCE" == "1" ]]; then
    break
  fi

  echo "sleeping for $INTERVAL_SECONDS seconds"
  sleep "$INTERVAL_SECONDS"
done
