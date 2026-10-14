import { useState } from "react";
import { Plus, Search, X } from "lucide-react";
import { api } from "./api";
import { Modal } from "./Modal";
import {
  ConfigOption,
  OptionInput,
  defaultValue,
  matchesProvider,
  type Provider,
  type RcloneOption,
} from "./ConfigOption";

export interface RemoteConfig {
  name: string;
  type: string;
  parameters: Record<string, string>;
  secrets: string[];
}
interface Wizard {
  name: string;
  session_id: string | null;
  option: RcloneOption | null;
  error: string | null;
  complete: boolean;
}

export function RemoteEditor({
  remote,
  providers,
  onClose,
  onChanged,
}: {
  remote?: RemoteConfig;
  providers: Provider[];
  onClose: () => void;
  onChanged: () => void;
}) {
  const [name, setName] = useState(remote?.name ?? "");
  const [kind, setKind] = useState(remote?.type ?? "");
  const [providerSearch, setProviderSearch] = useState("");
  const [optionSearch, setOptionSearch] = useState("");
  const [advanced, setAdvanced] = useState(false);
  const [custom, setCustom] = useState<string[]>([]);
  const [customKey, setCustomKey] = useState("");
  const [changes, setChanges] = useState<Record<string, string | null>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [wizard, setWizard] = useState<Wizard | null>(null);
  const [answer, setAnswer] = useState("");
  const [partial, setPartial] = useState(false);
  const [closing, setClosing] = useState(false);
  const provider = providers.find((p) => p.Name === kind);
  const options = provider?.Options ?? [];
  const providerOption = options.find((option) => option.Name === "provider");
  const providerName =
    changes.provider === null
      ? ""
      : (changes.provider ??
        remote?.parameters.provider ??
        (providerOption ? defaultValue(providerOption) : ""));
  const changedCount = Object.keys(changes).length;
  const storedKeys = [
    ...Object.keys(remote?.parameters ?? {}),
    ...(remote?.secrets ?? []),
  ];
  const shownInSchema = (option: RcloneOption) =>
    !((option.Hide ?? 0) & 2) && matchesProvider(option.Provider, providerName);
  const extraKeys = [
    ...new Set([...storedKeys, ...Object.keys(changes), ...custom]),
  ].filter(
    (key) =>
      !options.some((option) => option.Name === key && shownInSchema(option)),
  );
  const visibleOptions = options.filter(
    (option) =>
      !((option.Hide ?? 0) & 2) &&
      matchesProvider(option.Provider, providerName) &&
      (!option.Advanced || advanced || !!optionSearch) &&
      (!optionSearch ||
        `${option.Name} ${option.Help}`
          .toLowerCase()
          .includes(optionSearch.toLowerCase())),
  );
  function change(key: string, value: string | null | undefined) {
    setChanges((previous) => {
      const next = { ...previous };
      if (
        value === undefined ||
        (value === remote?.parameters[key] && !remote?.secrets.includes(key))
      )
        delete next[key];
      else return { ...previous, [key]: value };
      return next;
    });
  }
  function close() {
    if (busy) return;
    if (partial || changedCount || (!remote && name)) setClosing(true);
    else onClose();
  }
  async function submit(event: React.SyntheticEvent, configure = false) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      let result: Wizard;
      if (wizard) {
        if (!wizard.session_id || !wizard.option)
          throw new Error(
            "Configuration cannot continue. Close and reopen the remote to retry.",
          );
        result = await api<Wizard>(
          `/config/remotes/${encodeURIComponent(wizard.name)}/continue`,
          "POST",
          {
            session_id: wizard.session_id,
            answer: answer || defaultValue(wizard.option),
          },
        );
      } else {
        const parameters = Object.fromEntries(
          Object.entries(changes).filter(
            (entry): entry is [string, string] => entry[1] !== null,
          ),
        );
        const unset = Object.entries(changes)
          .filter(([, value]) => value === null)
          .map(([key]) => key);
        result = remote
          ? await api<Wizard>(
              `/config/remotes/${encodeURIComponent(name)}`,
              "PUT",
              { parameters, unset, ...(configure ? { configure: true } : {}) },
            )
          : await api<Wizard>("/config/remotes", "POST", {
              name,
              type: kind,
              parameters,
            });
      }
      setPartial(true);
      onChanged();
      if (result.complete && !result.error) onClose();
      else {
        setWizard(result);
        setAnswer("");
        setError(result.error ?? "");
      }
    } catch (e) {
      setError(
        `${(e as Error).message} Configuration changes may already have been saved; close and reopen to inspect them if needed.`,
      );
      // Rclone can write fields before a later provider step fails.
      setPartial(true);
      onChanged();
    } finally {
      setBusy(false);
    }
  }
  function field(option: RcloneOption) {
    return (
      <ConfigOption
        key={option.Name}
        option={option}
        stored={remote?.parameters[option.Name]}
        masked={remote?.secrets.includes(option.Name) ?? false}
        change={changes[option.Name]}
        onChange={(value) => change(option.Name, value)}
        provider={providerName}
        creating={!remote}
      />
    );
  }
  return (
    <Modal onClose={close} title="remote-title">
      <section className="dialog remote-editor" aria-labelledby="remote-title">
        <header>
          <div>
            <p className="eyebrow">REMOTE CONFIGURATION</p>
            <h2 id="remote-title">
              {wizard
                ? `Finish ${name}`
                : remote
                  ? `Edit ${name}`
                  : "New remote"}
            </h2>
          </div>
          <button
            className="icon"
            title="Close remote editor"
            disabled={busy}
            onClick={close}
          >
            <X size={20} />
          </button>
        </header>
        <form onSubmit={submit}>
          <fieldset disabled={busy} className="config-fields">
            {wizard ? (
              <>
                <p className="warning">
                  Settings have been saved, but provider setup is unfinished.
                  Closing will keep saved settings and leave authorization
                  incomplete.
                </p>
                {wizard.option ? (
                  <div className="wizard-question">
                    <div className="provider-help wizard-help">
                      {wizard.option.Help}
                    </div>
                    <OptionInput
                      option={wizard.option}
                      value={answer}
                      onChange={setAnswer}
                      required={
                        !!wizard.option.Required && !defaultValue(wizard.option)
                      }
                      placeholder={
                        defaultValue(wizard.option)
                          ? `Default: ${defaultValue(wizard.option)}`
                          : "Enter answer"
                      }
                      secret={
                        wizard.option.IsPassword ||
                        wizard.option.Sensitive ||
                        wizard.option.Name === "config_token"
                      }
                    />
                  </div>
                ) : (
                  <p className="error">
                    No further question is available. Close and reopen the
                    remote to continue setup.
                  </p>
                )}
              </>
            ) : (
              <>
                <div className="form-grid">
                  <label>
                    Remote name
                    <input
                      aria-label="Remote name"
                      value={name}
                      required
                      maxLength={200}
                      disabled={!!remote}
                      onChange={(e) => setName(e.target.value)}
                    />
                  </label>
                  {remote && (
                    <label>
                      Backend
                      <input value={kind} disabled />
                    </label>
                  )}
                </div>
                {!remote && (
                  <div className="provider-picker">
                    <label className="search-field">
                      <Search size={16} />
                      <input
                        aria-label="Search backends"
                        placeholder="Search storage backends"
                        value={providerSearch}
                        onChange={(e) => setProviderSearch(e.target.value)}
                      />
                    </label>
                    <label>
                      Backend
                      <select
                        aria-label="Backend"
                        required
                        value={kind}
                        onChange={(e) => {
                          setKind(e.target.value);
                          setChanges({});
                          setCustom([]);
                          setOptionSearch("");
                          setAdvanced(false);
                        }}
                      >
                        <option value="">Choose a backend</option>
                        {providers
                          .filter(
                            (p) =>
                              p.Name === kind ||
                              (!p.Hide &&
                                `${p.Name} ${p.Description}`
                                  .toLowerCase()
                                  .includes(providerSearch.toLowerCase())),
                          )
                          .map((p) => (
                            <option key={p.Name} value={p.Name}>
                              {p.Description} ({p.Name})
                            </option>
                          ))}
                      </select>
                    </label>
                  </div>
                )}
                {kind && (
                  <>
                    <div className="config-toolbar">
                      <label className="search-field">
                        <Search size={16} />
                        <input
                          aria-label="Search options"
                          placeholder="Search options"
                          value={optionSearch}
                          onChange={(e) => setOptionSearch(e.target.value)}
                        />
                      </label>
                      <label className="check">
                        <input
                          type="checkbox"
                          checked={advanced}
                          onChange={(e) => setAdvanced(e.target.checked)}
                        />
                        Advanced
                      </label>
                    </div>
                    <div className="config-options">
                      {visibleOptions.map(field)}
                      {!visibleOptions.length && (
                        <p className="hint">No matching options.</p>
                      )}
                    </div>
                    <details className="custom-options">
                      <summary>
                        Custom and hidden keys
                        {extraKeys.length > 0 ? ` (${extraKeys.length})` : ""}
                      </summary>
                      {extraKeys.map((key) =>
                        field(
                          options.find((option) => option.Name === key) ?? {
                            Name: key,
                            Sensitive: true,
                          },
                        ),
                      )}
                      <div className="input-action">
                        <input
                          aria-label="Custom key"
                          placeholder="Key name"
                          value={customKey}
                          onChange={(e) => setCustomKey(e.target.value)}
                        />
                        <button
                          type="button"
                          title="Add custom key"
                          disabled={!customKey.trim()}
                          onClick={() => {
                            const key = customKey.trim();
                            if (
                              key === "type" ||
                              /[\r\n\0]/.test(key) ||
                              key.length > 200
                            ) {
                              setError(
                                "Use a single-line key other than type, up to 200 characters.",
                              );
                              return;
                            }
                            if (
                              options.some(
                                (option) =>
                                  option.Name === key && shownInSchema(option),
                              )
                            ) {
                              setOptionSearch(key);
                              setAdvanced(true);
                            } else {
                              setCustom((previous) => [
                                ...new Set([...previous, key]),
                              ]);
                            }
                            setCustomKey("");
                            setError("");
                          }}
                        >
                          <Plus size={16} />
                        </button>
                      </div>
                    </details>
                  </>
                )}
              </>
            )}
          </fieldset>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <footer>
            <span className="hint">
              {!wizard && remote
                ? `${changedCount} changed ${changedCount === 1 ? "key" : "keys"}`
                : ""}
            </span>
            {!wizard && remote && (
              <button
                type="button"
                disabled={busy || changedCount > 0}
                title={
                  changedCount
                    ? "Save or undo edits before provider setup"
                    : "Open provider authorization and setup questions"
                }
                onClick={(event) => void submit(event, true)}
              >
                Provider setup
              </button>
            )}
            <button type="button" disabled={busy} onClick={close}>
              {wizard ? "Close setup" : "Cancel"}
            </button>
            <button
              className="primary"
              disabled={
                busy ||
                (wizard
                  ? !wizard.option || !wizard.session_id
                  : !kind || (!!remote && !changedCount))
              }
            >
              {busy
                ? "Saving…"
                : wizard
                  ? "Continue"
                  : remote
                    ? "Save changes"
                    : "Create remote"}
            </button>
          </footer>
        </form>
        {closing && (
          <Modal
            confirmation
            onClose={() => setClosing(false)}
            title="close-config-title"
          >
            <section className="dialog small">
              <h2 id="close-config-title">
                {partial ? "Leave setup?" : "Discard changes?"}
              </h2>
              <p className={partial ? "warning" : "hint"}>
                {partial
                  ? "Already saved settings will remain. Authorization may be incomplete. Reopen the remote to review its settings; delete the remote to remove its configuration."
                  : "Your unsaved edits will be discarded."}
              </p>
              <footer>
                <button onClick={() => setClosing(false)}>Keep editing</button>
                <button className="primary" onClick={onClose}>
                  {partial ? "Keep saved settings and close" : "Discard edits"}
                </button>
              </footer>
            </section>
          </Modal>
        )}
      </section>
    </Modal>
  );
}
