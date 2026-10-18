import { useState } from "react";
import { Modal } from "./Modal";
import { Files } from "./Files";
import { X, Folder, File, FolderOpen } from "lucide-react";
import {
  api,
  blankTask,
  taskInput,
  type Task,
  type TaskInput,
  type Operation,
  type Remote,
  type Run,
} from "./api";
export const operationText: Record<Operation, string> = {
  copy: "Copy files. Keep extra destination files.",
  sync: "Make destination match source. Extra destination files will be deleted.",
  move: "Transfer files, then remove successfully transferred source files.",
  bisync:
    "Synchronize both ways, including deletions. Initialize a baseline before scheduling.",
};
const presets = [
  ["", "Manual only"],
  ["0 * * * *", "Every hour"],
  ["0 2 * * *", "Daily at 02:00"],
  ["0 2 * * 0", "Sunday at 02:00"],
];
export function TaskEditor({
  task,
  initial,
  transfer = false,
  remotes,
  onClose,
  onSaved,
}: {
  task?: Task;
  initial?: TaskInput;
  transfer?: boolean;
  remotes: Remote[];
  onClose: () => void;
  onSaved: (run?: Run) => void;
}) {
  const [draft, setDraft] = useState<TaskInput>(
    initial || (task ? taskInput(task) : blankTask()),
  );
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<"run" | "save">(transfer ? "run" : "save");
  const [picker, setPicker] = useState<"source" | "destination" | null>(null);
  const [custom, setCustom] = useState(
    !!draft.schedule && !presets.some(([v]) => v === draft.schedule),
  );
  const locked = task?.operation === "bisync";
  const selectedCount =
    draft.selected_folders.length + draft.selected_files.length;
  const grouped = selectedCount > 0;
  const change = <K extends keyof TaskInput>(key: K, value: TaskInput[K]) =>
    setDraft({ ...draft, [key]: value });
  async function save(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const input = {
        ...draft,
        options: {
          ...draft.options,
          exclude: draft.options.exclude.filter(
            (pattern) => pattern.length > 0,
          ),
        },
      };
      if (mode === "run")
        onSaved(
          await api<Run>("/transfers", "POST", {
            ...input,
            schedule: null,
            enabled: false,
          }),
        );
      else {
        await api(
          task ? `/tasks/${task.id}` : "/tasks",
          task ? "PUT" : "POST",
          input,
        );
        onSaved();
      }
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  if (picker)
    return (
      <Modal onClose={() => setPicker(null)} title="folder-picker-title">
        <section className="dialog folder-picker">
          <Files
            remotes={remotes}
            picker={{
              field: picker,
              path: draft[picker],
              selected:
                picker === "source"
                  ? [
                      ...draft.selected_folders.map((name) => ({
                        name,
                        is_dir: true,
                      })),
                      ...draft.selected_files.map((name) => ({
                        name,
                        is_dir: false,
                      })),
                    ]
                  : [],
              requireSelection: picker === "source" && grouped,
              select: (path, entries) => {
                setDraft({
                  ...draft,
                  [picker]: path,
                  ...(picker === "source"
                    ? {
                        selected_folders: entries
                          .filter((entry) => entry.is_dir)
                          .map((entry) => entry.name),
                        selected_files: entries
                          .filter((entry) => !entry.is_dir)
                          .map((entry) => entry.name),
                      }
                    : {}),
                });
                setPicker(null);
              },
              cancel: () => setPicker(null),
            }}
          />
        </section>
      </Modal>
    );
  return (
    <Modal onClose={() => !busy && onClose()} title="editor-title">
      <section className="dialog" aria-labelledby="editor-title">
        <header>
          <div>
            <p className="eyebrow">TRANSFER TASK</p>
            <h2 id="editor-title">
              {task ? "Edit task" : transfer ? "Transfer items" : "New task"}
            </h2>
          </div>
          <button
            className="icon"
            onClick={onClose}
            title="Close"
            disabled={busy}
          >
            <X size={20} />
          </button>
        </header>
        <form onSubmit={save}>
          {transfer && !task && (
            <div
              className="segmented"
              role="group"
              aria-label="Transfer action"
            >
              <button
                type="button"
                aria-pressed={mode === "run"}
                disabled={busy || draft.operation === "bisync"}
                onClick={() => setMode("run")}
              >
                Run now
              </button>
              <button
                type="button"
                aria-pressed={mode === "save"}
                disabled={busy}
                onClick={() => setMode("save")}
              >
                Save as task
              </button>
            </div>
          )}
          <label>
            Name
            <input
              autoFocus
              required
              maxLength={200}
              value={draft.name}
              onChange={(e) => change("name", e.target.value)}
              placeholder="Daily project backup"
            />
          </label>
          <label>
            Operation
            <select
              value={draft.operation}
              disabled={locked}
              onChange={(e) =>
                setDraft({
                  ...draft,
                  operation: e.target.value as Operation,
                  destructive_confirmed: false,
                })
              }
            >
              <option value="copy">Copy</option>
              <option value="sync">Sync</option>
              <option value="move">Move</option>
              <option
                value="bisync"
                disabled={
                  mode === "run" || (!!task && task.operation !== "bisync")
                }
              >
                Bisync
              </option>
            </select>
            <span className="hint">
              {grouped && draft.operation === "sync"
                ? "Match selected files and folders at the destination. Extra files inside selected folders will be deleted. Other destination items are kept."
                : operationText[draft.operation]}
            </span>
          </label>
          {(["source", "destination"] as const).map((field) => (
            <label key={field}>
              {field === "source" ? "Source" : "Destination"}
              <div className="input-action">
                <input
                  required
                  disabled={locked}
                  readOnly={field === "source" && grouped}
                  placeholder="remote:folder or /media/folder"
                  value={draft[field]}
                  onChange={(e) => change(field, e.target.value)}
                />
                <button
                  type="button"
                  className="icon"
                  disabled={locked}
                  title={`Browse ${field}`}
                  onClick={() => setPicker(field)}
                >
                  <FolderOpen size={18} />
                </button>
              </div>
              {field === "source" && grouped && (
                <div className="folder-selection">
                  <div className="selection-heading">
                    <span>
                      {selectedCount} selected items
                    </span>
                    <button
                      type="button"
                      disabled={locked}
                      onClick={() => setPicker("source")}
                    >
                      Edit selection
                    </button>
                  </div>
                  <ul className="selected-folders">
                    {draft.selected_folders.map((name) => (
                      <li key={`folder:${name}`}>
                        <Folder size={15} />
                        {name}
                      </li>
                    ))}
                    {draft.selected_files.map((name) => (
                      <li key={`file:${name}`}>
                        <File size={15} />
                        {name}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </label>
          ))}
          {locked && (
            <p className="hint">
              Bisync endpoints and exclusions belong to its baseline. Create a
              new task to change them.
            </p>
          )}
          {mode === "save" && (
            <div className="form-grid">
              <label>
                Schedule · UTC
                <select
                  value={custom ? "custom" : draft.schedule || ""}
                  onChange={(e) => {
                    setCustom(e.target.value === "custom");
                    setDraft({
                      ...draft,
                      schedule:
                        e.target.value === "custom"
                          ? ""
                          : e.target.value || null,
                      enabled: !!e.target.value,
                    });
                  }}
                >
                  {presets.map(([value, label]) => (
                    <option key={value} value={value}>
                      {label}
                    </option>
                  ))}
                  <option value="custom">Custom cron</option>
                </select>
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={draft.enabled}
                  disabled={!draft.schedule}
                  onChange={(e) => change("enabled", e.target.checked)}
                />
                Schedule enabled
              </label>
            </div>
          )}
          {mode === "save" && custom && (
            <label>
              Cron expression · UTC
              <input
                required
                placeholder="minute hour day month weekday"
                value={draft.schedule || ""}
                onChange={(e) => change("schedule", e.target.value)}
              />
            </label>
          )}
          <details>
            <summary>Transfer options</summary>
            <div className="form-grid">
              <label>
                Concurrent file transfers
                <input
                  type="number"
                  min="1"
                  max="64"
                  required
                  value={draft.options.transfers}
                  onChange={(e) =>
                    change("options", {
                      ...draft.options,
                      transfers: Number(e.target.value),
                    })
                  }
                />
              </label>
              <label>
                Checkers
                <input
                  type="number"
                  min="1"
                  max="128"
                  required
                  value={draft.options.checkers}
                  onChange={(e) =>
                    change("options", {
                      ...draft.options,
                      checkers: Number(e.target.value),
                    })
                  }
                />
              </label>
            </div>
            <label>
              Bandwidth limit
              <input
                placeholder="Unlimited · e.g. 10M"
                value={draft.options.bandwidth_limit || ""}
                onChange={(e) =>
                  change("options", {
                    ...draft.options,
                    bandwidth_limit: e.target.value || null,
                  })
                }
              />
            </label>
            <label>
              Exclusions · one pattern per line
              <textarea
                disabled={locked}
                rows={3}
                value={draft.options.exclude.join("\n")}
                onChange={(e) =>
                  change("options", {
                    ...draft.options,
                    exclude: e.target.value.split("\n"),
                  })
                }
              />
            </label>
          </details>
          <label className="check">
            <input
              type="checkbox"
              checked={draft.dry_run}
              onChange={(e) => change("dry_run", e.target.checked)}
            />
            {mode === "run" ? "Dry run" : "Dry run by default"}
          </label>
          {draft.operation !== "copy" && (
            <label className="check warning">
              <input
                required
                type="checkbox"
                checked={draft.destructive_confirmed}
                onChange={(e) =>
                  change("destructive_confirmed", e.target.checked)
                }
              />
              I understand this operation can delete files. I authorize manual
              and scheduled runs with these settings.
            </label>
          )}
          {error && (
            <p role="alert" className="error">
              {error}
            </p>
          )}
          <footer>
            <button type="button" onClick={onClose} disabled={busy}>
              Cancel
            </button>
            <button className="primary" disabled={busy}>
              {busy
                ? mode === "run"
                  ? "Starting…"
                  : "Saving…"
                : mode === "run"
                  ? "Start transfer"
                  : "Save task"}
            </button>
          </footer>
        </form>
      </section>
    </Modal>
  );
}
