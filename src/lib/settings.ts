// Saving one card's settings without undoing another's. set_settings replaces the whole
// AppSettings, and each card edits from its own copy, which can be older than a change
// made elsewhere (the weekly report's hide setting, another card). So a card sends the
// newest settings it can see with only its own fields taken from its draft.
import type { AppSettings } from "./types";

/** The budget card's own field. Each account's budget is saved apart (set_account_settings). */
export const BUDGET_FIELDS = ["show_in_menu_bar"] as const;
export const LOCAL_MODEL_FIELDS = [
  "ollama_url", "ollama_model", "local_context_limit", "local_output_limit",
  "model_labels_enabled", "allow_remote_ollama",
] as const;

export function withOwnFields(
  latest: AppSettings | undefined,
  draft: AppSettings,
  fields: readonly (keyof AppSettings)[],
): AppSettings {
  if (!latest) return draft;
  const own = Object.fromEntries(fields.map((k) => [k, draft[k]])) as Partial<AppSettings>;
  return { ...latest, ...own };
}
