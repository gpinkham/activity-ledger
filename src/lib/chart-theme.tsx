import { useMemo } from "react";
import { useTheme } from "../state/theme";

const DARK = {
  grid: "#222634",
  muted: "#8b91a3",
  text: "#e6e8ee",
  surface: "#12141c",
  border: "#262a36",
  ramp: ["#2dd4bf", "#22d3ee", "#60a5fa", "#a78bfa", "#e879f9", "#fbbf24"],
};
const LIGHT = {
  grid: "#e5e7eb",
  muted: "#626878",
  text: "#14171f",
  surface: "#ffffff",
  border: "#e2e5ec",
  ramp: ["#0d9488", "#0891b2", "#2563eb", "#7c3aed", "#c026d3", "#d97706"],
};

export function useChartTokens() {
  const { theme } = useTheme();
  return useMemo(() => {
    const t = theme === "dark" ? DARK : LIGHT;
    return {
      ...t,
      tick: { fill: t.muted, fontSize: 11 },
      tickSmall: { fill: t.muted, fontSize: 10 },
      axisLine: { stroke: t.grid },
      tooltip: {
        contentStyle: {
          background: t.surface,
          border: `1px solid ${t.border}`,
          borderRadius: 8,
          color: t.text,
          fontSize: 12,
          boxShadow: "0 10px 30px -12px rgba(0,0,0,0.55)",
        } as React.CSSProperties,
        cursor: { fill: t.muted, fillOpacity: 0.08 },
        labelStyle: { color: t.text },
        itemStyle: { color: t.text },
      },
    };
  }, [theme]);
}

/** Vertical gradient def for area/bar fills: strong top → faded bottom. */
export function VGradient({ id, color, from = 0.85, to = 0.04 }: { id: string; color: string; from?: number; to?: number }) {
  return (
    <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stopColor={color} stopOpacity={from} />
      <stop offset="100%" stopColor={color} stopOpacity={to} />
    </linearGradient>
  );
}

/** Horizontal gradient def for horizontal bars: faded left → strong right. */
export function HGradient({ id, color, from = 0.25, to = 0.95 }: { id: string; color: string; from?: number; to?: number }) {
  return (
    <linearGradient id={id} x1="0" y1="0" x2="1" y2="0">
      <stop offset="0%" stopColor={color} stopOpacity={from} />
      <stop offset="100%" stopColor={color} stopOpacity={to} />
    </linearGradient>
  );
}
