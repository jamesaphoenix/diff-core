import { useEffect, useRef, useId } from "react";
import mermaid from "mermaid";

interface MermaidGraphProps {
  code: string;
}

// Initialize mermaid once with dark theme
mermaid.initialize({
  startOnLoad: false,
  theme: "dark",
  themeVariables: {
    darkMode: true,
    background: "#1e1e2e",
    primaryColor: "#89b4fa",
    primaryTextColor: "#cdd6f4",
    primaryBorderColor: "#45475a",
    lineColor: "#a6adc8",
    secondaryColor: "#313244",
    tertiaryColor: "#45475a",
    fontFamily: "'JetBrains Mono', 'Fira Code', monospace",
    fontSize: "12px",
  },
});

/** Renders a Mermaid diagram from a code string. */
export default function MermaidGraph({ code }: MermaidGraphProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const uniqueId = useId().replace(/:/g, "-");

  useEffect(() => {
    if (!code || !containerRef.current) return;

    let cancelled = false;

    async function render() {
      try {
        const { svg } = await mermaid.render(
          `mermaid-${uniqueId}`,
          code,
        );
        if (!cancelled && containerRef.current) {
          containerRef.current.innerHTML = svg;
        }
      } catch {
        if (!cancelled && containerRef.current) {
          containerRef.current.innerHTML =
            '<p class="mermaid-error">Failed to render diagram</p>';
        }
      }
    }

    render();

    return () => {
      cancelled = true;
    };
  }, [code, uniqueId]);

  if (!code) {
    return null;
  }

  return <div ref={containerRef} className="mermaid-container" />;
}
