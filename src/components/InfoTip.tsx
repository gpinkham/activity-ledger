export function InfoTip({ text, label = "More info" }: { text: string; label?: string }) {
  return (
    <span className="group relative inline-flex align-middle">
      <button
        type="button"
        aria-label={label}
        className="ml-1 h-3.5 w-3.5 grid place-items-center rounded-full border border-line text-[9px] leading-none text-muted hover:text-ink hover:border-accent cursor-help focus:outline-none focus:ring-1 focus:ring-accent"
      >
        i
      </button>
      <span
        role="tooltip"
        className="pointer-events-none absolute left-1/2 top-full z-40 mt-1.5 w-64 -translate-x-1/2 rounded-md border border-line bg-elevated p-2 text-[11px] font-normal normal-case leading-snug text-ink opacity-0 shadow-glow transition-opacity duration-150 group-hover:opacity-100 group-focus-within:opacity-100"
      >
        {text}
      </span>
    </span>
  );
}
