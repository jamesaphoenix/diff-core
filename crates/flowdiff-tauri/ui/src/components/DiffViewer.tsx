import { useState, useCallback, useRef } from "react";
import { DiffEditor } from "@monaco-editor/react";
import type { FileDiffContent, ReviewComment } from "../types";

interface DiffViewerProps {
  fileDiff: FileDiffContent | null;
  /** Called when user selects lines and clicks "Comment" in the modified editor. */
  onCommentRequest?: (startLine: number, endLine: number, selectedCode: string) => void;
  /** Comments for the current file (code-level only). */
  codeComments?: ReviewComment[];
}

/** Monaco-based side-by-side diff viewer for the center panel. */
export default function DiffViewer({ fileDiff, onCommentRequest, codeComments }: DiffViewerProps) {
  const [selectionRange, setSelectionRange] = useState<{ startLine: number; endLine: number } | null>(null);
  const [commentBtnPos, setCommentBtnPos] = useState<{ top: number; left: number } | null>(null);
  const editorRef = useRef<any>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  const handleEditorMount = useCallback(
    (editor: any) => {
      editorRef.current = editor;

      // Get the modified editor (right side of the diff)
      const modifiedEditor = editor.getModifiedEditor();
      if (!modifiedEditor) return;

      // Listen for selection changes
      modifiedEditor.onDidChangeCursorSelection((e: any) => {
        const sel = e.selection;
        // Only show the comment button for multi-line selections
        if (sel && sel.startLineNumber !== sel.endLineNumber) {
          const startLine = Math.min(sel.startLineNumber, sel.endLineNumber);
          const endLine = Math.max(sel.startLineNumber, sel.endLineNumber);
          setSelectionRange({ startLine, endLine });

          // Position the comment button near the selection end
          const coords = modifiedEditor.getScrolledVisiblePosition({
            lineNumber: endLine,
            column: 1,
          });
          if (coords && containerRef.current) {
            const containerRect = containerRef.current.getBoundingClientRect();
            const editorDom = modifiedEditor.getDomNode();
            const editorRect = editorDom?.getBoundingClientRect();
            if (editorRect) {
              setCommentBtnPos({
                top: coords.top + (editorRect.top - containerRect.top) + coords.height + 4,
                left: editorRect.left - containerRect.left + 40,
              });
            }
          }
        } else {
          setSelectionRange(null);
          setCommentBtnPos(null);
        }
      });

      // Apply gutter decorations for existing code comments
      if (codeComments && codeComments.length > 0) {
        applyCommentDecorations(modifiedEditor, codeComments);
      }
    },
    [codeComments],
  );

  const handleCommentClick = useCallback(() => {
    if (!selectionRange || !editorRef.current || !onCommentRequest) return;

    const modifiedEditor = editorRef.current.getModifiedEditor();
    if (!modifiedEditor) return;

    const model = modifiedEditor.getModel();
    if (!model) return;

    // Extract the selected code text
    const { startLine, endLine } = selectionRange;
    const lines: string[] = [];
    for (let i = startLine; i <= endLine; i++) {
      lines.push(model.getLineContent(i));
    }
    const selectedCode = lines.join("\n");

    onCommentRequest(startLine, endLine, selectedCode);
    setSelectionRange(null);
    setCommentBtnPos(null);
  }, [selectionRange, onCommentRequest]);

  if (!fileDiff) {
    return (
      <div className="empty-state">Select a file to view its diff.</div>
    );
  }

  // Map our language strings to Monaco language IDs
  const monacoLang = mapLanguage(fileDiff.language);

  return (
    <div ref={containerRef} style={{ position: "relative", width: "100%", height: "100%" }}>
      <DiffEditor
        original={fileDiff.old_content || ""}
        modified={fileDiff.new_content || ""}
        language={monacoLang}
        theme="flowdiff-dark"
        options={{
          readOnly: true,
          renderSideBySide: true,
          enableSplitViewResizing: true,
          automaticLayout: true,
          scrollBeyondLastLine: false,
          minimap: { enabled: false },
          fontSize: 12,
          fontFamily:
            "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
          lineNumbers: "on",
          glyphMargin: true,
          folding: true,
          renderWhitespace: "selection",
          scrollbar: {
            verticalScrollbarSize: 8,
            horizontalScrollbarSize: 8,
          },
        }}
        beforeMount={(monaco) => {
          // Define custom dark theme matching the app's Catppuccin palette
          monaco.editor.defineTheme("flowdiff-dark", {
            base: "vs-dark",
            inherit: true,
            rules: [],
            colors: {
              "editor.background": "#1e1e2e",
              "editor.foreground": "#cdd6f4",
              "editorLineNumber.foreground": "#6c7086",
              "editorLineNumber.activeForeground": "#a6adc8",
              "editor.selectionBackground": "#45475a",
              "editor.inactiveSelectionBackground": "#31324480",
              "editorIndentGuide.background1": "#31324480",
              "editorIndentGuide.activeBackground1": "#45475a",
              "diffEditor.insertedTextBackground": "#a6e3a118",
              "diffEditor.removedTextBackground": "#f38ba818",
              "diffEditor.insertedLineBackground": "#a6e3a110",
              "diffEditor.removedLineBackground": "#f38ba810",
              "scrollbar.shadow": "#00000000",
              "scrollbarSlider.background": "#45475a80",
              "scrollbarSlider.hoverBackground": "#6c7086",
              "scrollbarSlider.activeBackground": "#a6adc8",
            },
          });
        }}
        onMount={handleEditorMount}
      />

      {/* Inline "Comment" button appearing below the selected lines */}
      {selectionRange && commentBtnPos && onCommentRequest && (
        <button
          className="inline-comment-btn"
          style={{ top: commentBtnPos.top, left: commentBtnPos.left }}
          onClick={handleCommentClick}
          title={`Comment on lines ${selectionRange.startLine}-${selectionRange.endLine}`}
        >
          &#128172; Comment
        </button>
      )}
    </div>
  );
}

/** Apply gutter decorations for code-level comments. */
function applyCommentDecorations(editor: any, comments: ReviewComment[]) {
  const decorations = comments
    .filter((c) => c.start_line != null && c.end_line != null)
    .map((c) => ({
      range: {
        startLineNumber: c.start_line!,
        startColumn: 1,
        endLineNumber: c.end_line!,
        endColumn: 1,
      },
      options: {
        isWholeLine: true,
        linesDecorationsClassName: "comment-gutter-marker",
        className: "comment-line-highlight",
        hoverMessage: { value: c.text },
      },
    }));

  if (decorations.length > 0) {
    editor.createDecorationsCollection(decorations);
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
    prisma: "graphql", // Closest Monaco match for Prisma
    plaintext: "plaintext",
  };
  return map[lang] || "plaintext";
}
