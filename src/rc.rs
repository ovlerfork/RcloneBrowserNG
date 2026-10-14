use crate::runner::Config;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

#[derive(Clone)]
pub struct Connection {
    client: reqwest::Client,
    pub generation: String,
}
impl Connection {
    pub async fn call(&self, method: &str, body: Value) -> Result<Value> {
        let response = self
            .client
            .post(format!("http://localhost/{method}"))
            .json(&body)
            .send()
            .await
            .context("rclone RPC unavailable; the operation was not retried")?;
        let status = response.status();
        if !status.is_success() {
            // Upstream errors contain request inputs, which may include credentials.
            bail!(
                "rclone rejected {method} (HTTP {}); check the supplied settings",
                status.as_u16()
            );
        }
        if response
            .content_length()
            .is_some_and(|n| n > 16 * 1024 * 1024)
        {
            bail!("rclone response exceeds 16 MiB");
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                bail!("rclone response exceeds 16 MiB");
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).context("invalid rclone RPC response")
    }
}
struct Daemon {
    child: Child,
    connection: Connection,
    _directory: tempfile::TempDir,
}
pub struct RcManager {
    daemon: Mutex<Option<Daemon>>,
}
impl Default for RcManager {
    fn default() -> Self {
        Self {
            daemon: Mutex::new(None),
        }
    }
}
impl RcManager {
    pub async fn connection(&self, config: &Config) -> Result<Connection> {
        let mut guard = self.daemon.lock().await;
        if let Some(daemon) = guard.as_mut() {
            if daemon.child.try_wait()?.is_none() {
                return Ok(daemon.connection.clone());
            }
            *guard = None;
        }
        let directory = tempfile::Builder::new().prefix("rbng-").tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let socket = directory.path().join("rc.sock");
        let mut command = Command::new(&config.binary);
        command
            .args(["rcd", "--rc-no-auth", "--rc-addr"])
            .arg(format!("unix://{}", socket.display()))
            .arg("--config")
            .arg(&config.rclone_config)
            .args(["--ask-password=false", "--rc-job-expire-duration", "1h"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
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
        let mut child = command
            .spawn()
            .context("could not start private rclone RPC daemon")?;
        let connection = Connection {
            client: reqwest::Client::builder()
                .unix_socket(socket)
                .timeout(Duration::from_secs(60))
                .no_proxy()
                .build()?,
            generation: uuid::Uuid::new_v4().to_string(),
        };
        let ready = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if child.try_wait()?.is_some() { bail!("rclone daemon exited during startup; rclone 1.75.2 or newer with Unix RPC is required"); }
                if connection.call("core/version", json!({})).await.is_ok() { break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let endpoints = connection.call("rc/list", json!({})).await?;
            for required in ["sync/copy", "sync/sync", "sync/move", "sync/bisync", "operations/list", "operations/stat", "operations/copyfile", "operations/movefile", "operations/mkdir", "operations/purge", "operations/deletefile", "config/providers", "config/get", "config/listremotes", "config/create", "config/update", "config/unset", "config/delete", "fscache/clear", "job/status", "job/stop", "core/stats", "core/bwlimit"] {
                if !endpoints["commands"].as_array().is_some_and(|commands| commands.iter().any(|c| c["Path"] == required)) {
                    bail!("rclone is missing required RPC endpoint {required}; install rclone 1.75.2 or newer");
                }
            }
            Ok::<_, anyhow::Error>(())
        }).await;
        match ready {
            Ok(Ok(())) => {}
            other => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                match other {
                    Ok(Err(error)) => return Err(error),
                    _ => bail!("rclone daemon startup timed out"),
                }
            }
        }
        *guard = Some(Daemon {
            child,
            connection: connection.clone(),
            _directory: directory,
        });
        Ok(connection)
    }
    pub async fn stop(&self) {
        if let Some(mut daemon) = self.daemon.lock().await.take() {
            let _ = daemon.child.kill().await;
            let _ = daemon.child.wait().await;
        }
    }
}

/// Split a complete path into the filesystem root and path within that filesystem.
pub fn filesystem_path(path: &str) -> (String, String) {
    if path.starts_with('/') {
        ("/".into(), path.trim_start_matches('/').into())
    } else {
        let (root, remote) = path.split_once(':').expect("validated remote path");
        (format!("{root}:"), remote.into())
    }
}
