use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use croner::{
    Cron,
    parser::{CronParser, Seconds, Year},
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Copy,
    Sync,
    Move,
    Bisync,
}

impl Operation {
    pub fn command(&self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Sync => "sync",
            Self::Move => "move",
            Self::Bisync => "bisync",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskOptions {
    pub transfers: u32,
    pub checkers: u32,
    pub bandwidth_limit: Option<String>,
    pub exclude: Vec<String>,
}
impl Default for TaskOptions {
    fn default() -> Self {
        Self {
            transfers: 4,
            checkers: 8,
            bandwidth_limit: None,
            exclude: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    pub name: String,
    pub operation: Operation,
    pub source: String,
    pub destination: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub destructive_confirmed: bool,
    #[serde(default)]
    pub options: TaskOptions,
}
fn yes() -> bool {
    true
}

impl TaskInput {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.len() > 200 {
            bail!("name must contain 1–200 characters");
        }
        validate_path(&self.source)?;
        validate_path(&self.destination)?;
        if paths_overlap(&self.source, &self.destination) {
            bail!("source and destination must not be the same path or contain one another");
        }
        if self.operation != Operation::Copy && !self.destructive_confirmed {
            bail!("sync, move, and bisync require destructive_confirmed: true");
        }
        if let Some(schedule) = &self.schedule {
            parse_schedule(schedule)?;
        }
        if !(1..=64).contains(&self.options.transfers)
            || !(1..=128).contains(&self.options.checkers)
        {
            bail!("transfers must be 1–64 and checkers 1–128");
        }
        if let Some(limit) = &self.options.bandwidth_limit
            && (limit.is_empty()
                || limit.len() > 30
                || !limit
                    .chars()
                    .all(|c| c.is_ascii_digit() || ".kKmMgGtTpPbBiI:".contains(c)))
        {
            bail!("bandwidth_limit must be a rate such as 10M or 1M:2M");
        }
        if self.options.exclude.len() > 100
            || self
                .options
                .exclude
                .iter()
                .any(|p| p.len() > 1024 || p.contains(['\0', '\n', '\r']))
        {
            bail!("exclude supports up to 100 single-line patterns, each at most 1024 bytes");
        }
        Ok(())
    }
}

pub fn parse_schedule(value: &str) -> Result<Cron> {
    if value.split_whitespace().count() != 5 {
        bail!("schedule must contain five cron fields: minute hour day month weekday (UTC)");
    }
    Ok(CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(value)?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(flatten)]
    pub input: TaskInput,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}
impl RunStatus {
    pub fn active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub time: DateTime<Utc>,
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub task_id: Option<String>,
    pub task_name: String,
    pub operation: Operation,
    pub source: String,
    pub destination: String,
    pub dry_run: bool,
    pub resync: bool,
    pub status: RunStatus,
    pub trigger: String,
    pub queued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub progress: Option<serde_json::Value>,
    pub logs: Vec<LogEntry>,
    pub cancel_requested: bool,
    pub options: TaskOptions,
    #[serde(default)]
    pub single_file: bool,
}

impl Run {
    pub fn new(task: &Task, trigger: &str, dry_run: bool, resync: bool) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: Some(task.id.clone()),
            task_name: task.input.name.clone(),
            operation: task.input.operation.clone(),
            source: task.input.source.clone(),
            destination: task.input.destination.clone(),
            dry_run,
            resync,
            status: RunStatus::Queued,
            trigger: trigger.into(),
            queued_at: Utc::now(),
            started_at: None,
            finished_at: None,
            exit_code: None,
            error: None,
            progress: None,
            logs: vec![],
            cancel_requested: false,
            options: task.input.options.clone(),
            single_file: false,
        }
    }
}

pub fn validate_path(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 4096 || value.contains(['\0', '\n', '\r']) {
        bail!("path must be nonempty, single-line, and at most 4096 bytes");
    }
    if value.starts_with('/') {
        return Ok(());
    }
    if let Some((remote, _)) = value.split_once(':')
        && !remote.is_empty()
        && remote
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-. ".contains(c))
        && !remote.starts_with('-')
    {
        return Ok(());
    }
    bail!("path must be an absolute local path or a configured remote:path")
}

fn normalized(value: &str) -> (String, Vec<String>) {
    let (root, path) = if value.starts_with('/') {
        ("local".to_string(), canonical_local(Path::new(value)))
    } else {
        let (remote, path) = value.split_once(':').unwrap_or((value, ""));
        (format!("remote:{remote}"), PathBuf::from(path))
    };
    let mut parts = vec![];
    for part in path.components() {
        match part {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    (root, parts)
}
fn canonical_local(path: &Path) -> PathBuf {
    if let Ok(resolved) = path.canonicalize() {
        return resolved;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        return canonical_local(parent).join(name);
    }
    path.to_path_buf()
}
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let (a_root, a_parts) = normalized(a);
    let (b_root, b_parts) = normalized(b);
    a_root == b_root && (a_parts.starts_with(&b_parts) || b_parts.starts_with(&a_parts))
}
pub fn is_root(value: &str) -> bool {
    normalized(value).1.is_empty()
}
