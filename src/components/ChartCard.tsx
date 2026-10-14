import type { ReactNode } from "react";

export function ChartCard({ title, subtitle, children }: { title: string; subtitle?: string; children: ReactNode }) {
  return (
    <section className="rounded-xl border border-line bg-surface p-4 shadow-glow">
      <div className="mb-3">
        <h2 className="display text-sm text-ink">{title}</h2>
        {subtitle && <p className="text-xs text-muted mt-0.5">{subtitle}</p>}
      </div>
      <div className="overflow-x-auto">{children}</div>
    </section>
  );
}
