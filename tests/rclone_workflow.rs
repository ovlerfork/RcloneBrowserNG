use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use rclone_browser_ng::{
    model::{Run, RunStatus, TaskInput},
    runner::{Config, Service},
};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
use tempfile::TempDir;
use tower::ServiceExt;

fn service(dir: &TempDir) -> Arc<Service> {
    std::fs::write(dir.path().join("rclone.conf"), "[disk]\ntype = local\n").unwrap();
    Service::new(Config {
        binary: std::env::var("RCLONE_BINARY").unwrap_or_else(|_| "rclone".into()),
        rclone_config: dir.path().join("rclone.conf"),
        data_dir: dir.path().join("state"),
        web_root: dir.path().join("web"),
        bind_addr: "127.0.0.1:0".into(),
    })
    .unwrap()
}
fn task(
    service: &Service,
    operation: &str,
    source: &Path,
    destination: &Path,
    extra: serde_json::Value,
) -> String {
    let mut value = json!({"name":operation,"operation":operation,"source":source,"destination":destination,"destructive_confirmed":true});
    for (key, val) in extra.as_object().unwrap() {
        value[key] = val.clone();
    }
    let input: TaskInput = serde_json::from_value(value).unwrap();
    service.store.save_task(None, input).unwrap().id
}
async fn finished(service: &Service, id: &str) -> Run {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let run = service.store.run(id).unwrap().unwrap();
            if !run.status.active() {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("run did not finish")
}
async fn run(service: &Service, task: &str, dry_run: bool, resync: bool) -> Run {
    let run = service
        .store
        .enqueue(task, "manual", Some(dry_run), resync, None)
        .unwrap()
        .unwrap();
    service.notify.notify_waiters();
    finished(service, &run.id).await
}

#[tokio::test]
#[ignore = "requires an installed rclone binary"]
async fn real_rclone_copy_sync_move_and_bisync_preserve_their_file_semantics() {
    let dir = TempDir::new().unwrap();
    let service = service(&dir);
    let workers = service.start();
    let source = dir.path().join("source with spaces");
    let destination = dir.path().join("destination");
    let moved = dir.path().join("moved");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(source.join("keep.txt"), "source data").unwrap();
    std::fs::write(destination.join("stale.txt"), "stale").unwrap();
    let copy = task(&service, "copy", &source, &destination, json!({}));
    let copied = run(&service, &copy, false, false).await;
    assert_eq!(copied.status, RunStatus::Succeeded, "{:?}", copied.logs);
    assert_eq!(
        std::fs::read(destination.join("keep.txt")).unwrap(),
        b"source data"
    );
    assert!(destination.join("stale.txt").exists());
    let sync = task(&service, "sync", &source, &destination, json!({}));
    assert_eq!(
        run(&service, &sync, true, false).await.status,
        RunStatus::Succeeded
    );
    assert!(destination.join("stale.txt").exists());
    assert_eq!(
        run(&service, &sync, false, false).await.status,
        RunStatus::Succeeded
    );
    assert!(!destination.join("stale.txt").exists());
    let moving = task(&service, "move", &destination, &moved, json!({}));
    assert_eq!(
        run(&service, &moving, false, false).await.status,
        RunStatus::Succeeded
    );
    assert!(!destination.join("keep.txt").exists());
    assert_eq!(
        std::fs::read(moved.join("keep.txt")).unwrap(),
        b"source data"
    );
    let bisync = task(&service, "bisync", &source, &moved, json!({}));
    let initialization = run(&service, &bisync, false, true).await;
    assert_eq!(
        initialization.status,
        RunStatus::Succeeded,
        "{:?}",
        initialization.logs
    );
    std::fs::write(source.join("from-source.txt"), "a").unwrap();
    std::fs::write(moved.join("from-destination.txt"), "b").unwrap();
    let bidirectional = run(&service, &bisync, false, false).await;
    assert_eq!(
        bidirectional.status,
        RunStatus::Succeeded,
        "{:?}",
        bidirectional.logs
    );
    assert!(moved.join("from-source.txt").exists());
    assert!(source.join("from-destination.txt").exists());
    let listed = service
        .rc
        .connection(&service.config)
        .await
        .unwrap()
        .call("operations/list", json!({"fs":source,"remote":""}))
        .await
        .unwrap();
    assert_eq!(listed["list"].as_array().unwrap().len(), 3);
    service.shutdown.cancel();
    for worker in workers {
        worker.await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an installed rclone binary"]
async fn real_rclone_cancellation_and_shutdown_reap_running_transfers() {
    let dir = TempDir::new().unwrap();
    let service = service(&dir);
    let source = dir.path().join("source");
    let destination = dir.path().join("destination");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("large.bin"), vec![42u8; 1024 * 1024]).unwrap();
    let task = task(
        &service,
        "copy",
        &source,
        &destination,
        json!({"options":{"bandwidth_limit":"1k"}}),
    );
    let workers = service.start();
    let run = service
        .store
        .enqueue(&task, "manual", None, false, None)
        .unwrap()
        .unwrap();
    service.notify.notify_waiters();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        service.store.run(&run.id).unwrap().unwrap().status,
        RunStatus::Running
    );
    let alias = service
        .store
        .save_task(
            None,
            serde_json::from_value(json!({
                "name": "Alias copy", "operation": "copy",
                "source": format!("disk:{}", source.display()),
                "destination": dir.path().join("alias destination")
            }))
            .unwrap(),
        )
        .unwrap();
    let queued = service
        .store
        .enqueue(&alias.id, "manual", None, false, None)
        .unwrap()
        .unwrap();
    service.notify.notify_waiters();
    let app = rclone_browser_ng::api::router(service.clone());
    for (route, body) in [
        ("mkdir", json!({"path": dir.path().join("new directory")})),
        (
            "delete",
            json!({"path": source.join("large.bin"), "is_dir": false, "confirmed": true}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/files/{route}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/files?path={}", source.display()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        service.store.run(&queued.id).unwrap().unwrap().status,
        RunStatus::Queued
    );
    service.store.cancel(&run.id).unwrap();
    assert_eq!(
        finished(&service, &run.id).await.status,
        RunStatus::Cancelled
    );
    assert_eq!(
        finished(&service, &queued.id).await.status,
        RunStatus::Succeeded
    );
    let run = service
        .store
        .enqueue(&task, "manual", None, false, None)
        .unwrap()
        .unwrap();
    service.notify.notify_waiters();
    tokio::time::sleep(Duration::from_secs(1)).await;
    service.shutdown.cancel();
    for worker in workers {
        worker.await.unwrap();
    }
    assert_eq!(
        service.store.run(&run.id).unwrap().unwrap().status,
        RunStatus::Interrupted
    );
}

async fn api_request(
    service: &Arc<Service>,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = rclone_browser_ng::api::router(service.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    if path.starts_with("/api/config/") {
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

#[tokio::test]
#[ignore = "requires rclone 1.75.2 or newer"]
async fn config_edit_preserves_secrets_and_rejects_duplicate_create() {
    let dir = TempDir::new().unwrap();
    let service = service(&dir);
    let secret = "long plaintext password containing many letters and numbers 12345";
    let (status, value) = api_request(&service, "POST", "/api/config/remotes", json!({"name":"ssh","type":"sftp","parameters":{"host":"localhost","pass":secret,"custom_key":"private custom value"}})).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["complete"], true);
    let rc = service.rc.connection(&service.config).await.unwrap();
    let before = rc.call("config/get", json!({"name":"ssh"})).await.unwrap();
    assert_ne!(before["pass"], secret);
    let (_, shown) = api_request(&service, "GET", "/api/config/remotes/ssh", json!(null)).await;
    assert!(
        shown["secrets"]
            .as_array()
            .unwrap()
            .contains(&json!("host"))
    );
    assert_eq!(shown["parameters"]["pass"], serde_json::Value::Null);
    assert!(
        shown["secrets"]
            .as_array()
            .unwrap()
            .contains(&json!("pass"))
    );
    assert!(
        shown["secrets"]
            .as_array()
            .unwrap()
            .contains(&json!("custom_key"))
    );
    assert!(!shown.to_string().contains("private custom value"));
    let (status, _) = api_request(
        &service,
        "POST",
        "/api/config/remotes",
        json!({"name":"ssh","type":"local","parameters":{}}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = api_request(
        &service,
        "PUT",
        "/api/config/remotes/ssh",
        json!({"parameters":{"host":"new.example"},"unset":["custom_key"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after = rc.call("config/get", json!({"name":"ssh"})).await.unwrap();
    assert_eq!(after["pass"], before["pass"]);
    assert_eq!(after["host"], "new.example");
    assert!(after.get("custom_key").is_none());
    let (status, value) = api_request(
        &service,
        "PUT",
        "/api/config/remotes/ssh",
        json!({"configure":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let reconfigured = rc.call("config/get", json!({"name":"ssh"})).await.unwrap();
    assert_eq!(reconfigured["pass"], before["pass"]);
    let guard = service.file_gate.lock().await;
    assert_eq!(
        api_request(
            &service,
            "PUT",
            "/api/config/remotes/ssh",
            json!({"parameters":{"host":"busy"}})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    drop(guard);
    assert_eq!(
        api_request(&service, "DELETE", "/api/config/remotes/ssh", json!(null))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        api_request(&service, "GET", "/api/config/remotes/ssh", json!(null))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    service.rc.stop().await;
}

#[tokio::test]
#[ignore = "requires rclone 1.75.2 or newer"]
async fn editing_a_browsed_alias_changes_subsequent_file_operations() {
    let dir = TempDir::new().unwrap();
    let service = service(&dir);
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::write(first.join("old.txt"), "old").unwrap();
    std::fs::write(second.join("new.txt"), "new").unwrap();
    let (status, value) = api_request(
        &service,
        "POST",
        "/api/config/remotes",
        json!({"name":"alias","type":"alias","parameters":{"remote":first}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let (status, listing) =
        api_request(&service, "GET", "/api/files?path=alias:", json!(null)).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing[0]["name"], "old.txt");
    let (status, value) = api_request(
        &service,
        "PUT",
        "/api/config/remotes/alias",
        json!({"parameters":{"remote":second}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    // Browsing stays available while a transfer owns the execution lock.
    let guard = service.file_gate.lock().await;
    let (status, listing) =
        api_request(&service, "GET", "/api/files?path=alias:", json!(null)).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing[0]["name"], "new.txt");
    drop(guard);
    let (status, value) = api_request(
        &service,
        "POST",
        "/api/files/mkdir",
        json!({"path":"alias:created"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert!(second.join("created").is_dir());
    assert!(!first.join("created").exists());
    service.rc.stop().await;
}

#[tokio::test]
#[ignore = "requires rclone 1.75.2 or newer"]
async fn oauth_wizard_binds_state_and_returns_headless_authorization_prompt() {
    let dir = TempDir::new().unwrap();
    let service = service(&dir);
    let (status, mut wizard) = api_request(
        &service,
        "POST",
        "/api/config/remotes",
        json!({"name":"cloud","type":"drive","parameters":{}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wizard}");
    assert_eq!(wizard["option"]["Name"], "config_shared_client_id");
    let abandoned_session = wizard["session_id"].clone();
    let (status, reopened) = api_request(
        &service,
        "PUT",
        "/api/config/remotes/cloud",
        json!({"configure":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    assert_eq!(reopened["complete"], false, "{reopened}");
    assert_eq!(reopened["option"]["Name"], "config_shared_client_id");
    assert_ne!(reopened["session_id"], abandoned_session);
    wizard = reopened;
    let session = wizard["session_id"].clone();
    assert_eq!(
        api_request(
            &service,
            "POST",
            "/api/config/remotes/disk/continue",
            json!({"session_id":session,"answer":"true"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, next) = api_request(
        &service,
        "POST",
        "/api/config/remotes/cloud/continue",
        json!({"session_id":session,"answer":"true"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{next}");
    wizard = next;
    assert_eq!(wizard["option"]["Name"], "config_token", "{wizard}");
    assert!(
        wizard["option"]["Help"]
            .as_str()
            .unwrap()
            .contains("rclone authorize")
    );
    let (_, invalid) = api_request(
        &service,
        "POST",
        "/api/config/remotes/cloud/continue",
        json!({"session_id":wizard["session_id"],"answer":"invalid sensitive answer"}),
    )
    .await;
    assert!(!invalid.to_string().contains("invalid sensitive answer"));
    assert_eq!(invalid["complete"], false, "{invalid}");
    assert!(invalid["error"].is_string(), "{invalid}");
    assert_eq!(invalid["option"]["Name"], "config_token");
    let (status, retried) = api_request(
        &service,
        "POST",
        "/api/config/remotes/cloud/continue",
        json!({"session_id":invalid["session_id"],"answer":"another invalid token"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retried}");
    assert_eq!(retried["complete"], false);
    assert_eq!(retried["option"]["Name"], "config_token");
    assert!(!retried.to_string().contains("another invalid token"));
    assert_eq!(
        api_request(
            &service,
            "POST",
            "/api/config/remotes/cloud/continue",
            json!({"session_id":session,"answer":"true"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    service.rc.stop().await;
}

#[tokio::test]
#[ignore = "requires rclone 1.75.2 or newer"]
async fn per_job_flags_file_sources_and_daemon_restart_keep_execution_isolated() {
    let dir = TempDir::new().unwrap();
    let service = service(&dir);
    let source = dir.path().join("source");
    let destination = dir.path().join("destination");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("keep.txt"), "kept").unwrap();
    std::fs::write(source.join("skip.tmp"), "excluded").unwrap();
    let workers = service.start();
    let filtered = task(
        &service,
        "copy",
        &source,
        &destination,
        json!({"options":{"exclude":["*.tmp"],"transfers":1,"checkers":2,"bandwidth_limit":"10M:10M"}}),
    );
    assert_eq!(
        run(&service, &filtered, false, false).await.status,
        RunStatus::Succeeded
    );
    assert!(destination.join("keep.txt").exists());
    assert!(!destination.join("skip.tmp").exists());
    let rc = service.rc.connection(&service.config).await.unwrap();
    let bandwidth = rc.call("core/bwlimit", json!({})).await.unwrap();
    assert_eq!(bandwidth["rate"], "off");
    let (status, listed) = api_request(
        &service,
        "GET",
        &format!("/api/files?path={}", source.display()),
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == source.join("keep.txt").to_str().unwrap())
    );
    let file_target = dir.path().join("file-target");
    let file = task(
        &service,
        "copy",
        &source.join("keep.txt"),
        &file_target,
        json!({}),
    );
    assert_eq!(
        run(&service, &file, true, false).await.status,
        RunStatus::Succeeded
    );
    assert!(!file_target.exists());
    assert_eq!(
        run(&service, &file, false, false).await.status,
        RunStatus::Succeeded
    );
    assert_eq!(
        std::fs::read(file_target.join("keep.txt")).unwrap(),
        b"kept"
    );
    let (status,moving)=api_request(&service,"POST","/api/files/move",json!({"source":file_target.join("keep.txt"),"destination":file_target.join("renamed.txt"),"confirmed":true})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        finished(&service, moving["id"].as_str().unwrap())
            .await
            .status,
        RunStatus::Succeeded
    );
    assert!(!file_target.join("keep.txt").exists());
    assert!(file_target.join("renamed.txt").exists());
    std::fs::write(source.join("large.bin"), vec![0u8; 1024 * 1024]).unwrap();
    let slow = task(
        &service,
        "copy",
        &source,
        &dir.path().join("slow"),
        json!({"options":{"bandwidth_limit":"1k"}}),
    );
    let active = service
        .store
        .enqueue(&slow, "manual", None, false, None)
        .unwrap()
        .unwrap();
    service.notify.notify_waiters();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        service.store.run(&active.id).unwrap().unwrap().status,
        RunStatus::Running
    );
    service.rc.stop().await;
    assert_eq!(
        finished(&service, &active.id).await.status,
        RunStatus::Failed
    );
    assert_eq!(
        run(&service, &file, false, false).await.status,
        RunStatus::Succeeded
    );
    let restored = service.rc.connection(&service.config).await.unwrap();
    assert_ne!(rc.generation, restored.generation);
    service.shutdown.cancel();
    for worker in workers {
        worker.await.unwrap();
    }
}
