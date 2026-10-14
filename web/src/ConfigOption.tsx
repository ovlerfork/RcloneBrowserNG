import { useId } from "react";
import { RotateCcw, Trash2 } from "lucide-react";

export interface RcloneOption {
  Name: string;
  Help?: string;
  Type?: string;
  DefaultStr?: string;
  Default?: unknown;
  Examples?: { Value: string; Help?: string; Provider?: string }[];
  Provider?: string;
  Hide?: number;
  Advanced?: boolean;
  Required?: boolean;
  Exclusive?: boolean;
  IsPassword?: boolean;
  Sensitive?: boolean;
}
export interface Provider {
  Name: string;
  Description: string;
  Options: RcloneOption[];
  Hide?: boolean;
}
export const defaultValue = (option: RcloneOption) =>
  option.DefaultStr ?? (option.Default == null ? "" : String(option.Default));
const booleanValue = (value: string) => {
  if (["1", "t", "T", "TRUE", "true", "True"].includes(value)) return "true";
  if (["0", "f", "F", "FALSE", "false", "False"].includes(value)) return "false";
  return value;
};
export function matchesProvider(rule: string | undefined, provider: string) {
  if (!rule || !provider) return true;
  const exclude = rule.startsWith("!");
  const matches = (exclude ? rule.slice(1) : rule)
    .split(",")
    .includes(provider);
  return exclude ? !matches : matches;
}

export function OptionInput({
  option,
  value,
  onChange,
  disabled,
  required,
  placeholder,
  provider = "",
  secret = false,
}: {
  option: RcloneOption;
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  required?: boolean;
  placeholder?: string;
  provider?: string;
  secret?: boolean;
}) {
  const id = useId();
  const examples =
    option.Examples?.filter((example) =>
      matchesProvider(example.Provider, provider),
    ) ?? [];
  const choices =
    option.Type === "bool"
      ? ["true", "false"].map((value) => ({
          Value: value,
          Help:
            examples.find((example) => booleanValue(example.Value) === value)
              ?.Help || (value === "true" ? "Yes" : "No"),
        }))
      : examples;
  const selectedValue = option.Type === "bool" ? booleanValue(value) : value;
  const common = {
    id,
    "aria-label": option.Name,
    value,
    disabled,
    required,
    onChange: (
      event: React.ChangeEvent<HTMLInputElement | HTMLSelectElement>,
    ) => onChange(event.target.value),
  };
  return (
    <>
      <label htmlFor={id} className="option-label">
        {option.Name}
        {required && <span className="required">Required</span>}
      </label>
      {!secret && (option.Exclusive || option.Type === "bool") ? (
        <select {...common} value={selectedValue}>
          <option value="">{placeholder || "Not set"}</option>
          {selectedValue &&
            !choices.some((example) => example.Value === selectedValue) && (
              <option value={value}>{value}</option>
            )}
          {choices.map((example, index) => (
            <option key={`${example.Value}-${index}`} value={example.Value}>
              {example.Value}
              {example.Help ? ` · ${example.Help.split("\n")[0]}` : ""}
            </option>
          ))}
        </select>
      ) : (
        <input
          {...common}
          type={secret ? "password" : "text"}
          autoComplete={secret ? "new-password" : "off"}
          spellCheck={false}
          placeholder={placeholder}
          list={!secret && examples.length ? `${id}-examples` : undefined}
          inputMode={
            option.Type?.match(/^(u?int|float)/) ? "decimal" : undefined
          }
        />
      )}
      {!secret &&
        !option.Exclusive &&
        option.Type !== "bool" &&
        examples.length > 0 && (
          <datalist id={`${id}-examples`}>
            {examples.map((example, index) => (
              <option key={`${example.Value}-${index}`} value={example.Value}>
                {example.Help}
              </option>
            ))}
          </datalist>
        )}
    </>
  );
}

export function ConfigOption({
  option,
  stored,
  masked,
  change,
  onChange,
  provider,
  creating,
}: {
  option: RcloneOption;
  stored?: string;
  masked: boolean;
  change?: string | null;
  onChange: (value: string | null | undefined) => void;
  provider: string;
  creating: boolean;
}) {
  const secret = masked || option.IsPassword || option.Sensitive;
  const configured = masked || stored !== undefined;
  const removed = change === null;
  const untouchedSecret = secret && configured && change === undefined;
  const fallback = defaultValue(option);
  return (
    <section className={`config-option ${removed ? "option-removed" : ""}`}>
      <div className="option-controls">
        <div className="option-value">
          <OptionInput
            option={option}
            value={
              typeof change === "string"
                ? change
                : secret || removed
                  ? ""
                  : (stored ?? "")
            }
            onChange={onChange}
            disabled={removed || !!untouchedSecret}
            secret={!!secret}
            required={
              !!option.Required &&
              !fallback &&
              !configured &&
              !removed &&
              creating
            }
            placeholder={
              removed
                ? "Will be removed"
                : untouchedSecret
                  ? "Configured · value hidden"
                  : fallback
                    ? `Default: ${fallback}`
                    : "Not set"
            }
            provider={provider}
          />
        </div>
        <div className="option-actions">
          {untouchedSecret && (
            <button type="button" onClick={() => onChange("")}>
              Replace
            </button>
          )}
          {change !== undefined && (
            <button
              type="button"
              className="icon"
              title={`Undo change to ${option.Name}`}
              onClick={() => onChange(undefined)}
            >
              <RotateCcw size={16} />
            </button>
          )}
          {configured && !removed && (
            <button
              type="button"
              className="icon danger"
              title={`Remove ${option.Name}`}
              onClick={() => onChange(null)}
            >
              <Trash2 size={16} />
            </button>
          )}
        </div>
      </div>
      {option.Help && (
        <details className="option-help">
          <summary>{option.Help.split("\n")[0]}</summary>
          <div className="provider-help">{option.Help}</div>
        </details>
      )}
      <div className="option-meta">
        {removed
          ? "Remove on save"
          : change !== undefined
            ? "Changed"
            : configured
              ? "Configured"
              : fallback
                ? `Default: ${fallback}`
                : option.Type || "string"}
        {secret ? " · Sensitive value" : ""}
      </div>
    </section>
  );
}
