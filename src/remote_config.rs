use crate::{api::ApiError, rc::Connection, runner::Service};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Session {
    name: String,
    state: String,
    generation: String,
    touched: Instant,
    option: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    parameters: HashMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    #[serde(default)]
    parameters: HashMap<String, String>,
    #[serde(default)]
    unset: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Continue {
    session_id: String,
    answer: String,
}
type App = State<Arc<Service>>;
type Result<T> = std::result::Result<T, ApiError>;

pub fn routes() -> Router<Arc<Service>> {
    Router::new()
        .route("/api/config/providers", get(providers))
        .route("/api/config/remotes", post(create))
        .route(
            "/api/config/remotes/{name}",
            get(remote).put(update).delete(delete),
        )
        .route("/api/config/remotes/{name}/continue", post(continue_config))
}
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 200
        || name.starts_with('-')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-. ".contains(c))
    {
        return Err(ApiError::bad(
            "remote name must contain letters, digits, spaces, underscores, dots, or hyphens and cannot start with a hyphen",
        ));
    }
    Ok(())
}
fn validate_parameters(parameters: &HashMap<String, String>, unset: &[String]) -> Result<()> {
    if parameters.len() + unset.len() > 512
        || parameters.keys().chain(unset).any(|key| {
            key.is_empty() || key.len() > 200 || key == "type" || key.contains(['\0', '\n', '\r'])
        })
    {
        return Err(ApiError::bad(
            "parameter names must be nonempty single-line strings; type cannot be changed or removed",
        ));
    }
    if unset.iter().any(|key| parameters.contains_key(key)) {
        return Err(ApiError::bad(
            "a parameter cannot be updated and removed together",
        ));
    }
    if parameters.values().any(|v| v.contains(['\0', '\n', '\r'])) {
        return Err(ApiError::bad(
            "configuration values must be single-line strings",
        ));
    }
    Ok(())
}
async fn call(service: &Service, rc: &Connection, endpoint: &str, body: Value) -> Result<Value> {
    match rc.call(endpoint, body).await {
        Ok(value) => Ok(value),
        Err(error) => {
            service.rc.stop().await;
            Err(ApiError::upstream(error))
        }
    }
}
async fn connection(service: &Service) -> Result<Connection> {
    service
        .rc
        .connection(&service.config)
        .await
        .map_err(ApiError::upstream)
}
fn busy() -> ApiError {
    ApiError::conflict(
        "wait for active transfers or configuration/file changes before editing configuration",
    )
}
async fn names(service: &Service, rc: &Connection) -> Result<Vec<String>> {
    let response = call(service, rc, "config/listremotes", json!({})).await?;
    Ok(response["remotes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_str().map(str::to_owned))
        .collect())
}
async fn require_remote(service: &Service, rc: &Connection, name: &str) -> Result<()> {
    validate_name(name)?;
    if !names(service, rc).await?.iter().any(|n| n == name) {
        return Err(ApiError::missing());
    }
    Ok(())
}
async fn providers(State(service): App) -> Result<Json<Value>> {
    let _guard = service.file_gate.try_lock().map_err(|_| busy())?;
    let rc = connection(&service).await?;
    Ok(Json(
        call(&service, &rc, "config/providers", json!({})).await?,
    ))
}
async fn remote(State(service): App, Path(name): Path<String>) -> Result<Json<Value>> {
    let _guard = service.file_gate.try_lock().map_err(|_| busy())?;
    let rc = connection(&service).await?;
    require_remote(&service, &rc, &name).await?;
    let values = call(&service, &rc, "config/get", json!({"name":name})).await?;
    let kind = values["type"].as_str().unwrap_or("unknown");
    let schema = call(&service, &rc, "config/providers", json!({})).await?;
    let options = schema["providers"]
        .as_array()
        .and_then(|providers| providers.iter().find(|p| p["Name"] == kind))
        .and_then(|p| p["Options"].as_array());
    let visible: HashSet<&str> = options
        .into_iter()
        .flatten()
        .filter(|o| o["IsPassword"] != true && o["Sensitive"] != true)
        .filter_map(|o| o["Name"].as_str())
        .collect();
    let mut parameters = serde_json::Map::new();
    let mut secrets = Vec::new();
    for (key, value) in values
        .as_object()
        .ok_or_else(|| ApiError::upstream("invalid remote configuration response"))?
    {
        if key == "type" {
            continue;
        }
        if visible.contains(key.as_str()) {
            parameters.insert(key.clone(), value.clone());
        } else {
            secrets.push(key.clone());
        }
    }
    Ok(Json(
        json!({"name":name,"type":kind,"parameters":parameters,"secrets":secrets}),
    ))
}
async fn create(State(service): App, Json(body): Json<Create>) -> Result<Json<Value>> {
    validate_name(&body.name)?;
    validate_parameters(&body.parameters, &[])?;
    let _guard = service.file_gate.try_lock().map_err(|_| busy())?;
    let rc = connection(&service).await?;
    if names(&service, &rc).await?.contains(&body.name) {
        return Err(ApiError::conflict("a remote with this name already exists"));
    }
    let providers = call(&service, &rc, "config/providers", json!({})).await?;
    if !providers["providers"]
        .as_array()
        .is_some_and(|p| p.iter().any(|p| p["Name"] == body.kind))
    {
        return Err(ApiError::bad("unknown rclone provider"));
    }
    let result = call(&service, &rc, "config/create", json!({"name":body.name,"type":body.kind,"parameters":body.parameters,"opt":{"nonInteractive":true,"obscure":true}})).await?;
    wizard(&service, &rc, &body.name, result).await
}
async fn update(
    State(service): App,
    Path(name): Path<String>,
    Json(body): Json<Update>,
) -> Result<Json<Value>> {
    validate_parameters(&body.parameters, &body.unset)?;
    let _guard = service.file_gate.try_lock().map_err(|_| busy())?;
    let rc = connection(&service).await?;
    require_remote(&service, &rc, &name).await?;
    service
        .config_sessions
        .lock()
        .await
        .retain(|_, session| session.name != name);
    if !body.unset.is_empty() {
        call(
            &service,
            &rc,
            "config/unset",
            json!({"name":name,"keys":body.unset}),
        )
        .await?;
    }
    let result = if body.parameters.is_empty() {
        json!({})
    } else {
        call(&service, &rc,"config/update",json!({"name":name,"parameters":body.parameters,"opt":{"nonInteractive":true,"obscure":true}})).await?
    };
    wizard(&service, &rc, &name, result).await
}
async fn delete(State(service): App, Path(name): Path<String>) -> Result<StatusCode> {
    let _guard = service.file_gate.try_lock().map_err(|_| busy())?;
    let rc = connection(&service).await?;
    require_remote(&service, &rc, &name).await?;
    call(&service, &rc, "config/delete", json!({"name":name})).await?;
    service
        .config_sessions
        .lock()
        .await
        .retain(|_, session| session.name != name);
    Ok(StatusCode::NO_CONTENT)
}
async fn continue_config(
    State(service): App,
    Path(name): Path<String>,
    Json(body): Json<Continue>,
) -> Result<Json<Value>> {
    let _guard = service.file_gate.try_lock().map_err(|_| busy())?;
    let rc = connection(&service).await?;
    let mut sessions = service.config_sessions.lock().await;
    let session = sessions
        .get(&body.session_id)
        .ok_or_else(|| ApiError::conflict("configuration session expired; reopen the remote"))?;
    if session.name != name
        || session.generation != rc.generation
        || session.touched.elapsed() > Duration::from_secs(1800)
    {
        return Err(ApiError::conflict(
            "configuration session expired or belongs to another remote",
        ));
    }
    let state = session.state.clone();
    let previous_option = session.option.clone();
    sessions.remove(&body.session_id);
    drop(sessions);
    let mut result = call(&service, &rc,"config/update",json!({"name":name,"parameters":{},"opt":{"nonInteractive":true,"continue":true,"state":state,"result":body.answer,"obscure":true}})).await?;
    if result["Option"].is_null()
        && result["Error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    {
        result["Option"] = previous_option;
    }
    wizard(&service, &rc, &name, result).await
}
async fn wizard(
    service: &Service,
    rc: &Connection,
    name: &str,
    mut result: Value,
) -> Result<Json<Value>> {
    // A browser cannot complete a provider's callback on the container's loopback interface.
    for _ in 0..4 {
        if result["Option"]["Name"] != "config_is_local" {
            break;
        }
        result = call(service, rc,"config/update",json!({"name":name,"parameters":{},"opt":{"nonInteractive":true,"continue":true,"state":result["State"],"result":"false","obscure":true}})).await?;
    }
    let state = result["State"].as_str().unwrap_or("");
    let mut sessions = service.config_sessions.lock().await;
    sessions.retain(|_, s| s.touched.elapsed() < Duration::from_secs(1800) && s.name != name);
    let session_id = if state.is_empty() {
        None
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        sessions.insert(
            id.clone(),
            Session {
                name: name.into(),
                state: state.into(),
                generation: rc.generation.clone(),
                touched: Instant::now(),
                option: result["Option"].clone(),
            },
        );
        Some(id)
    };
    // Invalid answers may be echoed in Error. Keep native option help, but never return that error text.
    let error = result["Error"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|_| "rclone rejected the answer; check the requested format and try again");
    let mut option = result["Option"].clone();
    if (option["IsPassword"] == true
        || option["Sensitive"] == true
        || option["Name"] == "config_token")
        && let Some(object) = option.as_object_mut()
    {
        for key in ["Value", "ValueStr", "Default", "DefaultStr"] {
            object.insert(key.into(), json!(""));
        }
    }
    Ok(Json(
        json!({"name":name,"session_id":session_id,"option":option,"error":error,"complete":state.is_empty()}),
    ))
}
