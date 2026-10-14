use crate::{
    model::{LogEntry, Operation, Run, RunStatus, parse_schedule},
    store::{Store, schedule_minute},
};
use anyhow::{Context, Result, bail};
use chrono::{Timelike, Utc};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::{Notify, RwLock},
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
    pub concurrency: usize,
}
impl Config {
    pub fn from_env() -> Result<Self> {
        let concurrency = std::env::var("MAX_CONCURRENT_RUNS")
            .unwrap_or_else(|_| "2".into())
            .parse::<usize>()?;
        if !(1..=16).contains(&concurrency) {
            bail!("MAX_CONCURRENT_RUNS must be 1–16");
        }
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
            concurrency,
        })
    }
}

pub struct Service {
    pub config: Config,
    pub store: Store,
    pub notify: Notify,
    pub shutdown: CancellationToken,
    pub file_gate: RwLock<()>,
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
            file_gate: RwLock::new(()),
        }))
    }
    pub fn start(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        let mut handles = vec![];
        for _ in 0..self.config.concurrency {
            let service = self.clone();
            handles.push(tokio::spawn(async move {
                service.worker().await;
            }));
        }
        let service = self.clone();
        handles.push(tokio::spawn(async move {
            service.scheduler().await;
        }));
        handles
    }
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.config.binary);
        command
            .arg("--config")
            .arg(&self.config.rclone_config)
            .arg("--ask-password=false")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        #[cfg(target_os = "linux")]
        unsafe {
            let parent = libc::getpid();
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::other(
                        "service exited before rclone started",
                    ));
                }
                Ok(())
            });
        }
        command
    }
    pub async fn output(&self, args: &[&str]) -> Result<Vec<u8>> {
        let mut child = self
            .command()
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start rclone")?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::try_join!(
                read_capped(stdout, 16 * 1024 * 1024),
                read_capped(stderr, 64 * 1024),
                async { Ok::<_, anyhow::Error>(child.wait().await?) }
            )
        })
        .await;
        match result {
            Ok(Ok((out, _, status))) if status.success() => Ok(out),
            Ok(Ok((_, err, status))) => bail!(
                "rclone exited with {}: {}",
                status,
                String::from_utf8_lossy(&err).trim()
            ),
            Ok(Err(error)) => {
                terminate(&mut child).await;
                Err(error)
            }
            Err(_) => {
                terminate(&mut child).await;
                bail!("rclone request timed out after 60 seconds");
            }
        }
    }
    async fn worker(self: Arc<Self>) {
        loop {
            if self.shutdown.is_cancelled() {
                break;
            }
            let gate = tokio::select! { _ = self.shutdown.cancelled() => break, gate = self.file_gate.read() => gate };
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
        let mut command = self.command();
        command.arg(if run.single_file {
            "moveto"
        } else {
            run.operation.command()
        });
        command
            .arg(&run.source)
            .arg(&run.destination)
            .arg("--transfers")
            .arg(run.options.transfers.to_string())
            .arg("--checkers")
            .arg(run.options.checkers.to_string())
            .args([
                "--use-json-log",
                "--log-level",
                "INFO",
                "--stats",
                "1s",
                "--stats-log-level",
                "NOTICE",
            ]);
        if run.dry_run {
            command.arg("--dry-run");
        }
        if let Some(limit) = &run.options.bandwidth_limit {
            command.arg("--bwlimit").arg(limit);
        }
        for pattern in &run.options.exclude {
            command.arg("--exclude").arg(pattern);
        }
        if run.operation == Operation::Bisync {
            // Each task owns its baselines. They survive runs and service restarts.
            let directory = self.config.data_dir.join("bisync").join(
                run.task_id
                    .as_ref()
                    .context("bisync requires a saved task")?,
            );
            std::fs::create_dir_all(&directory)?;
            command.arg("--workdir").arg(directory);
            if run.resync {
                command.arg("--resync");
            }
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start rclone")?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out_service = self.clone();
        let out_id = run.id.clone();
        let output = tokio::spawn(async move { out_service.capture(&out_id, stdout).await });
        let err_service = self.clone();
        let err_id = run.id.clone();
        let errors = tokio::spawn(async move { err_service.capture(&err_id, stderr).await });
        let (status, exit_code, error) = loop {
            tokio::select! {
                status = child.wait() => {
                    let status = status?;
                    break (if status.success() { RunStatus::Succeeded } else { RunStatus::Failed }, status.code(), (!status.success()).then(|| format!("rclone exited with {status}; see run logs")));
                }
                _ = self.shutdown.cancelled() => {
                    terminate(&mut child).await;
                    break (RunStatus::Interrupted, None, Some("Service stopped; transfer may be partial".into()));
                }
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if self.store.run(&run.id)?.is_some_and(|r| r.cancel_requested) {
                        terminate(&mut child).await;
                        break (RunStatus::Cancelled, None, Some("Cancelled; completed transfers and deletions are not rolled back".into()));
                    }
                }
            }
        };
        output.await??;
        errors.await??;
        self.store.finish(&run.id, status, exit_code, error)?;
        Ok(())
    }
    async fn capture(&self, id: &str, mut reader: impl AsyncRead + Unpin) -> Result<()> {
        let mut chunk = [0u8; 4096];
        let mut line = Vec::new();
        loop {
            let count = reader.read(&mut chunk).await?;
            if count == 0 {
                if !line.is_empty() {
                    self.record_line(id, &line)?;
                }
                break;
            }
            for byte in &chunk[..count] {
                if *byte == b'\n' {
                    self.record_line(id, &line)?;
                    line.clear();
                } else if line.len() < 16 * 1024 {
                    line.push(*byte);
                }
            }
        }
        Ok(())
    }
    fn record_line(&self, id: &str, line: &[u8]) -> Result<()> {
        if line.is_empty() {
            return Ok(());
        }
        let value = serde_json::from_slice::<serde_json::Value>(line).ok();
        let message = value
            .as_ref()
            .and_then(|v| v.get("msg"))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(line).into_owned());
        let level = value
            .as_ref()
            .and_then(|v| v.get("level"))
            .and_then(|v| v.as_str())
            .unwrap_or("info")
            .to_string();
        let progress = value.as_ref().and_then(|v| v.get("stats")).cloned();
        self.store.log(
            id,
            LogEntry {
                time: Utc::now(),
                level,
                message,
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

async fn read_capped(mut reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    (&mut reader)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        bail!(
            "rclone response exceeds {} bytes; browse a smaller directory",
            limit
        );
    }
    Ok(bytes)
}
async fn terminate(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }
    if tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .is_err()
    {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}
