import { useEffect, useState } from "react";
import { HardDrive, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { api, type Remote } from "./api";
import { type Provider } from "./ConfigOption";
import { Modal } from "./Modal";
import { RemoteEditor, type RemoteConfig } from "./RemoteEditor";

export function Remotes({
  remotes,
  onRemotesChange,
}: {
  remotes: Remote[];
  onRemotesChange: (remotes: Remote[]) => void;
}) {
  const [providers, setProviders] = useState<Provider[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [editor, setEditor] = useState<{ remote?: RemoteConfig } | null>(null);
  const [deleting, setDeleting] = useState<Remote | null>(null);
  async function refresh() {
    setLoading(true);
    setError("");
    try {
      const [list, schema] = await Promise.all([
        api<Remote[]>("/remotes"),
        api<{ providers: Provider[] }>("/config/providers"),
      ]);
      onRemotesChange(list);
      setProviders(schema.providers);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }
  useEffect(() => {
    void refresh();
  }, []);
  async function refreshRemotes() {
    try {
      onRemotesChange(await api<Remote[]>("/remotes"));
    } catch (e) {
      setError((e as Error).message);
    }
  }
  async function edit(name: string) {
    setBusy(true);
    setError("");
    try {
      setEditor({
        remote: await api<RemoteConfig>(
          `/config/remotes/${encodeURIComponent(name)}`,
        ),
      });
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function remove() {
    if (!deleting) return;
    setBusy(true);
    setError("");
    try {
      await api(
        `/config/remotes/${encodeURIComponent(deleting.name)}`,
        "DELETE",
      );
      setDeleting(null);
      await refreshRemotes();
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
          <p className="eyebrow">STORAGE CONNECTIONS</p>
          <h1>Remotes</h1>
        </div>
        <div className="actions">
          <button
            className="icon"
            title="Refresh remotes"
            disabled={loading || busy}
            onClick={() => void refresh()}
          >
            <RefreshCw size={18} />
          </button>
          <button
            className="primary"
            disabled={loading || busy || !providers.length}
            onClick={() => setEditor({})}
          >
            <Plus size={18} />
            New remote
          </button>
        </div>
      </div>
      {error && !deleting && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      {loading ? (
        <div className="empty">Loading configuration…</div>
      ) : !remotes.length ? (
        <div className="empty">
          <HardDrive size={32} />
          <h2>No remotes configured</h2>
        </div>
      ) : (
        <div className="remote-list">
          {remotes.map((remote) => (
            <article className="remote-row" key={remote.name}>
              <HardDrive size={21} />
              <div>
                <h3>{remote.name}</h3>
                <span className="hint">
                  {providers.find((p) => p.Name === remote.type)?.Description ??
                    remote.type}
                </span>
              </div>
              <div className="actions">
                <button
                  className="icon"
                  title={`Edit ${remote.name}`}
                  disabled={busy || !providers.length}
                  onClick={() => void edit(remote.name)}
                >
                  <Pencil size={17} />
                </button>
                <button
                  className="icon danger"
                  title={`Delete ${remote.name}`}
                  disabled={busy}
                  onClick={() => {
                    setDeleting(remote);
                    setError("");
                  }}
                >
                  <Trash2 size={17} />
                </button>
              </div>
            </article>
          ))}
        </div>
      )}
      {editor && (
        <RemoteEditor
          remote={editor.remote}
          providers={providers}
          onClose={() => setEditor(null)}
          onChanged={() => void refreshRemotes()}
        />
      )}
      {deleting && (
        <Modal
          confirmation
          title="delete-remote-title"
          onClose={() => !busy && setDeleting(null)}
        >
          <section className="dialog small">
            <h2 id="delete-remote-title">Delete remote</h2>
            <p className="warning">
              Remove “{deleting.name}” from rclone configuration? Stored
              credentials will be removed. Files in storage remain, but tasks
              using this remote will fail until it is configured again.
            </p>
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            <footer>
              <button disabled={busy} onClick={() => setDeleting(null)}>
                Cancel
              </button>
              <button
                className="destructive"
                disabled={busy}
                onClick={() => void remove()}
              >
                {busy ? "Deleting…" : "Delete remote"}
              </button>
            </footer>
          </section>
        </Modal>
      )}
    </>
  );
}
