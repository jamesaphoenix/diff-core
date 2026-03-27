import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Editor } from "@monaco-editor/react";
import type { FileChange, FileDiffContent, FlowEdge, FlowGroup } from "../types";
import { FLOWDIFF_MONACO_THEME, registerFlowdiffMonacoTheme } from "./monacoTheme";

type OutlineKind = "operation" | "interface" | "type" | "class" | "constant" | "dependency";

type OutlineSectionKey = "operations" | "interfaces" | "classes" | "constants" | "dependencies";

export interface SourceFocusRequest {
  filePath: string;
  symbol: string;
  token: number;
}

interface OutlineItem {
  id: string;
  kind: OutlineKind;
  name: string;
  detail: string;
  startLine?: number;
  endLine?: number;
  changed?: boolean;
  targetFile?: string;
  targetSymbol?: string;
}

interface OutlineSection {
  key: OutlineSectionKey;
  label: string;
  items: OutlineItem[];
}

interface SourceExplorerProps {
  fileDiff: FileDiffContent | null;
  selectedGroup: FlowGroup | null;
  selectedFileChange: FileChange | null;
  focusRequest?: SourceFocusRequest | null;
  onNavigateToSymbol?: (filePath: string, symbolName?: string) => void;
}

interface ParsedDefinition {
  name: string;
  kind: Exclude<OutlineKind, "dependency">;
  startLine: number;
  endLine: number;
}

const SECTION_ORDER: OutlineSectionKey[] = [
  "operations",
  "interfaces",
  "classes",
  "constants",
  "dependencies",
];

const SECTION_LABELS: Record<OutlineSectionKey, string> = {
  operations: "Operations",
  interfaces: "Interfaces & Types",
  classes: "Classes",
  constants: "Constants",
  dependencies: "Dependencies",
};

/** Monaco-backed symbol explorer for the currently selected file. */
export default function SourceExplorer({
  fileDiff,
  selectedGroup,
  selectedFileChange,
  focusRequest,
  onNavigateToSymbol,
}: SourceExplorerProps) {
  const editorRef = useRef<any>(null);
  const decorationsRef = useRef<any>(null);
  const outline = useMemo(
    () => buildOutline(fileDiff, selectedGroup, selectedFileChange),
    [fileDiff, selectedGroup, selectedFileChange],
  );
  const allItems = useMemo(
    () => outline.sections.flatMap((section) => section.items),
    [outline.sections],
  );
  const [selectedItemId, setSelectedItemId] = useState<string | null>(null);

  useEffect(() => {
    if (!fileDiff) {
      setSelectedItemId(null);
      return;
    }
    const defaultItem = allItems.find((item) => item.changed && item.startLine != null) ??
      allItems.find((item) => item.startLine != null) ??
      allItems[0] ??
      null;
    setSelectedItemId(defaultItem?.id ?? null);
  }, [fileDiff?.path, allItems, fileDiff]);

  useEffect(() => {
    if (!focusRequest || !fileDiff || focusRequest.filePath !== fileDiff.path) return;
    const focused = allItems.find((item) => symbolMatches(item.name, focusRequest.symbol));
    if (focused) {
      setSelectedItemId(focused.id);
    }
  }, [allItems, fileDiff, focusRequest]);

  const selectedItem = useMemo(
    () => allItems.find((item) => item.id === selectedItemId) ?? null,
    [allItems, selectedItemId],
  );

  const applyEditorFocus = useCallback((item: OutlineItem | null) => {
    const editor = editorRef.current;
    if (!editor || item?.startLine == null) return;
    const endLine = item.endLine ?? item.startLine;
    editor.revealLineInCenter(item.startLine);
    editor.setSelection({
      startLineNumber: item.startLine,
      startColumn: 1,
      endLineNumber: endLine,
      endColumn: editor.getModel()?.getLineMaxColumn(endLine) ?? 1,
    });
    if (decorationsRef.current) {
      decorationsRef.current.clear();
    }
    decorationsRef.current = editor.createDecorationsCollection([{
      range: {
        startLineNumber: item.startLine,
        startColumn: 1,
        endLineNumber: endLine,
        endColumn: 1,
      },
      options: {
        isWholeLine: true,
        className: "source-symbol-highlight",
        linesDecorationsClassName: "source-symbol-glyph",
      },
    }]);
  }, []);

  useEffect(() => {
    applyEditorFocus(selectedItem);
  }, [applyEditorFocus, selectedItem]);

  const handleItemClick = useCallback((item: OutlineItem) => {
    if (item.targetFile && item.targetFile !== fileDiff?.path) {
      onNavigateToSymbol?.(item.targetFile, item.targetSymbol);
      return;
    }
    setSelectedItemId(item.id);
  }, [fileDiff?.path, onNavigateToSymbol]);

  if (!fileDiff) {
    return <div className="empty-state">Select a file to inspect its source.</div>;
  }

  const sourceText = fileDiff.new_content || fileDiff.old_content || "";
  const selectedMeta = selectedItem
    ? `${kindLabel(selectedItem.kind)}${selectedItem.startLine ? ` • L${selectedItem.startLine}${selectedItem.endLine && selectedItem.endLine !== selectedItem.startLine ? `-${selectedItem.endLine}` : ""}` : ""}`
    : "Read-only source view";

  return (
    <div className="source-explorer" data-testid="source-explorer">
      <aside className="source-outline">
        <div className="source-outline-header">
          <div className="source-outline-eyebrow">Subsystem Source</div>
          <div className="source-outline-file" title={fileDiff.path}>{fileDiff.path}</div>
          <div className="source-outline-meta">
            {selectedFileChange && (
              <span className="source-meta-pill">{selectedFileChange.role}</span>
            )}
            <span className="source-meta-pill">{fileDiff.language}</span>
            {selectedFileChange && (
              <span className="source-meta-pill">
                {selectedFileChange.symbols_changed.length} changed
              </span>
            )}
            {selectedGroup && (
              <span className="source-meta-pill">{selectedGroup.name}</span>
            )}
          </div>
        </div>

        <div className="source-outline-sections">
          {outline.sections.map((section) => (
            <section key={section.key} className="source-outline-section">
              <div className="source-outline-section-header">
                <span>{section.label}</span>
                <span className="source-outline-section-count">{section.items.length}</span>
              </div>
              {section.items.length > 0 ? (
                <div className="source-outline-list">
                  {section.items.map((item) => (
                    <button
                      key={item.id}
                      className={`source-outline-item ${selectedItemId === item.id ? "active" : ""}`}
                      onClick={() => handleItemClick(item)}
                      title={item.detail}
                      type="button"
                    >
                      <span className={`source-outline-kind source-outline-kind-${item.kind}`}>
                        {kindLabel(item.kind)}
                      </span>
                      <span className="source-outline-copy">
                        <span className="source-outline-name">{item.name}</span>
                        <span className="source-outline-detail">{item.detail}</span>
                      </span>
                      {item.changed && (
                        <span className="source-outline-changed">Changed</span>
                      )}
                    </button>
                  ))}
                </div>
              ) : (
                <div className="source-outline-empty">None in this file.</div>
              )}
            </section>
          ))}
        </div>
      </aside>

      <div className="source-editor-pane">
        <div className="source-editor-header">
          <div>
            <div className="source-editor-title">{selectedItem?.name ?? fileDiff.path}</div>
            <div className="source-editor-subtitle">{selectedMeta}</div>
          </div>
          {selectedItem?.targetFile && selectedItem.targetFile !== fileDiff.path && (
            <button
              className="btn source-nav-btn"
              onClick={() => onNavigateToSymbol?.(selectedItem.targetFile!, selectedItem.targetSymbol)}
              type="button"
            >
              Open Linked Symbol
            </button>
          )}
        </div>

        <div className="source-editor-surface">
          <Editor
            value={sourceText}
            language={mapLanguage(fileDiff.language)}
            theme={FLOWDIFF_MONACO_THEME}
            beforeMount={registerFlowdiffMonacoTheme}
            onMount={(editor) => {
              editorRef.current = editor;
              applyEditorFocus(selectedItem);
            }}
            options={{
              readOnly: true,
              readOnlyMessage: { value: "" },
              minimap: { enabled: true, maxColumn: 80 },
              automaticLayout: true,
              scrollBeyondLastLine: false,
              fontSize: 13,
              fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
              lineNumbers: "on",
              glyphMargin: true,
              folding: true,
              renderWhitespace: "selection",
              stickyScroll: { enabled: true },
              padding: { top: 16, bottom: 16 },
              scrollbar: {
                verticalScrollbarSize: 10,
                horizontalScrollbarSize: 10,
              },
            }}
          />
        </div>
      </div>
    </div>
  );
}

function buildOutline(
  fileDiff: FileDiffContent | null,
  selectedGroup: FlowGroup | null,
  selectedFileChange: FileChange | null,
): { sections: OutlineSection[] } {
  if (!fileDiff) {
    return {
      sections: SECTION_ORDER.map((key) => ({
        key,
        label: SECTION_LABELS[key],
        items: [],
      })),
    };
  }

  const sourceText = fileDiff.new_content || fileDiff.old_content || "";
  const definitions = parseDefinitions(fileDiff.language, sourceText);
  const changedSymbols = new Set(
    (selectedFileChange?.symbols_changed ?? []).map((symbol) => normalizeSymbol(symbol)),
  );
  const buckets: Record<OutlineSectionKey, OutlineItem[]> = {
    operations: [],
    interfaces: [],
    classes: [],
    constants: [],
    dependencies: [],
  };
  const seenIds = new Set<string>();
  const seenNormalized = new Set<string>();

  for (const definition of definitions) {
    const section = definition.kind === "operation"
      ? "operations"
      : definition.kind === "interface" || definition.kind === "type"
        ? "interfaces"
        : definition.kind === "class"
          ? "classes"
          : "constants";
    const id = `${definition.kind}:${definition.name}:${definition.startLine}`;
    const item: OutlineItem = {
      id,
      kind: definition.kind,
      name: definition.name,
      detail: `L${definition.startLine}${definition.endLine !== definition.startLine ? `-${definition.endLine}` : ""}`,
      startLine: definition.startLine,
      endLine: definition.endLine,
      changed: changedSymbols.has(normalizeSymbol(definition.name)),
    };
    buckets[section].push(item);
    seenIds.add(item.id);
    seenNormalized.add(normalizeSymbol(definition.name));
  }

  if (selectedGroup?.entrypoint?.file === fileDiff.path) {
    const normalizedEntrypoint = normalizeSymbol(selectedGroup.entrypoint.symbol);
    if (!seenNormalized.has(normalizedEntrypoint)) {
      const routeLine = findSymbolLine(sourceText, selectedGroup.entrypoint.symbol)
        ?? findRouteLine(sourceText, selectedGroup.entrypoint.symbol)
        ?? 1;
      buckets.operations.unshift({
        id: `entrypoint:${selectedGroup.entrypoint.symbol}`,
        kind: "operation",
        name: selectedGroup.entrypoint.symbol,
        detail: `Entrypoint • L${routeLine}`,
        startLine: routeLine,
        endLine: routeLine,
        changed: true,
      });
      seenNormalized.add(normalizedEntrypoint);
    }
  }

  for (const symbol of selectedFileChange?.symbols_changed ?? []) {
    const normalized = normalizeSymbol(symbol);
    if (seenNormalized.has(normalized)) continue;
    const line = findSymbolLine(sourceText, symbol);
    if (line == null) continue;
    buckets.operations.push({
      id: `fallback:${symbol}:${line}`,
      kind: "operation",
      name: symbol,
      detail: `Changed symbol • L${line}`,
      startLine: line,
      endLine: line,
      changed: true,
    });
    seenNormalized.add(normalized);
  }

  for (const item of buildDependencyItems(fileDiff.path, selectedGroup?.edges ?? [])) {
    if (seenIds.has(item.id)) continue;
    buckets.dependencies.push(item);
    seenIds.add(item.id);
  }

  return {
    sections: SECTION_ORDER.map((key) => ({
      key,
      label: SECTION_LABELS[key],
      items: buckets[key],
    })),
  };
}

function buildDependencyItems(filePath: string, edges: FlowEdge[]): OutlineItem[] {
  const seen = new Set<string>();
  const items: OutlineItem[] = [];

  for (const edge of edges) {
    const from = parseEdgeEndpoint(edge.from);
    const to = parseEdgeEndpoint(edge.to);

    let item: OutlineItem | null = null;
    if (from.file === filePath) {
      const id = `dep:out:${edge.edge_type}:${to.file}:${to.symbol}`;
      item = {
        id,
        kind: "dependency",
        name: to.symbol || shortPath(to.file),
        detail: `${edge.edge_type} → ${shortPath(to.file)}`,
        targetFile: to.file,
        targetSymbol: to.symbol || undefined,
      };
    } else if (to.file === filePath) {
      const id = `dep:in:${edge.edge_type}:${from.file}:${from.symbol}`;
      item = {
        id,
        kind: "dependency",
        name: from.symbol || shortPath(from.file),
        detail: `${edge.edge_type} ← ${shortPath(from.file)}`,
        targetFile: from.file,
        targetSymbol: from.symbol || undefined,
      };
    }

    if (!item || seen.has(item.id)) continue;
    seen.add(item.id);
    items.push(item);
  }

  return items;
}

function parseDefinitions(language: string, sourceText: string): ParsedDefinition[] {
  const lines = sourceText.split("\n");
  if (language === "typescript" || language === "javascript") {
    return parseTsLikeDefinitions(lines);
  }
  if (language === "python") {
    return parsePythonDefinitions(lines);
  }
  if (language === "go") {
    return parseGoDefinitions(lines);
  }
  if (language === "rust") {
    return parseRustDefinitions(lines);
  }
  return [];
}

function parseTsLikeDefinitions(lines: string[]): ParsedDefinition[] {
  const results: ParsedDefinition[] = [];
  let braceDepth = 0;
  let currentClass: { name: string; depth: number } | null = null;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const trimmed = line.trim();
    if (!trimmed) {
      braceDepth += countChar(line, "{") - countChar(line, "}");
      continue;
    }

    const classMatch = trimmed.match(/^(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)/);
    if (classMatch) {
      results.push({
        name: classMatch[1],
        kind: "class",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
      currentClass = {
        name: classMatch[1],
        depth: braceDepth + countChar(line, "{"),
      };
    }

    const interfaceMatch = trimmed.match(/^(?:export\s+)?interface\s+([A-Za-z_$][\w$]*)/);
    if (interfaceMatch) {
      results.push({
        name: interfaceMatch[1],
        kind: "interface",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
    }

    const typeMatch = trimmed.match(/^(?:export\s+)?type\s+([A-Za-z_$][\w$]*)\s*=/);
    if (typeMatch) {
      results.push({
        name: typeMatch[1],
        kind: "type",
        startLine: index + 1,
        endLine: findStatementEnd(lines, index),
      });
    }

    const functionMatch = trimmed.match(/^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(/);
    if (functionMatch) {
      results.push({
        name: functionMatch[1],
        kind: "operation",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
    }

    const constArrowMatch = trimmed.match(/^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>/);
    if (constArrowMatch) {
      results.push({
        name: constArrowMatch[1],
        kind: "operation",
        startLine: index + 1,
        endLine: findStatementEnd(lines, index),
      });
    } else {
      const constantMatch = trimmed.match(/^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=/);
      if (constantMatch) {
        results.push({
          name: constantMatch[1],
          kind: "constant",
          startLine: index + 1,
          endLine: findStatementEnd(lines, index),
        });
      }
    }

    if (currentClass) {
      const methodMatch = trimmed.match(/^(?:public\s+|private\s+|protected\s+)?(?:static\s+)?(?:async\s+)?([A-Za-z_$][\w$]*)\s*\(/);
      if (methodMatch && methodMatch[1] !== "constructor" && !/^(if|for|while|switch|catch)$/.test(methodMatch[1])) {
        results.push({
          name: `${currentClass.name}.${methodMatch[1]}`,
          kind: "operation",
          startLine: index + 1,
          endLine: findBlockEnd(lines, index),
        });
      }
    }

    braceDepth += countChar(line, "{") - countChar(line, "}");
    if (currentClass && braceDepth < currentClass.depth) {
      currentClass = null;
    }
  }

  return dedupeDefinitions(results);
}

function parsePythonDefinitions(lines: string[]): ParsedDefinition[] {
  const results: ParsedDefinition[] = [];
  let currentClass: string | null = null;
  let currentIndent = 0;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const trimmed = line.trim();
    const indent = line.length - line.trimStart().length;
    if (!trimmed) continue;

    const classMatch = trimmed.match(/^class\s+([A-Za-z_][\w]*)/);
    if (classMatch) {
      currentClass = classMatch[1];
      currentIndent = indent;
      results.push({
        name: classMatch[1],
        kind: "class",
        startLine: index + 1,
        endLine: findIndentedBlockEnd(lines, index, indent),
      });
      continue;
    }

    const defMatch = trimmed.match(/^def\s+([A-Za-z_][\w]*)\s*\(/);
    if (defMatch) {
      results.push({
        name: currentClass && indent > currentIndent ? `${currentClass}.${defMatch[1]}` : defMatch[1],
        kind: "operation",
        startLine: index + 1,
        endLine: findIndentedBlockEnd(lines, index, indent),
      });
    }

    if (currentClass && indent <= currentIndent && !trimmed.startsWith("@")) {
      currentClass = null;
    }
  }

  return dedupeDefinitions(results);
}

function parseGoDefinitions(lines: string[]): ParsedDefinition[] {
  const results: ParsedDefinition[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const trimmed = lines[index].trim();
    if (!trimmed) continue;

    const funcMatch = trimmed.match(/^func\s+(?:\([^)]*\)\s*)?([A-Za-z_][\w]*)\s*\(/);
    if (funcMatch) {
      results.push({
        name: funcMatch[1],
        kind: "operation",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
      continue;
    }

    const interfaceMatch = trimmed.match(/^type\s+([A-Za-z_][\w]*)\s+interface\b/);
    if (interfaceMatch) {
      results.push({
        name: interfaceMatch[1],
        kind: "interface",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
      continue;
    }

    const structMatch = trimmed.match(/^type\s+([A-Za-z_][\w]*)\s+struct\b/);
    if (structMatch) {
      results.push({
        name: structMatch[1],
        kind: "class",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
      continue;
    }

    const constMatch = trimmed.match(/^(?:const|var)\s+([A-Za-z_][\w]*)/);
    if (constMatch) {
      results.push({
        name: constMatch[1],
        kind: "constant",
        startLine: index + 1,
        endLine: findStatementEnd(lines, index),
      });
    }
  }
  return dedupeDefinitions(results);
}

function parseRustDefinitions(lines: string[]): ParsedDefinition[] {
  const results: ParsedDefinition[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const trimmed = lines[index].trim();
    if (!trimmed) continue;

    const fnMatch = trimmed.match(/^(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][\w]*)\s*\(/);
    if (fnMatch) {
      results.push({
        name: fnMatch[1],
        kind: "operation",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
      continue;
    }

    const traitMatch = trimmed.match(/^(?:pub\s+)?trait\s+([A-Za-z_][\w]*)/);
    if (traitMatch) {
      results.push({
        name: traitMatch[1],
        kind: "interface",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
      continue;
    }

    const structMatch = trimmed.match(/^(?:pub\s+)?(?:struct|enum)\s+([A-Za-z_][\w]*)/);
    if (structMatch) {
      results.push({
        name: structMatch[1],
        kind: "class",
        startLine: index + 1,
        endLine: findBlockEnd(lines, index),
      });
      continue;
    }

    const constMatch = trimmed.match(/^(?:pub\s+)?const\s+([A-Za-z_][\w]*)/);
    if (constMatch) {
      results.push({
        name: constMatch[1],
        kind: "constant",
        startLine: index + 1,
        endLine: findStatementEnd(lines, index),
      });
    }
  }
  return dedupeDefinitions(results);
}

function dedupeDefinitions(definitions: ParsedDefinition[]): ParsedDefinition[] {
  const seen = new Set<string>();
  const deduped: ParsedDefinition[] = [];
  for (const def of definitions) {
    const key = `${def.kind}:${def.name}:${def.startLine}`;
    if (seen.has(key)) continue;
    seen.add(key);
    deduped.push(def);
  }
  return deduped;
}

function findBlockEnd(lines: string[], startIndex: number): number {
  let balance = 0;
  let sawOpen = false;
  for (let index = startIndex; index < lines.length; index += 1) {
    const line = stripQuotedContent(lines[index]);
    const opens = countChar(line, "{");
    const closes = countChar(line, "}");
    balance += opens - closes;
    if (opens > 0) sawOpen = true;
    if (sawOpen && balance <= 0) {
      return index + 1;
    }
  }
  return Math.min(lines.length, startIndex + 1);
}

function findStatementEnd(lines: string[], startIndex: number): number {
  let balance = 0;
  let sawStructuralToken = false;
  for (let index = startIndex; index < lines.length; index += 1) {
    const line = stripQuotedContent(lines[index]);
    balance += countChar(line, "{") - countChar(line, "}");
    balance += countChar(line, "(") - countChar(line, ")");
    balance += countChar(line, "[") - countChar(line, "]");
    if (/[{([=]/.test(line)) sawStructuralToken = true;
    if ((balance <= 0 && /[;}]\s*$/.test(line)) || (!sawStructuralToken && index > startIndex && line.trim() === "")) {
      return index + 1;
    }
  }
  return Math.min(lines.length, startIndex + 1);
}

function findIndentedBlockEnd(lines: string[], startIndex: number, startIndent: number): number {
  for (let index = startIndex + 1; index < lines.length; index += 1) {
    const line = lines[index];
    const trimmed = line.trim();
    if (!trimmed) continue;
    const indent = line.length - line.trimStart().length;
    if (indent <= startIndent) {
      return index;
    }
  }
  return lines.length;
}

function findSymbolLine(sourceText: string, symbol: string): number | null {
  const target = symbol.split(".").pop()?.split("::").pop()?.trim();
  if (!target) return null;
  const lines = sourceText.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    if (lines[index].includes(target)) {
      return index + 1;
    }
  }
  return null;
}

function findRouteLine(sourceText: string, symbol: string): number | null {
  const route = symbol.match(/\b(GET|POST|PUT|PATCH|DELETE)\s+(.+)/i);
  if (!route) return null;
  const method = route[1].toLowerCase();
  const path = route[2].trim();
  const lines = sourceText.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    if (lines[index].includes(`.${method}(`) && lines[index].includes(path)) {
      return index + 1;
    }
  }
  return null;
}

function parseEdgeEndpoint(value: string): { file: string; symbol: string } {
  const parts = value.split("::");
  return {
    file: parts[0] ?? value,
    symbol: parts.slice(1).join("::"),
  };
}

function symbolMatches(name: string, symbol: string): boolean {
  return normalizeSymbol(name) === normalizeSymbol(symbol);
}

function normalizeSymbol(value: string): string {
  return value
    .split("::")
    .pop()
    ?.split(".")
    .pop()
    ?.replace(/[^\w$]/g, "")
    .toLowerCase() ?? value.toLowerCase();
}

function stripQuotedContent(value: string): string {
  return value.replace(/"[^"]*"|'[^']*'|`[^`]*`/g, "");
}

function countChar(value: string, char: string): number {
  let count = 0;
  for (const current of value) {
    if (current === char) count += 1;
  }
  return count;
}

function kindLabel(kind: OutlineKind): string {
  switch (kind) {
    case "operation":
      return "Fn";
    case "interface":
      return "If";
    case "type":
      return "Ty";
    case "class":
      return "Cl";
    case "constant":
      return "Ct";
    case "dependency":
      return "Dp";
    default:
      return "Sy";
  }
}

function mapLanguage(lang: string): string {
  const map: Record<string, string> = {
    typescript: "typescript",
    javascript: "javascript",
    python: "python",
    rust: "rust",
    json: "json",
    toml: "toml",
    yaml: "yaml",
    markdown: "markdown",
    css: "css",
    html: "html",
    sql: "sql",
    shell: "shell",
    go: "go",
    java: "java",
    ruby: "ruby",
    plaintext: "plaintext",
  };
  return map[lang] || "plaintext";
}

function shortPath(path: string): string {
  const parts = path.split("/");
  return parts.length <= 2 ? path : parts.slice(-2).join("/");
}
