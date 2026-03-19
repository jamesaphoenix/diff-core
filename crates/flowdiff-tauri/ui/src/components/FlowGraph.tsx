import { useMemo, useCallback } from "react";
import {
  ReactFlow,
  Background,
  MiniMap,
  type Node,
  type Edge,
  type NodeMouseHandler,
  Position,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import dagre from "@dagrejs/dagre";
import type { FlowEdge, FileChange, EdgeType } from "../types";

interface FlowGraphProps {
  edges: FlowEdge[];
  files: FileChange[];
  onNodeClick?: (filePath: string) => void;
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
  g.setGraph({ rankdir: "TB", ranksep: 60, nodesep: 40, marginx: 20, marginy: 20 });

  const nodeWidth = 200;
  const nodeHeight = 56;

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
  // Build a lookup of file path → FileChange
  const fileMap = new Map<string, FileChange>();
  for (const f of files) {
    fileMap.set(f.path, f);
  }

  // Collect unique file-level nodes from edges
  const nodeIds = new Set<string>();
  for (const e of flowEdges) {
    nodeIds.add(extractFilePath(e.from));
    nodeIds.add(extractFilePath(e.to));
  }
  // Also add files that have no edges but are in the group
  for (const f of files) {
    nodeIds.add(f.path);
  }

  const nodes: Node[] = Array.from(nodeIds).map((id) => {
    const file = fileMap.get(id);
    const parts = id.split("/");
    const label = parts.length <= 2 ? id : parts.slice(-2).join("/");
    return {
      id,
      type: "flowNode",
      data: {
        label,
        role: file?.role || "Utility",
        additions: file?.changes.additions ?? 0,
        deletions: file?.changes.deletions ?? 0,
        filePath: id,
      },
      position: { x: 0, y: 0 },
    };
  });

  // Deduplicate to file-level edges
  const deduped = deduplicateEdges(flowEdges);

  const edges: Edge[] = deduped.map((e, i) => {
    const primaryType = e.types[0];
    const color = EDGE_COLORS[primaryType] || "#6c7086";
    const label = e.types.map((t) => EDGE_LABELS[t]).join(", ");
    return {
      id: `e-${i}`,
      source: e.from,
      target: e.to,
      label,
      style: { stroke: color, strokeWidth: 2 },
      labelStyle: { fill: "#a6adc8", fontSize: 10, fontFamily: "'JetBrains Mono', monospace" },
      labelBgStyle: { fill: "#1e1e2e", fillOpacity: 0.85 },
      labelBgPadding: [4, 2] as [number, number],
      animated: primaryType === "Calls",
      type: "smoothstep",
    };
  });

  return layoutGraph(nodes, edges);
}

/** Custom node component for flow graph nodes. */
function FlowNode({ data }: { data: Record<string, unknown> }) {
  const role = data.role as string;
  const label = data.label as string;
  const additions = data.additions as number;
  const deletions = data.deletions as number;
  const roleColor = ROLE_COLORS[role] || "#6c7086";

  return (
    <div className="flow-node">
      <div className="flow-node-header">
        <span className="flow-node-role" style={{ color: roleColor }}>{role}</span>
        <span className="flow-node-changes">
          <span className="flow-node-add">+{additions}</span>
          <span className="flow-node-del">-{deletions}</span>
        </span>
      </div>
      <div className="flow-node-label">{label}</div>
    </div>
  );
}

const nodeTypes = { flowNode: FlowNode };

/** Max nodes before falling back to a simple list. */
const MAX_INTERACTIVE_NODES = 100;

export default function FlowGraph({ edges, files, onNodeClick }: FlowGraphProps) {
  const { nodes, edges: rfEdges } = useMemo(
    () => buildGraph(edges, files),
    [edges, files],
  );

  const handleNodeClick: NodeMouseHandler = useCallback(
    (_event, node) => {
      const filePath = node.data.filePath as string;
      if (filePath && onNodeClick) {
        onNodeClick(filePath);
      }
    },
    [onNodeClick],
  );

  // Fallback for very large graphs
  if (nodes.length > MAX_INTERACTIVE_NODES) {
    return (
      <div className="flow-graph-fallback">
        <p>Graph too large ({nodes.length} nodes). Showing edge list instead.</p>
      </div>
    );
  }

  if (nodes.length === 0) {
    return null;
  }

  return (
    <div className="flow-graph-container" data-testid="flow-graph">
      <ReactFlow
        nodes={nodes}
        edges={rfEdges}
        nodeTypes={nodeTypes}
        onNodeClick={handleNodeClick}
        fitView
        fitViewOptions={{ padding: 0.2 }}
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
        {nodes.length >= 6 && (
          <MiniMap
            nodeColor={(n) => ROLE_COLORS[(n.data as Record<string, unknown>).role as string] || "#6c7086"}
            maskColor="rgba(30, 30, 46, 0.7)"
            style={{ background: "#181825", border: "1px solid #45475a" }}
          />
        )}
      </ReactFlow>
    </div>
  );
}
