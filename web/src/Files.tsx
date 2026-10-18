import { useEffect, useRef, useState } from "react";
import { Modal } from "./Modal";
import {
  Folder,
  File,
  ArrowUp,
  RefreshCw,
  Plus,
  Trash2,
  ArrowRight,
} from "lucide-react";
import { api, bytes, date, type Entry, type Remote } from "./api";
const join = (path: string, name: string) =>
  path + (path.endsWith("/") || path.endsWith(":") ? "" : "/") + name;
function crumbs(path: string) {
  const remote = !path.startsWith("/");
  const index = path.indexOf(":");
  const root = remote ? path.slice(0, index + 1) : "/";
  const parts = (remote ? path.slice(index + 1) : path.slice(1))
    .split("/")
    .filter(Boolean);
  return [
    { name: root, path: root },
    ...parts.map((name, i) => ({
      name,
      path: join(root, parts.slice(0, i + 1).join("/")),
    })),
  ];
}
export function Files({
  remotes,
  picker,
  onTransfer,
}: {
  remotes: Remote[];
  picker?: {
    field: "source" | "destination";
    path: string;
    selectedFolders: string[];
    requireSelection: boolean;
    select: (path: string, folders: string[]) => void;
    cancel: () => void;
  };
  onTransfer?: (path: string, folders: string[]) => void;
}) {
  const [path, setPath] = useState(picker?.path || "/media");
  const [input, setInput] = useState(path);
  const [selected, setSelected] = useState<string[]>(
    picker?.selectedFolders || [],
  );
  const selectAll = useRef<HTMLInputElement>(null);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [mutation, setMutation] = useState<{
    kind: "mkdir" | "delete" | "move";
    entry?: Entry;
  } | null>(null);
  const [value, setValue] = useState("");
  function go(next: string) {
    if (next !== path) setSelected([]);
    setPath(next);
    setInput(next);
  }
  useEffect(() => {
    let valid = true;
    setLoading(true);
    setEntries([]);
    setError("");
    api<Entry[]>(`/files?path=${encodeURIComponent(path)}`)
      .then((data) => {
        if (valid)
          setEntries(
            data.sort(
              (a, b) =>
                Number(b.is_dir) - Number(a.is_dir) ||
                a.name.localeCompare(b.name),
            ),
          );
      })
      .catch((e) => {
        if (valid) {
          setError(e.message);
          setEntries([]);
        }
      })
      .finally(() => {
        if (valid) setLoading(false);
      });
    return () => {
      valid = false;
    };
  }, [path, revision]);
  const trail = crumbs(path);
  const selectable = picker?.field !== "destination";
  const folders = entries.filter((entry) => entry.is_dir);
  const allSelected =
    folders.length > 0 &&
    folders.every((entry) => selected.includes(entry.name));
  useEffect(() => {
    if (selectAll.current)
      selectAll.current.indeterminate = selected.length > 0 && !allSelected;
  }, [selected, allSelected]);
  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!mutation) return;
    setBusy(true);
    setError("");
    try {
      const entry = mutation.entry;
      if (mutation.kind === "mkdir") {
        const created = join(path, value);
        await api("/files/mkdir", "POST", { path: created });
        if (picker) go(created);
      } else if (mutation.kind === "delete" && entry)
        await api("/files/delete", "POST", {
          path: entry.path,
          is_dir: entry.is_dir,
          confirmed: true,
        });
      else if (entry)
        await api("/files/move", "POST", {
          source: entry.path,
          destination: value,
          is_dir: entry.is_dir,
          confirmed: true,
        });
      setMutation(null);
      setRevision((v) => v + 1);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <div className="section-heading">
        <div>
          <p className="eyebrow">STORAGE</p>
          {picker ? (
            <h2 id="folder-picker-title">Choose {picker.field}</h2>
          ) : (
            <h1>File browser</h1>
          )}
        </div>
        <button
          onClick={() => {
            setMutation({ kind: "mkdir" });
            setValue("");
          }}
          disabled={loading}
        >
          <Plus size={17} />
          New folder
        </button>
      </div>
      {picker && (
        <div className="selection-banner">
          <span>
            {selectable && selected.length
              ? `${selected.length} folders selected`
              : `Select ${picker.field} folder`}
          </span>
          <div>
            <button onClick={picker.cancel}>Cancel</button>
            <button
              className="primary"
              disabled={
                loading || (picker.requireSelection && !selected.length)
              }
              onClick={() => picker.select(path, selected)}
            >
              {selectable && (selected.length || picker.requireSelection)
                ? "Use selected folders"
                : "Use this folder"}
            </button>
          </div>
        </div>
      )}
      <div className="locations">
        <button onClick={() => go("/media")}>
          <Folder size={16} />
          Local files
        </button>
        {remotes.map((remote) => (
          <button key={remote.name} onClick={() => go(`${remote.name}:`)}>
            <Folder size={16} />
            {remote.name}
            <span className="hint">{remote.type}</span>
          </button>
        ))}
      </div>
      <form
        className="path-bar"
        onSubmit={(e) => {
          e.preventDefault();
          go(input);
        }}
      >
        <button
          type="button"
          className="icon"
          title="Parent folder"
          disabled={trail.length < 2}
          onClick={() => go(trail[trail.length - 2].path)}
        >
          <ArrowUp size={18} />
        </button>
        <input
          aria-label="Folder path"
          required
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <button>Open</button>
        <button
          type="button"
          className="icon"
          title="Refresh files"
          onClick={() => setRevision((v) => v + 1)}
        >
          <RefreshCw size={17} />
        </button>
      </form>
      <nav className="breadcrumbs" aria-label="Folder breadcrumb">
        {trail.map((crumb) => (
          <button key={crumb.path} onClick={() => go(crumb.path)}>
            {crumb.name}
          </button>
        ))}
      </nav>
      {selectable && (
        <div className="file-selection">
          <span>{selected.length} folders selected</span>
          <div className="actions">
            <button onClick={() => setSelected([])} disabled={!selected.length}>
              Clear
            </button>
            {!picker && (
              <button
                className="primary"
                disabled={!selected.length || loading}
                onClick={() => onTransfer?.(path, selected)}
              >
                <ArrowRight size={16} />
                Transfer
              </button>
            )}
          </div>
        </div>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {loading ? (
        <div className="empty">Loading files…</div>
      ) : (
        <div className="table-wrap">
          <table className="file-table">
            <thead>
              <tr>
                {selectable && (
                  <th className="selection-cell">
                    <input
                      ref={selectAll}
                      type="checkbox"
                      aria-label="Select all folders"
                      checked={allSelected}
                      disabled={!folders.length}
                      onChange={(event) =>
                        setSelected(
                          event.target.checked
                            ? folders.map((entry) => entry.name)
                            : [],
                        )
                      }
                    />
                  </th>
                )}
                <th>Name</th>
                <th className="file-size">Size</th>
                <th className="file-modified">Modified</th>
                {!picker && <th aria-label="Actions" />}
              </tr>
            </thead>
            <tbody>
              {entries.map((entry) => (
                <tr
                  key={entry.path}
                  className={selected.includes(entry.name) ? "selected" : ""}
                >
                  {selectable && (
                    <td className="selection-cell">
                      {entry.is_dir && (
                        <input
                          type="checkbox"
                          aria-label={`Select ${entry.name}`}
                          checked={selected.includes(entry.name)}
                          onChange={(event) =>
                            setSelected(
                              event.target.checked
                                ? [...selected, entry.name]
                                : selected.filter(
                                    (name) => name !== entry.name,
                                  ),
                            )
                          }
                        />
                      )}
                    </td>
                  )}
                  <td>
                    {entry.is_dir ? (
                      <button
                        className="file-name"
                        onClick={() => go(entry.path)}
                      >
                        <Folder size={19} />
                        {entry.name}
                      </button>
                    ) : (
                      <span className="file-name">
                        <File size={18} />
                        {entry.name}
                      </span>
                    )}
                  </td>
                  <td className="file-size">
                    {entry.is_dir ? "—" : bytes(entry.size)}
                  </td>
                  <td className="file-modified">{date(entry.modified)}</td>
                  {!picker && (
                    <td>
                      <div className="actions">
                        <button
                          className="icon"
                          title={`Move ${entry.name}`}
                          onClick={() => {
                            setMutation({ kind: "move", entry });
                            setValue(entry.path);
                          }}
                        >
                          <ArrowRight size={16} />
                        </button>
                        <button
                          className="icon danger"
                          title={`Delete ${entry.name}`}
                          onClick={() => setMutation({ kind: "delete", entry })}
                        >
                          <Trash2 size={16} />
                        </button>
                      </div>
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
          {!entries.length && !error && (
            <div className="empty">This folder is empty.</div>
          )}
        </div>
      )}
      {mutation && (
        <Modal onClose={() => !busy && setMutation(null)} title="file-action">
          <section className="dialog small" aria-labelledby="file-action">
            <h2 id="file-action">
              {mutation.kind === "mkdir"
                ? "New folder"
                : mutation.kind === "move"
                  ? "Move item"
                  : "Delete item"}
            </h2>
            <form onSubmit={submit}>
              {mutation.entry && (
                <p className="path-text">{mutation.entry.path}</p>
              )}
              {mutation.kind === "delete" ? (
                <p className="warning">
                  Permanently delete{" "}
                  {mutation.entry?.is_dir
                    ? "this folder and all its contents"
                    : "this file"}
                  ? This cannot be undone.
                </p>
              ) : (
                <label>
                  {mutation.kind === "mkdir"
                    ? "Folder name"
                    : "Full destination path"}
                  <input
                    autoFocus
                    required
                    value={value}
                    onChange={(e) => setValue(e.target.value)}
                    pattern={mutation.kind === "mkdir" ? "[^/]+" : undefined}
                  />
                </label>
              )}
              {mutation.kind === "move" && (
                <p className="warning">
                  Move queues a transfer and removes transferred source files.
                  Existing destination files may be replaced.
                </p>
              )}
              {error && (
                <p className="error" role="alert">
                  {error}
                </p>
              )}
              <footer>
                <button type="button" onClick={() => setMutation(null)}>
                  Cancel
                </button>
                <button
                  className={
                    mutation.kind === "delete" ? "destructive" : "primary"
                  }
                  disabled={busy}
                >
                  {busy
                    ? "Working…"
                    : mutation.kind === "delete"
                      ? "Delete permanently"
                      : mutation.kind === "move"
                        ? "Confirm move"
                        : "Create folder"}
                </button>
              </footer>
            </form>
          </section>
        </Modal>
      )}
    </>
  );
}
