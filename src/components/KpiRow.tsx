import { InfoTip } from "./InfoTip";

export interface Kpi { label: string; value: string; hint?: string; tooltip?: string }

export function KpiRow({ items }: { items: Kpi[] }) {
  return (
    <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
      {items.map((k) => (
        <div
          key={k.label}
          className="relative rounded-xl border border-line bg-surface p-3 shadow-glow"
        >
          <div className="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-accent to-transparent opacity-70" />
          <div className="text-xs text-muted flex items-center">
            {k.label}
            {k.tooltip && <InfoTip text={k.tooltip} label={`About ${k.label}`} />}
          </div>
          <div className="text-xl font-semibold tabular-nums text-ink">{k.value}</div>
          {k.hint && <div className="text-[11px] text-muted opacity-70">{k.hint}</div>}
        </div>
      ))}
    </div>
  );
}
