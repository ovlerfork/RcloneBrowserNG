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
    let listed: serde_json::Value = serde_json::from_slice(
        &service
            .output(&["lsjson", source.to_str().unwrap()])
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 3);
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
