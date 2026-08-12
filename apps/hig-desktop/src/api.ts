import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  AppSnapshot,
  ArchiveInspection,
  CacheReport,
  DaemonStatus,
  DesktopPackRequest,
  BenchmarkRequest,
  ProjectStatus,
  Settings,
  TaskStatus,
} from "./types";

export const api = {
  bootstrap: () => invoke<AppSnapshot>("bootstrap_app"),
  settings: () => invoke<Settings>("get_settings"),
  updateSettings: (settings: Settings) => invoke<Settings>("update_settings", { settings }),
  initializeProject: (directory: string, cacheDir?: string, excludes: string[] = []) =>
    invoke<ProjectStatus>("initialize_project", { directory, cacheDir: cacheDir ?? null, excludes }),
  projectStatus: (directory: string) => invoke<ProjectStatus>("get_project_status", { directory }),
  rebuildProject: (directory: string) => invoke<TaskStatus>("submit_project_rebuild", { directory }),
  startPack: (request: DesktopPackRequest) => invoke<TaskStatus>("start_pack", { request }),
  startUnpack: (request: { archiveFile: string; outputDir: string; password: string | null; overwrite: boolean }) => invoke<TaskStatus>("start_unpack", { request }),
  inspectArchive: (path: string, password?: string) =>
    invoke<ArchiveInspection>("inspect_archive", { path, password: password || null }),
  tasks: () => invoke<TaskStatus[]>("list_daemon_tasks"),
  task: (taskId: string, cacheDir?: string) => invoke<TaskStatus>("get_task_status", { taskId, cacheDir: cacheDir || null }),
  taskResult: (taskId: string, cacheDir: string) => invoke("get_task_result", { taskId, cacheDir }),
  cancelTask: (taskId: string, cacheDir?: string) => invoke<TaskStatus>("cancel_task", { taskId, cacheDir: cacheDir || null }),
  clearTaskHistory: () => invoke<boolean>("clear_local_task_history"),
  sessionStatus: (cacheDir?: string) => invoke("get_session_status", { cacheDir: cacheDir || null }),
  unlockSession: (password: string, ttlSecs: number, cacheDir?: string) =>
    invoke("unlock_session", { cacheDir: cacheDir || null, password, ttlSecs }),
  clearSession: (cacheDir?: string) => invoke<boolean>("clear_session", { cacheDir: cacheDir || null }),
  cacheStatus: (cacheDir?: string) => invoke<CacheReport>("get_cache_status", { cacheDir: cacheDir || null }),
  previewCacheGc: (cacheDir?: string) => invoke<CacheReport>("preview_cache_gc", { cacheDir: cacheDir || null }),
  submitCacheGc: (cacheDir?: string) => invoke<TaskStatus>("submit_cache_gc", { cacheDir: cacheDir || null }),
  previewCacheCompact: (cacheDir?: string) => invoke<CacheReport>("preview_cache_compact", { cacheDir: cacheDir || null }),
  submitCacheCompact: (cacheDir?: string) => invoke<TaskStatus>("submit_cache_compact", { cacheDir: cacheDir || null }),
  daemonStatus: (cacheDir?: string) => invoke<DaemonStatus>("get_daemon_status", { cacheDir: cacheDir || null }),
  startDaemon: (cacheDir?: string, ttlSecs?: number) => invoke<DaemonStatus>("start_daemon", { cacheDir: cacheDir || null, ttlSecs: ttlSecs || null }),
  restartDaemon: (cacheDir?: string, ttlSecs?: number, force = false) => invoke<DaemonStatus>("restart_daemon", { cacheDir: cacheDir || null, ttlSecs: ttlSecs || null, force }),
  stopDaemon: (cacheDir?: string, force = false) => invoke<boolean>("stop_desktop_daemon", { cacheDir: cacheDir || null, force }),
  startBenchmark: (request: BenchmarkRequest) => invoke<TaskStatus>("start_benchmark", { request }),
  benchmarkStatus: (taskId: string) => invoke<TaskStatus>("get_benchmark_status", { taskId }),
  cancelBenchmark: (taskId: string) => invoke<TaskStatus>("cancel_benchmark", { taskId }),
  chooseDirectory: async () => {
    const path = await open({ directory: true, multiple: false });
    return typeof path === "string" ? path : null;
  },
  chooseArchive: async () => {
    const path = await open({ multiple: false, filters: [{ name: "Hig Archive", extensions: ["hig"] }] });
    return typeof path === "string" ? path : null;
  },
  chooseArchiveOutput: async () =>
    save({ filters: [{ name: "Hig Archive", extensions: ["hig"] }], defaultPath: "archive.hig" }),
};

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) return String(error.message);
  return "The operation could not be completed.";
}
