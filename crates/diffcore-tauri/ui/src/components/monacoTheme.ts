export const DIFFCORE_MONACO_THEME = "diffcore-dark";

export function registerDiffcoreMonacoTheme(monaco: any) {
  monaco.editor.defineTheme(DIFFCORE_MONACO_THEME, {
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
      "editorGutter.background": "#1e1e2e",
      "editorWidget.background": "#181825",
      "editorWidget.border": "#45475a",
      "editorStickyScroll.background": "#181825",
      "editorStickyScrollHover.background": "#313244",
      "editorOverviewRuler.border": "#00000000",
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
}
