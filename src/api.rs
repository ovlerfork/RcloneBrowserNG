use crate::{
    model::{Operation, Run, Task, TaskInput, TaskOptions, is_root, validate_path},
    runner::Service,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};

type App = State<Arc<Service>>;
type ApiResult<T> = Result<T, ApiError>;

pub struct ApiError(StatusCode, String);
impl ApiError {
    pub(crate) fn upstream(error: impl ToString) -> Self {
        Self(StatusCode::BAD_GATEWAY, error.to_string())
    }
    pub(crate) fn bad(error: impl ToString) -> Self {
        Self(StatusCode::BAD_REQUEST, error.to_string())
    }
    pub(crate) fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "not found".into())
    }
    pub(crate) fn conflict(error: impl ToString) -> Self {
        Self(StatusCode::CONFLICT, error.to_string())
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!(%error, "API request failed");
        Self(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

pub fn router(service: Arc<Service>) -> Router {
    let static_files = ServeDir::new(&service.config.web_root)
        .not_found_service(ServeFile::new(service.config.web_root.join("index.html")));
    Router::new()
        .merge(crate::remote_config::routes())
        .route(
            "/api/health",
            get(|| async { Json(json!({"status": "ok"})) }),
        )
        .route("/api/settings", get(settings))
        .route("/api/remotes", get(remotes))
        .route("/api/files", get(files))
        .route("/api/files/mkdir", post(mkdir))
        .route("/api/files/delete", post(delete_file))
        .route("/api/files/move", post(move_file))
        .route("/api/tasks", get(tasks).post(create_task))
        .route(
            "/api/tasks/{id}",
            get(task).put(update_task).delete(delete_task),
        )
        .route("/api/tasks/{id}/run", post(start_run))
        .route("/api/runs", get(runs))
        .route("/api/runs/{id}", get(run))
        .route("/api/runs/{id}/cancel", post(cancel_run))
        .route("/api/{*path}", get(|| async { ApiError::missing() }))
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(same_origin))
        .with_state(service)
}

async fn same_origin(
    headers: HeaderMap,
    method: Method,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS)
        && let Some(origin) = headers.get("origin")
    {
        let origin = origin.to_str().unwrap_or("");
        let authority = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"));
        if authority.is_none() || authority != headers.get("host").and_then(|v| v.to_str().ok()) {
            return ApiError(
                StatusCode::FORBIDDEN,
                "cross-origin mutations are not allowed".into(),
            )
            .into_response();
        }
    }
    let config_response = request.uri().path().starts_with("/api/config/");
    let mut response = next.run(request).await;
    if config_response {
        response
            .headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
    }
    if (response.status().is_client_error() || response.status().is_server_error())
        && response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .is_none_or(|v| !v.starts_with("application/json"))
    {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap_or_default();
        let mut response =
            ApiError(status, String::from_utf8_lossy(&bytes).into_owned()).into_response();
        if config_response {
            response
                .headers_mut()
                .insert("cache-control", "no-store".parse().unwrap());
        }
        return response;
    }
    response
}

async fn settings(State(service): App) -> Json<Value> {
    Json(
        json!({"rclone_available": service.rc.connection(&service.config).await.is_ok(), "config_path": service.config.rclone_config,
        "concurrency": 1, "timezone": "UTC"}),
    )
}
async fn remotes(State(service): App) -> ApiResult<Json<Value>> {
    let rc = service.rc.connection(&service.config).await?;
    let names = rc.call("config/listremotes", json!({})).await?;
    let mut items = Vec::new();
    for name in names["remotes"].as_array().into_iter().flatten() {
        let config = rc.call("config/get", json!({"name":name})).await?;
        items.push(json!({"name":name,"type":config["type"].as_str().unwrap_or("unknown")}));
    }
    Ok(Json(json!(items)))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}
async fn files(State(service): App, Query(query): Query<FileQuery>) -> ApiResult<Json<Value>> {
    validate_path(&query.path).map_err(ApiError::bad)?;
    let rc = service.rc.connection(&service.config).await?;
    let listed = rc
        .call("operations/list", json!({"fs":query.path,"remote":""}))
        .await?;
    let entries = listed["list"]
        .as_array()
        .ok_or_else(|| ApiError::upstream("invalid directory response"))?;
    let result: Vec<_> = entries.iter().map(|entry| {
        let name = entry["Name"].as_str().unwrap_or_default();
        let separator = if query.path.ends_with(['/', ':']) { "" } else { "/" };
        json!({"name": name, "path": format!("{}{separator}{}", query.path, entry["Path"].as_str().unwrap_or(name)),
            "size": entry["Size"], "is_dir": entry["IsDir"], "modified": entry["ModTime"]})
    }).collect();
    Ok(Json(json!(result)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathBody {
    path: String,
}
async fn mkdir(State(service): App, Json(body): Json<PathBody>) -> ApiResult<Json<Value>> {
    validate_path(&body.path).map_err(ApiError::bad)?;
    let _guard = service.file_gate.try_lock().map_err(|_| {
        ApiError::conflict("wait for active transfers or file changes before changing files")
    })?;
    let rc = service.rc.connection(&service.config).await?;
    let (fs, remote) = crate::rc::filesystem_path(&body.path);
    if let Err(error) = rc
        .call("operations/mkdir", json!({"fs":fs,"remote":remote}))
        .await
    {
        service.rc.stop().await;
        return Err(ApiError::upstream(error));
    }
    Ok(Json(json!({"ok": true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteBody {
    path: String,
    is_dir: bool,
    #[serde(default)]
    confirmed: bool,
}
async fn delete_file(State(service): App, Json(body): Json<DeleteBody>) -> ApiResult<Json<Value>> {
    if !body.confirmed {
        return Err(ApiError::bad("deletion requires confirmed: true"));
    }
    validate_path(&body.path).map_err(ApiError::bad)?;
    if is_root(&body.path) {
        return Err(ApiError::bad("cannot delete a filesystem or remote root"));
    }
    let _guard = service.file_gate.try_lock().map_err(|_| {
        ApiError::conflict("wait for active transfers or file changes before deleting files")
    })?;
    let rc = service.rc.connection(&service.config).await?;
    let (fs, remote) = crate::rc::filesystem_path(&body.path);
    if let Err(error) = rc
        .call(
            if body.is_dir {
                "operations/purge"
            } else {
                "operations/deletefile"
            },
            json!({"fs":fs,"remote":remote}),
        )
        .await
    {
        service.rc.stop().await;
        return Err(ApiError::upstream(error));
    }
    Ok(Json(json!({"ok": true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveBody {
    source: String,
    destination: String,
    #[serde(default)]
    is_dir: bool,
    #[serde(default)]
    confirmed: bool,
}
async fn move_file(
    State(service): App,
    Json(body): Json<MoveBody>,
) -> ApiResult<(StatusCode, Json<Run>)> {
    if !body.confirmed {
        return Err(ApiError::bad("moving files requires confirmed: true"));
    }
    if is_root(&body.source) {
        return Err(ApiError::bad("cannot move a filesystem or remote root"));
    }
    let input = TaskInput {
        name: "File move".into(),
        operation: Operation::Move,
        source: body.source,
        destination: body.destination,
        enabled: false,
        schedule: None,
        dry_run: false,
        destructive_confirmed: true,
        options: TaskOptions::default(),
    };
    input.validate().map_err(ApiError::bad)?;
    let now = chrono::Utc::now();
    let mut run = Run::new(
        &Task {
            id: String::new(),
            input,
            created_at: now,
            updated_at: now,
        },
        "manual",
        false,
        false,
    );
    run.task_id = None;
    run.single_file = !body.is_dir;
    service.store.enqueue_run(&run)?;
    service.notify.notify_waiters();
    Ok((StatusCode::ACCEPTED, Json(run)))
}
async fn tasks(State(service): App) -> ApiResult<Json<Vec<Task>>> {
    Ok(Json(service.store.tasks()?))
}
async fn task(State(service): App, Path(id): Path<String>) -> ApiResult<Json<Task>> {
    Ok(Json(
        service.store.task(&id)?.ok_or_else(ApiError::missing)?,
    ))
}
async fn create_task(
    State(service): App,
    Json(input): Json<TaskInput>,
) -> ApiResult<(StatusCode, Json<Task>)> {
    input.validate().map_err(ApiError::bad)?;
    Ok((
        StatusCode::CREATED,
        Json(service.store.save_task(None, input)?),
    ))
}
async fn update_task(
    State(service): App,
    Path(id): Path<String>,
    Json(input): Json<TaskInput>,
) -> ApiResult<Json<Task>> {
    input.validate().map_err(ApiError::bad)?;
    let old = service.store.task(&id)?.ok_or_else(ApiError::missing)?;
    // A bisync baseline belongs to its endpoints and filters. Changing them needs a new task and explicit initialization.
    if (old.input.operation == Operation::Bisync || input.operation == Operation::Bisync)
        && (old.input.operation != input.operation
            || old.input.source != input.source
            || old.input.destination != input.destination
            || old.input.options.exclude != input.options.exclude)
    {
        return Err(ApiError::conflict(
            "create a new bisync task when changing endpoints or filters, then explicitly initialize it",
        ));
    }
    Ok(Json(service.store.save_task(Some(&id), input)?))
}
async fn delete_task(State(service): App, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if !service.store.delete_task(&id).map_err(ApiError::conflict)? {
        return Err(ApiError::missing());
    }
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunBody {
    dry_run: Option<bool>,
    #[serde(default)]
    resync: bool,
    #[serde(default)]
    confirmed: bool,
}
async fn start_run(
    State(service): App,
    Path(id): Path<String>,
    body: Option<Json<RunBody>>,
) -> ApiResult<(StatusCode, Json<Run>)> {
    let body = body.map(|j| j.0).unwrap_or_default();
    let task = service.store.task(&id)?.ok_or_else(ApiError::missing)?;
    if body.resync && (task.input.operation != Operation::Bisync || !body.confirmed) {
        return Err(ApiError::bad(
            "resync requires a bisync task and confirmed: true",
        ));
    }
    let run = service
        .store
        .enqueue(&id, "manual", body.dry_run, body.resync, None)
        .map_err(ApiError::conflict)?
        .ok_or_else(|| ApiError::conflict("run was not queued"))?;
    service.notify.notify_waiters();
    Ok((StatusCode::ACCEPTED, Json(run)))
}
#[derive(Deserialize)]
struct RunsQuery {
    task_id: Option<String>,
    limit: Option<usize>,
}
async fn runs(State(service): App, Query(query): Query<RunsQuery>) -> ApiResult<Json<Vec<Run>>> {
    Ok(Json(service.store.runs(
        query.task_id.as_deref(),
        query.limit.unwrap_or(100).clamp(1, 500),
    )?))
}
async fn run(State(service): App, Path(id): Path<String>) -> ApiResult<Json<Run>> {
    Ok(Json(service.store.run(&id)?.ok_or_else(ApiError::missing)?))
}
async fn cancel_run(State(service): App, Path(id): Path<String>) -> ApiResult<Json<Run>> {
    let run = service.store.cancel(&id)?.ok_or_else(ApiError::missing)?;
    service.notify.notify_waiters();
    Ok(Json(run))
}
