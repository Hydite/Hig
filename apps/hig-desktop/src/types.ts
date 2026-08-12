export type NavId = "projects" | "create" | "open" | "tasks" | "cache" | "runtime" | "diagnostics" | "settings";

export interface AppError {
  code: string;
  message: string;
  recoverable: boolean;
}

export interface Settings {
  defaultOutputDir: string | null;
  defaultSpeed: "balanced" | "fastest";
  defaultEncryption: "password" | "none";
  sessionTtlSecs: number;
  recentProjects: string[];
  language: "system" | "en" | "zh-CN";
  knownCacheDirs: string[];
}

export interface AppSnapshot {
  version: string;
  platform: string;
  settings: Settings;
  daemonActive: boolean;
  sessionActive: boolean;
}

export interface ProjectStatus {
  initialized: boolean;
  projectId: number[] | null;
  root: string;
  cacheDir: string;
  snapshotValidity: "Building" | "Ready" | "Dirty" | "Invalid";
  generation: number;
  eventSequence: number;
  files: number;
  pendingEvents: number;
  dirtyFiles: number;
  dirtyGroups: number;
  watcherBackend: string;
  watcherOverflowCount: number;
  preparedBytes: number;
  lastEventAgeMs: number;
}

export interface TaskStatus {
  id: string;
  kind: "Pack" | "Unpack" | "Inspect" | "ProjectRebuild" | "CacheMaintenance" | "Benchmark";
  phase: string;
  filesDone: number;
  filesTotal: number | null;
  bytesDone: number;
  bytesTotal: number | null;
  elapsedUs: number;
  message: string | null;
  outputPath: string | null;
  archiveBytes: number | null;
  inputBytes: number | null;
  error: AppError | null;
  cancellable: boolean;
  cacheDir: string;
  disconnected: boolean;
  resultExpired: boolean;
}

export interface BatchOptions {
  enabled: boolean;
  smallFileThreshold: number;
  maxBatchRawBytes: number;
}

export interface ChunkOptions {
  enabled: boolean;
  chunkFileThreshold: number;
  chunkSize: number;
}

export interface DesktopPackRequest {
  inputDir: string;
  outputFile: string;
  password: string | null;
  useSession: boolean;
  encryption: "Password" | "None";
  speed: "Balanced" | "Fastest";
  cacheDir: string | null;
  threads: number | null;
  level: number | null;
  useCache: boolean;
  format: "HigV1" | "HigV2";
  manifestFormat: "Compact" | "Legacy";
  batch: BatchOptions;
  chunk: ChunkOptions;
  kdfProfile: "Secure" | "Interactive" | null;
  trustMetadata: boolean;
  projectMode: "Auto" | "Off" | "Required";
  solid: "Auto" | "Off";
}

export interface DaemonStatus {
  active: boolean;
  uptimeSecs: number;
  ttlSecs: number;
  activeJobs: number;
  queuedJobs: number;
  jobsCompleted: number;
  cacheDir: string;
  journalBytes: number;
  sessionActive: boolean;
  sessionAgeSecs: number;
  watchedProjects: number;
  projectReadyCount: number;
  projectPendingEvents: number;
}

export interface BenchmarkRequest {
  inputDir: string;
  suite: "source" | "lobehub" | "lobehub-watch" | "small500" | "textmix" | "repeat4m" | "random8m" | "binarymix" | "all";
  cacheDir: string | null;
  benchDir: string | null;
  workers: number | null;
  compare: boolean;
  password: string;
}

export interface ArchiveFile {
  relativePath: string;
  size: number;
  modifiedUnixNs: number;
  permissions: number;
  contentHash: number[];
}

export interface ArchiveInspection {
  format: "HigV1" | "HigV2";
  encrypted: boolean;
  files: ArchiveFile[];
  inputBytes: number;
  archiveBytes: number;
}

export interface CacheReport {
  totalBytes: number;
  budgetBytes: number;
  files: number;
  removableBytes: number;
  removedBytes: number;
  compactedBytes: number;
  generation: number;
  dryRun: boolean;
  journalBytes: number;
  journalEntries: number;
  journalCompactRecommended: boolean;
  journalEstimatedReclaimedBytes: number;
}
