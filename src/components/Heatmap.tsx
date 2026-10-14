import { Fragment } from "react";
import type { HeatCell } from "../lib/types";

const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

// teal (low) → magenta (high), matching the neon data ramp.
const LOW = [45, 212, 191];
const HIGH = [232, 121, 249];
function rampColor(t: number): string {
  const c = LOW.map((lo, i) => Math.round(lo + (HIGH[i] - lo) * t));
  return `rgb(${c[0]}, ${c[1]}, ${c[2]})`;
}

export function Heatmap({ cells }: { cells: HeatCell[] }) {
  const max = cells.reduce((m, c) => Math.max(m, c.count), 0) || 1;
  const lookup = new Map(cells.map((c) => [`${c.weekday}-${c.hour}`, c.count]));
  return (
    <div className="text-[10px]">
      <div className="grid gap-px" style={{ gridTemplateColumns: `32px repeat(24, 1fr)` }}>
        <div />
        {Array.from({ length: 24 }, (_, h) => (
          <div key={h} className="text-center text-muted">{h % 3 === 0 ? h : ""}</div>
        ))}
        {DAYS.map((day, wd) => (
          <Fragment key={wd}>
            <div key={`l-${wd}`} className="pr-1 text-right text-muted leading-4">{day}</div>
            {Array.from({ length: 24 }, (_, h) => {
              const v = lookup.get(`${wd}-${h}`) ?? 0;
              const t = v === 0 ? 0 : v / max;
              const alpha = v === 0 ? 0 : 0.25 + 0.75 * t;
              return (
                <div
                  key={`${wd}-${h}`}
                  title={`${day} ${h}:00, ${v}`}
                  className="h-4 rounded-[2px] bg-elevated"
                  style={
                    v === 0
                      ? undefined
                      : { backgroundColor: rampColor(t), opacity: alpha, boxShadow: t > 0.6 ? `0 0 6px -1px ${rampColor(t)}` : undefined }
                  }
                />
              );
            })}
          </Fragment>
        ))}
      </div>
    </div>
  );
}
