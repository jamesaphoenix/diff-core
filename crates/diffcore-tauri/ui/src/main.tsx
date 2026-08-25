import React from "react";
import ReactDOM from "react-dom/client";
import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import App from "./App";
import { initBackend } from "./backend";
import "./styles.css";
import { applyTheme, loadThemePrefs, resolveThemeId } from "./themes";

// Apply the persisted theme before first paint to avoid a flash of the default
applyTheme(resolveThemeId(loadThemePrefs(), window.matchMedia("(prefers-color-scheme: dark)").matches));

// Use locally bundled Monaco instead of CDN (required for Tauri production builds
// where CSP blocks external network requests)
loader.config({ monaco });

initBackend().then(() => {
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
});
