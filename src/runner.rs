use crate::{
    model::{LogEntry, Operation, Run, RunStatus, parse_schedule},
    store::{Store, schedule_minute},
};
use anyhow::{Context, Result, bail};
use chrono::{Timelike, Utc};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, Notify},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Config {
    pub binary: String,
    pub rclone_config: PathBuf,
    pub data_dir: PathBuf,
    pub web_root: PathBuf,
    pub bind_addr: String,
}
impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            binary: std::env::var("RCLONE_BINARY").unwrap_or_else(|_| "rclone".into()),
            rclone_config: std::env::var("RCLONE_CONFIG")
                .unwrap_or_else(|_| "/config/rclone/rclone.conf".into())
                .into(),
            data_dir: std::env::var("DATA_DIR")
                .unwrap_or_else(|_| "/config/service".into())
                .into(),
            web_root: std::env::var("WEB_ROOT")
                .unwrap_or_else(|_| "web/dist".into())
                .into(),
            bind_addr: std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into()),
        })
    }
}

pub struct Service {
    pub config: Config,
    pub store: Store,
    pub notify: Notify,
    pub shutdown: CancellationToken,
    pub file_gate: Mutex<()>,
    pub rc: crate::rc::RcManager,
    pub config_sessions: Mutex<std::collections::HashMap<String, crate::remote_config::Session>>,
}
impl Service {
    pub fn new(config: Config) -> Result<Arc<Self>> {
        std::fs::create_dir_all(config.data_dir.join("bisync"))?;
        let store = Store::open(&config.data_dir)?;
        Ok(Arc::new(Self {
            config,
            store,
            notify: Notify::new(),
            shutdown: CancellationToken::new(),
            file_gate: Mutex::new(()),
            rc: crate::rc::RcManager::default(),
            config_sessions: Mutex::new(std::collections::HashMap::new()),
        }))
    }
    pub fn start(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        let service = self.clone();
        let mut handles = vec![tokio::spawn(async move {
            service.clone().worker().await;
            service.rc.stop().await;
        })];
        let service = self.clone();
        handles.push(tokio::spawn(async move {
            service.scheduler().await;
        }));
        handles
    }
    async fn worker(self: Arc<Self>) {
        loop {
            if self.shutdown.is_cancelled() {
                break;
            }
            let gate = tokio::select! { _ = self.shutdown.cancelled() => break, gate = self.file_gate.lock() => gate };
            let run = match self.store.claim() {
                Ok(run) => run,
                Err(error) => {
                    tracing::error!(%error, "cannot claim run");
                    None
                }
            };
            if let Some(run) = run {
                if let Err(error) = self.execute(&run).await {
                    tracing::error!(run_id = %run.id, %error, "run failed");
                    if let Err(write_error) =
                        self.store
                            .finish(&run.id, RunStatus::Failed, None, Some(error.to_string()))
                    {
                        tracing::error!(%write_error, "cannot persist failed run");
                    }
                }
                self.notify.notify_waiters();
            } else {
                drop(gate);
                tokio::select! { _ = self.shutdown.cancelled() => break, _ = self.notify.notified() => {}, _ = tokio::time::sleep(Duration::from_millis(500)) => {} }
            }
        }
    }
    async fn execute(self: &Arc<Self>, run: &Run) -> Result<()> {
        if self.shutdown.is_cancelled() {
            self.store.finish(
                &run.id,
                RunStatus::Interrupted,
                None,
                Some("Service stopping".into()),
            )?;
            return Ok(());
        }
        if self.store.run(&run.id)?.is_some_and(|r| r.cancel_requested) {
            self.store
                .finish(&run.id, RunStatus::Cancelled, None, None)?;
            return Ok(());
        }
        let rc = self.rc.connection(&self.config).await?;
        rc.call(
            "core/bwlimit",
            serde_json::json!({"rate": run.options.bandwidth_limit.as_deref().unwrap_or("off")}),
        )
        .await?;
        let result = self.execute_job(run, &rc).await;
        if result.is_err() {
            self.rc.stop().await;
            return result;
        }
        if rc
            .call("core/bwlimit", serde_json::json!({"rate":"off"}))
            .await
            .is_err()
        {
            self.rc.stop().await;
        }
        result
    }
    async fn execute_job(&self, run: &Run, rc: &crate::rc::Connection) -> Result<()> {
        use serde_json::json;
        let group = format!("run-{}", run.id);
        let mut body = json!({"_async":true,"_group":group,
            "_config":{"DryRun":run.dry_run,"Transfers":run.options.transfers,"Checkers":run.options.checkers},
            "_filter":{"ExcludeRule":run.options.exclude}});
        let endpoint;
        if run.operation == Operation::Bisync {
            let directory = self.config.data_dir.join("bisync").join(
                run.task_id
                    .as_ref()
                    .context("bisync requires a saved task")?,
            );
            std::fs::create_dir_all(&directory)?;
            body["path1"] = json!(run.source);
            body["path2"] = json!(run.destination);
            body["workdir"] = json!(directory);
            body["resync"] = json!(run.resync);
            body["dryRun"] = json!(run.dry_run);
            body["maxDelete"] = json!(50);
            endpoint = "sync/bisync".to_owned();
        } else {
            let (src_fs, src_remote) = crate::rc::filesystem_path(&run.source);
            let stat = rc
                .call("operations/stat", json!({"fs":src_fs,"remote":src_remote}))
                .await?;
            let single_file = run.single_file || stat["item"]["IsDir"] == false;
            if single_file {
                if run.operation == Operation::Sync {
                    bail!("sync requires a directory source; use copy or move for a file");
                }
                let destination = if run.single_file {
                    run.destination.clone()
                } else {
                    format!(
                        "{}/{}",
                        run.destination.trim_end_matches('/'),
                        src_remote.rsplit('/').next().context("missing filename")?
                    )
                };
                let (dst_fs, dst_remote) = crate::rc::filesystem_path(&destination);
                body["srcFs"] = json!(src_fs);
                body["srcRemote"] = json!(src_remote);
                body["dstFs"] = json!(dst_fs);
                body["dstRemote"] = json!(dst_remote);
                endpoint = format!("operations/{}file", run.operation.command());
            } else {
                body["srcFs"] = json!(run.source);
                body["dstFs"] = json!(run.destination);
                endpoint = format!("sync/{}", run.operation.command());
            }
        }
        self.record(&run.id, "info", "Starting rclone RPC job", None)?;
        let started = match rc.call(&endpoint, body).await {
            Ok(started) => started,
            Err(error) => {
                // The server may have accepted an async job before the response was lost.
                self.rc.stop().await;
                return Err(
                    error.context("job start failed; daemon stopped to prevent untracked work")
                );
            }
        };
        let Some(job_id) = started["jobid"].as_u64() else {
            self.rc.stop().await;
            bail!("rclone returned no job ID; daemon stopped to prevent untracked work");
        };
        let mut stopping = None;
        let mut stop_started = None;
        let mut last_stats = std::time::Instant::now() - Duration::from_secs(1);
        loop {
            let shutdown = self.shutdown.is_cancelled();
            let cancelled = self.store.run(&run.id)?.is_some_and(|r| r.cancel_requested);
            if stopping.is_none() && (shutdown || cancelled) {
                stopping = Some(if shutdown {
                    RunStatus::Interrupted
                } else {
                    RunStatus::Cancelled
                });
                stop_started = Some(std::time::Instant::now());
                if rc.call("job/stop", json!({"jobid":job_id})).await.is_err() {
                    self.rc.stop().await;
                    self.store.finish(
                        &run.id,
                        stopping.unwrap(),
                        None,
                        Some("Transfer stopped; completed changes are not rolled back".into()),
                    )?;
                    return Ok(());
                }
            }
            let status = match rc.call("job/status", json!({"jobid":job_id})).await {
                Ok(status) => status,
                Err(error) => {
                    self.rc.stop().await;
                    return Err(error.context("lost active rclone job; transfer may be partial"));
                }
            };
            if (last_stats.elapsed() >= Duration::from_secs(1) || status["finished"] == true)
                && let Ok(mut stats) = rc.call("core/stats", json!({"group":group})).await
            {
                if let Some(object) = stats.as_object_mut() {
                    object.remove("lastError");
                }
                let summary = format!(
                    "Transferred {} bytes, {} files; {} errors",
                    stats["bytes"].as_u64().unwrap_or(0),
                    stats["transfers"].as_u64().unwrap_or(0),
                    stats["errors"].as_u64().unwrap_or(0)
                );
                self.record(&run.id, "info", &summary, Some(stats))?;
                last_stats = std::time::Instant::now();
            }
            if status["finished"] == true {
                let success = status["success"] == true;
                let terminal = stopping.unwrap_or(if success {
                    RunStatus::Succeeded
                } else {
                    RunStatus::Failed
                });
                let error =
                    if terminal == RunStatus::Cancelled || terminal == RunStatus::Interrupted {
                        Some("Transfer stopped; completed changes are not rolled back".to_owned())
                    } else if !success {
                        // Job errors may contain provider credentials embedded in paths or provider responses.
                        Some(
                        "rclone job failed; check endpoints, credentials, and transfer settings"
                            .to_owned(),
                    )
                    } else {
                        None
                    };
                self.record(
                    &run.id,
                    if success { "info" } else { "error" },
                    error.as_deref().unwrap_or("Transfer completed"),
                    None,
                )?;
                self.store.finish(&run.id, terminal, None, error)?;
                return Ok(());
            }
            if stop_started.is_some_and(|time| time.elapsed() > Duration::from_secs(5)) {
                self.rc.stop().await;
                self.store.finish(
                    &run.id,
                    stopping.unwrap(),
                    None,
                    Some(
                        "rclone stopped after cancellation timeout; transfer may be partial".into(),
                    ),
                )?;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    fn record(
        &self,
        id: &str,
        level: &str,
        message: &str,
        progress: Option<serde_json::Value>,
    ) -> Result<()> {
        self.store.log(
            id,
            LogEntry {
                time: Utc::now(),
                level: level.into(),
                message: message.into(),
            },
            progress,
        )
    }
    async fn scheduler(self: Arc<Self>) {
        let mut last_minute = schedule_minute(Utc::now());
        loop {
            tokio::select! { _ = self.shutdown.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
            let now = Utc::now();
            let minute = schedule_minute(now);
            if minute == last_minute {
                continue;
            }
            last_minute = minute;
            let time = now.with_second(0).unwrap().with_nanosecond(0).unwrap();
            let result = self.schedule_at(time);
            if let Err(error) = result {
                tracing::error!(%error, "scheduler failed");
            }
        }
    }
    pub fn schedule_at(&self, time: chrono::DateTime<Utc>) -> Result<()> {
        for task in self.store.tasks()? {
            if !task.input.enabled {
                continue;
            }
            let Some(pattern) = &task.input.schedule else {
                continue;
            };
            if parse_schedule(pattern)?.is_time_matching(&time)? {
                self.store.enqueue(
                    &task.id,
                    "schedule",
                    None,
                    false,
                    Some(schedule_minute(time)),
                )?;
            }
        }
        self.notify.notify_waiters();
        Ok(())
    }
}
