import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  ArrowRight,
  ArrowRightLeft,
  Boxes,
  Clock3,
  FolderOpen,
  HardDrive,
  Plus,
  Play,
  Pencil,
  Trash2,
  FlaskConical,
  X,
  Square,
  RefreshCw,
} from "lucide-react";
import {
  api,
  active,
  bytes,
  date,
  taskInput,
  type Task,
  type TaskInput,
  type Run,
  type Remote,
  type Settings,
} from "./api";
import { TaskEditor } from "./TaskEditor";
import { Files } from "./Files";
import { Remotes } from "./Remotes";
import { Modal } from "./Modal";
import "./style.css";
function App() {
  const [view, setView] = useState<"tasks" | "runs" | "files" | "remotes">(
    "tasks",
  );
  const [tasks, setTasks] = useState<Task[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  const [remotes, setRemotes] = useState<Remote[]>([]);
  const [settings, setSettings] = useState<Settings>();
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [editor, setEditor] = useState<{
    task?: Task;
    initial?: TaskInput;
  } | null>(null);
  const [picker, setPicker] = useState<{
    field: "source" | "destination";
    draft: TaskInput;
    task?: Task;
  } | null>(null);
  const [detail, setDetail] = useState<Run | null>(null);
  const [filter, setFilter] = useState("");
  const [confirm, setConfirm] = useState<{
    title: string;
    message: string;
    action: () => Promise<void>;
  } | null>(null);
  async function refresh() {
    const [t, r] = await Promise.all([
      api<Task[]>("/tasks"),
      api<Run[]>("/runs?limit=500"),
    ]);
    setTasks(t);
    setRuns(r);
  }
  useEffect(() => {
    let live = true;
    async function tick() {
      try {
        await refresh();
      } catch (e) {
        if (live) setError((e as Error).message);
      } finally {
        if (live) setLoading(false);
      }
    }
    void tick();
    api<Remote[]>("/remotes")
      .then(setRemotes)
      .catch((e) => setError(e.message));
    api<Settings>("/settings")
      .then(setSettings)
      .catch((e) => setError(e.message));
    const timer = setInterval(tick, 3000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, []);
  useEffect(() => {
    if (!detail) return;
    let live = true;
    const id = detail.id;
    async function tick() {
      try {
        const run = await api<Run>(`/runs/${id}`);
        if (live) setDetail(run);
      } catch (e) {
        if (live) setError((e as Error).message);
      }
    }
    void tick();
    const timer = setInterval(tick, 1500);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [detail?.id]);
  async function perform(action: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      await action();
      await refresh();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function launch(task: Task, dry?: boolean, resync = false) {
    const run = await api<Run>(`/tasks/${task.id}/run`, "POST", {
      ...(dry === undefined ? {} : { dry_run: dry }),
      resync,
      confirmed: resync,
    });
    setDetail(run);
  }
  const running = runs.filter(active);
  const visibleRuns = runs.filter((r) => !filter || r.task_id === filter);
  return (
    <div className="shell">
      <aside>
        <a
          className="brand"
          href="#"
          onClick={(e) => {
            e.preventDefault();
            setView("tasks");
          }}
        >
          <span className="brand-mark">
            <ArrowRightLeft size={22} />
          </span>
          <span>
            rclone<span className="brand-sub">BROWSER NG</span>
          </span>
        </a>
        <p className="nav-caption">WORKSPACE</p>
        <nav>
          {(
            [
              ["tasks", Boxes, "Tasks"],
              ["runs", Clock3, "Activity"],
              ["files", FolderOpen, "Files"],
              ["remotes", HardDrive, "Remotes"],
            ] as const
          ).map(([id, Icon, label]) => (
            <button
              key={id}
              className={view === id ? "selected" : ""}
              onClick={() => setView(id)}
            >
              <Icon size={19} />
              {label}
              {id === "runs" && running.length > 0 && (
                <span className="nav-count">{running.length}</span>
              )}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <span
            className={`dot ${settings?.rclone_available ? "online" : ""}`}
          />
          {settings
            ? settings.rclone_available
              ? "Rclone connected"
              : "Rclone unavailable"
            : "Connecting…"}
          <p>Transfer queue · UTC</p>
        </div>
      </aside>
      <main>
        <div className="topbar">
          <span>
            Workspace /{" "}
            {view === "runs"
              ? "Activity"
              : view === "files"
                ? "Files"
                : view === "remotes"
                  ? "Remotes"
                  : "Tasks"}
          </span>
          <span className="top-status">
            <span className="dot online" />
            {running.length} active
          </span>
        </div>
        <div className="content">
          {error && (
            <div className="error" role="alert">
              {error}
            </div>
          )}
          {view === "tasks" && (
            <>
              <div className="section-heading">
                <div>
                  <p className="eyebrow">TRANSFER CONTROL</p>
                  <h1>Your tasks</h1>
                  <p className="subtitle">Move your files forward.</p>
                </div>
                <button className="primary" onClick={() => setEditor({})}>
                  <Plus size={18} />
                  New task
                </button>
              </div>
              <div className="stats">
                <div>
                  <span>Saved tasks</span>
                  <strong>{tasks.length.toString().padStart(2, "0")}</strong>
                </div>
                <div>
                  <span>In progress</span>
                  <strong>{running.length.toString().padStart(2, "0")}</strong>
                </div>
                <div>
                  <span>Scheduled</span>
                  <strong>
                    {tasks
                      .filter((t) => t.enabled && t.schedule)
                      .length.toString()
                      .padStart(2, "0")}
                  </strong>
                </div>
              </div>
              <div className="list-heading">
                <h2>
                  All tasks <span>{tasks.length}</span>
                </h2>
                <span>Schedules run in UTC</span>
              </div>
              {loading ? (
                <div className="empty">Loading tasks…</div>
              ) : !tasks.length ? (
                <div className="empty">
                  <Boxes size={32} />
                  <h2>Your next transfer starts here</h2>
                  <p>Create a task to copy, sync, or move files.</p>
                  <button className="primary" onClick={() => setEditor({})}>
                    <Plus size={17} />
                    New task
                  </button>
                </div>
              ) : (
                <div className="task-list">
                  {tasks.map((task) => {
                    const current = running.find((r) => r.task_id === task.id);
                    const last = runs.find((r) => r.task_id === task.id);
                    return (
                      <article className="task" key={task.id}>
                        <div className="task-heading">
                          <div className="task-title">
                            <span className={`operation ${task.operation}`}>
                              <ArrowRightLeft size={19} />
                            </span>
                            <div>
                              <h3>{task.name}</h3>
                              <span className="operation-label">
                                {task.operation}
                                {task.dry_run ? " · dry run" : ""}
                              </span>
                            </div>
                          </div>
                          <div className="actions">
                            <button
                              className="icon"
                              title={`Edit ${task.name}`}
                              onClick={() => setEditor({ task })}
                            >
                              <Pencil size={16} />
                            </button>
                            <button
                              className="icon danger"
                              disabled={busy || !!current}
                              title={`Delete ${task.name}`}
                              onClick={() =>
                                setConfirm({
                                  title: "Delete task",
                                  message: `Delete “${task.name}”? Run history will be retained.`,
                                  action: async () => {
                                    await api(`/tasks/${task.id}`, "DELETE");
                                  },
                                })
                              }
                            >
                              <Trash2 size={16} />
                            </button>
                          </div>
                        </div>
                        <div className="route">
                          <div>
                            <span>SOURCE</span>
                            <p>{task.source}</p>
                          </div>
                          <ArrowRight size={18} />
                          <div>
                            <span>DESTINATION</span>
                            <p>{task.destination}</p>
                          </div>
                        </div>
                        <div className="task-footer">
                          <div className="schedule">
                            <Clock3 size={15} />
                            <span>{task.schedule || "Manual only"}</span>
                            {task.schedule && (
                              <label className="toggle" title="Enable schedule">
                                <input
                                  aria-label={`Enable schedule for ${task.name}`}
                                  type="checkbox"
                                  checked={task.enabled}
                                  disabled={busy}
                                  onChange={(e) =>
                                    void perform(() =>
                                      api(`/tasks/${task.id}`, "PUT", {
                                        ...taskInput(task),
                                        enabled: e.target.checked,
                                      }),
                                    )
                                  }
                                />
                                <span />
                              </label>
                            )}
                          </div>
                          <div className="actions">
                            {last && (
                              <button
                                className={`badge ${last.status}`}
                                onClick={() => setDetail(last)}
                              >
                                {last.status}
                              </button>
                            )}
                            {task.operation === "bisync" && (
                              <button
                                disabled={busy || !!current}
                                title="Initialize or rebuild bisync baseline"
                                onClick={() =>
                                  setConfirm({
                                    title: "Initialize bisync",
                                    message:
                                      "This runs bisync with resync and rebuilds its baseline. Both locations may be modified and conflicting files replaced. Review and back up both locations first. Continue?",
                                    action: () => launch(task, false, true),
                                  })
                                }
                              >
                                <RefreshCw size={15} />
                                Initialize
                              </button>
                            )}
                            <button
                              disabled={busy || !!current}
                              title="Preview without changing files"
                              onClick={() =>
                                void perform(() => launch(task, true))
                              }
                            >
                              <FlaskConical size={15} />
                              Dry run
                            </button>
                            <button
                              className="primary"
                              disabled={busy || !!current}
                              onClick={() => void perform(() => launch(task))}
                            >
                              <Play size={14} />
                              {current ? "Active" : "Run"}
                            </button>
                          </div>
                        </div>
                      </article>
                    );
                  })}
                </div>
              )}
            </>
          )}
          {view === "runs" && (
            <>
              <div className="section-heading">
                <div>
                  <p className="eyebrow">EXECUTION HISTORY</p>
                  <h1>Activity</h1>
                </div>
                <select
                  aria-label="Filter by task"
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                >
                  <option value="">All tasks</option>
                  {tasks.map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.name}
                    </option>
                  ))}
                </select>
              </div>
              <div className="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>Task</th>
                      <th>Status</th>
                      <th>Transferred</th>
                      <th>Queued</th>
                      <th />
                    </tr>
                  </thead>
                  <tbody>
                    {visibleRuns.map((run) => (
                      <tr key={run.id}>
                        <td>
                          <button
                            className="text-link"
                            onClick={() => setDetail(run)}
                          >
                            {run.task_name}
                          </button>
                          <span className="cell-sub">
                            {run.operation} ·{" "}
                            {run.dry_run
                              ? "dry run"
                              : run.resync
                                ? "initialization"
                                : run.trigger}
                          </span>
                        </td>
                        <td>
                          <span className={`badge ${run.status}`}>
                            {run.status}
                          </span>
                        </td>
                        <td>{bytes(run.progress?.bytes)}</td>
                        <td>{date(run.queued_at)}</td>
                        <td>
                          <button
                            className="icon"
                            title="Run details"
                            onClick={() => setDetail(run)}
                          >
                            <ArrowRight size={17} />
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {!visibleRuns.length && (
                  <div className="empty">
                    {loading ? "Loading activity…" : "No runs yet."}
                  </div>
                )}
              </div>
              <p className="hint">Showing the latest 500 runs.</p>
            </>
          )}
          {view === "remotes" && (
            <Remotes remotes={remotes} onRemotesChange={setRemotes} />
          )}
          {view === "files" && (
            <Files
              remotes={remotes}
              choose={
                picker
                  ? {
                      field: picker.field,
                      select: (path) => {
                        setEditor({
                          task: picker.task,
                          initial: { ...picker.draft, [picker.field]: path },
                        });
                        setPicker(null);
                        setView("tasks");
                      },
                    }
                  : undefined
              }
              cancelChoose={() => {
                if (picker)
                  setEditor({ task: picker.task, initial: picker.draft });
                setPicker(null);
                setView("tasks");
              }}
            />
          )}
        </div>
      </main>
      {editor && (
        <TaskEditor
          task={editor.task}
          initial={editor.initial}
          onClose={() => setEditor(null)}
          onSaved={() => {
            setEditor(null);
            void perform(refresh);
          }}
          onBrowse={(field, draft) => {
            setPicker({ field, draft, task: editor.task });
            setEditor(null);
            setView("files");
          }}
        />
      )}
      {detail && (
        <Modal onClose={() => setDetail(null)} title="run-title">
          <section className="dialog run-detail" aria-labelledby="run-title">
            <header>
              <div>
                <p className="eyebrow">RUN DETAILS</p>
                <h2 id="run-title">{detail.task_name}</h2>
              </div>
              <button
                className="icon"
                title="Close"
                onClick={() => setDetail(null)}
              >
                <X size={20} />
              </button>
            </header>
            <div className="detail-summary">
              <span className={`badge ${detail.status}`}>{detail.status}</span>
              <span>
                {detail.operation} ·{" "}
                {detail.dry_run
                  ? "dry run"
                  : detail.resync
                    ? "initialization"
                    : detail.trigger}
              </span>
              {detail.exit_code !== null && (
                <span>Exit {detail.exit_code}</span>
              )}
            </div>
            <p className="path-text">
              {detail.source}
              <br />
              <ArrowRight size={14} />
              <br />
              {detail.destination}
            </p>
            <div className="run-metrics">
              <div>
                <span>Transferred</span>
                <strong>
                  {bytes(detail.progress?.bytes)} /{" "}
                  {bytes(detail.progress?.totalBytes)}
                </strong>
              </div>
              <div>
                <span>Speed</span>
                <strong>{bytes(detail.progress?.speed)}/s</strong>
              </div>
              <div>
                <span>Errors</span>
                <strong>{detail.progress?.errors ?? "—"}</strong>
              </div>
            </div>
            {!!detail.progress?.totalBytes && (
              <progress
                max={detail.progress.totalBytes}
                value={detail.progress.bytes || 0}
              />
            )}
            <p className="hint">
              Queued {date(detail.queued_at)}
              {detail.finished_at
                ? ` · Finished ${date(detail.finished_at)}`
                : ""}
            </p>
            {detail.error && <p className="error">{detail.error}</p>}
            <h3>Execution log</h3>
            <pre className="logs" aria-label="Execution log">
              {detail.logs.length
                ? detail.logs
                    .map(
                      (log) =>
                        `${date(log.time)} [${log.level}] ${log.message}`,
                    )
                    .join("\n")
                : "Waiting for log output…"}
            </pre>
            <footer>
              {active(detail) && (
                <button
                  className="destructive"
                  disabled={busy || detail.cancel_requested}
                  onClick={() =>
                    setConfirm({
                      title: "Cancel run",
                      message:
                        "Stop this transfer? Files already copied, moved, or deleted will not be restored.",
                      action: async () => {
                        setDetail(
                          await api<Run>(`/runs/${detail.id}/cancel`, "POST"),
                        );
                      },
                    })
                  }
                >
                  <Square size={14} />
                  {detail.cancel_requested ? "Cancelling…" : "Cancel run"}
                </button>
              )}
              <button onClick={() => setDetail(null)}>Close</button>
            </footer>
          </section>
        </Modal>
      )}
      {confirm && (
        <Modal
          onClose={() => !busy && setConfirm(null)}
          title="confirm-title"
          confirmation
        >
          <section className="dialog small" aria-labelledby="confirm-title">
            <h2 id="confirm-title">{confirm.title}</h2>
            <p>{confirm.message}</p>
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            <footer>
              <button onClick={() => setConfirm(null)} disabled={busy}>
                Go back
              </button>
              <button
                className="destructive"
                disabled={busy}
                onClick={() =>
                  void perform(async () => {
                    await confirm.action();
                    setConfirm(null);
                  })
                }
              >
                {busy ? "Working…" : "Confirm"}
              </button>
            </footer>
          </section>
        </Modal>
      )}
    </div>
  );
}
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
