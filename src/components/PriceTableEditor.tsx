import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { getPriceTable, setPriceTable } from "../lib/ipc";
import type { PriceTable } from "../lib/types";

export function PriceTableEditor() {
  const qc = useQueryClient();
  const table = useQuery({ queryKey: ["priceTable"], queryFn: getPriceTable });
  const save = useMutation({
    mutationFn: (t: PriceTable) => setPriceTable(t),
    onSuccess: () => qc.invalidateQueries(),
  });

  if (!table.data) return null;
  const models = Object.entries(table.data.models);

  const update = (model: string, field: "input_per_m" | "output_per_m", value: number) => {
    const next: PriceTable = {
      ...table.data!,
      models: { ...table.data!.models, [model]: { ...table.data!.models[model], [field]: value } },
    };
    save.mutate(next);
  };

  return (
    <table className="text-sm w-full">
      <thead>
        <tr className="text-left text-muted">
          <th className="py-1">Model</th><th>Input $/1M</th><th>Output $/1M</th>
        </tr>
      </thead>
      <tbody>
        {models.map(([model, price]) => (
          <tr key={model} className="border-t border-line/60">
            <td className="py-1 pr-4 font-mono text-xs text-ink">
              {model}
              {price.note && <p className="font-sans text-muted max-w-md">{price.note}</p>}
            </td>
            <td>
              <input type="number" step="0.01" defaultValue={price.input_per_m}
                onBlur={(e) => update(model, "input_per_m", Number(e.target.value))}
                className="w-24 border border-line rounded-md px-1.5 py-0.5 bg-surface text-ink" />
            </td>
            <td>
              <input type="number" step="0.01" defaultValue={price.output_per_m}
                onBlur={(e) => update(model, "output_per_m", Number(e.target.value))}
                className="w-24 border border-line rounded-md px-1.5 py-0.5 bg-surface text-ink" />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
