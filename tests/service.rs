use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::TimeZone;
use rclone_browser_ng::{
    api,
    model::{RunStatus, TaskInput},
    runner::{Config, Service},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

fn service(directory: &TempDir) -> Arc<Service> {
    Service::new(Config {
        binary: "rclone".into(),
        rclone_config: directory.path().join("rclone.conf"),
        data_dir: directory.path().join("state"),
        web_root: directory.path().join("web"),
        bind_addr: "127.0.0.1:0".into(),
        concurrency: 2,
    })
    .unwrap()
}
fn input(name: &str, source: &str, destination: &str) -> TaskInput {
    serde_json::from_value(
        json!({"name":name,"operation":"copy","source":source,"destination":destination}),
    )
    .unwrap()
}
async fn request(app: Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
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
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

#[tokio::test]
async fn task_api_requires_destructive_acknowledgment_and_freezes_runs() {
    let dir = TempDir::new().unwrap();
    let service = service(&dir);
    let app = api::router(service.clone());
    let body = json!({"name":"Mirror","operation":"sync","source":"test:source","destination":"test:destination"});
    assert_eq!(
        request(app.clone(), "POST", "/api/tasks", body.clone())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut body = body;
    body["destructive_confirmed"] = json!(true);
    let (status, task) = request(app.clone(), "POST", "/api/tasks", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = task["id"].as_str().unwrap();
    let path = format!("/api/tasks/{id}/run");
    let (status, run) = request(app.clone(), "POST", &path, json!({"dry_run":true})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        request(app.clone(), "POST", &path, json!({})).await.0,
        StatusCode::CONFLICT
    );
    body["destination"] = json!("test:other");
    assert_eq!(
        request(app.clone(), "PUT", &format!("/api/tasks/{id}"), body)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        service
            .store
            .run(run["id"].as_str().unwrap())
            .unwrap()
            .unwrap()
            .destination,
        "test:destination"
    );
    assert_eq!(
        request(
            app.clone(),
            "DELETE",
            &format!("/api/tasks/{id}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let run_id = run["id"].as_str().unwrap();
    let (_, cancelled) = request(
        app.clone(),
        "POST",
        &format!("/api/runs/{run_id}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(
        request(app, "DELETE", &format!("/api/tasks/{id}"), Value::Null)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert!(service.store.run(run_id).unwrap().is_some());
}

#[test]
fn scheduling_deduplicates_and_resource_locks_serialize_overlapping_tasks() {
    let dir = TempDir::new().unwrap();
    let service = service(&dir);
    let mut first = input("First", "test:a", "test:b");
    first.schedule = Some("*/5 * * * *".into());
    let first = service.store.save_task(None, first).unwrap();
    let second = service
        .store
        .save_task(None, input("Second", "test:b/sub", "test:c"))
        .unwrap();
    let third = service
        .store
        .save_task(None, input("Third", "other:a", "other:b"))
        .unwrap();
    let time = chrono::Utc
        .with_ymd_and_hms(2026, 10, 14, 12, 5, 0)
        .unwrap();
    service.schedule_at(time).unwrap();
    let run = service.store.claim().unwrap().unwrap();
    assert_eq!(run.task_id.as_deref(), Some(first.id.as_str()));
    service
        .store
        .enqueue(&second.id, "manual", None, false, None)
        .unwrap();
    let independent = service
        .store
        .enqueue(&third.id, "manual", None, false, None)
        .unwrap()
        .unwrap();
    assert_eq!(service.store.claim().unwrap().unwrap().id, independent.id);
    assert!(service.store.claim().unwrap().is_none());
    service
        .store
        .finish(&run.id, RunStatus::Succeeded, Some(0), None)
        .unwrap();
    service.schedule_at(time).unwrap();
    assert_eq!(service.store.runs(Some(&first.id), 10).unwrap().len(), 1);
    assert_eq!(
        service.store.claim().unwrap().unwrap().task_id.as_deref(),
        Some(second.id.as_str())
    );
    // Sunday uses the ordinary cron value 0; day-of-month and weekday have OR semantics.
    let cron = rclone_browser_ng::model::parse_schedule("0 0 1 * 0").unwrap();
    assert!(
        cron.is_time_matching(&chrono::Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap())
            .unwrap()
    );
}

#[test]
fn restart_interrupts_active_work_and_preserves_history_without_replay() {
    let dir = TempDir::new().unwrap();
    let first = service(&dir);
    let task = first
        .store
        .save_task(None, input("Copy", "test:a", "test:b"))
        .unwrap();
    let run = first
        .store
        .enqueue(&task.id, "manual", None, false, None)
        .unwrap()
        .unwrap();
    assert!(Service::new(first.config.clone()).is_err());
    first.store.claim().unwrap();
    let second_task = first
        .store
        .save_task(None, input("Queued", "other:a", "other:b"))
        .unwrap();
    let queued = first
        .store
        .enqueue(&second_task.id, "manual", None, false, None)
        .unwrap()
        .unwrap();
    drop(first);
    let restarted = service(&dir);
    for id in [run.id, queued.id] {
        assert_eq!(
            restarted.store.run(&id).unwrap().unwrap().status,
            RunStatus::Interrupted
        );
    }
    assert!(restarted.store.claim().unwrap().is_none());
    assert_eq!(restarted.store.tasks().unwrap().len(), 2);
}

#[tokio::test]
async fn mutation_api_rejects_cross_origin_and_root_deletion() {
    let dir = TempDir::new().unwrap();
    let app = api::router(service(&dir));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tasks")
                .header("host", "localhost:8080")
                .header("origin", "https://untrusted.example")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let (status, error) = request(
        app,
        "POST",
        "/api/files/delete",
        json!({"path":"test:folder/..","is_dir":true,"confirmed":true}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error["error"].as_str().unwrap().contains("root"));
}
