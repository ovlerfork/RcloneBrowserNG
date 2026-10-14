import { useState } from "react";
import { Modal } from "./Modal";
import { X, FolderOpen } from "lucide-react";
import {
  api,
  blankTask,
  taskInput,
  type Task,
  type TaskInput,
  type Operation,
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
  onClose,
  onSaved,
  onBrowse,
}: {
  task?: Task;
  initial?: TaskInput;
  onClose: () => void;
  onSaved: () => void;
  onBrowse: (field: "source" | "destination", draft: TaskInput) => void;
}) {
  const [draft, setDraft] = useState<TaskInput>(
    initial || (task ? taskInput(task) : blankTask()),
  );
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [custom, setCustom] = useState(
    !!draft.schedule && !presets.some(([v]) => v === draft.schedule),
  );
  const locked = task?.operation === "bisync";
  const change = <K extends keyof TaskInput>(key: K, value: TaskInput[K]) =>
    setDraft({ ...draft, [key]: value });
  async function save(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api(task ? `/tasks/${task.id}` : "/tasks", task ? "PUT" : "POST", {
        ...draft,
        options: {
          ...draft.options,
          exclude: draft.options.exclude.filter(
            (pattern) => pattern.length > 0,
          ),
        },
      });
      onSaved();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal onClose={onClose} title="editor-title">
      <section className="dialog" aria-labelledby="editor-title">
        <header>
          <div>
            <p className="eyebrow">TRANSFER TASK</p>
            <h2 id="editor-title">{task ? "Edit task" : "New task"}</h2>
          </div>
          <button className="icon" onClick={onClose} title="Close">
            <X size={20} />
          </button>
        </header>
        <form onSubmit={save}>
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
                disabled={!!task && task.operation !== "bisync"}
              >
                Bisync
              </option>
            </select>
            <span className="hint">{operationText[draft.operation]}</span>
          </label>
          {(["source", "destination"] as const).map((field) => (
            <label key={field}>
              {field === "source" ? "Source" : "Destination"}
              <div className="input-action">
                <input
                  required
                  disabled={locked}
                  placeholder="remote:folder or /media/folder"
                  value={draft[field]}
                  onChange={(e) => change(field, e.target.value)}
                />
                <button
                  type="button"
                  className="icon"
                  disabled={locked}
                  title={`Browse ${field}`}
                  onClick={() => onBrowse(field, draft)}
                >
                  <FolderOpen size={18} />
                </button>
              </div>
            </label>
          ))}
          {locked && (
            <p className="hint">
              Bisync endpoints and exclusions belong to its baseline. Create a
              new task to change them.
            </p>
          )}
          <div className="form-grid">
            <label>
              Schedule · UTC
              <select
                value={custom ? "custom" : draft.schedule || ""}
                onChange={(e) => {
                  setCustom(e.target.value === "custom");
                  change(
                    "schedule",
                    e.target.value === "custom" ? "" : e.target.value || null,
                  );
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
          {custom && (
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
                Transfers
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
            Dry run by default
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
            <button type="button" onClick={onClose}>
              Cancel
            </button>
            <button className="primary" disabled={busy}>
              {busy ? "Saving…" : "Save task"}
            </button>
          </footer>
        </form>
      </section>
    </Modal>
  );
}
