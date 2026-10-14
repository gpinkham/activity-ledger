import type { Group, LabelCoverage, LabelSource } from "./labelSource";

export interface Filters { from?: string; to?: string; project?: string }

export interface Totals {
  sessions: number; messages: number; input_tokens: number; output_tokens: number;
  cache_creation: number; cache_read: number; est_cost_usd: number; saved_usd: number; cache_hit_ratio: number;
}
export interface ByModel { model: string; messages: number; tokens: number; cost_usd: number }
export interface ByTool { tool: string; count: number; is_mcp: boolean }
export interface ByDay { day: string; messages: number; cost_usd: number }
export interface ByProject {
  project: string; sessions: number; tokens: number; cost_usd: number;
  tool_calls: number; edit_write: number; web: number;
}
export interface HeatCell { weekday: number; hour: number; count: number }
export interface ModelDayCount { day: string; model: string; messages: number }
export interface Workflow {
  bash: number; edit_write: number; user_modified: number; edit_write_total: number;
  web_search: number; web_fetch: number; heatmap: HeatCell[]; model_mix: ModelDayCount[];
}
export interface ModelPrice {
  input_per_m: number; output_per_m: number;
  cache_read_per_m?: number | null; fast_input_per_m?: number | null; fast_output_per_m?: number | null;
}
export type FastMode = "auto" | "never" | "always";
export interface PriceTable {
  models: Record<string, ModelPrice>; default: ModelPrice;
  us_only?: boolean; fast_mode?: FastMode; prices_as_of?: string | null;
}
export interface RefreshSummary { files_scanned: number; files_parsed: number }

export interface SessionSummary {
  session_id: string; project: string; title: string | null;
  started_at: string | null; ended_at: string | null; duration_secs: number;
  message_count: number; tokens: number; cost_usd: number; tool_calls: number; models: string[];
}
export interface SessionTokens { input: number; output: number; cache_creation: number; cache_read: number }
export interface SessionModel { model: string; messages: number; tokens: number; cost_usd: number }
export interface TimelineEvent {
  ts: string; kind: "message" | "tool"; label: string;
  model: string | null; output_tokens: number | null; is_mcp: boolean | null; count: number;
}
export interface SessionDetail {
  summary: SessionSummary; cache_hit_ratio: number;
  tokens: SessionTokens; by_model: SessionModel[]; events: TimelineEvent[];
}

export interface ActivityRow {
  /** "unlabelled" only in the LLM view, for segments the local model has not labelled. */
  activity: string; group: Group; segments: number; calls: number;
  input: number; output: number; cache_write: number; cache_read: number;
  cost_usd: number; share: number; ctx_p50: number; ctx_p95: number; out_p95: number;
  local_candidate: boolean; local_reasons: string[];
  /** The local model's labels for this row's segments, largest first (0.1.2). */
  model_labels?: { label: string; segments: number }[];
  /** In the LLM view, the rule labels of this row's segments, largest first. */
  rule_labels?: { label: string; segments: number }[];
}
export interface DayActivity { day: string; activity: string; cost_usd: number }
export interface ActivityReport {
  rows: ActivityRow[]; by_day: DayActivity[];
  totals: { cost_usd: number; overhead_usd: number; local_candidate_usd: number; calls: number; segments: number };
  reconcile: { sessions: number; ours_usd: number; claude_code_usd: number };
  unknown_models: string[];
  /** Which labels rows and days are grouped by. */
  label_source: LabelSource;
  /** How much the local model labelled; the same in both views. */
  coverage: LabelCoverage;
}
export interface AppSettings {
  ollama_url: string; ollama_model: string;
  local_context_limit: number; local_output_limit: number;
  model_labels_enabled: boolean; allow_remote_ollama: boolean;
}
export interface LabelProgress {
  running: boolean; loading: boolean; total: number; done: number; labelled: number; failed: number; stopped: string | null;
}
