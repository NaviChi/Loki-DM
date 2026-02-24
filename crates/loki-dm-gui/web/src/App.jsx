import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke as invokeTauri } from "@tauri-apps/api/core";

const TOOLBAR_ITEMS = [
  { id: "openAdd", label: "Add URL", icon: "icons/add-url.svg" },
  { id: "resumeSelected", label: "Resume", icon: "icons/resume.svg" },
  { id: "pauseSelected", label: "Stop", icon: "icons/stop.svg" },
  { id: "stopAll", label: "Stop All", icon: "icons/stop-all.svg" },
  { id: "deleteSelected", label: "Delete", icon: "icons/delete.svg" },
  { id: "deleteCompleted", label: "Delete C.", icon: "icons/delete-completed.svg" },
  { id: "openOptions", label: "Options", icon: "icons/options.svg" },
  { id: "scheduler", label: "Scheduler", icon: "icons/scheduler.svg" },
  { id: "runQueue", label: "Start Queue", icon: "icons/start-queue.svg" },
  { id: "stopQueue", label: "Stop Queue", icon: "icons/stop-queue.svg" },
  { id: "grabber", label: "Grabber", icon: "icons/grabber.svg" },
  { id: "share", label: "Tell a Friend", icon: "icons/share.svg" },
];

const MENU_ITEMS = ["Tasks", "File", "Downloads", "View", "Help", "Registration"];
const SIDE_PANELS = [
  { id: "preview", label: "Preview" },
  { id: "scheduler", label: "Scheduler" },
  { id: "grabber", label: "Grabber" },
  { id: "browser", label: "Browser" },
];

const INITIAL_FORM = {
  url: "",
  savePath: "",
  connections: "8",
  category: "",
  mirrors: "",
  headers: "",
};

const INITIAL_SCHEDULER_FORM = {
  name: "",
  url: "",
  savePath: "",
  category: "",
  connections: "8",
  startInSecs: "0",
  intervalSecs: "",
};

const INITIAL_GRABBER_FORM = {
  rootUrl: "",
  depth: "2",
  extensions: "",
  sameHostOnly: true,
  respectRobots: true,
  queueResults: true,
  startDownloads: false,
  outputDir: "",
  connections: "8",
};

function parseHeaders(raw) {
  return raw
    .split(";")
    .map((chunk) => chunk.trim())
    .filter(Boolean)
    .map((chunk) => {
      const sep = chunk.indexOf(":");
      if (sep < 1) {
        return null;
      }
      return {
        key: chunk.slice(0, sep).trim(),
        value: chunk.slice(sep + 1).trim(),
      };
    })
    .filter(Boolean)
    .filter((pair) => pair.key && pair.value);
}

function parseMirrors(raw) {
  return raw
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
}

function parseCsv(raw) {
  return raw
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
}

function toPositiveInt(value, fallback) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return fallback;
  }
  return Math.max(1, Math.round(parsed));
}

function toNonNegativeInt(value, fallback = 0) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < 0) {
    return fallback;
  }
  return Math.max(0, Math.round(parsed));
}

function statusBadgeClasses(status) {
  switch (status) {
    case "Complete":
      return "bg-emerald-500/20 text-emerald-400 border-emerald-500/45";
    case "Downloading":
    case "Retrying":
      return "bg-blue-500/20 text-blue-400 border-blue-500/45";
    case "Paused":
      return "bg-amber-500/20 text-amber-400 border-amber-500/45";
    case "Failed":
    case "Cancelled":
      return "bg-rose-500/20 text-rose-400 border-rose-500/45";
    default:
      return "bg-slate-500/20 text-slate-300 border-slate-500/45";
  }
}

function formatBytes(bytes) {
  if (!bytes || bytes <= 0) {
    return "0 B";
  }
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  const precision = index >= 2 ? 2 : 1;
  return `${value.toFixed(precision)} ${units[index]}`;
}

function formatRate(speed) {
  if (!speed || speed <= 0) {
    return "--";
  }
  const mib = speed / (1024 * 1024);
  if (mib >= 1) {
    return `${mib.toFixed(2)} MiB/s`;
  }
  return `${Math.round(speed / 1024)} KiB/s`;
}

function formatEta(eta) {
  if (eta === null || eta === undefined) {
    return "--";
  }
  let sec = Number(eta);
  if (!Number.isFinite(sec) || sec < 0) {
    return "--";
  }
  const hours = Math.floor(sec / 3600);
  sec -= hours * 3600;
  const mins = Math.floor(sec / 60);
  sec -= mins * 60;
  if (hours > 0) {
    return `${hours}h ${String(mins).padStart(2, "0")}m`;
  }
  if (mins > 0) {
    return `${mins}m ${String(sec).padStart(2, "0")}s`;
  }
  return `${sec}s`;
}

function formatEpochMs(epochMs) {
  if (!epochMs || epochMs <= 0) {
    return "--";
  }
  try {
    return new Date(epochMs).toLocaleString();
  } catch {
    return "--";
  }
}

function iconEmoji(fileName = "") {
  const ext = fileName.split(".").pop()?.toLowerCase();
  if (!ext) return "⬇";
  if (["png", "jpg", "jpeg", "gif", "webp", "svg"].includes(ext)) return "🖼";
  if (["mp4", "mkv", "avi", "mov", "webm"].includes(ext)) return "🎬";
  if (["mp3", "flac", "wav", "aac", "ogg"].includes(ext)) return "🎵";
  if (["zip", "rar", "7z", "gz", "tar"].includes(ext)) return "📦";
  if (["exe", "msi", "deb", "rpm", "dmg", "pkg"].includes(ext)) return "⚙";
  if (["pdf", "doc", "docx", "txt", "rtf"].includes(ext)) return "📄";
  return "⬇";
}

function sparklinePoints(values, width = 120, height = 30) {
  if (!Array.isArray(values) || values.length === 0) {
    return "";
  }
  const max = Math.max(...values, 1);
  const step = values.length > 1 ? width / (values.length - 1) : width;
  return values
    .map((value, index) => {
      const x = Math.round(index * step);
      const y = Math.round(height - (value / max) * (height - 4) - 2);
      return `${x},${y}`;
    })
    .join(" ");
}

async function invoke(command, args = {}) {
  return invokeTauri(command, args);
}

function panelTabClasses(active) {
  return `input-box h-8 px-2 text-xs ${active ? "border-blue-500 text-blue-300" : ""}`;
}

const VIRTUAL_CARD_HEIGHT = 238;
const VIRTUAL_OVERSCAN = 4;

export default function App() {
  const [dashboard, setDashboard] = useState(null);
  const [selectedCategory, setSelectedCategory] = useState("All Downloads");
  const [selectedDownloadId, setSelectedDownloadId] = useState(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [showAddModal, setShowAddModal] = useState(false);
  const [activityScrollTop, setActivityScrollTop] = useState(0);
  const [activityViewportHeight, setActivityViewportHeight] = useState(640);
  const [form, setForm] = useState(INITIAL_FORM);
  const [busy, setBusy] = useState(false);
  const [localStatus, setLocalStatus] = useState("Ready");
  const [activeSidePanel, setActiveSidePanel] = useState("preview");
  const [schedulerForm, setSchedulerForm] = useState(INITIAL_SCHEDULER_FORM);
  const [grabberForm, setGrabberForm] = useState(INITIAL_GRABBER_FORM);
  const [grabberResult, setGrabberResult] = useState(null);
  const [browserForm, setBrowserForm] = useState({
    integrationEnabled: true,
    interceptAllDownloads: false,
    nativeHostName: "com.loki.dm",
    nativeHostBinaryPath: "",
    chromeExtensionId: "REPLACE_WITH_EXTENSION_ID",
    firefoxExtensionId: "loki-dm@example.org",
    manifestOutputDir: "",
  });
  const [browserReport, setBrowserReport] = useState("");
  const [browserInitialized, setBrowserInitialized] = useState(false);

  const urlInputRef = useRef(null);
  const activityScrollRef = useRef(null);

  const refresh = useCallback(async () => {
    const snapshot = await invoke("dashboard_state");
    setDashboard(snapshot);
    setLocalStatus(snapshot.statusMessage || "Ready");
    setSelectedDownloadId((current) => {
      if (current && snapshot.downloads.some((row) => row.id === current)) {
        return current;
      }
      return snapshot.downloads.length ? snapshot.downloads[0].id : null;
    });
  }, []);

  useEffect(() => {
    let mounted = true;
    let timer = null;

    const tick = async () => {
      if (!mounted) return;
      try {
        await refresh();
      } catch (error) {
        if (mounted) {
          setLocalStatus(String(error));
        }
      }
      if (mounted) {
        timer = window.setTimeout(tick, 700);
      }
    };

    tick();

    return () => {
      mounted = false;
      if (timer) {
        window.clearTimeout(timer);
      }
    };
  }, [refresh]);

  useEffect(() => {
    const onEsc = (event) => {
      if (event.key === "Escape") {
        setShowAddModal(false);
      }
    };
    window.addEventListener("keydown", onEsc);
    return () => window.removeEventListener("keydown", onEsc);
  }, []);

  useEffect(() => {
    setActivityScrollTop(0);
    if (activityScrollRef.current) {
      activityScrollRef.current.scrollTop = 0;
    }
  }, [searchQuery, selectedCategory]);

  useEffect(() => {
    const node = activityScrollRef.current;
    if (!node) {
      return undefined;
    }

    const updateHeight = () => {
      setActivityViewportHeight(node.clientHeight || 640);
    };

    updateHeight();

    if (typeof ResizeObserver === "undefined") {
      return undefined;
    }

    const observer = new ResizeObserver(updateHeight);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!dashboard?.browserSettings || browserInitialized) {
      return;
    }

    setBrowserForm({
      integrationEnabled: Boolean(dashboard.browserSettings.integrationEnabled),
      interceptAllDownloads: Boolean(dashboard.browserSettings.interceptAllDownloads),
      nativeHostName: dashboard.browserSettings.nativeHostName || "com.loki.dm",
      nativeHostBinaryPath: dashboard.browserSettings.nativeHostBinaryPath || "",
      chromeExtensionId:
        dashboard.browserSettings.chromeExtensionId || "REPLACE_WITH_EXTENSION_ID",
      firefoxExtensionId:
        dashboard.browserSettings.firefoxExtensionId || "loki-dm@example.org",
      manifestOutputDir: dashboard.browserSettings.manifestOutputDir || "",
    });
    setBrowserInitialized(true);
  }, [browserInitialized, dashboard?.browserSettings]);

  useEffect(() => {
    const mode = dashboard?.themeMode?.toLowerCase() || "dark";
    if (mode === "light") {
      document.documentElement.classList.remove("dark");
      return;
    }
    if (mode === "auto") {
      const preferDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      document.documentElement.classList.toggle("dark", preferDark);
      return;
    }
    document.documentElement.classList.add("dark");
  }, [dashboard?.themeMode]);

  const filteredDownloads = useMemo(() => {
    const all = dashboard?.downloads || [];
    const category = selectedCategory;
    const search = searchQuery.trim().toLowerCase();
    const bySearch = (rows) =>
      rows.filter((row) => {
        if (!search) return true;
        return (
          row.fileName.toLowerCase().includes(search) ||
          row.url.toLowerCase().includes(search) ||
          (row.message || "").toLowerCase().includes(search)
        );
      });
    if (category === "All Downloads") {
      return bySearch(all);
    }
    if (category === "Unfinished") {
      return bySearch(all.filter((row) => ["Queued", "Downloading", "Paused", "Retrying"].includes(row.status)));
    }
    if (category === "Finished") {
      return bySearch(all.filter((row) => row.status === "Complete"));
    }
    if (category === "Queues") {
      return bySearch(all.filter((row) => row.queueItemId !== null && row.queueItemId !== undefined));
    }
    return bySearch(all.filter((row) => row.category.toLowerCase() === category.toLowerCase()));
  }, [dashboard?.downloads, searchQuery, selectedCategory]);

  const selectedDownload = useMemo(() => {
    if (!dashboard || selectedDownloadId == null) {
      return null;
    }
    return dashboard.downloads.find((row) => row.id === selectedDownloadId) || null;
  }, [dashboard, selectedDownloadId]);

  const summary = useMemo(() => {
    const all = filteredDownloads.length;
    const unfinished = filteredDownloads.filter((row) =>
      ["Queued", "Downloading", "Paused", "Retrying"].includes(row.status),
    ).length;
    const finished = filteredDownloads.filter((row) => row.status === "Complete").length;
    return { all, unfinished, finished };
  }, [filteredDownloads]);

  const virtualizedRows = useMemo(() => {
    const total = filteredDownloads.length;
    if (total === 0) {
      return {
        visible: [],
        topPadding: 0,
        bottomPadding: 0,
      };
    }

    const viewport = Math.max(200, activityViewportHeight);
    const start = Math.max(
      0,
      Math.floor(activityScrollTop / VIRTUAL_CARD_HEIGHT) - VIRTUAL_OVERSCAN,
    );
    const visibleCount =
      Math.ceil(viewport / VIRTUAL_CARD_HEIGHT) + VIRTUAL_OVERSCAN * 2;
    const end = Math.min(total, start + visibleCount);

    return {
      visible: filteredDownloads.slice(start, end),
      topPadding: start * VIRTUAL_CARD_HEIGHT,
      bottomPadding: Math.max(0, (total - end) * VIRTUAL_CARD_HEIGHT),
    };
  }, [activityScrollTop, activityViewportHeight, filteredDownloads]);

  const handleActivityScroll = useCallback((event) => {
    setActivityScrollTop(event.currentTarget.scrollTop);
  }, []);

  const runCommand = useCallback(
    async (command, args = {}) => {
      setBusy(true);
      try {
        const snapshot = await invoke(command, args);
        if (snapshot && typeof snapshot === "object" && Array.isArray(snapshot.downloads)) {
          setDashboard(snapshot);
          setLocalStatus(snapshot.statusMessage || "Ready");
        } else {
          await refresh();
        }
        return snapshot;
      } catch (error) {
        setLocalStatus(String(error));
        throw error;
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const submitDownload = useCallback(
    async (queueOnly) => {
      const url = form.url.trim();
      if (!url) {
        setLocalStatus("URL is required");
        return;
      }

      const request = {
        url,
      };

      const savePath = form.savePath.trim();
      const category = form.category.trim();
      const mirrors = parseMirrors(form.mirrors);
      const headers = parseHeaders(form.headers);
      const conn = Number(form.connections);

      if (savePath) request.savePath = savePath;
      if (category) request.category = category;
      if (mirrors.length) request.mirrors = mirrors;
      if (headers.length) request.headers = headers;
      if (Number.isFinite(conn) && conn > 0) {
        request.connections = Math.min(64, Math.max(1, Math.round(conn)));
      }

      await runCommand(queueOnly ? "queue_download" : "start_download", {
        request,
      });

      setForm((current) => ({
        ...current,
        url: "",
      }));
      setShowAddModal(false);
    },
    [form, runCommand],
  );

  const runIfSelected = useCallback(
    async (command) => {
      if (selectedDownloadId == null) {
        setLocalStatus("Select a download first");
        return;
      }
      await runCommand(command, { id: selectedDownloadId });
    },
    [runCommand, selectedDownloadId],
  );

  const stopAll = useCallback(async () => {
    if (!dashboard) {
      return;
    }
    const active = dashboard.downloads.filter((row) =>
      ["Queued", "Downloading", "Retrying"].includes(row.status),
    );
    for (const row of active) {
      await invoke("pause_download", { id: row.id });
    }
    await refresh();
  }, [dashboard, refresh]);

  const submitSchedulerJob = useCallback(async () => {
    if (!schedulerForm.url.trim()) {
      setLocalStatus("Scheduler URL is required");
      return;
    }

    const payload = {
      name: schedulerForm.name.trim() || null,
      request: {
        url: schedulerForm.url.trim(),
      },
      startInSecs: toNonNegativeInt(schedulerForm.startInSecs, 0),
      enabled: true,
    };

    if (schedulerForm.savePath.trim()) {
      payload.request.savePath = schedulerForm.savePath.trim();
    }
    if (schedulerForm.category.trim()) {
      payload.request.category = schedulerForm.category.trim();
    }

    const conn = toPositiveInt(schedulerForm.connections, 8);
    payload.request.connections = Math.min(64, conn);

    const interval = toNonNegativeInt(schedulerForm.intervalSecs, 0);
    if (interval > 0) {
      payload.intervalSecs = interval;
    }

    await runCommand("add_scheduler_job", { request: payload });
    setLocalStatus("Scheduler job added");
  }, [runCommand, schedulerForm]);

  const runSpiderScan = useCallback(async () => {
    if (!grabberForm.rootUrl.trim()) {
      setLocalStatus("Grabber root URL is required");
      return;
    }

    setBusy(true);
    try {
      const request = {
        rootUrl: grabberForm.rootUrl.trim(),
        depth: toNonNegativeInt(grabberForm.depth, 2),
        extensions: parseCsv(grabberForm.extensions),
        sameHostOnly: grabberForm.sameHostOnly,
        respectRobots: grabberForm.respectRobots,
        queueResults: grabberForm.queueResults,
        startDownloads: grabberForm.startDownloads,
        connections: toPositiveInt(grabberForm.connections, 8),
      };
      if (grabberForm.outputDir.trim()) {
        request.outputDir = grabberForm.outputDir.trim();
      }

      const result = await invoke("spider_scan", { request });
      setGrabberResult(result);
      setLocalStatus(
        `Grabber complete: ${result.uniqueUrlCount} URLs (queued ${result.queuedCount}, started ${result.startedCount})`,
      );
      await refresh();
    } catch (error) {
      setLocalStatus(String(error));
    } finally {
      setBusy(false);
    }
  }, [grabberForm, refresh]);

  const saveBrowserSettings = useCallback(async () => {
    const request = {
      integrationEnabled: browserForm.integrationEnabled,
      interceptAllDownloads: browserForm.interceptAllDownloads,
      nativeHostName: browserForm.nativeHostName,
      nativeHostBinaryPath: browserForm.nativeHostBinaryPath,
      chromeExtensionId: browserForm.chromeExtensionId,
      firefoxExtensionId: browserForm.firefoxExtensionId,
      manifestOutputDir: browserForm.manifestOutputDir,
    };
    await runCommand("update_browser_settings", { request });
    setLocalStatus("Browser settings saved");
  }, [browserForm, runCommand]);

  const runNativeHostAction = useCallback(
    async (command) => {
      setBusy(true);
      try {
        const request = {
          hostName: browserForm.nativeHostName,
          binaryPath: browserForm.nativeHostBinaryPath,
          chromeExtensionId: browserForm.chromeExtensionId,
          firefoxExtensionId: browserForm.firefoxExtensionId,
          manifestOutputDir: browserForm.manifestOutputDir,
        };
        const result = await invoke(command, { request });
        const lines = [result.message, ...(result.detailLines || [])];
        setBrowserReport(lines.join("\n"));
        setLocalStatus(result.message);
        await refresh();
      } catch (error) {
        setLocalStatus(String(error));
      } finally {
        setBusy(false);
      }
    },
    [browserForm, refresh],
  );

  const toolbarActions = {
    openAdd: () => {
      setActiveSidePanel("preview");
      setShowAddModal(true);
      window.setTimeout(() => urlInputRef.current?.focus(), 40);
    },
    resumeSelected: () => runIfSelected("resume_download"),
    pauseSelected: () => runIfSelected("pause_download"),
    stopAll: () => stopAll(),
    deleteSelected: () => runIfSelected("remove_download"),
    deleteCompleted: () => runCommand("delete_completed"),
    openOptions: () => setActiveSidePanel("browser"),
    scheduler: () => setActiveSidePanel("scheduler"),
    runQueue: () => runCommand("run_queue"),
    stopQueue: () => runCommand("stop_queue"),
    grabber: () => setActiveSidePanel("grabber"),
    share: () => window.open("https://github.com/navi/loki-dm", "_blank", "noopener,noreferrer"),
  };

  return (
    <>
      <div className="app-shell min-h-screen p-3 text-slate-100">
        <div className="mx-auto grid min-h-[calc(100vh-0.75rem)] max-w-[1880px] grid-rows-[auto_auto_auto_minmax(0,1fr)_auto] gap-3">
          <header className="surface grid grid-cols-1 gap-3 p-4 lg:grid-cols-[280px_1fr_auto]">
            <div className="flex items-center gap-3">
              <div className="grid h-10 w-10 place-items-center rounded-2xl bg-gradient-to-br from-violet-500 via-indigo-500 to-cyan-500 text-lg text-white shadow-[0_0_24px_rgba(106,154,255,.7)]">
                ⚡
              </div>
              <div>
                <div className="text-sm font-semibold tracking-wide">Loki DM</div>
                <div className="text-xs text-slate-400">High-performance Download Manager</div>
              </div>
            </div>

            <nav className="flex flex-wrap items-center gap-2 text-sm">
              {MENU_ITEMS.map((item) => (
                <button
                  key={item}
                  type="button"
                  className="rounded-lg px-2 py-1 text-slate-300 transition hover:bg-white/10 hover:text-white"
                >
                  {item}
                </button>
              ))}
            </nav>

            <div className="flex flex-wrap items-center justify-end gap-2">
              <button type="button" className="chip-btn" onClick={() => runCommand("set_theme_mode", { mode: "dark" })}>
                Dark
              </button>
              <button type="button" className="chip-btn" onClick={() => runCommand("set_theme_mode", { mode: "light" })}>
                Light
              </button>
              <button type="button" className="chip-btn" onClick={() => runCommand("set_theme_mode", { mode: "auto" })}>
                Auto
              </button>
              <div className="ml-1 rounded-xl border border-white/15 bg-white/5 px-3 py-1 text-xs text-slate-200">
                {dashboard?.queueRunning ? "Queue running" : "Queue stopped"}
              </div>
            </div>
          </header>

          <section className="surface flex flex-wrap items-center gap-2 p-2">
            {TOOLBAR_ITEMS.map((item) => (
              <button
                key={item.id}
                type="button"
                disabled={busy}
                className="toolbar-btn"
                title={item.label}
                onClick={() => toolbarActions[item.id]?.()}
              >
                <img src={item.icon} alt="" className="h-5 w-5" />
              </button>
            ))}
          </section>

          <section className="surface grid items-end gap-2 p-3 md:grid-cols-[1.2fr_auto_auto]">
            <label className="input-label">
              Search downloads
              <input
                className="input-box"
                placeholder="Search file, URL, status message…"
                value={searchQuery}
                onChange={(event) => setSearchQuery(event.target.value)}
              />
            </label>

            <button
              type="button"
              className="action-btn h-10 px-4 text-sm"
              onClick={() => setShowAddModal(true)}
            >
              Add URL
            </button>

            <div className="flex gap-2">
              <button type="button" className="chip-btn h-10 px-4" onClick={() => runCommand("run_queue")}>
                Run Queue
              </button>
              <button type="button" className="chip-btn h-10 px-4" onClick={() => runCommand("stop_queue")}>
                Stop
              </button>
            </div>
          </section>

          <main className="grid min-h-0 gap-3 xl:grid-cols-[240px_minmax(0,1fr)_380px]">
            <aside className="surface grid min-h-0 grid-rows-[auto_minmax(0,1fr)]">
              <div className="border-b border-white/10 px-4 py-3 text-sm font-semibold tracking-wide text-slate-200">Categories</div>
              <ul className="grid min-h-0 gap-1 overflow-auto p-3">
                {(dashboard?.categories || []).map((category) => (
                  <li
                    key={category.name}
                    className={`tree-row ${selectedCategory === category.name ? "active" : ""}`}
                    onClick={() => setSelectedCategory(category.name)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        setSelectedCategory(category.name);
                      }
                    }}
                    role="button"
                    tabIndex={0}
                  >
                    <img src="icons/category-folder.svg" alt="" className="h-4 w-4" />
                    <span>{category.name}</span>
                    <span className="ml-auto text-xs text-slate-400">{category.count}</span>
                  </li>
                ))}
              </ul>
            </aside>

            <section className="surface grid min-h-0 grid-rows-[auto_minmax(0,1fr)]">
              <div className="flex flex-wrap items-center justify-between gap-2 border-b border-white/10 px-4 py-3">
                <strong className="text-base text-slate-100">Activity Feed</strong>
                <span className="text-xs text-slate-300">
                  All: {summary.all} | Unfinished: {summary.unfinished} | Finished: {summary.finished}
                </span>
              </div>

              <div
                ref={activityScrollRef}
                className="activity-feed min-h-0 overflow-auto p-3"
                onScroll={handleActivityScroll}
              >
                {filteredDownloads.length === 0 && (
                  <div className="rounded-2xl border border-white/10 bg-white/5 px-4 py-5 text-sm text-slate-400">
                    No downloads in this filter.
                  </div>
                )}

                {virtualizedRows.topPadding > 0 && (
                  <div style={{ height: `${virtualizedRows.topPadding}px` }} />
                )}

                {virtualizedRows.visible.map((row) => {
                  const pct = Math.max(0, Math.min(100, Math.round((row.progress || 0) * 100)));
                  const active = row.id === selectedDownloadId;
                  const sparkline = sparklinePoints(row.speedHistory || []);
                  return (
                    <article
                      key={row.id}
                      className={`activity-card ${active ? "active" : ""}`}
                      onClick={() => setSelectedDownloadId(row.id)}
                      onDoubleClick={() => runCommand("open_output_folder", { output_path: row.outputPath })}
                    >
                      <div className="flex items-start gap-3">
                        <div className="file-pill">{iconEmoji(row.fileName)}</div>
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center justify-between gap-2">
                            <div className="min-w-0">
                              <div className="truncate text-sm font-semibold text-slate-100" title={row.outputPath}>
                                {row.fileName}
                              </div>
                              <div className="mt-0.5 truncate text-xs text-slate-400">{row.url}</div>
                            </div>
                            <span className={`rounded-lg border px-2 py-1 text-[11px] ${statusBadgeClasses(row.status)}`}>
                              {row.status}
                            </span>
                          </div>

                          <div className="mt-3 grid gap-2 md:grid-cols-[minmax(0,1fr)_130px] md:items-end">
                            <div>
                              <div className="progress-track">
                                <div className="progress-fill" style={{ width: `${pct}%` }} />
                                <div className="progress-label">
                                  {pct}% • {formatRate(row.speedBps)} • ETA {formatEta(row.etaSeconds)}
                                </div>
                              </div>
                              <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-slate-400">
                                <span>{formatBytes(Math.max(row.totalBytes || 0, row.downloadedBytes || 0))}</span>
                                <span>Conn {row.activeConnections}/{row.targetConnections}</span>
                                <span className="truncate">{row.message || "running"}</span>
                              </div>
                            </div>
                            <div className="sparkline-wrap">
                              {sparkline ? (
                                <svg viewBox="0 0 120 30" className="h-9 w-full">
                                  <defs>
                                    <linearGradient id={`spark-${row.id}`} x1="0" x2="1" y1="0" y2="0">
                                      <stop offset="0%" stopColor="#8b5cf6" />
                                      <stop offset="100%" stopColor="#22d3ee" />
                                    </linearGradient>
                                  </defs>
                                  <polyline
                                    fill="none"
                                    stroke={`url(#spark-${row.id})`}
                                    strokeWidth="2"
                                    strokeLinecap="round"
                                    strokeLinejoin="round"
                                    points={sparkline}
                                  />
                                </svg>
                              ) : (
                                <div className="text-center text-[11px] text-slate-500">waiting for speed data</div>
                              )}
                            </div>
                          </div>

                          <div className="mt-3 flex flex-wrap gap-2">
                            <button className="chip-btn px-3 py-1.5 text-xs" type="button" onClick={() => runCommand("pause_download", { id: row.id })}>
                              Pause
                            </button>
                            <button className="chip-btn px-3 py-1.5 text-xs" type="button" onClick={() => runCommand("resume_download", { id: row.id })}>
                              Resume
                            </button>
                            <button className="chip-btn px-3 py-1.5 text-xs" type="button" onClick={() => runCommand("cancel_download", { id: row.id })}>
                              Cancel
                            </button>
                            <button className="chip-btn px-3 py-1.5 text-xs" type="button" onClick={() => runCommand("open_output_folder", { output_path: row.outputPath })}>
                              Open Folder
                            </button>
                          </div>
                        </div>
                      </div>
                    </article>
                  );
                })}

                {virtualizedRows.bottomPadding > 0 && (
                  <div style={{ height: `${virtualizedRows.bottomPadding}px` }} />
                )}
              </div>
            </section>

            <aside className="surface grid min-h-0 grid-rows-[auto_auto_minmax(0,1fr)]">
              <div className="border-b border-white/10 px-4 py-3 text-sm font-semibold text-slate-100">Control Panel</div>

              <div className="m-2 mb-0 flex flex-wrap gap-1">
                {SIDE_PANELS.map((panel) => (
                  <button
                    key={panel.id}
                    type="button"
                    className={panelTabClasses(activeSidePanel === panel.id)}
                    onClick={() => setActiveSidePanel(panel.id)}
                  >
                    {panel.label}
                  </button>
                ))}
              </div>

              <div className="m-2 mt-0 grid min-h-0 overflow-auto rounded-2xl border border-white/10 bg-black/25 p-3">
                {activeSidePanel === "preview" && (
                  <div className="grid min-h-0 grid-rows-[auto_auto_auto_1fr] gap-3">
                    <div>
                      <div className="grid h-16 w-16 place-items-center rounded-2xl bg-gradient-to-br from-violet-500 via-indigo-500 to-cyan-500 text-3xl text-white shadow-[0_0_28px_rgba(116,145,255,0.55)]">
                        {iconEmoji(selectedDownload?.fileName)}
                      </div>
                      <div className="mt-3 truncate text-sm font-semibold text-slate-100" title={selectedDownload?.fileName || ""}>
                        {selectedDownload?.fileName || "No download selected"}
                      </div>
                      <div className="mt-1 text-xs text-slate-400">
                        {selectedDownload
                          ? `${selectedDownload.status} • ${formatRate(selectedDownload.speedBps)}`
                          : "Select a download card to inspect details"}
                      </div>
                    </div>

                    <div className="progress-track">
                      <div
                        className="progress-fill"
                        style={{ width: `${Math.max(0, Math.min(100, Math.round((selectedDownload?.progress || 0) * 100)))}%` }}
                      />
                    </div>

                    <div className="grid grid-cols-2 gap-2">
                      <button className="action-btn h-10 text-xs" type="button" onClick={() => runIfSelected("pause_download")}>Pause</button>
                      <button className="action-btn h-10 text-xs" type="button" onClick={() => runIfSelected("resume_download")}>Resume</button>
                      <button className="action-btn h-10 text-xs" type="button" onClick={() => runIfSelected("cancel_download")}>Cancel</button>
                      <button
                        className="action-btn h-10 text-xs"
                        type="button"
                        disabled={!selectedDownload}
                        onClick={() => selectedDownload && runCommand("open_output_folder", { output_path: selectedDownload.outputPath })}
                      >
                        Open Folder
                      </button>
                    </div>

                    <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] rounded-xl border border-white/10 bg-white/5 p-2">
                      <div className="mb-2 flex items-center justify-between text-sm font-semibold text-slate-100">
                        <span>Queue</span>
                        <div className="flex gap-1">
                          <button className="chip-btn h-8 px-2 text-xs" type="button" onClick={() => runCommand("run_queue")}>Run</button>
                          <button className="chip-btn h-8 px-2 text-xs" type="button" onClick={() => runCommand("stop_queue")}>Stop</button>
                        </div>
                      </div>
                      <ul className="grid min-h-0 gap-1 overflow-auto text-xs">
                        {(dashboard?.queueItems || []).length === 0 && (
                          <li className="rounded-md border border-white/10 px-2 py-2 text-slate-400">Queue is empty.</li>
                        )}
                        {(dashboard?.queueItems || []).map((item) => (
                          <li key={item.id} className="rounded-md border border-white/10 bg-white/5 px-2 py-2">
                            <div className="truncate font-semibold text-slate-100">#{item.id} {item.fileName}</div>
                            <div className="truncate text-slate-400">{item.category} • {item.priority} • {item.status}</div>
                            {item.lastError && <div className="truncate text-rose-400">{item.lastError}</div>}
                          </li>
                        ))}
                      </ul>
                    </div>
                  </div>
                )}

                {activeSidePanel === "scheduler" && (
                  <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] gap-2">
                    <div className="grid gap-2">
                      <label className="input-label">
                        Name
                        <input
                          className="input-box h-9"
                          value={schedulerForm.name}
                          onChange={(event) => setSchedulerForm((current) => ({ ...current, name: event.target.value }))}
                          placeholder="optional"
                        />
                      </label>
                      <label className="input-label">
                        URL
                        <input
                          className="input-box h-9"
                          value={schedulerForm.url}
                          onChange={(event) => setSchedulerForm((current) => ({ ...current, url: event.target.value }))}
                          placeholder="https://example.com/file.bin"
                        />
                      </label>
                      <div className="grid grid-cols-2 gap-2">
                        <label className="input-label">
                          Start In (sec)
                          <input
                            className="input-box h-9"
                            type="number"
                            min={0}
                            value={schedulerForm.startInSecs}
                            onChange={(event) => setSchedulerForm((current) => ({ ...current, startInSecs: event.target.value }))}
                          />
                        </label>
                        <label className="input-label">
                          Every (sec)
                          <input
                            className="input-box h-9"
                            type="number"
                            min={0}
                            value={schedulerForm.intervalSecs}
                            onChange={(event) => setSchedulerForm((current) => ({ ...current, intervalSecs: event.target.value }))}
                            placeholder="blank for once"
                          />
                        </label>
                      </div>
                      <div className="grid grid-cols-2 gap-2">
                        <label className="input-label">
                          Save Path
                          <input
                            className="input-box h-9"
                            value={schedulerForm.savePath}
                            onChange={(event) => setSchedulerForm((current) => ({ ...current, savePath: event.target.value }))}
                            placeholder="auto"
                          />
                        </label>
                        <label className="input-label">
                          Category
                          <input
                            className="input-box h-9"
                            value={schedulerForm.category}
                            onChange={(event) => setSchedulerForm((current) => ({ ...current, category: event.target.value }))}
                            placeholder="auto"
                          />
                        </label>
                      </div>
                      <div className="grid grid-cols-2 gap-2">
                        <label className="input-label">
                          Connections
                          <input
                            className="input-box h-9"
                            type="number"
                            min={1}
                            max={64}
                            value={schedulerForm.connections}
                            onChange={(event) => setSchedulerForm((current) => ({ ...current, connections: event.target.value }))}
                          />
                        </label>
                        <button
                          type="button"
                          className="action-btn mt-[18px] h-9"
                          onClick={submitSchedulerJob}
                        >
                          Add Job
                        </button>
                      </div>
                    </div>

                    <ul className="grid min-h-0 gap-1 overflow-auto text-xs">
                      {(dashboard?.schedulerJobs || []).length === 0 && (
                        <li className="rounded-md border border-white/10 px-2 py-2 text-slate-400">
                          No scheduler jobs.
                        </li>
                      )}
                      {(dashboard?.schedulerJobs || []).map((job) => (
                        <li key={job.id} className="rounded-md border border-white/10 bg-white/5 px-2 py-2">
                          <div className="truncate font-semibold text-slate-100">#{job.id} {job.name}</div>
                          <div className="truncate text-slate-400">{job.kind} • {job.url}</div>
                          <div className="truncate text-slate-400">Next: {formatEpochMs(job.nextRunEpochMs)}</div>
                          <div className="mt-1 flex gap-1">
                            <button
                              type="button"
                              className="chip-btn h-8 px-2 text-xs"
                              onClick={() => runCommand("set_scheduler_job_enabled", { id: job.id, enabled: !job.enabled })}
                            >
                              {job.enabled ? "Disable" : "Enable"}
                            </button>
                            <button
                              type="button"
                              className="chip-btn h-8 px-2 text-xs"
                              onClick={() => runCommand("remove_scheduler_job", { id: job.id })}
                            >
                              Remove
                            </button>
                          </div>
                        </li>
                      ))}
                    </ul>
                  </div>
                )}

                {activeSidePanel === "grabber" && (
                  <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] gap-2">
                    <div className="grid gap-2">
                      <label className="input-label">
                        Root URL
                        <input
                          className="input-box h-9"
                          value={grabberForm.rootUrl}
                          onChange={(event) => setGrabberForm((current) => ({ ...current, rootUrl: event.target.value }))}
                          placeholder="https://example.com"
                        />
                      </label>
                      <div className="grid grid-cols-2 gap-2">
                        <label className="input-label">
                          Depth
                          <input
                            className="input-box h-9"
                            type="number"
                            min={0}
                            max={8}
                            value={grabberForm.depth}
                            onChange={(event) => setGrabberForm((current) => ({ ...current, depth: event.target.value }))}
                          />
                        </label>
                        <label className="input-label">
                          Connections
                          <input
                            className="input-box h-9"
                            type="number"
                            min={1}
                            max={64}
                            value={grabberForm.connections}
                            onChange={(event) => setGrabberForm((current) => ({ ...current, connections: event.target.value }))}
                          />
                        </label>
                      </div>
                      <label className="input-label">
                        Extensions (csv)
                        <input
                          className="input-box h-9"
                          value={grabberForm.extensions}
                          onChange={(event) => setGrabberForm((current) => ({ ...current, extensions: event.target.value }))}
                          placeholder="zip, exe, mp4"
                        />
                      </label>
                      <label className="input-label">
                        Output Dir (optional)
                        <input
                          className="input-box h-9"
                          value={grabberForm.outputDir}
                          onChange={(event) => setGrabberForm((current) => ({ ...current, outputDir: event.target.value }))}
                          placeholder="auto"
                        />
                      </label>
                      <div className="grid grid-cols-2 gap-2 text-xs text-slate-400">
                        <label className="flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={grabberForm.sameHostOnly}
                            onChange={(event) => setGrabberForm((current) => ({ ...current, sameHostOnly: event.target.checked }))}
                          />
                          Same host only
                        </label>
                        <label className="flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={grabberForm.respectRobots}
                            onChange={(event) => setGrabberForm((current) => ({ ...current, respectRobots: event.target.checked }))}
                          />
                          Respect robots.txt
                        </label>
                        <label className="flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={grabberForm.queueResults}
                            onChange={(event) => setGrabberForm((current) => ({ ...current, queueResults: event.target.checked }))}
                          />
                          Queue results
                        </label>
                        <label className="flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={grabberForm.startDownloads}
                            onChange={(event) => setGrabberForm((current) => ({ ...current, startDownloads: event.target.checked }))}
                          />
                          Start immediately
                        </label>
                      </div>
                      <button type="button" className="action-btn h-9" onClick={runSpiderScan}>
                        Scan & Apply
                      </button>
                    </div>

                    <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] overflow-hidden rounded-md border border-white/10">
                      <div className="border-b border-white/10 px-2 py-1 text-xs text-slate-300">
                        {grabberResult
                          ? `URLs: ${grabberResult.uniqueUrlCount}, Queued: ${grabberResult.queuedCount}, Started: ${grabberResult.startedCount}, Duplicates: ${grabberResult.duplicateCount}`
                          : "No scan run yet"}
                      </div>
                      <ul className="min-h-0 overflow-auto p-2 text-xs text-slate-300">
                        {(grabberResult?.hits || []).slice(0, 200).map((hit) => (
                          <li key={`${hit.url}-${hit.depth}`} className="truncate py-0.5">
                            d{hit.depth}: {hit.url}
                          </li>
                        ))}
                      </ul>
                    </div>
                  </div>
                )}

                {activeSidePanel === "browser" && (
                  <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] gap-2">
                    <div className="grid gap-2">
                      <div className="grid grid-cols-2 gap-2 text-xs text-slate-400">
                        <label className="flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={browserForm.integrationEnabled}
                            onChange={(event) =>
                              setBrowserForm((current) => ({ ...current, integrationEnabled: event.target.checked }))
                            }
                          />
                          Integration enabled
                        </label>
                        <label className="flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={browserForm.interceptAllDownloads}
                            onChange={(event) =>
                              setBrowserForm((current) => ({ ...current, interceptAllDownloads: event.target.checked }))
                            }
                          />
                          Intercept all downloads
                        </label>
                      </div>

                      <label className="input-label">
                        Native host name
                        <input
                          className="input-box h-9"
                          value={browserForm.nativeHostName}
                          onChange={(event) =>
                            setBrowserForm((current) => ({ ...current, nativeHostName: event.target.value }))
                          }
                        />
                      </label>

                      <label className="input-label">
                        Binary path
                        <input
                          className="input-box h-9"
                          value={browserForm.nativeHostBinaryPath}
                          onChange={(event) =>
                            setBrowserForm((current) => ({ ...current, nativeHostBinaryPath: event.target.value }))
                          }
                          placeholder="auto: current executable"
                        />
                      </label>

                      <label className="input-label">
                        Chrome extension id
                        <input
                          className="input-box h-9"
                          value={browserForm.chromeExtensionId}
                          onChange={(event) =>
                            setBrowserForm((current) => ({ ...current, chromeExtensionId: event.target.value }))
                          }
                        />
                      </label>

                      <label className="input-label">
                        Firefox extension id
                        <input
                          className="input-box h-9"
                          value={browserForm.firefoxExtensionId}
                          onChange={(event) =>
                            setBrowserForm((current) => ({ ...current, firefoxExtensionId: event.target.value }))
                          }
                        />
                      </label>

                      <label className="input-label">
                        Manifest output dir
                        <input
                          className="input-box h-9"
                          value={browserForm.manifestOutputDir}
                          onChange={(event) =>
                            setBrowserForm((current) => ({ ...current, manifestOutputDir: event.target.value }))
                          }
                        />
                      </label>

                      <div className="grid grid-cols-2 gap-2">
                        <button type="button" className="chip-btn h-9" onClick={saveBrowserSettings}>Save</button>
                        <button type="button" className="chip-btn h-9" onClick={() => runNativeHostAction("generate_native_manifests")}>
                          Generate
                        </button>
                        <button type="button" className="chip-btn h-9" onClick={() => runNativeHostAction("install_native_host")}>Install</button>
                        <button type="button" className="chip-btn h-9" onClick={() => runNativeHostAction("validate_native_host")}>Validate</button>
                        <button type="button" className="chip-btn h-9 col-span-2" onClick={() => runNativeHostAction("uninstall_native_host")}>Uninstall</button>
                      </div>
                    </div>

                    <textarea
                      className="input-box min-h-[200px] h-full resize-none font-mono text-[11px]"
                      readOnly
                      value={browserReport || "Native-host report output appears here."}
                    />
                  </div>
                )}
              </div>
            </aside>
          </main>

          <footer className="surface flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-xs text-slate-400">
            <span>Filter: {selectedCategory}</span>
            <span>Downloads: {dashboard?.downloads?.length || 0}</span>
            <span>Active: {dashboard?.activeDownloads || 0}</span>
            <span>Queued: {dashboard?.queueSize || 0}</span>
            <span>Speed: {formatRate(dashboard?.globalSpeedBps || 0)}</span>
            <span className="ml-auto text-slate-200">{localStatus}</span>
          </footer>
        </div>
      </div>

      {showAddModal && (
        <div className="modal-backdrop" onClick={() => setShowAddModal(false)}>
          <div className="modal-shell" onClick={(event) => event.stopPropagation()}>
            <div className="mb-4 flex items-start justify-between gap-3">
              <div>
                <h2 className="text-lg font-semibold text-white">Add New Download</h2>
                <p className="mt-1 text-xs text-slate-300">
                  Enter URL and options. This panel appears only when needed.
                </p>
              </div>
              <button type="button" className="chip-btn px-3 py-1.5 text-xs" onClick={() => setShowAddModal(false)}>
                Close
              </button>
            </div>

            <div className="grid gap-3 md:grid-cols-2">
              <label className="input-label md:col-span-2">
                URL
                <input
                  ref={urlInputRef}
                  className="input-box"
                  placeholder="https://example.com/file.iso"
                  value={form.url}
                  onChange={(event) => setForm((current) => ({ ...current, url: event.target.value }))}
                />
              </label>

              <label className="input-label">
                Save To
                <input
                  className="input-box"
                  placeholder="auto"
                  value={form.savePath}
                  onChange={(event) => setForm((current) => ({ ...current, savePath: event.target.value }))}
                />
              </label>

              <label className="input-label">
                Category
                <input
                  className="input-box"
                  placeholder="auto"
                  value={form.category}
                  onChange={(event) => setForm((current) => ({ ...current, category: event.target.value }))}
                />
              </label>

              <label className="input-label">
                Connections
                <input
                  className="input-box"
                  type="number"
                  min={1}
                  max={64}
                  value={form.connections}
                  onChange={(event) => setForm((current) => ({ ...current, connections: event.target.value }))}
                />
              </label>

              <label className="input-label">
                Mirrors (csv)
                <input
                  className="input-box"
                  placeholder="mirror1, mirror2"
                  value={form.mirrors}
                  onChange={(event) => setForm((current) => ({ ...current, mirrors: event.target.value }))}
                />
              </label>

              <label className="input-label md:col-span-2">
                Headers
                <input
                  className="input-box"
                  placeholder="Authorization: Bearer token; Cookie: key=value"
                  value={form.headers}
                  onChange={(event) => setForm((current) => ({ ...current, headers: event.target.value }))}
                />
              </label>
            </div>

            <div className="mt-4 flex flex-wrap justify-end gap-2">
              <button type="button" className="chip-btn h-10 px-4" onClick={() => submitDownload(true)}>
                Queue
              </button>
              <button type="button" className="action-btn h-10 px-5" onClick={() => submitDownload(false)}>
                Start Download
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
