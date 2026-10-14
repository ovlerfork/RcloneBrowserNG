use crate::model::{LogEntry, Run, RunStatus, Task, TaskInput, parse_schedule};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{fs::File, path::Path, sync::Mutex};

pub struct Store {
    connection: Mutex<Connection>,
    _lock: File,
}

impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let lock = File::create(directory.join("service.lock"))?;
        lock.try_lock()
            .context("another service is already using this data directory")?;
        let connection = Connection::open(directory.join("service.sqlite3"))?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, json TEXT NOT NULL, last_schedule_minute INTEGER);
            CREATE TABLE IF NOT EXISTS runs (id TEXT PRIMARY KEY, task_id TEXT, status TEXT NOT NULL, queued_at TEXT NOT NULL, json TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS runs_status ON runs(status, queued_at);
            CREATE INDEX IF NOT EXISTS runs_task ON runs(task_id);")?;
        let store = Self {
            connection: Mutex::new(connection),
            _lock: lock,
        };
        store.interrupt_active(
            "Service restarted; verify the partial transfer before starting another run",
        )?;
        Ok(store)
    }

    pub fn tasks(&self) -> Result<Vec<Task>> {
        let db = self.connection.lock().unwrap();
        let mut statement = db.prepare("SELECT json FROM tasks ORDER BY rowid")?;
        let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    pub fn task(&self, id: &str) -> Result<Option<Task>> {
        let db = self.connection.lock().unwrap();
        let value: Option<String> = db
            .query_row("SELECT json FROM tasks WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub fn save_task(&self, id: Option<&str>, input: TaskInput) -> Result<Task> {
        input.validate()?;
        let db = self.connection.lock().unwrap();
        let now = Utc::now();
        let task = if let Some(id) = id {
            let json: Option<String> = db
                .query_row("SELECT json FROM tasks WHERE id=?1", [id], |r| r.get(0))
                .optional()?;
            let old: Task = serde_json::from_str(&json.context("task not found")?)?;
            Task {
                id: id.to_string(),
                input,
                created_at: old.created_at,
                updated_at: now,
            }
        } else {
            Task {
                id: uuid::Uuid::new_v4().to_string(),
                input,
                created_at: now,
                updated_at: now,
            }
        };
        // Keep the last scheduled minute across edits to prevent duplicate dispatch.
        db.execute("INSERT INTO tasks(id,json) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![task.id, serde_json::to_string(&task)?])?;
        Ok(task)
    }
    pub fn delete_task(&self, id: &str) -> Result<bool> {
        let db = self.connection.lock().unwrap();
        if db.query_row(
            "SELECT COUNT(*) FROM runs WHERE task_id=?1 AND status IN ('queued','running')",
            [id],
            |r| r.get::<_, i64>(0),
        )? > 0
        {
            bail!("task has an active run");
        }
        Ok(db.execute("DELETE FROM tasks WHERE id=?1", [id])? > 0)
    }
    pub fn enqueue(
        &self,
        task_id: &str,
        trigger: &str,
        dry_run: Option<bool>,
        resync: bool,
        minute: Option<i64>,
    ) -> Result<Option<Run>> {
        let mut db = self.connection.lock().unwrap();
        let tx = db.transaction()?;
        let json: String = tx
            .query_row("SELECT json FROM tasks WHERE id=?1", [task_id], |r| {
                r.get(0)
            })
            .optional()?
            .context("task not found")?;
        let task: Task = serde_json::from_str(&json)?;
        if trigger == "schedule" {
            if !task.input.enabled {
                return Ok(None);
            }
            let Some(pattern) = &task.input.schedule else {
                return Ok(None);
            };
            let minute = minute.context("scheduled run requires a minute")?;
            let due =
                DateTime::from_timestamp(minute * 60, 0).context("invalid schedule timestamp")?;
            if !parse_schedule(pattern)?.is_time_matching(&due)? {
                return Ok(None);
            }
            let previous: Option<i64> = tx.query_row(
                "SELECT last_schedule_minute FROM tasks WHERE id=?1",
                [task_id],
                |r| r.get(0),
            )?;
            if previous.is_some_and(|previous| minute <= previous) {
                return Ok(None);
            }
            tx.execute(
                "UPDATE tasks SET last_schedule_minute=?2 WHERE id=?1",
                params![task_id, minute],
            )?;
        }
        let active: i64 = tx.query_row(
            "SELECT COUNT(*) FROM runs WHERE task_id=?1 AND status IN ('queued','running')",
            [task_id],
            |r| r.get(0),
        )?;
        if active > 0 {
            if trigger == "schedule" {
                tx.commit()?;
                return Ok(None);
            }
            bail!("task already has an active run");
        }
        let run = Run::new(
            &task,
            trigger,
            dry_run.unwrap_or(task.input.dry_run),
            resync,
        );
        insert_run(&tx, &run)?;
        tx.commit()?;
        Ok(Some(run))
    }
    pub fn enqueue_run(&self, run: &Run) -> Result<()> {
        insert_run(&self.connection.lock().unwrap(), run)
    }
    pub fn run(&self, id: &str) -> Result<Option<Run>> {
        let db = self.connection.lock().unwrap();
        read_run(&db, id)
    }
    pub fn runs(&self, task_id: Option<&str>, limit: usize) -> Result<Vec<Run>> {
        let db = self.connection.lock().unwrap();
        let mut stmt = db.prepare("SELECT json FROM runs WHERE (?1 IS NULL OR task_id=?1) ORDER BY queued_at DESC,rowid DESC LIMIT ?2")?;
        let rows = stmt.query_map(params![task_id, i64::try_from(limit)?], |r| {
            r.get::<_, String>(0)
        })?;
        rows.map(|row| {
            let mut run: Run = serde_json::from_str(&row?)?;
            run.logs.clear();
            Ok(run)
        })
        .collect()
    }
    pub fn claim(&self) -> Result<Option<Run>> {
        let db = self.connection.lock().unwrap();
        let active = active_runs(&db)?;
        if active.iter().any(|run| run.status == RunStatus::Running) {
            return Ok(None);
        }
        let candidate = active.iter().find(|run| run.status == RunStatus::Queued);
        if let Some(run) = candidate {
            let mut run = run.clone();
            run.status = RunStatus::Running;
            run.started_at = Some(Utc::now());
            write_run(&db, &run)?;
            Ok(Some(run))
        } else {
            Ok(None)
        }
    }
    pub fn cancel(&self, id: &str) -> Result<Option<Run>> {
        self.update(id, |run| {
            if run.status.active() {
                run.cancel_requested = true;
                if run.status == RunStatus::Queued {
                    run.status = RunStatus::Cancelled;
                    run.finished_at = Some(Utc::now());
                }
            }
        })
    }
    pub fn finish(
        &self,
        id: &str,
        status: RunStatus,
        code: Option<i32>,
        error: Option<String>,
    ) -> Result<()> {
        self.update(id, |run| {
            run.status = if run.cancel_requested {
                RunStatus::Cancelled
            } else {
                status
            };
            run.exit_code = code;
            run.error = error;
            run.finished_at = Some(Utc::now());
        })?;
        Ok(())
    }
    pub fn log(
        &self,
        id: &str,
        entry: LogEntry,
        progress: Option<serde_json::Value>,
    ) -> Result<()> {
        self.update(id, |run| {
            if let Some(progress) = progress {
                run.progress = Some(progress);
            }
            run.logs.push(entry);
            if run.logs.len() > 500 {
                run.logs.drain(..run.logs.len() - 500);
            }
        })?;
        Ok(())
    }
    fn update(&self, id: &str, f: impl FnOnce(&mut Run)) -> Result<Option<Run>> {
        let db = self.connection.lock().unwrap();
        let Some(mut run) = read_run(&db, id)? else {
            return Ok(None);
        };
        f(&mut run);
        write_run(&db, &run)?;
        Ok(Some(run))
    }
    pub fn interrupt_active(&self, reason: &str) -> Result<()> {
        let db = self.connection.lock().unwrap();
        for mut run in active_runs(&db)? {
            run.status = RunStatus::Interrupted;
            run.finished_at = Some(Utc::now());
            run.error = Some(reason.into());
            write_run(&db, &run)?;
        }
        Ok(())
    }
}

fn active_runs(db: &Connection) -> Result<Vec<Run>> {
    let mut stmt = db.prepare(
        "SELECT json FROM runs WHERE status IN ('queued','running') ORDER BY queued_at,rowid",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
}
fn read_run(db: &Connection, id: &str) -> Result<Option<Run>> {
    let value: Option<String> = db
        .query_row("SELECT json FROM runs WHERE id=?1", [id], |r| r.get(0))
        .optional()?;
    value
        .map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}
fn insert_run(db: &Connection, run: &Run) -> Result<()> {
    db.execute(
        "INSERT INTO runs(id,task_id,status,queued_at,json) VALUES(?1,?2,'queued',?3,?4)",
        params![
            run.id,
            run.task_id,
            run.queued_at.to_rfc3339(),
            serde_json::to_string(run)?
        ],
    )?;
    Ok(())
}
fn write_run(db: &Connection, run: &Run) -> Result<()> {
    let status = serde_json::to_value(&run.status)?
        .as_str()
        .unwrap()
        .to_owned();
    db.execute(
        "UPDATE runs SET status=?2,json=?3 WHERE id=?1",
        params![run.id, status, serde_json::to_string(run)?],
    )?;
    Ok(())
}

pub fn schedule_minute(now: DateTime<Utc>) -> i64 {
    now.timestamp().div_euclid(60)
}
