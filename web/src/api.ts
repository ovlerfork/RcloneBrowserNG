export type Operation = "copy" | "sync" | "move" | "bisync";
export interface Options {
  transfers: number;
  checkers: number;
  bandwidth_limit: string | null;
  exclude: string[];
}
export interface TaskInput {
  name: string;
  operation: Operation;
  source: string;
  destination: string;
  enabled: boolean;
  schedule: string | null;
  dry_run: boolean;
  destructive_confirmed: boolean;
  options: Options;
}
export interface Task extends TaskInput {
  id: string;
  created_at: string;
  updated_at: string;
}
export interface Run {
  id: string;
  task_id: string | null;
  task_name: string;
  operation: Operation;
  source: string;
  destination: string;
  dry_run: boolean;
  resync: boolean;
  status: string;
  trigger: string;
  queued_at: string;
  started_at: string | null;
  finished_at: string | null;
  exit_code: number | null;
  error: string | null;
  progress: {
    bytes?: number;
    totalBytes?: number;
    speed?: number;
    transfers?: number;
    checks?: number;
    errors?: number;
    eta?: number;
  } | null;
  logs: { time: string; level: string; message: string }[];
  cancel_requested: boolean;
}
export interface Entry {
  name: string;
  path: string;
  size: number;
  is_dir: boolean;
  modified: string;
}
export interface Remote {
  name: string;
  type: string;
}
export interface Settings {
  rclone_available: boolean;
  config_path: string;
  concurrency: number;
  timezone: string;
}
export async function api<T>(
  path: string,
  method = "GET",
  body?: unknown,
): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method,
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    const result = await response
      .json()
      .catch(() => ({ error: response.statusText }));
    throw new Error(result.error || response.statusText);
  }
  return response.status === 204 ? (undefined as T) : response.json();
}
export const active = (run: Run) => ["queued", "running"].includes(run.status);
export const blankTask = (): TaskInput => ({
  name: "",
  operation: "copy",
  source: "",
  destination: "",
  enabled: false,
  schedule: null,
  dry_run: false,
  destructive_confirmed: false,
  options: { transfers: 4, checkers: 8, bandwidth_limit: null, exclude: [] },
});
export function taskInput(task: Task): TaskInput {
  const {
    name,
    operation,
    source,
    destination,
    enabled,
    schedule,
    dry_run,
    destructive_confirmed,
    options,
  } = task;
  return {
    name,
    operation,
    source,
    destination,
    enabled,
    schedule,
    dry_run,
    destructive_confirmed,
    options,
  };
}
export function bytes(value?: number): string {
  if (value === undefined || value < 0) return "—";
  if (value < 1024) return `${value} B`;
  const i = Math.min(4, Math.floor(Math.log(value) / Math.log(1024)));
  return `${(value / 1024 ** i).toFixed(1)} ${["B", "KiB", "MiB", "GiB", "TiB"][i]}`;
}
export const date = (value: string) => new Date(value).toLocaleString();
