import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Archive,
  ArchiveRestore,
  Activity,
  Boxes,
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  ChartNoAxesCombined,
  Database,
  FolderArchive,
  FolderOpen,
  Gauge,
  KeyRound,
  ListTree,
  LoaderCircle,
  LockKeyhole,
  Menu,
  PackageOpen,
  Play,
  RefreshCw,
  Search,
  Server,
  Settings as SettingsIcon,
  ShieldCheck,
  Square,
  Trash2,
  Unlock,
  X,
} from "lucide-react";
import { api } from "./api";
import { createTranslator, type I18nKey } from "./i18n";
import type {
  AppError,
  AppSnapshot,
  ArchiveInspection,
  CacheReport,
  NavId,
  ProjectStatus,
  Settings,
  TaskStatus,
  DesktopPackRequest,
  DaemonStatus,
  BenchmarkRequest,
} from "./types";

const navItems: Array<{ id: NavId; label: I18nKey; icon: typeof Archive }> = [
  { id: "projects", label: "nav.projects", icon: ListTree },
  { id: "create", label: "nav.create", icon: FolderArchive },
  { id: "open", label: "nav.open", icon: PackageOpen },
  { id: "tasks", label: "nav.tasks", icon: Gauge },
  { id: "cache", label: "nav.cache", icon: Database },
  { id: "runtime", label: "nav.runtime", icon: Server },
  { id: "diagnostics", label: "nav.diagnostics", icon: ChartNoAxesCombined },
  { id: "settings", label: "nav.settings", icon: SettingsIcon },
];

const emptySettings: Settings = {
  defaultOutputDir: null,
  defaultSpeed: "balanced",
  defaultEncryption: "password",
  sessionTtlSecs: 1800,
  recentProjects: [],
  language: "system",
  knownCacheDirs: [],
};

function normalizeSettings(settings: Settings): Settings {
  return { ...emptySettings, ...settings, language: settings.language ?? "system" };
}

function formatBytes(value: number | null | undefined): string {
  if (!value) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB"];
  const power = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** power).toFixed(power === 0 ? 0 : 1)} ${units[power]}`;
}

function basename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function statusTone(phase: string): "ok" | "warn" | "bad" | "live" {
  if (phase === "Completed" || phase === "Ready") return "ok";
  if (phase === "Failed" || phase === "Invalid") return "bad";
  if (phase === "Cancelled" || phase === "Dirty") return "warn";
  return "live";
}

function asStatusKey(phase: string): I18nKey {
  const key = `status.${phase}` as I18nKey;
  return key;
}

function asTaskKindKey(kind: TaskStatus["kind"]): I18nKey {
  return `task.kind.${kind}` as I18nKey;
}

type Translator = ReturnType<typeof createTranslator>;

function appErrorMessage(error: unknown, i18n: Translator): string {
  if (typeof error === "string") return i18n.error(undefined, error);
  if (error && typeof error === "object") {
    const maybe = error as Partial<AppError>;
    return i18n.error(maybe.code, maybe.message);
  }
  return i18n.error(undefined);
}

export default function App() {
  const [nav, setNav] = useState<NavId>("projects");
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [localSettings, setLocalSettings] = useState<Settings>(emptySettings);
  const [tasks, setTasks] = useState<TaskStatus[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [sessionDialog, setSessionDialog] = useState(false);
  const [sessionPassword, setSessionPassword] = useState("");

  const settings = localSettings;
  const i18n = useMemo(() => createTranslator(settings.language), [settings.language]);
  const { t } = i18n;

  useEffect(() => {
    api.bootstrap()
      .then((next) => {
        const normalized = normalizeSettings(next.settings);
        setLocalSettings(normalized);
        setSnapshot({ ...next, settings: normalized });
      })
      .catch((error) => setNotice(appErrorMessage(error, createTranslator(emptySettings.language))));
    const poll = window.setInterval(() => api.tasks().then(setTasks).catch(() => undefined), 500);
    return () => window.clearInterval(poll);
  }, []);

  const activeTasks = tasks.filter((task) => !["Completed", "Failed", "Cancelled"].includes(task.phase));
  const updateRuntimeSnapshot = useCallback((daemon: DaemonStatus) => setSnapshot((current) => current && { ...current, daemonActive: daemon.active, sessionActive: daemon.sessionActive }), []);
  const updateSnapshotSettings = (next: Settings) => {
    const normalized = normalizeSettings(next);
    setLocalSettings(normalized);
    setSnapshot((current) => current && { ...current, settings: normalized });
  };
  const onError = (error: unknown) => setNotice(appErrorMessage(error, i18n));
  const unlockSession = async () => {
    try {
      await api.unlockSession(sessionPassword, settings.sessionTtlSecs);
      setSessionPassword("");
      setSessionDialog(false);
      setSnapshot((current) => current && { ...current, daemonActive: true, sessionActive: true });
    } catch (error) {
      setSessionPassword("");
      onError(error);
    }
  };
  const clearSession = async () => {
    try {
      await api.clearSession();
      setSnapshot((current) => current && { ...current, sessionActive: false });
    } catch (error) {
      onError(error);
    }
  };

  return (
    <div className="app-shell" lang={i18n.language}>
      <aside className={sidebarOpen ? "sidebar is-open" : "sidebar"}>
        <div className="brand-row">
          <img src="/icon.png" alt="" className="brand-mark" />
          <div><strong>Hig</strong><span>{t("app.brandSubtitle")}</span></div>
          <button className="icon-button sidebar-close" onClick={() => setSidebarOpen(false)} aria-label={t("sidebar.close")}><X size={18} /></button>
        </div>
        <nav aria-label={t("sidebar.navigation")}>
          {navItems.map((item) => {
            const Icon = item.icon;
            return (
              <button key={item.id} className={nav === item.id ? "nav-item active" : "nav-item"} onClick={() => { setNav(item.id); setSidebarOpen(false); }}>
                <Icon size={18} /><span>{t(item.label)}</span>
                {item.id === "tasks" && activeTasks.length > 0 && <b>{activeTasks.length}</b>}
              </button>
            );
          })}
        </nav>
        <div className="sidebar-status">
          <div className="status-line"><span className={snapshot?.daemonActive ? "status-dot ok" : "status-dot"} />{snapshot?.daemonActive ? t("daemon.online") : t("daemon.offline")}</div>
          <button className="status-line session-action" onClick={() => snapshot?.sessionActive ? void clearSession() : setSessionDialog(true)}><KeyRound size={14} />{snapshot?.sessionActive ? t("session.unlocked") : t("session.locked")}</button>
          <small>v{snapshot?.version ?? "1.9.7"}</small>
        </div>
      </aside>

      <div className="workspace">
        <header className="topbar">
          <button className="icon-button menu-button" onClick={() => setSidebarOpen(true)} aria-label={t("sidebar.open")}><Menu size={20} /></button>
          <div><span className="eyebrow">{t("app.workspace")}</span><h1>{t(navItems.find((item) => item.id === nav)?.label ?? "nav.projects")}</h1></div>
          <div className="topbar-actions">
            {activeTasks.length > 0 && <button className="task-live" onClick={() => setNav("tasks")}><LoaderCircle size={15} className="spin" />{t("app.activeTasks", { count: activeTasks.length })}</button>}
            <span className="security-chip"><ShieldCheck size={15} />{t("app.secureDefaults")}</span>
          </div>
        </header>

        {notice && <div className="notice"><CircleAlert size={17} /><span>{notice}</span><button onClick={() => setNotice(null)} aria-label={t("app.dismiss")}><X size={16} /></button></div>}

        <main className="content">
          {nav === "projects" && <ProjectsPage settings={settings} i18n={i18n} onSettings={updateSnapshotSettings} onNavigate={setNav} onError={onError} />}
          {nav === "create" && <AdvancedCreatePage settings={settings} i18n={i18n} onTask={(task) => { setTasks((items) => [task, ...items]); setNav("tasks"); }} onError={onError} />}
          {nav === "open" && <AdvancedOpenArchivePage i18n={i18n} onTask={(task) => { setTasks((items) => [task, ...items]); setNav("tasks"); }} onError={onError} />}
          {nav === "tasks" && <TasksPage tasks={tasks} i18n={i18n} onTasks={setTasks} onError={onError} />}
          {nav === "cache" && <CachePage i18n={i18n} onTask={(task) => { setTasks((items) => [task, ...items]); setNav("tasks"); }} onError={onError} />}
          {nav === "runtime" && <RuntimePage settings={settings} i18n={i18n} onError={onError} onSnapshot={updateRuntimeSnapshot} />}
          {nav === "diagnostics" && <DiagnosticsPage i18n={i18n} onTask={(task) => { setTasks((items) => [task, ...items]); setNav("tasks"); }} onError={onError} />}
          {nav === "settings" && <SettingsPage settings={settings} i18n={i18n} onSettings={updateSnapshotSettings} onError={onError} />}
        </main>
      </div>
      {sessionDialog && <div className="modal-backdrop" role="presentation" onMouseDown={() => { setSessionPassword(""); setSessionDialog(false); }}><section className="modal" role="dialog" aria-modal="true" aria-labelledby="session-title" onMouseDown={(event) => event.stopPropagation()}><div className="modal-icon"><Unlock size={22} /></div><h2 id="session-title">{t("session.title")}</h2><p>{t("session.body", { minutes: Math.round(settings.sessionTtlSecs / 60) })}</p><label><span>{t("session.password")}</span><input autoFocus type="password" value={sessionPassword} onChange={(event) => setSessionPassword(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && sessionPassword) void unlockSession(); }} /></label><div className="modal-actions"><button className="secondary-button" onClick={() => { setSessionPassword(""); setSessionDialog(false); }}>{t("session.cancel")}</button><button className="primary-button" disabled={!sessionPassword} onClick={unlockSession}><KeyRound size={16} />{t("session.unlock")}</button></div></section></div>}
    </div>
  );
}

function ProjectsPage({ settings, i18n, onSettings, onNavigate, onError }: { settings: Settings; i18n: Translator; onSettings: (settings: Settings) => void; onNavigate: (nav: NavId) => void; onError: (error: unknown) => void }) {
  const { t } = i18n;
  const [statuses, setStatuses] = useState<Record<string, ProjectStatus>>({});
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    settings.recentProjects.forEach((path) => api.projectStatus(path).then((status) => setStatuses((current) => ({ ...current, [path]: status }))).catch(() => undefined));
  }, [settings.recentProjects]);

  const addProject = async () => {
    const directory = await api.chooseDirectory();
    if (!directory) return;
    const cacheDir = window.prompt(t("projects.cachePrompt"), "") || undefined;
    const excludes = (window.prompt(t("projects.excludesPrompt"), ".git,.hig,.hig-cache,node_modules,.next,dist,build") ?? "")
      .split(",").map((value) => value.trim()).filter(Boolean);
    setBusy(true);
    try {
      const status = await api.initializeProject(directory, cacheDir, excludes);
      setStatuses((current) => ({ ...current, [directory]: status }));
      const next = { ...settings, recentProjects: [directory, ...settings.recentProjects.filter((path) => path !== directory)].slice(0, 12) };
      onSettings(await api.updateSettings(next));
    } catch (error) { onError(error); } finally { setBusy(false); }
  };

  return (
    <div className="page-stack">
      <section className="page-heading">
        <div><h2>{t("projects.title")}</h2><p>{t("projects.subtitle")}</p></div>
        <button className="primary-button" onClick={addProject} disabled={busy}><FolderOpen size={17} />{t("projects.initialize")}</button>
      </section>
      {settings.recentProjects.length === 0 ? (
        <section className="empty-state"><ListTree size={28} /><h3>{t("projects.emptyTitle")}</h3><p>{t("projects.emptyBody")}</p><button className="secondary-button" onClick={addProject}>{t("projects.choose")}</button></section>
      ) : (
        <div className="project-list">
          {settings.recentProjects.map((path) => {
            const status = statuses[path];
            const state = status?.snapshotValidity ?? "Building";
            return <article className="project-row" key={path}>
              <div className={`project-icon ${statusTone(state)}`}><Boxes size={20} /></div>
              <div className="project-main"><div className="project-name"><strong>{basename(path)}</strong><span className={`badge ${statusTone(state)}`}>{t(asStatusKey(state))}</span></div><code title={path}>{path}</code></div>
              <dl><div><dt>{t("projects.files")}</dt><dd>{status?.files?.toLocaleString() ?? "—"}</dd></div><div><dt>{t("projects.generation")}</dt><dd>{status?.generation ?? "—"}</dd></div><div><dt>{t("projects.prepared")}</dt><dd>{formatBytes(status?.preparedBytes)}</dd></div><div><dt>{t("projects.pending")}</dt><dd>{status?.pendingEvents ?? "—"}</dd></div><div><dt>{t("projects.overflow")}</dt><dd>{status?.watcherOverflowCount ?? "—"}</dd></div></dl>
              <div className="row-actions"><button className="icon-button" title={t("projects.rebuild")} aria-label={t("projects.rebuild")} onClick={() => api.rebuildProject(path).then(() => onNavigate("tasks")).catch(onError)}><RefreshCw size={17} /></button><button className="secondary-button" onClick={() => onNavigate("create")}>{t("projects.archive")}<ChevronRight size={15} /></button></div>
            </article>;
          })}
        </div>
      )}
      <section className="metric-band"><div><span>{t("projects.metricMode")}</span><strong>{t("projects.metricVerified")}</strong></div><div><span>{t("projects.metricWatcher")}</span><strong>{t("projects.metricEventAware")}</strong></div><div><span>{t("projects.metricPrepared")}</span><strong>{t("projects.metricEncrypted")}</strong></div></section>
    </div>
  );
}

function AdvancedCreatePage({ settings, i18n, onTask, onError }: { settings: Settings; i18n: Translator; onTask: (task: TaskStatus) => void; onError: (error: unknown) => void }) {
  const { t } = i18n;
  const initial: DesktopPackRequest = {
    inputDir: "", outputFile: "", password: null, useSession: false,
    encryption: settings.defaultEncryption === "none" ? "None" : "Password",
    speed: settings.defaultSpeed === "fastest" ? "Fastest" : "Balanced",
    cacheDir: null, threads: null, level: null, useCache: true,
    format: "HigV2", manifestFormat: "Compact",
    batch: { enabled: true, smallFileThreshold: 65536, maxBatchRawBytes: 4194304 },
    chunk: { enabled: true, chunkFileThreshold: 8388608, chunkSize: 1048576 },
    kdfProfile: settings.defaultSpeed === "fastest" ? "Interactive" : "Secure",
    trustMetadata: settings.defaultSpeed === "fastest", projectMode: "Auto",
    solid: settings.defaultSpeed === "fastest" ? "Off" : "Auto",
  };
  const [form, setForm] = useState<DesktopPackRequest>(initial);
  const [password, setPassword] = useState("");
  const [advanced, setAdvanced] = useState(false);
  const [busy, setBusy] = useState(false);
  const update = <K extends keyof DesktopPackRequest>(key: K, value: DesktopPackRequest[K]) => setForm((current) => ({ ...current, [key]: value }));
  const selectInput = async () => { const path = await api.chooseDirectory(); if (path) setForm((current) => ({ ...current, inputDir: path, outputFile: current.outputFile || `${path}.hig` })); };
  const selectOutput = async () => { const path = await api.chooseArchiveOutput(); if (path) update("outputFile", path); };
  const setSpeed = (speed: DesktopPackRequest["speed"]) => setForm((current) => ({ ...current, speed, trustMetadata: speed === "Fastest", kdfProfile: speed === "Fastest" ? "Interactive" : "Secure", solid: speed === "Fastest" ? "Off" : "Auto" }));
  const setFormat = (format: DesktopPackRequest["format"]) => setForm((current) => format === "HigV1" ? { ...current, format, manifestFormat: "Legacy", batch: { ...current.batch, enabled: false }, chunk: { ...current.chunk, enabled: false }, solid: "Off" } : { ...current, format });
  const resetAdvanced = () => setForm((current) => ({ ...initial, inputDir: current.inputDir, outputFile: current.outputFile, encryption: current.encryption }));
  const submit = async () => {
    if (!form.inputDir || !form.outputFile) return onError(t("create.missingPath"));
    if (form.encryption === "Password" && !form.useSession && !password) return onError(t("create.missingPassword"));
    if (form.encryption === "None" && (password || form.useSession)) return onError(t("create.secretConflict"));
    setBusy(true);
    try {
      const task = await api.startPack({ ...form, password: form.encryption === "Password" && !form.useSession ? password : null });
      setPassword("");
      onTask(task);
    } catch (error) { setPassword(""); onError(error); } finally { setBusy(false); }
  };
  return <div className="page-stack form-page">
    <section className="page-heading"><div><h2>{t("create.title")}</h2><p>{t("create.subtitle")}</p></div></section>
    <section className="form-section"><h3>{t("create.source")}</h3><PathField label={t("create.input")} value={form.inputDir} placeholder={t("create.inputPlaceholder")} onChoose={selectInput} t={t} /><PathField label={t("create.output")} value={form.outputFile} placeholder={t("create.outputPlaceholder")} onChoose={selectOutput} t={t} /></section>
    <section className="form-section"><h3>{t("create.policy")}</h3><div className="field-grid"><label><span>{t("create.speed")}</span><select value={form.speed} onChange={(event) => setSpeed(event.target.value as DesktopPackRequest["speed"])}><option value="Balanced">{t("create.speedBalanced")}</option><option value="Fastest">{t("create.speedFastest")}</option></select></label><label><span>{t("create.encryption")}</span><select value={form.encryption} onChange={(event) => { const encryption = event.target.value as DesktopPackRequest["encryption"]; setPassword(""); setForm((current) => ({ ...current, encryption, useSession: encryption === "None" ? false : current.useSession })); }}><option value="Password">{t("create.encryptionPassword")}</option><option value="None">{t("create.encryptionNone")}</option></select></label></div>{form.encryption === "Password" && !form.useSession ? <label className="password-field"><span>{t("create.password")}</span><div><LockKeyhole size={17} /><input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="new-password" placeholder={t("create.passwordPlaceholder")} /></div></label> : form.encryption === "None" ? <div className="warning-line"><CircleAlert size={17} />{t("create.noEncryptionWarning")}</div> : <div className="security-note compact-note"><KeyRound size={18} /><span>{t("create.sessionSelected")}</span></div>}{form.speed === "Fastest" && <div className="warning-line"><CircleAlert size={17} />{t("create.fastestWarning")}</div>}</section>
    <section className="advanced-toggle"><button onClick={() => setAdvanced(!advanced)}>{t("create.advanced")}<ChevronRight className={advanced ? "rotate" : ""} size={16} /></button>{advanced && <div className="advanced-panel">
      <div className="advanced-group"><h4>{t("create.archiveGroup")}</h4><div className="field-grid"><label><span>{t("create.format")}</span><select value={form.format} onChange={(event) => setFormat(event.target.value as DesktopPackRequest["format"])}><option value="HigV2">HIGV2</option><option value="HigV1">HIGV1</option></select></label><label><span>{t("create.manifest")}</span><select disabled={form.format === "HigV1"} value={form.manifestFormat} onChange={(event) => update("manifestFormat", event.target.value as DesktopPackRequest["manifestFormat"])}><option value="Compact">{t("create.compact")}</option><option value="Legacy">{t("create.legacy")}</option></select></label><NumericField label={t("create.level")} value={form.level} min={-7} max={22} placeholder={t("create.auto")} onChange={(value) => update("level", value)} /><NumericField label={t("create.threads")} value={form.threads} min={1} max={1024} placeholder={t("create.auto")} onChange={(value) => update("threads", value)} /></div></div>
      <div className="advanced-group"><h4>{t("create.cacheGroup")}</h4><div className="field-grid"><label><span>{t("create.projectMode")}</span><select value={form.projectMode} onChange={(event) => update("projectMode", event.target.value as DesktopPackRequest["projectMode"])}><option value="Auto">{t("create.auto")}</option><option value="Off">{t("create.off")}</option><option value="Required">{t("create.required")}</option></select></label><label><span>{t("create.solidGroups")}</span><select disabled={form.format === "HigV1"} value={form.solid} onChange={(event) => update("solid", event.target.value as DesktopPackRequest["solid"])}><option value="Auto">{t("create.auto")}</option><option value="Off">{t("create.off")}</option></select></label><label><span>{t("create.kdf")}</span><select value={form.kdfProfile ?? "Secure"} onChange={(event) => update("kdfProfile", event.target.value as "Secure" | "Interactive")}><option value="Secure">{t("create.kdfSecure")}</option><option value="Interactive">{t("create.kdfInteractive")}</option></select></label><label><span>{t("create.cacheDirectory")}</span><input value={form.cacheDir ?? ""} placeholder={t("create.defaultPath")} onChange={(event) => update("cacheDir", event.target.value || null)} /></label></div><label className="check-row"><input type="checkbox" checked={form.useCache} onChange={(event) => update("useCache", event.target.checked)} /><span><strong>{t("create.useCache")}</strong></span></label>{form.encryption === "Password" && <label className="check-row"><input type="checkbox" checked={form.useSession} onChange={(event) => { setPassword(""); update("useSession", event.target.checked); }} /><span><strong>{t("create.useSession")}</strong><small>{t("create.useSessionHelp")}</small></span></label>}<label className="check-row"><input type="checkbox" checked={form.trustMetadata} onChange={(event) => update("trustMetadata", event.target.checked)} /><span><strong>{t("create.trustMetadata")}</strong><small>{t("create.trustMetadataHelp")}</small></span></label></div>
      <div className="advanced-group"><h4>{t("create.batch")}</h4><label className="check-row"><input type="checkbox" disabled={form.format === "HigV1"} checked={form.batch.enabled} onChange={(event) => update("batch", { ...form.batch, enabled: event.target.checked })} /><span><strong>{t("create.enableBatch")}</strong></span></label><div className="field-grid"><NumericField label={t("create.smallThreshold")} value={form.batch.smallFileThreshold} min={1} max={67108864} onChange={(value) => value !== null && update("batch", { ...form.batch, smallFileThreshold: value })} /><NumericField label={t("create.maxBatch")} value={form.batch.maxBatchRawBytes} min={1} max={268435456} onChange={(value) => value !== null && update("batch", { ...form.batch, maxBatchRawBytes: value })} /></div></div>
      <div className="advanced-group"><h4>{t("create.chunks")}</h4><label className="check-row"><input type="checkbox" disabled={form.format === "HigV1"} checked={form.chunk.enabled} onChange={(event) => update("chunk", { ...form.chunk, enabled: event.target.checked })} /><span><strong>{t("create.enableChunk")}</strong></span></label><div className="field-grid"><NumericField label={t("create.chunkThreshold")} value={form.chunk.chunkFileThreshold} min={65536} max={1073741824} onChange={(value) => value !== null && update("chunk", { ...form.chunk, chunkFileThreshold: value })} /><NumericField label={t("create.chunkSize")} value={form.chunk.chunkSize} min={65536} max={67108864} onChange={(value) => value !== null && update("chunk", { ...form.chunk, chunkSize: value })} /></div></div>
      <button className="secondary-button reset-button" onClick={resetAdvanced}>{t("create.resetDefaults")}</button>
    </div>}</section>
    <footer className="form-footer"><div><ShieldCheck size={18} /><span>{form.encryption === "Password" ? t("create.securityPassword") : t("create.securityNone")}</span></div><button className="primary-button" onClick={submit} disabled={busy}><Play size={17} />{busy ? t("create.starting") : t("create.submit")}</button></footer>
  </div>;
}

function NumericField({ label, value, min, max, placeholder, onChange }: { label: string; value: number | null; min: number; max: number; placeholder?: string; onChange: (value: number | null) => void }) {
  return <label><span>{label}</span><input type="number" min={min} max={max} value={value ?? ""} placeholder={placeholder} onChange={(event) => onChange(event.target.value === "" ? null : Number(event.target.value))} /></label>;
}

function PathField({ label, value, placeholder, onChoose, t }: { label: string; value: string; placeholder: string; onChoose: () => void; t: Translator["t"] }) {
  return <label className="path-field"><span>{label}</span><div><code title={value || placeholder}>{value || placeholder}</code><button className="icon-button" onClick={onChoose} type="button" aria-label={t("path.choose", { label })}><FolderOpen size={18} /></button></div></label>;
}

function AdvancedOpenArchivePage({ i18n, onTask, onError }: { i18n: Translator; onTask: (task: TaskStatus) => void; onError: (error: unknown) => void }) {
  const { t } = i18n;
  const [path, setPath] = useState("");
  const [password, setPassword] = useState("");
  const [inspection, setInspection] = useState<ArchiveInspection | null>(null);
  const [query, setQuery] = useState("");
  const [output, setOutput] = useState("");
  const [overwrite, setOverwrite] = useState(false);
  const [page, setPage] = useState(0);
  const [sort, setSort] = useState<"path" | "size" | "time">("path");
  const pageSize = 250;
  const files = useMemo(() => {
    const filtered = inspection?.files.filter((file) => file.relativePath.toLowerCase().includes(query.toLowerCase())) ?? [];
    return [...filtered].sort((a, b) => sort === "size" ? b.size - a.size : sort === "time" ? b.modifiedUnixNs - a.modifiedUnixNs : a.relativePath.localeCompare(b.relativePath));
  }, [inspection, query, sort]);
  const visible = files.slice(page * pageSize, (page + 1) * pageSize);
  const choose = async () => { const next = await api.chooseArchive(); if (next) { setPath(next); setInspection(null); setPage(0); } };
  const inspect = async () => { try { setInspection(await api.inspectArchive(path, password)); setPassword(""); } catch (error) { setPassword(""); onError(error); } };
  const extract = async () => {
    if (!output) return onError(t("open.missingOutput"));
    if (overwrite && !window.confirm(t("open.overwriteConfirm"))) return;
    try { const task = await api.startUnpack({ archiveFile: path, outputDir: output, password: password || null, overwrite }); setPassword(""); onTask(task); } catch (error) { setPassword(""); onError(error); }
  };
  return <div className="page-stack"><section className="page-heading"><div><h2>{t("open.title")}</h2><p>{t("open.subtitle")}</p></div><button className="secondary-button" onClick={choose}><FolderOpen size={17} />{t("open.choose")}</button></section><section className="archive-toolbar"><code title={path || t("open.noArchive")}>{path || t("open.noArchive")}</code><label><LockKeyhole size={16} /><input type="password" placeholder={t("open.passwordPlaceholder")} value={password} onChange={(event) => setPassword(event.target.value)} /></label><button className="primary-button" disabled={!path} onClick={inspect}><Search size={16} />{t("open.inspect")}</button></section>{inspection ? <><section className="archive-summary"><div><span>{t("open.format")}</span><strong>{inspection.format}</strong></div><div><span>{t("open.files")}</span><strong>{inspection.files.length.toLocaleString()}</strong></div><div><span>{t("open.original")}</span><strong>{formatBytes(inspection.inputBytes)}</strong></div><div><span>{t("open.archive")}</span><strong>{formatBytes(inspection.archiveBytes)}</strong></div><div><span>{t("open.security")}</span><strong>{inspection.encrypted ? t("open.encrypted") : t("open.integrityOnly")}</strong></div></section><section className="file-browser"><div className="table-toolbar"><h3>{t("open.contents")}</h3><div className="table-controls"><select value={sort} onChange={(event) => { setSort(event.target.value as typeof sort); setPage(0); }}><option value="path">{t("open.sortPath")}</option><option value="size">{t("open.sortSize")}</option><option value="time">{t("open.sortTime")}</option></select><label><Search size={15} /><input value={query} onChange={(event) => { setQuery(event.target.value); setPage(0); }} placeholder={t("open.filter")} /></label></div></div><div className="file-table" role="table"><div className="file-row extended header" role="row"><span>{t("open.path")}</span><span>{t("open.size")}</span><span>{t("open.modified")}</span><span>{t("open.hash")}</span></div>{visible.map((file) => <div className="file-row extended" role="row" key={file.relativePath}><span title={file.relativePath}>{file.relativePath}</span><span>{formatBytes(file.size)}</span><span>{new Date(file.modifiedUnixNs / 1_000_000).toLocaleDateString()}</span><span title={file.contentHash.map((value) => value.toString(16).padStart(2, "0")).join("")}>{file.contentHash.slice(0, 4).map((value) => value.toString(16).padStart(2, "0")).join("")}</span></div>)}</div><div className="pager"><button className="secondary-button" disabled={page === 0} onClick={() => setPage((value) => value - 1)}>{t("open.previous")}</button><span>{t("open.page", { current: page + 1, total: Math.max(1, Math.ceil(files.length / pageSize)) })}</span><button className="secondary-button" disabled={(page + 1) * pageSize >= files.length} onClick={() => setPage((value) => value + 1)}>{t("open.next")}</button></div></section><section className="extract-bar"><PathField label={t("open.extractTo")} value={output} placeholder={t("open.extractPlaceholder")} t={t} onChoose={async () => { const selected = await api.chooseDirectory(); if (selected) setOutput(selected); }} /><div className="extract-actions"><label className="check-row"><input type="checkbox" checked={overwrite} onChange={(event) => setOverwrite(event.target.checked)} /><span><strong>{t("open.overwrite")}</strong></span></label><button className="primary-button" onClick={extract}><ArchiveRestore size={17} />{t("open.extract")}</button></div></section></> : <section className="empty-state compact"><Archive size={28} /><h3>{t("open.emptyTitle")}</h3><p>{t("open.emptyBody")}</p></section>}</div>;
}

function TasksPage({ tasks, i18n, onTasks, onError }: { tasks: TaskStatus[]; i18n: Translator; onTasks: (tasks: TaskStatus[]) => void; onError: (error: unknown) => void }) {
  const { t } = i18n;
  const cancel = async (task: TaskStatus) => { try { const next = task.kind === "Benchmark" ? await api.cancelBenchmark(task.id) : await api.cancelTask(task.id, task.cacheDir); onTasks(tasks.map((value) => value.id === task.id && value.cacheDir === task.cacheDir ? next : value)); } catch (error) { onError(error); } };
  const clearCompleted = async () => { await api.clearTaskHistory(); onTasks(tasks.filter((task) => !["Completed", "Failed", "Cancelled"].includes(task.phase))); };
  return <div className="page-stack"><section className="page-heading"><div><h2>{t("tasks.title")}</h2><p>{t("tasks.subtitle")}</p></div><div className="heading-actions"><span className="count-label">{t("tasks.count", { count: tasks.length })}</span>{tasks.some((task) => ["Completed", "Failed", "Cancelled"].includes(task.phase)) && <button className="secondary-button" onClick={() => void clearCompleted()}>{t("tasks.clearLocal")}</button>}</div></section>{tasks.length === 0 ? <section className="empty-state"><Gauge size={28} /><h3>{t("tasks.emptyTitle")}</h3><p>{t("tasks.emptyBody")}</p></section> : <div className="task-list">{[...tasks].reverse().map((task) => <article className="task-row" key={`${task.cacheDir}:${task.id}`}><div className={`task-state ${statusTone(task.phase)}`}>{task.phase === "Completed" ? <CheckCircle2 size={20} /> : task.phase === "Failed" ? <CircleAlert size={20} /> : task.phase === "Cancelled" ? <Square size={18} /> : <LoaderCircle className="spin" size={20} />}</div><div className="task-main"><div><strong>{t(asTaskKindKey(task.kind))}</strong><span className={`badge ${statusTone(task.phase)}`}>{t(asStatusKey(task.phase))}</span>{task.disconnected && <span className="badge warn">{t("tasks.disconnected")}</span>}</div><code title={task.outputPath ?? task.id}>{task.outputPath ?? task.id.slice(0, 12)}</code><div className="progress-track"><span style={{ width: task.bytesTotal ? `${Math.min(100, task.bytesDone / task.bytesTotal * 100)}%` : task.phase === "Completed" ? "100%" : "35%" }} /></div><small>{task.resultExpired ? t("tasks.resultExpired") : task.message ?? t(asStatusKey(task.phase))}{task.archiveBytes ? ` · ${formatBytes(task.archiveBytes)}` : ""}</small>{task.error && <p className="task-error">{appErrorMessage(task.error, i18n)} <code>{task.error.code}</code></p>}</div><div className="task-actions">{task.cancellable && <button className="icon-button danger" title={t("tasks.cancel")} aria-label={t("tasks.cancel")} onClick={() => cancel(task)}><Square size={16} /></button>}</div></article>)}</div>}</div>;
}

function CachePage({ i18n, onTask, onError }: { i18n: Translator; onTask: (task: TaskStatus) => void; onError: (error: unknown) => void }) {
  const { t } = i18n;
  const [report, setReport] = useState<CacheReport | null>(null);
  const refresh = () => api.cacheStatus().then(setReport).catch(onError);
  useEffect(() => {
    void api.cacheStatus().then(setReport).catch(onError);
  }, [onError]);
  const maintain = async (kind: "gc" | "compact") => { try { const preview = kind === "gc" ? await api.previewCacheGc() : await api.previewCacheCompact(); const bytes = formatBytes(preview.removableBytes || preview.journalEstimatedReclaimedBytes); if (window.confirm(kind === "gc" ? t("cache.confirmGc", { bytes }) : t("cache.confirmCompact", { bytes }))) { onTask(kind === "gc" ? await api.submitCacheGc() : await api.submitCacheCompact()); } } catch (error) { onError(error); } };
  return <div className="page-stack"><section className="page-heading"><div><h2>{t("cache.title")}</h2><p>{t("cache.subtitle")}</p></div><button className="icon-button" onClick={refresh} title={t("cache.refresh")} aria-label={t("cache.refresh")}><RefreshCw size={18} /></button></section>{report ? <><section className="cache-usage"><div className="cache-gauge"><span style={{ width: `${Math.min(100, report.totalBytes / Math.max(1, report.budgetBytes) * 100)}%` }} /></div><div><strong>{formatBytes(report.totalBytes)}</strong><span>{t("cache.budget", { bytes: formatBytes(report.budgetBytes) })}</span></div></section><section className="stats-grid"><div><span>{t("cache.objects")}</span><strong>{report.files.toLocaleString()}</strong></div><div><span>{t("cache.generation")}</span><strong>{report.generation}</strong></div><div><span>{t("cache.journal")}</span><strong>{formatBytes(report.journalBytes)}</strong></div><div><span>{t("cache.entries")}</span><strong>{report.journalEntries.toLocaleString()}</strong></div></section><section className="maintenance-list"><div><div><Trash2 size={20} /><span><strong>{t("cache.gc")}</strong><small>{t("cache.gcBody")}</small></span></div><button className="secondary-button" onClick={() => maintain("gc")}>{t("cache.gcAction")}</button></div><div><div><Database size={20} /><span><strong>{t("cache.compact")}</strong><small>{report.journalCompactRecommended ? t("cache.compactRecommended") : t("cache.compactHealthy")}</small></span></div><button className="secondary-button" onClick={() => maintain("compact")}>{t("cache.compactAction")}</button></div></section></> : <section className="empty-state compact"><Database size={28} /><h3>{t("cache.emptyTitle")}</h3><p>{t("cache.emptyBody")}</p></section>}</div>;
}

function RuntimePage({ settings, i18n, onError, onSnapshot }: { settings: Settings; i18n: Translator; onError: (error: unknown) => void; onSnapshot: (status: DaemonStatus) => void }) {
  const { t } = i18n;
  const [cacheDir, setCacheDir] = useState(settings.knownCacheDirs[0] ?? "");
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const refresh = () => api.daemonStatus(cacheDir || undefined).then((next) => { setStatus(next); onSnapshot(next); }).catch(onError);
  useEffect(() => { void api.daemonStatus(cacheDir || undefined).then((next) => { setStatus(next); onSnapshot(next); }).catch(() => setStatus(null)); }, [cacheDir, onSnapshot]);
  const start = async () => { try { const next = await api.startDaemon(cacheDir || undefined, settings.sessionTtlSecs); setStatus(next); onSnapshot(next); } catch (error) { onError(error); } };
  const stop = async () => { try { if (status?.activeJobs && !window.confirm(t("runtime.stopBusy"))) return; await api.stopDaemon(cacheDir || undefined, Boolean(status?.activeJobs)); setStatus(null); onSnapshot({ ...(status ?? emptyDaemonStatus(cacheDir)), active: false, sessionActive: false }); } catch (error) { onError(error); } };
  const restart = async () => { try { if (!window.confirm(t("runtime.restartWarning"))) return; const next = await api.restartDaemon(cacheDir || undefined, settings.sessionTtlSecs, Boolean(status?.activeJobs)); setStatus(next); onSnapshot(next); } catch (error) { onError(error); } };
  return <div className="page-stack"><section className="page-heading"><div><h2>{t("runtime.title")}</h2><p>{t("runtime.subtitle")}</p></div><button className="icon-button" onClick={refresh} aria-label={t("runtime.refresh")}><RefreshCw size={18} /></button></section><section className="form-section"><h3>{t("runtime.cacheBinding")}</h3><label><span>{t("runtime.cacheDirectory")}</span><input value={cacheDir} onChange={(event) => setCacheDir(event.target.value)} placeholder={t("create.defaultPath")} /></label></section>{status?.active ? <><section className="stats-grid runtime-grid"><div><span>{t("runtime.uptime")}</span><strong>{Math.floor(status.uptimeSecs / 60)}m</strong></div><div><span>{t("runtime.activeJobs")}</span><strong>{status.activeJobs}</strong></div><div><span>{t("runtime.queuedJobs")}</span><strong>{status.queuedJobs}</strong></div><div><span>{t("runtime.projects")}</span><strong>{status.watchedProjects}</strong></div><div><span>{t("runtime.journal")}</span><strong>{formatBytes(status.journalBytes)}</strong></div><div><span>{t("runtime.session")}</span><strong>{status.sessionActive ? t("session.unlocked") : t("session.locked")}</strong></div></section><section className="runtime-actions"><button className="secondary-button" onClick={restart}><RefreshCw size={16} />{t("runtime.restart")}</button><button className="secondary-button danger-button" onClick={stop}><Square size={16} />{t("runtime.stop")}</button></section></> : <section className="empty-state compact"><Server size={28} /><h3>{t("runtime.offline")}</h3><p>{t("runtime.offlineBody")}</p><button className="primary-button" onClick={start}><Play size={16} />{t("runtime.start")}</button></section>}</div>;
}

function emptyDaemonStatus(cacheDir: string): DaemonStatus {
  return { active: false, uptimeSecs: 0, ttlSecs: 0, activeJobs: 0, queuedJobs: 0, jobsCompleted: 0, cacheDir, journalBytes: 0, sessionActive: false, sessionAgeSecs: 0, watchedProjects: 0, projectReadyCount: 0, projectPendingEvents: 0 };
}

function DiagnosticsPage({ i18n, onTask, onError }: { i18n: Translator; onTask: (task: TaskStatus) => void; onError: (error: unknown) => void }) {
  const { t } = i18n;
  const [input, setInput] = useState("");
  const [benchDir, setBenchDir] = useState("");
  const [cacheDir, setCacheDir] = useState("");
  const [suite, setSuite] = useState<BenchmarkRequest["suite"]>("source");
  const [workers, setWorkers] = useState<number | null>(null);
  const [compare, setCompare] = useState(true);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const run = async () => {
    if (!input || !password) return onError(t("diagnostics.missing"));
    setBusy(true);
    try { const task = await api.startBenchmark({ inputDir: input, suite, cacheDir: cacheDir || null, benchDir: benchDir || null, workers, compare, password }); setPassword(""); onTask(task); } catch (error) { setPassword(""); onError(error); } finally { setBusy(false); }
  };
  return <div className="page-stack form-page"><section className="page-heading"><div><h2>{t("diagnostics.title")}</h2><p>{t("diagnostics.subtitle")}</p></div></section><section className="form-section"><h3>{t("diagnostics.corpus")}</h3><PathField label={t("create.input")} value={input} placeholder={t("create.inputPlaceholder")} onChoose={async () => { const value = await api.chooseDirectory(); if (value) setInput(value); }} t={t} /><PathField label={t("diagnostics.benchDir")} value={benchDir} placeholder={t("diagnostics.systemTemp")} onChoose={async () => { const value = await api.chooseDirectory(); if (value) setBenchDir(value); }} t={t} /></section><section className="form-section"><h3>{t("diagnostics.options")}</h3><div className="field-grid"><label><span>{t("diagnostics.suite")}</span><select value={suite} onChange={(event) => setSuite(event.target.value as BenchmarkRequest["suite"])}>{["source", "lobehub", "lobehub-watch", "small500", "textmix", "repeat4m", "random8m", "binarymix", "all"].map((value) => <option key={value} value={value}>{value}</option>)}</select></label><NumericField label={t("diagnostics.workers")} value={workers} min={1} max={1024} placeholder={t("create.auto")} onChange={setWorkers} /><label><span>{t("create.cacheDirectory")}</span><input value={cacheDir} onChange={(event) => setCacheDir(event.target.value)} placeholder={t("create.defaultPath")} /></label><label><span>{t("create.password")}</span><input type="password" value={password} onChange={(event) => setPassword(event.target.value)} placeholder={t("create.passwordPlaceholder")} /></label></div><label className="check-row"><input type="checkbox" checked={compare} onChange={(event) => setCompare(event.target.checked)} /><span><strong>{t("diagnostics.compare")}</strong><small>{t("diagnostics.compareHelp")}</small></span></label><div className="warning-line"><CircleAlert size={17} />{t("diagnostics.warning")}</div></section><footer className="form-footer"><span>{t("diagnostics.outputHelp")}</span><button className="primary-button" disabled={busy} onClick={run}><Activity size={17} />{busy ? t("diagnostics.starting") : t("diagnostics.run")}</button></footer></div>;
}

function SettingsPage({ settings, i18n, onSettings, onError }: { settings: Settings; i18n: Translator; onSettings: (settings: Settings) => void; onError: (error: unknown) => void }) {
  const { t } = i18n;
  const [draft, setDraft] = useState(settings);
  useEffect(() => setDraft(settings), [settings]);
  const save = async () => { try { onSettings(await api.updateSettings(normalizeSettings(draft))); } catch (error) { onError(error); } };
  const clearRecent = () => setDraft({ ...draft, recentProjects: [] });
  const previewLanguage = (language: Settings["language"]) => {
    const next = { ...draft, language };
    setDraft(next);
    onSettings(next);
  };
  return <div className="page-stack form-page"><section className="page-heading"><div><h2>{t("settings.title")}</h2><p>{t("settings.subtitle")}</p></div></section><section className="form-section"><h3>{t("settings.archiveDefaults")}</h3><div className="field-grid"><label><span>{t("settings.language")}</span><select aria-label={t("settings.language")} value={draft.language ?? "system"} onChange={(event) => previewLanguage(event.target.value as Settings["language"])}><option value="system">{t("settings.languageSystem")}</option><option value="en">{t("settings.languageEnglish")}</option><option value="zh-CN">{t("settings.languageChinese")}</option></select></label><label><span>{t("settings.speed")}</span><select value={draft.defaultSpeed} onChange={(event) => setDraft({ ...draft, defaultSpeed: event.target.value as Settings["defaultSpeed"] })}><option value="balanced">{t("create.speedBalanced")}</option><option value="fastest">{t("create.speedFastest")}</option></select></label><label><span>{t("settings.encryption")}</span><select value={draft.defaultEncryption} onChange={(event) => setDraft({ ...draft, defaultEncryption: event.target.value as Settings["defaultEncryption"] })}><option value="password">{t("settings.password")}</option><option value="none">{t("settings.none")}</option></select></label><label><span>{t("settings.ttl")}</span><select value={draft.sessionTtlSecs} onChange={(event) => setDraft({ ...draft, sessionTtlSecs: Number(event.target.value) })}><option value={900}>{t("settings.15m")}</option><option value={1800}>{t("settings.30m")}</option><option value={3600}>{t("settings.1h")}</option><option value={7200}>{t("settings.2h")}</option></select></label></div></section><section className="form-section"><h3>{t("settings.security")}</h3><div className="security-note"><ShieldCheck size={21} /><div><strong>{t("settings.securityTitle")}</strong><p>{t("settings.securityBody")}</p></div></div></section><footer className="form-footer"><span>{t("settings.recent", { count: draft.recentProjects.length })}</span><div className="footer-actions"><button className="secondary-button" onClick={clearRecent} disabled={draft.recentProjects.length === 0}>{t("settings.clearRecent")}</button><button className="primary-button" onClick={save}>{t("settings.save")}</button></div></footer></div>;
}
