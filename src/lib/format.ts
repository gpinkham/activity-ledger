export const usd = (n: number) =>
  n >= 1 ? `$${n.toFixed(2)}` : `$${n.toFixed(n < 0.01 ? 4 : 3)}`;

export const compact = (n: number) => {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return `${n}`;
};

export const pct = (n: number) => `${(n * 100).toFixed(1)}%`;
