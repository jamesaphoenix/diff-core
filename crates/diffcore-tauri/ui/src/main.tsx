import React from "react";
import ReactDOM from "react-dom/client";
import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import App from "./App";
import { initBackend } from "./backend";
import "./styles.css";

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
