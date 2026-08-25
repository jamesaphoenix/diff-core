/** Theme registry: single source of truth for app CSS variables and Monaco themes. */

export interface ThemeColors {
  bgPrimary: string;
  bgSecondary: string;
  bgDeep: string;
  bgSurface: string;
  bgHover: string;
  textPrimary: string;
  textSecondary: string;
  textMuted: string;
  border: string;
  accent: string;
  accentHover: string;
  accentAlt: string;
  info: string;
  warn: string;
  riskHigh: string;
  riskMedium: string;
  riskLow: string;
}

export interface Theme {
  id: string;
  label: string;
  scheme: "light" | "dark";
  colors: ThemeColors;
}

export const THEMES: Theme[] = [
  {
    id: "catppuccin-mocha",
    label: "Catppuccin Mocha",
    scheme: "dark",
    colors: {
      bgPrimary: "#1e1e2e", bgSecondary: "#181825", bgDeep: "#11111b",
      bgSurface: "#313244", bgHover: "#45475a",
      textPrimary: "#cdd6f4", textSecondary: "#a6adc8", textMuted: "#6c7086",
      border: "#45475a",
      accent: "#89b4fa", accentHover: "#74c7ec", accentAlt: "#cba6f7",
      info: "#89dceb", warn: "#f9e2af",
      riskHigh: "#f38ba8", riskMedium: "#fab387", riskLow: "#a6e3a1",
    },
  },
  {
    id: "dracula",
    label: "Dracula",
    scheme: "dark",
    colors: {
      bgPrimary: "#282a36", bgSecondary: "#21222c", bgDeep: "#191a21",
      bgSurface: "#343746", bgHover: "#44475a",
      textPrimary: "#f8f8f2", textSecondary: "#bfc7d5", textMuted: "#6272a4",
      border: "#44475a",
      accent: "#bd93f9", accentHover: "#d6acff", accentAlt: "#ff79c6",
      info: "#8be9fd", warn: "#f1fa8c",
      riskHigh: "#ff5555", riskMedium: "#ffb86c", riskLow: "#50fa7b",
    },
  },
  {
    id: "nord",
    label: "Nord",
    scheme: "dark",
    colors: {
      bgPrimary: "#2e3440", bgSecondary: "#292e39", bgDeep: "#242933",
      bgSurface: "#3b4252", bgHover: "#434c5e",
      textPrimary: "#eceff4", textSecondary: "#d8dee9", textMuted: "#616e88",
      border: "#4c566a",
      accent: "#88c0d0", accentHover: "#8fbcbb", accentAlt: "#b48ead",
      info: "#81a1c1", warn: "#ebcb8b",
      riskHigh: "#bf616a", riskMedium: "#d08770", riskLow: "#a3be8c",
    },
  },
  {
    id: "gruvbox-dark",
    label: "Gruvbox Dark",
    scheme: "dark",
    colors: {
      bgPrimary: "#282828", bgSecondary: "#1d2021", bgDeep: "#141617",
      bgSurface: "#3c3836", bgHover: "#504945",
      textPrimary: "#ebdbb2", textSecondary: "#d5c4a1", textMuted: "#928374",
      border: "#504945",
      accent: "#83a598", accentHover: "#8ec07c", accentAlt: "#d3869b",
      info: "#83a598", warn: "#fabd2f",
      riskHigh: "#fb4934", riskMedium: "#fe8019", riskLow: "#b8bb26",
    },
  },
  {
    id: "tokyo-night",
    label: "Tokyo Night",
    scheme: "dark",
    colors: {
      bgPrimary: "#1a1b26", bgSecondary: "#16161e", bgDeep: "#13131a",
      bgSurface: "#24283b", bgHover: "#414868",
      textPrimary: "#c0caf5", textSecondary: "#a9b1d6", textMuted: "#565f89",
      border: "#414868",
      accent: "#7aa2f7", accentHover: "#7dcfff", accentAlt: "#bb9af7",
      info: "#7dcfff", warn: "#e0af68",
      riskHigh: "#f7768e", riskMedium: "#ff9e64", riskLow: "#9ece6a",
    },
  },
  {
    id: "one-dark",
    label: "One Dark",
    scheme: "dark",
    colors: {
      bgPrimary: "#282c34", bgSecondary: "#21252b", bgDeep: "#181a1f",
      bgSurface: "#2c313a", bgHover: "#3e4451",
      textPrimary: "#abb2bf", textSecondary: "#9da5b4", textMuted: "#5c6370",
      border: "#3e4451",
      accent: "#61afef", accentHover: "#56b6c2", accentAlt: "#c678dd",
      info: "#56b6c2", warn: "#e5c07b",
      riskHigh: "#e06c75", riskMedium: "#d19a66", riskLow: "#98c379",
    },
  },
  {
    id: "solarized-dark",
    label: "Solarized Dark",
    scheme: "dark",
    colors: {
      bgPrimary: "#002b36", bgSecondary: "#00212b", bgDeep: "#001b23",
      bgSurface: "#073642", bgHover: "#0e4b5c",
      textPrimary: "#93a1a1", textSecondary: "#839496", textMuted: "#586e75",
      border: "#0e4b5c",
      accent: "#268bd2", accentHover: "#2aa198", accentAlt: "#6c71c4",
      info: "#2aa198", warn: "#b58900",
      riskHigh: "#dc322f", riskMedium: "#cb4b16", riskLow: "#859900",
    },
  },
  {
    id: "catppuccin-latte",
    label: "Catppuccin Latte",
    scheme: "light",
    colors: {
      bgPrimary: "#eff1f5", bgSecondary: "#e6e9ef", bgDeep: "#dce0e8",
      bgSurface: "#ccd0da", bgHover: "#bcc0cc",
      textPrimary: "#4c4f69", textSecondary: "#5c5f77", textMuted: "#8c8fa1",
      border: "#bcc0cc",
      accent: "#1e66f5", accentHover: "#209fb5", accentAlt: "#8839ef",
      info: "#04a5e5", warn: "#df8e1d",
      riskHigh: "#d20f39", riskMedium: "#fe640b", riskLow: "#40a02b",
    },
  },
  {
    id: "solarized-light",
    label: "Solarized Light",
    scheme: "light",
    colors: {
      bgPrimary: "#fdf6e3", bgSecondary: "#f6efdc", bgDeep: "#eee8d5",
      bgSurface: "#eee8d5", bgHover: "#e6dfc8",
      textPrimary: "#586e75", textSecondary: "#657b83", textMuted: "#93a1a1",
      border: "#d9d2ba",
      accent: "#268bd2", accentHover: "#2aa198", accentAlt: "#6c71c4",
      info: "#2aa198", warn: "#b58900",
      riskHigh: "#dc322f", riskMedium: "#cb4b16", riskLow: "#859900",
    },
  },
  {
    id: "github-light",
    label: "GitHub Light",
    scheme: "light",
    colors: {
      bgPrimary: "#ffffff", bgSecondary: "#f6f8fa", bgDeep: "#eaeef2",
      bgSurface: "#f6f8fa", bgHover: "#eaeef2",
      textPrimary: "#1f2328", textSecondary: "#424a53", textMuted: "#6e7781",
      border: "#d0d7de",
      accent: "#0969da", accentHover: "#0550ae", accentAlt: "#8250df",
      info: "#218bff", warn: "#9a6700",
      riskHigh: "#cf222e", riskMedium: "#bc4c00", riskLow: "#1a7f37",
    },
  },
];

export type ThemeMode = "light" | "dark" | "system";

export interface ThemePrefs {
  mode: ThemeMode;
  light: string;
  dark: string;
}

const DEFAULT_PREFS: ThemePrefs = {
  mode: "dark",
  light: "catppuccin-latte",
  dark: "catppuccin-mocha",
};

const STORAGE_KEY = "diffcore.theme";

export function loadThemePrefs(): ThemePrefs {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_PREFS;
    const parsed = JSON.parse(raw);
    const valid = (id: unknown) => THEMES.some((t) => t.id === id);
    return {
      mode: ["light", "dark", "system"].includes(parsed.mode) ? parsed.mode : DEFAULT_PREFS.mode,
      light: valid(parsed.light) ? parsed.light : DEFAULT_PREFS.light,
      dark: valid(parsed.dark) ? parsed.dark : DEFAULT_PREFS.dark,
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function saveThemePrefs(prefs: ThemePrefs) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // Non-fatal: theme just won't persist
  }
}

export function resolveThemeId(prefs: ThemePrefs, systemDark: boolean): string {
  const dark = prefs.mode === "dark" || (prefs.mode === "system" && systemDark);
  return dark ? prefs.dark : prefs.light;
}

export function getTheme(id: string): Theme {
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}

function kebab(key: string): string {
  return key.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
}

function hexToRgbTriplet(hex: string): string {
  if (!/^#[0-9a-f]{6}$/i.test(hex)) {
    throw new Error(`Theme colors must be 6-digit hex, got "${hex}"`);
  }
  const n = parseInt(hex.slice(1), 16);
  return `${(n >> 16) & 0xff}, ${(n >> 8) & 0xff}, ${n & 0xff}`;
}

/** Set all theme CSS variables (`--x` and `--x-rgb`) on the document root. */
export function applyTheme(id: string) {
  const theme = getTheme(id);
  const style = document.documentElement.style;
  for (const [key, hex] of Object.entries(theme.colors)) {
    style.setProperty(`--${kebab(key)}`, hex);
    style.setProperty(`--${kebab(key)}-rgb`, hexToRgbTriplet(hex));
  }
  style.setProperty("color-scheme", theme.scheme);
}

export function monacoThemeId(id: string): string {
  return `diffcore-${id}`;
}

/** Register one Monaco theme per palette. */
export function defineMonacoThemes(monaco: typeof import("monaco-editor")) {
  for (const t of THEMES) {
    const c = t.colors;
    monaco.editor.defineTheme(monacoThemeId(t.id), {
      base: t.scheme === "dark" ? "vs-dark" : "vs",
      inherit: true,
      rules: [],
      colors: {
        "editor.background": c.bgPrimary,
        "editor.foreground": c.textPrimary,
        "editorLineNumber.foreground": c.textMuted,
        "editorLineNumber.activeForeground": c.textSecondary,
        "editor.selectionBackground": c.bgHover,
        "editor.inactiveSelectionBackground": `${c.bgSurface}80`,
        "editorIndentGuide.background1": `${c.bgSurface}80`,
        "editorIndentGuide.activeBackground1": c.bgHover,
        "editorGutter.background": c.bgPrimary,
        "editorWidget.background": c.bgSecondary,
        "editorWidget.border": c.bgHover,
        "editorStickyScroll.background": c.bgSecondary,
        "editorStickyScrollHover.background": c.bgSurface,
        "editorOverviewRuler.border": "#00000000",
        "diffEditor.insertedTextBackground": `${c.riskLow}18`,
        "diffEditor.removedTextBackground": `${c.riskHigh}18`,
        "diffEditor.insertedLineBackground": `${c.riskLow}10`,
        "diffEditor.removedLineBackground": `${c.riskHigh}10`,
        "scrollbar.shadow": "#00000000",
        "scrollbarSlider.background": `${c.bgHover}80`,
        "scrollbarSlider.hoverBackground": c.textMuted,
        "scrollbarSlider.activeBackground": c.textSecondary,
      },
    });
  }
}
