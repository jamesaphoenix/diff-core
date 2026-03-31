import { useState, useMemo, useCallback, useRef, useEffect } from "react";
import {
  ReactFlow,
  Background,
  MiniMap,
  Controls,
  Handle,
  type Node,
  type Edge,
  type NodeMouseHandler,
  type EdgeProps,
  Position,
  getBezierPath,
  MarkerType,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import dagre from "@dagrejs/dagre";
import type { FlowEdge, FileChange, EdgeType } from "../types";

interface FlowGraphProps {
  edges: FlowEdge[];
  files: FileChange[];
  onNodeClick?: (filePath: string) => void;
  /** File path of the node to highlight during flow replay. */
  replayNodeId?: string | null;
}

/** Color per edge type — Catppuccin palette. */
const EDGE_COLORS: Record<EdgeType, string> = {
  Calls: "#89b4fa",     // blue
  Imports: "#cba6f7",   // lavender
  Writes: "#fab387",    // peach
  Reads: "#a6e3a1",     // green
  Extends: "#f9e2af",   // yellow
  Instantiates: "#89dceb", // sky
  Emits: "#f38ba8",     // red
  Handles: "#94e2d5",   // teal
};

const EDGE_LABELS: Record<EdgeType, string> = {
  Calls: "calls",
  Imports: "imports",
  Writes: "writes",
  Reads: "reads",
  Extends: "extends",
  Instantiates: "new",
  Emits: "emits",
  Handles: "handles",
};

/** Map FileRole → badge color. */
const ROLE_COLORS: Record<string, string> = {
  Entrypoint: "#89b4fa",
  Handler: "#89b4fa",
  Service: "#cba6f7",
  Repository: "#fab387",
  Model: "#a6e3a1",
  Utility: "#6c7086",
  Config: "#6c7086",
  Test: "#f9e2af",
  Infrastructure: "#6c7086",
};

/** Extract just the file path from a symbol string. */
function extractFilePath(symbol: string): string {
  return symbol.split("::")[0];
}

/** Shorten a file path to last 2 segments. */
function shortPath(path: string): string {
  const parts = path.split("/");
  if (parts.length <= 2) return path;
  return parts.slice(-2).join("/");
}

/** Deduplicate edges to file-level (multiple symbol edges between same files → one edge). */
function deduplicateEdges(
  edges: FlowEdge[],
): { from: string; to: string; types: EdgeType[] }[] {
  const map = new Map<string, EdgeType[]>();
  for (const e of edges) {
    const fromFile = extractFilePath(e.from);
    const toFile = extractFilePath(e.to);
    if (fromFile === toFile) continue; // skip self-edges
    const key = `${fromFile}→${toFile}`;
    const existing = map.get(key);
    if (existing) {
      if (!existing.includes(e.edge_type)) existing.push(e.edge_type);
    } else {
      map.set(key, [e.edge_type]);
    }
  }
  return Array.from(map.entries()).map(([key, types]) => {
    const [from, to] = key.split("→");
    return { from, to, types };
  });
}

/** Build dagre layout and return positioned nodes. */
function layoutGraph(
  nodes: Node[],
  edges: Edge[],
): { nodes: Node[]; edges: Edge[] } {
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  // Adaptive spacing — more room for small graphs so edges are clearly visible
  const isSmallGraph = nodes.length >= 2 && nodes.length <= 6;
  const ranksep = isSmallGraph ? 120 : 80;
  const nodesep = isSmallGraph ? 80 : 60;
  g.setGraph({ rankdir: "TB", ranksep, nodesep, marginx: 30, marginy: 30 });

  const nodeWidth = 220;
  const nodeHeight = 60;

  for (const node of nodes) {
    g.setNode(node.id, { width: nodeWidth, height: nodeHeight });
  }
  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  const positionedNodes = nodes.map((node) => {
    const pos = g.node(node.id);
    return {
      ...node,
      position: { x: pos.x - nodeWidth / 2, y: pos.y - nodeHeight / 2 },
      sourcePosition: Position.Bottom,
      targetPosition: Position.Top,
    };
  });

  return { nodes: positionedNodes, edges };
}

/** Build React Flow nodes and edges from FlowEdge[] + FileChange[]. */
function buildGraph(
  flowEdges: FlowEdge[],
  files: FileChange[],
): { nodes: Node[]; edges: Edge[] } {
  const fileMap = new Map<string, FileChange>();
  for (const f of files) {
    fileMap.set(f.path, f);
  }

  const nodeIds = new Set<string>();
  for (const e of flowEdges) {
    nodeIds.add(extractFilePath(e.from));
    nodeIds.add(extractFilePath(e.to));
  }
  for (const f of files) {
    nodeIds.add(f.path);
  }

  const nodes: Node[] = Array.from(nodeIds).map((id) => {
    const file = fileMap.get(id);
    const label = shortPath(id);
    return {
      id,
      type: "flowNode",
      data: {
        label,
        role: file?.role || "Utility",
        additions: file?.changes.additions ?? 0,
        deletions: file?.changes.deletions ?? 0,
        filePath: id,
        selected: false,
      },
      position: { x: 0, y: 0 },
    };
  });

  const deduped = deduplicateEdges(flowEdges);

  const edges: Edge[] = deduped.map((e, i) => {
    const primaryType = e.types[0];
    const color = EDGE_COLORS[primaryType] || "#6c7086";
    const label = e.types.map((t) => EDGE_LABELS[t]).join(", ");
    const hoverDetail = `${shortPath(e.from)} \u2192 ${shortPath(e.to)}`;

    return {
      id: `e-${i}`,
      source: e.from,
      target: e.to,
      type: "animatedBezier",
      data: {
        label,
        color,
        hoverDetail,
        dimmed: false,
      },
      style: { stroke: color, strokeWidth: 3 },
      markerEnd: {
        type: MarkerType.ArrowClosed,
        color,
        width: 24,
        height: 24,
      },
    };
  });

  return layoutGraph(nodes, edges);
}

/** (1) Custom animated bezier edge with flowing dot showing data direction. */
function AnimatedBezierEdge({
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  style,
  data,
  markerEnd,
}: EdgeProps) {
  const [hovered, setHovered] = useState(false);

  const [edgePath, labelX, labelY] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
  });

  const color = (data?.color as string) || (style?.stroke as string) || "#6c7086";
  const dimmed = data?.dimmed as boolean;
  const opacity = dimmed ? 0.15 : 1;
  const label = data?.label as string;
  const hoverDetail = data?.hoverDetail as string;

  return (
    <g
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      {/* Invisible wider hit area for hover detection */}
      <path d={edgePath} fill="none" stroke="transparent" strokeWidth={20} style={{ cursor: "pointer" }} />

      {/* Visible bezier edge */}
      <path
        d={edgePath}
        fill="none"
        stroke={color}
        strokeWidth={dimmed ? 1 : 3}
        opacity={opacity}
        markerEnd={markerEnd as string}
        className="react-flow__edge-path"
      />

      {/* Flowing dot animation along edge showing data direction */}
      {!dimmed && (
        <circle r={4} fill={color} opacity={0.9}>
          <animateMotion dur="2.5s" repeatCount="indefinite" path={edgePath} />
        </circle>
      )}

      {/* Edge label — pill badge always visible on edges */}
      {label && !dimmed && (
        <>
          <rect
            x={labelX - 32}
            y={labelY - 10}
            width={64}
            height={20}
            rx={10}
            fill={hovered ? "#313244" : "#1e1e2e"}
            fillOpacity={0.95}
            stroke={color}
            strokeWidth={hovered ? 1 : 0.5}
          />
          <text
            x={labelX}
            y={labelY + 4}
            textAnchor="middle"
            fill={hovered ? color : "#cdd6f4"}
            fontSize={11}
            fontWeight={500}
            fontFamily="'JetBrains Mono', monospace"
          >
            {label}
          </text>
        </>
      )}

      {/* (2) Hover tooltip with full from → to path detail */}
      {hovered && hoverDetail && !dimmed && (
        <>
          <rect
            x={labelX - 80}
            y={labelY + 14}
            width={160}
            height={18}
            rx={4}
            fill="#313244"
            fillOpacity={0.95}
            stroke={color}
            strokeWidth={0.5}
          />
          <text
            x={labelX}
            y={labelY + 27}
            textAnchor="middle"
            fill="#a6adc8"
            fontSize={9}
            fontFamily="'JetBrains Mono', monospace"
          >
            {hoverDetail}
          </text>
        </>
      )}
    </g>
  );
}

/** (3) Custom node component with hover tooltip. */
function FlowNode({ data }: { data: Record<string, unknown> }) {
  const role = data.role as string;
  const label = data.label as string;
  const filePath = data.filePath as string;
  const additions = data.additions as number;
  const deletions = data.deletions as number;
  const isSelected = data.selected as boolean;
  const isReplayActive = data.replayActive as boolean;
  const roleColor = ROLE_COLORS[role] || "#6c7086";

  const classNames = [
    "flow-node",
    isSelected ? "flow-node-selected" : "",
    isReplayActive ? "flow-node-replay" : "",
  ].filter(Boolean).join(" ");

  return (
    <div className={classNames}>
      <Handle type="target" position={Position.Top} className="flow-node-handle" />
      <div className="flow-node-header">
        <span className="flow-node-role" style={{ color: roleColor }}>{role}</span>
        <span className="flow-node-changes">
          <span className="flow-node-add">+{additions}</span>
          <span className="flow-node-del">-{deletions}</span>
        </span>
      </div>
      <div className="flow-node-label">{label}</div>
      {/* Hover tooltip with full file path and change summary */}
      <div className="flow-node-tooltip">
        <div className="flow-node-tooltip-path">{filePath}</div>
        <div className="flow-node-tooltip-stats">
          <span className="flow-node-add">+{additions}</span>{" "}
          <span className="flow-node-del">-{deletions}</span> lines changed
        </div>
      </div>
      <Handle type="source" position={Position.Bottom} className="flow-node-handle" />
    </div>
  );
}

const nodeTypes = { flowNode: FlowNode };
const edgeTypes = { animatedBezier: AnimatedBezierEdge };

/** Max nodes before falling back to a simple list. */
const MAX_INTERACTIVE_NODES = 100;

/** (8) Legend overlay showing edge type → color mapping. */
function Legend() {
  const [collapsed, setCollapsed] = useState(true);

  return (
    <div className="flow-legend">
      <button
        className="flow-legend-toggle"
        onClick={() => setCollapsed(!collapsed)}
        title="Toggle legend"
      >
        {collapsed ? "\u25C6 Legend" : "\u25BC Legend"}
      </button>
      {!collapsed && (
        <div className="flow-legend-items">
          {(Object.entries(EDGE_COLORS) as [EdgeType, string][]).map(([type, color]) => (
            <div key={type} className="flow-legend-item">
              <span className="flow-legend-line" style={{ backgroundColor: color }} />
              <span className="flow-legend-label">{EDGE_LABELS[type]}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default function FlowGraph({ edges, files, onNodeClick, replayNodeId }: FlowGraphProps) {
  const { nodes: initialNodes, edges: initialEdges } = useMemo(
    () => buildGraph(edges, files),
    [edges, files],
  );

  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const reactFlowInstance = useRef<any>(null);

  // The "active" node is either the replay node (if replaying) or the user-clicked node
  const activeNodeId = replayNodeId ?? selectedNodeId;

  // (6) Dim non-connected edges when a node is active
  const processedEdges = useMemo(() => {
    if (!activeNodeId) return initialEdges;
    return initialEdges.map((edge) => ({
      ...edge,
      data: {
        ...edge.data,
        dimmed: edge.source !== activeNodeId && edge.target !== activeNodeId,
      },
    }));
  }, [initialEdges, activeNodeId]);

  // Track active node for visual highlight + replay glow
  const processedNodes = useMemo(() => {
    return initialNodes.map((node) => ({
      ...node,
      data: {
        ...node.data,
        selected: node.id === activeNodeId,
        replayActive: replayNodeId != null && node.id === replayNodeId,
      },
    }));
  }, [initialNodes, activeNodeId, replayNodeId]);

  const handleNodeClick: NodeMouseHandler = useCallback(
    (_event, node) => {
      const filePath = node.data.filePath as string;
      setSelectedNodeId(node.id);
      // In fullscreen, close it first so the user sees the file in the diff viewer
      if (fullscreen) {
        setFullscreen(false);
      }
      if (filePath && onNodeClick) {
        onNodeClick(filePath);
      }
    },
    [onNodeClick, fullscreen],
  );

  const handlePaneClick = useCallback(() => {
    setSelectedNodeId(null);
  }, []);

  // Auto-fit view when the node set changes or fullscreen is toggled.
  // Small delay lets dagre layout / container resize settle before fitting.
  useEffect(() => {
    if (!reactFlowInstance.current) return;
    const timer = setTimeout(() => {
      reactFlowInstance.current?.fitView({ padding: 0.3, duration: 400 });
    }, 50);
    return () => clearTimeout(timer);
  }, [initialNodes, fullscreen]);

  // (4) Fullscreen: Escape key exits
  useEffect(() => {
    if (!fullscreen) return;
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") setFullscreen(false);
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [fullscreen]);

  // Fallback for very large graphs
  if (initialNodes.length > MAX_INTERACTIVE_NODES) {
    return (
      <div className="flow-graph-fallback">
        <p>Graph too large ({initialNodes.length} nodes). Showing edge list instead.</p>
      </div>
    );
  }

  if (initialNodes.length === 0) {
    return null;
  }

  return (
    <div
      ref={containerRef}
      className={`flow-graph-container ${fullscreen ? "flow-graph-fullscreen" : ""}`}
      data-testid="flow-graph"
    >
      {/* (4) Fullscreen toggle */}
      <button
        className="flow-fullscreen-btn"
        onClick={() => setFullscreen(!fullscreen)}
        title={fullscreen ? "Exit fullscreen (Esc)" : "Fullscreen"}
      >
        {fullscreen ? "\u2935" : "\u2922"}
      </button>

      <ReactFlow
        nodes={processedNodes}
        edges={processedEdges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onNodeClick={handleNodeClick}
        onPaneClick={handlePaneClick}
        onInit={(instance: any) => { reactFlowInstance.current = instance; }}
        fitView
        fitViewOptions={{ padding: 0.3, duration: 800 }}
        minZoom={0.3}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        nodesDraggable={true}
        nodesConnectable={false}
        elementsSelectable={true}
        panOnDrag={true}
        zoomOnScroll={true}
        colorMode="dark"
      >
        <Background color="#45475a" gap={16} size={1} />
        {initialNodes.length >= 15 && (
          <MiniMap
            nodeColor={(n) => ROLE_COLORS[(n.data as Record<string, unknown>).role as string] || "#6c7086"}
            maskColor="rgba(30, 30, 46, 0.7)"
            style={{ background: "#181825", border: "1px solid #45475a", width: 120, height: 80 }}
          />
        )}
        <Controls showInteractive={false} />
      </ReactFlow>

      {/* (8) Legend overlay */}
      <Legend />
    </div>
  );
}
