import { useMemo, useState } from "react";
import type { FlowGroup } from "../types";

interface RiskHeatmapProps {
  groups: FlowGroup[];
  selectedGroupId: string | null;
  onSelectGroup: (group: FlowGroup) => void;
}

interface TreemapRect {
  group: FlowGroup;
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Total line changes for a group — used as treemap weight. */
function groupWeight(group: FlowGroup): number {
  let total = 0;
  for (const f of group.files) {
    total += f.changes.additions + f.changes.deletions;
  }
  return Math.max(total, 1); // Minimum weight of 1 so empty groups still appear
}

/**
 * Interpolate risk score [0, 1] to an RGB color string.
 * Uses Catppuccin palette: green (#a6e3a1) → peach (#fab387) → red (#f38ba8).
 */
function riskColor(score: number): string {
  const clamped = Math.max(0, Math.min(1, score));
  let r: number, g: number, b: number;
  if (clamped <= 0.5) {
    const t = clamped / 0.5;
    r = Math.round(166 + (250 - 166) * t);
    g = Math.round(227 + (179 - 227) * t);
    b = Math.round(161 + (135 - 161) * t);
  } else {
    const t = (clamped - 0.5) / 0.5;
    r = Math.round(250 + (243 - 250) * t);
    g = Math.round(179 + (139 - 179) * t);
    b = Math.round(135 + (168 - 135) * t);
  }
  return `rgb(${r}, ${g}, ${b})`;
}

/**
 * Squarified treemap layout.
 * Places items as close to square as possible within the given rectangle.
 */
function squarify(
  items: { group: FlowGroup; weight: number }[],
  x: number,
  y: number,
  w: number,
  h: number,
): TreemapRect[] {
  if (items.length === 0 || w <= 0 || h <= 0) return [];
  if (items.length === 1) {
    return [{ group: items[0].group, x, y, w, h }];
  }

  const total = items.reduce((s, g) => s + g.weight, 0);
  if (total === 0) return [];

  const sorted = [...items].sort((a, b) => b.weight - a.weight);
  const isWide = w >= h;

  // Find best strip split: greedily add items while aspect ratio improves
  let bestWorstRatio = Infinity;
  let bestSplit = 1;

  for (let i = 1; i <= sorted.length; i++) {
    const stripWeight = sorted.slice(0, i).reduce((s, g) => s + g.weight, 0);
    const fraction = stripWeight / total;
    const stripLength = isWide ? w * fraction : h * fraction;
    const crossLength = isWide ? h : w;

    let worstRatio = 0;
    for (let j = 0; j < i; j++) {
      const cellFraction = sorted[j].weight / stripWeight;
      const cellLength = crossLength * cellFraction;
      if (cellLength > 0 && stripLength > 0) {
        const ratio = Math.max(stripLength / cellLength, cellLength / stripLength);
        worstRatio = Math.max(worstRatio, ratio);
      }
    }

    if (worstRatio <= bestWorstRatio) {
      bestWorstRatio = worstRatio;
      bestSplit = i;
    } else {
      break;
    }
  }

  const topItems = sorted.slice(0, bestSplit);
  const restItems = sorted.slice(bestSplit);
  const topWeight = topItems.reduce((s, g) => s + g.weight, 0);
  const fraction = topWeight / total;

  const rects: TreemapRect[] = [];

  if (isWide) {
    const stripW = w * fraction;
    let cy = y;
    for (const item of topItems) {
      const cellH = h * (item.weight / topWeight);
      rects.push({ group: item.group, x, y: cy, w: stripW, h: cellH });
      cy += cellH;
    }
    if (restItems.length > 0) {
      rects.push(...squarify(restItems, x + stripW, y, w - stripW, h));
    }
  } else {
    const stripH = h * fraction;
    let cx = x;
    for (const item of topItems) {
      const cellW = w * (item.weight / topWeight);
      rects.push({ group: item.group, x: cx, y, w: cellW, h: stripH });
      cx += cellW;
    }
    if (restItems.length > 0) {
      rects.push(...squarify(restItems, x, y + stripH, w, h - stripH));
    }
  }

  return rects;
}

/** Short name for display — truncate long group names. */
function truncateName(name: string, maxLen: number): string {
  if (name.length <= maxLen) return name;
  return name.slice(0, maxLen - 1) + "\u2026";
}

const CONTAINER_WIDTH = 268; // fits within 300px right panel with padding
const CONTAINER_HEIGHT = 160;

export default function RiskHeatmap({ groups, selectedGroupId, onSelectGroup }: RiskHeatmapProps) {
  const [hoveredId, setHoveredId] = useState<string | null>(null);

  const rects = useMemo(() => {
    const items = groups.map((group) => ({
      group,
      weight: groupWeight(group),
    }));
    return squarify(items, 0, 0, CONTAINER_WIDTH, CONTAINER_HEIGHT);
  }, [groups]);

  if (groups.length === 0) return null;

  return (
    <div className="heatmap-container">
      <svg
        width={CONTAINER_WIDTH}
        height={CONTAINER_HEIGHT}
        viewBox={`0 0 ${CONTAINER_WIDTH} ${CONTAINER_HEIGHT}`}
        className="heatmap-svg"
      >
        {rects.map((rect) => {
          const isSelected = rect.group.id === selectedGroupId;
          const isHovered = rect.group.id === hoveredId;
          const color = riskColor(rect.group.risk_score);
          const totalChanges = groupWeight(rect.group);
          // Only show label if cell is big enough
          const showLabel = rect.w > 50 && rect.h > 28;
          const showScore = rect.w > 36 && rect.h > 16;

          return (
            <g
              key={rect.group.id}
              className="heatmap-cell-group"
              onClick={() => onSelectGroup(rect.group)}
              onMouseEnter={() => setHoveredId(rect.group.id)}
              onMouseLeave={() => setHoveredId(null)}
              style={{ cursor: "pointer" }}
            >
              {/* Cell background */}
              <rect
                x={rect.x + 1}
                y={rect.y + 1}
                width={Math.max(rect.w - 2, 0)}
                height={Math.max(rect.h - 2, 0)}
                rx={3}
                fill={color}
                fillOpacity={isSelected ? 0.45 : isHovered ? 0.35 : 0.25}
                stroke={isSelected ? "#cdd6f4" : isHovered ? color : "transparent"}
                strokeWidth={isSelected ? 2 : 1.5}
              />
              {/* Group name */}
              {showLabel && (
                <text
                  x={rect.x + rect.w / 2}
                  y={rect.y + rect.h / 2 - 5}
                  textAnchor="middle"
                  dominantBaseline="middle"
                  className="heatmap-label"
                  fill="#cdd6f4"
                >
                  {truncateName(rect.group.name, Math.floor(rect.w / 6))}
                </text>
              )}
              {/* Risk score */}
              {showScore && (
                <text
                  x={rect.x + rect.w / 2}
                  y={rect.y + rect.h / 2 + (showLabel ? 9 : 0)}
                  textAnchor="middle"
                  dominantBaseline="middle"
                  className="heatmap-score"
                  fill={color}
                >
                  {rect.group.risk_score.toFixed(2)}
                </text>
              )}
              {/* Tooltip on hover */}
              {isHovered && (
                <title>
                  {rect.group.name}
{`Risk: ${rect.group.risk_score.toFixed(2)} | Files: ${rect.group.files.length} | Changes: +${totalChanges}`}
                </title>
              )}
            </g>
          );
        })}
      </svg>
    </div>
  );
}
