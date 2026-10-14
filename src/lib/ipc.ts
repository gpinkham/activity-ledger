import { invoke } from "@tauri-apps/api/core";
import type {
  Filters, Totals, ByModel, ByTool, ByDay, ByProject, Workflow, PriceTable, RefreshSummary,
  SessionSummary, SessionDetail, ActivityReport, AppSettings, LabelProgress,
} from "./types";
import type { LabelSource } from "./labelSource";

export const refresh = () => invoke<RefreshSummary>("refresh");
export const getTotals = (filters: Filters) => invoke<Totals>("get_totals", { filters });
export const getByModel = (filters: Filters) => invoke<ByModel[]>("get_by_model", { filters });
export const getByTool = (filters: Filters) => invoke<ByTool[]>("get_by_tool", { filters });
export const getByDay = (filters: Filters) => invoke<ByDay[]>("get_by_day", { filters });
export const getByProject = (filters: Filters) => invoke<ByProject[]>("get_by_project", { filters });
export const getWorkflow = (filters: Filters) => invoke<Workflow>("get_workflow", { filters });
export const getPriceTable = () => invoke<PriceTable>("get_price_table");
export const setPriceTable = (table: PriceTable) => invoke<void>("set_price_table", { table });
export const exportBundle = (filters: Filters) => invoke<string>("export_bundle", { filters });
export const saveExport = (path: string, contents: string) => invoke<void>("save_export", { path, contents });

export const getSessions = (filters: Filters) => invoke<SessionSummary[]>("get_sessions", { filters });
export const getSessionDetail = (sessionId: string) => invoke<SessionDetail>("get_session_detail", { sessionId });

export const getActivity = (filters: Filters, labelSource: LabelSource) =>
  invoke<ActivityReport>("get_activity", { filters, labelSource });
export const getSettings = () => invoke<AppSettings>("get_settings");
export const setSettings = (settings: AppSettings) => invoke<void>("set_settings", { settings });
export const runModelLabels = (filters: Filters) => invoke<void>("run_model_labels", { filters });
export const getLabelProgress = () => invoke<LabelProgress>("get_label_progress");
export const stopModelLabels = () => invoke<void>("stop_model_labels");
