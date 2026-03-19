import { DiffEditor } from "@monaco-editor/react";
import type { FileDiffContent } from "../types";

interface DiffViewerProps {
  fileDiff: FileDiffContent | null;
}

/** Monaco-based side-by-side diff viewer for the center panel. */
export default function DiffViewer({ fileDiff }: DiffViewerProps) {
  if (!fileDiff) {
    return (
      <div className="empty-state">Select a file to view its diff.</div>
    );
  }

  // Map our language strings to Monaco language IDs
  const monacoLang = mapLanguage(fileDiff.language);

  return (
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
        glyphMargin: false,
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
    />
  );
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
