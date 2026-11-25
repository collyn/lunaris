//! A Lunaris computer's own settings (its agent's config: direct access,
//! how connections are accepted, clipboard and files, RustDesk
//! compatibility, Wayland capture), read and changed by the server's
//! administrators.
//!
//! Nothing of them is kept here: each request goes to the computer's agent
//! (`ServerToAgentMessage::Settings`) and its answer
//! (`AgentMessage::SettingsAnswer`) is passed on, as source changes are
//! (sources.rs). The person at that computer decides whether the server may
//! change anything ("Allow the server to change this computer's settings",
//! off by default and never changed from here): while it is off the agent
//! answers with the settings to read and refuses the rest. The access
//! password passes only when asked for on its own (POST .../password
//! `{"action":"show"}`) or set (`"set"`), once, and is never logged.
//!
//! An agent from before this does not answer: after `ANSWER_TIMEOUT` the
//! client is told the agent is probably too old (`code` "agent_too_old"),
//! unless that agent has answered a settings request before (`code`
//! "timeout": it was only slow, and a change may still be made).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use common::{AgentSettingsChange, AgentSettingsMsg, SecretString, ServerToAgentMessage, SettingsRequest};
use serde::Deserialize;
use tokio::sync::oneshot;
use tracing::info;

use crate::signaling::SignalingState;

/// How long an agent has to answer (it reads a file: an agent that knows
/// the message answers at once).
#[cfg(not(test))]
const ANSWER_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(test)]
const ANSWER_TIMEOUT: Duration = Duration::from_millis(300);

/// What an agent answered.
struct Answer {
    settings: Option<AgentSettingsMsg>,
    password: Option<SecretString>,
    error: Option<String>,
    code: Option<String>,
}

/// Requests waiting for their agent: request id -> (the agent asked, who
/// waits); and the agents that have answered one (they know the message).
#[derive(Default)]
pub struct Pending(Mutex<HashMap<String, (String, oneshot::Sender<Answer>)>>, Mutex<HashSet<String>>);

/// A request's place in `Pending`, given up however the request ends
/// (also when the HTTP request is dropped while it waits).
struct Waiting<'a> {
    state: &'a SignalingState,
    request_id: String,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.state.host_settings.0.lock().unwrap().remove(&self.request_id);
    }
}

/// An agent answered. Only the agent that was asked is believed.
pub(crate) fn answered(
    state: &Arc<SignalingState>,
    agent_id: &str,
    request_id: &str,
    settings: Option<AgentSettingsMsg>,
    password: Option<SecretString>,
    error: Option<String>,
    code: Option<String>,
) {
    let mut pending = state.host_settings.0.lock().unwrap();
    if !pending.get(request_id).is_some_and(|(asked, _)| asked == agent_id) {
        return;
    }
    state.host_settings.1.lock().unwrap().insert(agent_id.to_string());
    if let Some((_, waiting)) = pending.remove(request_id) {
        let _ = waiting.send(Answer { settings, password, error, code });
    }
}

fn refused(status: StatusCode, code: &str, text: &str) -> axum::response::Response {
    (status, Json(serde_json::json!({ "error": text, "code": code }))).into_response()
}

const OFFLINE: &str = "That computer is offline. Its settings can be changed while it is connected.";
const TOO_OLD: &str = "That computer did not answer: its Lunaris agent is probably too old for this. Update it first.";
const NO_ANSWER: &str = "That computer did not answer in time. A change may still be made there: look again in a moment.";

/// Not kept by a browser or anything between it and the server.
fn not_stored(response: impl IntoResponse) -> axum::response::Response {
    let mut response = response.into_response();
    response.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    response
}

/// Asks the agent of `host_id` and waits for its answer; the error is the
/// response that says why not.
async fn ask(state: &Arc<SignalingState>, host_id: &str, request: SettingsRequest, requester: &str) -> Result<Answer, axum::response::Response> {
    let agent = state.agents.read().unwrap().get(host_id).cloned();
    let Some(agent) = agent else {
        return Err(refused(StatusCode::CONFLICT, "offline", OFFLINE));
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    let (answer_tx, answer_rx) = oneshot::channel();
    state.host_settings.0.lock().unwrap().insert(request_id.clone(), (host_id.to_string(), answer_tx));
    let _waiting = Waiting { state, request_id: request_id.clone() };
    if !matches!(request, SettingsRequest::Get) {
        // The password is not in it (`SecretString`).
        info!("Settings of {}: {:?} asked for by {}", host_id, request, requester);
    }
    let sent = agent.send(ServerToAgentMessage::Settings { request_id: request_id.clone(), request, requester: Some(requester.to_string()) });
    let answer = match sent {
        Ok(()) => tokio::time::timeout(ANSWER_TIMEOUT, answer_rx).await,
        Err(()) => return Err(refused(StatusCode::CONFLICT, "offline", OFFLINE)),
    };
    let gone = || !state.agents.read().unwrap().contains_key(host_id);
    let knows_settings = || state.host_settings.1.lock().unwrap().contains(host_id);
    let answer = match answer {
        Ok(Ok(answer)) => answer,
        Ok(Err(_)) => return Err(refused(StatusCode::CONFLICT, "offline", OFFLINE)),
        // It went away meanwhile: that is why it did not answer.
        Err(_) if gone() => return Err(refused(StatusCode::CONFLICT, "offline", OFFLINE)),
        // It has answered before: only slow this time.
        Err(_) if knows_settings() => return Err(refused(StatusCode::GATEWAY_TIMEOUT, "timeout", NO_ANSWER)),
        Err(_) => return Err(refused(StatusCode::GATEWAY_TIMEOUT, "agent_too_old", TOO_OLD)),
    };
    if let Some(error) = &answer.error {
        let code = answer.code.as_deref().unwrap_or(common::SETTINGS_FAILED);
        let status = match code {
            common::SETTINGS_NOT_ALLOWED => StatusCode::FORBIDDEN,
            common::SETTINGS_INVALID => StatusCode::BAD_REQUEST,
            _ => StatusCode::BAD_GATEWAY,
        };
        return Err(refused(status, code, error));
    }
    Ok(answer)
}

/// The settings of an answer, as the response.
fn settings_response(answer: Answer) -> axum::response::Response {
    match answer.settings {
        Some(settings) => Json(settings).into_response(),
        None => refused(StatusCode::BAD_GATEWAY, common::SETTINGS_FAILED, "That computer did not say what its settings are"),
    }
}

/// GET /api/admin/hosts/{id}/settings: the settings (never the password);
/// `remote_changes` false means they are for reading only.
pub async fn get_settings(admin: crate::auth::AdminUser, Path(host_id): Path<String>, State(state): State<Arc<SignalingState>>) -> impl IntoResponse {
    match ask(&state, &host_id, SettingsRequest::Get, &admin.username).await {
        Ok(answer) => settings_response(answer),
        Err(response) => response,
    }
}

/// The fields a change may have (`AgentSettingsChange`); anything else is
/// refused, `remote_changes` above all.
const CHANGE_FIELDS: [&str; 11] = [
    "direct_enabled",
    "direct_port",
    "approve_mode",
    "server_sessions",
    "allow_clipboard",
    "allow_file_transfer",
    "file_access",
    "rustdesk_enabled",
    "rustdesk_port",
    "rustdesk_file_transfer",
    "wayland_capture",
];

/// The change in a request's body, checked; or the response that says why
/// it is not one.
fn change_of(body: serde_json::Value) -> Result<AgentSettingsChange, axum::response::Response> {
    let bad = |text: &str| refused(StatusCode::BAD_REQUEST, common::SETTINGS_INVALID, text);
    let Some(fields) = body.as_object() else {
        return Err(bad("Send the settings to change as a JSON object"));
    };
    if fields.contains_key("remote_changes") || fields.contains_key("allow_server_settings") {
        return Err(refused(
            StatusCode::FORBIDDEN,
            common::SETTINGS_NOT_ALLOWED,
            "Whether the server may change a computer's settings is only set at that computer",
        ));
    }
    if let Some(unknown) = fields.keys().find(|key| !CHANGE_FIELDS.contains(&key.as_str())) {
        return Err(bad(&format!("Unknown setting {}", unknown.chars().take(64).collect::<String>())));
    }
    let change: AgentSettingsChange = serde_json::from_value(body).map_err(|_| bad("A setting has a value of the wrong kind"))?;
    if change == AgentSettingsChange::default() {
        return Err(bad("Nothing to change"));
    }
    // The port in effect there is not known here (0): the agent checks it
    // against its own again.
    if let Some(error) = common::settings_change_error(&change, 0) {
        return Err(bad(&error));
    }
    Ok(change)
}

/// PUT /api/admin/hosts/{id}/settings: changes the settings in the body (the
/// fields of `AgentSettingsChange`; those left out stay); answered with the
/// settings as they are now.
pub async fn put_settings(
    admin: crate::auth::AdminUser,
    Path(host_id): Path<String>,
    State(state): State<Arc<SignalingState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let change = match change_of(body) {
        Ok(change) => change,
        Err(response) => return response,
    };
    match ask(&state, &host_id, SettingsRequest::Change { change }, &admin.username).await {
        Ok(answer) => settings_response(answer),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordForm {
    /// "show", "random" or "set".
    pub action: String,
    #[serde(default)]
    pub password: Option<String>,
}

/// POST /api/admin/hosts/{id}/settings/password: `{"action":"show"}` answers
/// `{"password"}`; `{"action":"random"}` and `{"action":"set","password"}`
/// give the computer a new access password and answer with the settings
/// (not the password: it is shown when asked for).
pub async fn password(
    admin: crate::auth::AdminUser,
    Path(host_id): Path<String>,
    State(state): State<Arc<SignalingState>>,
    Json(form): Json<PasswordForm>,
) -> impl IntoResponse {
    let request = match (form.action.as_str(), form.password) {
        ("show", None) => SettingsRequest::ShowPassword,
        ("random", None) => SettingsRequest::NewPassword { password: None },
        ("set", Some(password)) if common::valid_access_password(&password) => {
            SettingsRequest::NewPassword { password: Some(SecretString(password)) }
        }
        ("set", _) => {
            return refused(
                StatusCode::BAD_REQUEST,
                common::SETTINGS_INVALID,
                &format!(
                    "The password needs {} to {} characters, without spaces at either end",
                    common::MIN_ACCESS_PASSWORD_CHARS,
                    common::MAX_ACCESS_PASSWORD_CHARS
                ),
            )
        }
        _ => return refused(StatusCode::BAD_REQUEST, common::SETTINGS_INVALID, "The action is \"show\", \"random\" or \"set\""),
    };
    let show = matches!(request, SettingsRequest::ShowPassword);
    match ask(&state, &host_id, request, &admin.username).await {
        Ok(Answer { password: Some(SecretString(password)), .. }) if show => not_stored(Json(serde_json::json!({ "password": password }))),
        Ok(_) if show => refused(StatusCode::BAD_GATEWAY, common::SETTINGS_FAILED, "That computer did not say its password"),
        Ok(answer) => settings_response(answer),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::AgentMessage;

    async fn state() -> Arc<SignalingState> {
        let db = crate::security::tests::test_db().await;
        Arc::new(SignalingState::new(db, "t".into(), None))
    }

    fn admin() -> crate::auth::AdminUser {
        crate::auth::AdminUser { user_id: "admin".into(), username: "admin".into() }
    }

    fn connect(state: &Arc<SignalingState>, host: &str) -> crate::queue::Inbox<ServerToAgentMessage> {
        let (tx, rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert(host.into(), tx);
        rx
    }

    async fn body(response: impl IntoResponse) -> (StatusCode, serde_json::Value, Option<String>) {
        let response = response.into_response();
        let status = response.status();
        let cache = response.headers().get(axum::http::header::CACHE_CONTROL).and_then(|v| v.to_str().ok()).map(str::to_string);
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null), cache)
    }

    fn settings(remote_changes: bool) -> AgentSettingsMsg {
        AgentSettingsMsg {
            remote_changes,
            direct_enabled: true,
            direct_port: 8091,
            has_password: true,
            approve_mode: "password-click".into(),
            server_sessions: "trusted".into(),
            allow_clipboard: true,
            allow_file_transfer: true,
            file_access: "home".into(),
            rustdesk_port: 21118,
            wayland_capture: "auto".into(),
            version: "0.6.7".into(),
            ..Default::default()
        }
    }

    /// A pretend agent: answers the next request with `answer(request)`.
    fn agent_answers(
        state: &Arc<SignalingState>,
        mut to_agent: crate::queue::Inbox<ServerToAgentMessage>,
        host: &'static str,
        answer: impl FnOnce(&SettingsRequest) -> (Option<AgentSettingsMsg>, Option<SecretString>, Option<(&'static str, &'static str)>) + Send + 'static,
    ) -> tokio::task::JoinHandle<(SettingsRequest, crate::queue::Inbox<ServerToAgentMessage>)> {
        let state = state.clone();
        tokio::spawn(async move {
            let Some(ServerToAgentMessage::Settings { request_id, request, requester }) = to_agent.recv().await else { panic!("no Settings") };
            assert_eq!(requester.as_deref(), Some("admin"));
            let (settings, password, refusal) = answer(&request);
            // Another agent's answer to the same request is not believed.
            crate::signaling::agent_message(
                &state,
                "other",
                AgentMessage::SettingsAnswer { request_id: request_id.clone(), settings: None, password: None, error: Some("forged".into()), code: None },
            )
            .await;
            crate::signaling::agent_message(
                &state,
                host,
                AgentMessage::SettingsAnswer {
                    request_id,
                    settings,
                    password,
                    error: refusal.map(|r| r.1.to_string()),
                    code: refusal.map(|r| r.0.to_string()),
                },
            )
            .await;
            (request, to_agent)
        })
    }

    #[tokio::test]
    async fn the_settings_are_read_from_the_agent_without_the_password() {
        let state = state().await;
        let to_agent = connect(&state, "h1");
        let agent = agent_answers(&state, to_agent, "h1", |request| {
            assert_eq!(*request, SettingsRequest::Get);
            (Some(settings(false)), None, None)
        });
        let (status, answer, _) = body(get_settings(admin(), Path("h1".into()), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["remote_changes"], false);
        assert_eq!(answer["direct_port"], 8091);
        assert!(answer.get("password").is_none() && answer.get("direct_password").is_none());
        agent.await.unwrap();
        assert!(state.host_settings.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_change_is_checked_here_and_passed_on() {
        let state = state().await;
        for (body_json, status) in [
            (serde_json::json!({ "direct_port": 80 }), StatusCode::BAD_REQUEST),
            (serde_json::json!({ "approve_mode": "always" }), StatusCode::BAD_REQUEST),
            (serde_json::json!({ "wayland_capture": "kms" }), StatusCode::BAD_REQUEST),
            (serde_json::json!({ "file_access": "/etc" }), StatusCode::BAD_REQUEST),
            (serde_json::json!({ "direct_password": "x" }), StatusCode::BAD_REQUEST),
            (serde_json::json!({ "direct_port": "9000" }), StatusCode::BAD_REQUEST),
            (serde_json::json!({}), StatusCode::BAD_REQUEST),
            (serde_json::json!([1]), StatusCode::BAD_REQUEST),
            // The switch is only ever set at the computer.
            (serde_json::json!({ "remote_changes": true }), StatusCode::FORBIDDEN),
            (serde_json::json!({ "allow_server_settings": true, "direct_port": 9000 }), StatusCode::FORBIDDEN),
        ] {
            let (got, _, _) = body(put_settings(admin(), Path("h1".into()), State(state.clone()), Json(body_json.clone())).await).await;
            assert_eq!(got, status, "{}", body_json);
        }
        // Offline (nothing was sent anywhere above either).
        let (status, answer, _) = body(put_settings(admin(), Path("h1".into()), State(state.clone()), Json(serde_json::json!({ "direct_port": 9000 }))).await).await;
        assert_eq!((status, answer["code"].as_str()), (StatusCode::CONFLICT, Some("offline")));

        let to_agent = connect(&state, "h1");
        let agent = agent_answers(&state, to_agent, "h1", |request| {
            let SettingsRequest::Change { change } = request else { panic!("{:?}", request) };
            assert_eq!(*change, AgentSettingsChange { direct_port: Some(9000), allow_clipboard: Some(false), ..Default::default() });
            let mut now = settings(true);
            now.direct_port = 9000;
            now.allow_clipboard = false;
            (Some(now), None, None)
        });
        let change = serde_json::json!({ "direct_port": 9000, "allow_clipboard": false });
        let (status, answer, _) = body(put_settings(admin(), Path("h1".into()), State(state.clone()), Json(change)).await).await;
        assert_eq!((status, answer["direct_port"].as_u64(), answer["allow_clipboard"].as_bool()), (StatusCode::OK, Some(9000), Some(false)));
        agent.await.unwrap();
    }

    #[tokio::test]
    async fn the_agent_refusing_is_told_as_it_is() {
        let state = state().await;
        let to_agent = connect(&state, "h1");
        let agent = agent_answers(&state, to_agent, "h1", |_| (None, None, Some((common::SETTINGS_NOT_ALLOWED, "not allowed there"))));
        let (status, answer, _) = body(put_settings(admin(), Path("h1".into()), State(state.clone()), Json(serde_json::json!({ "allow_clipboard": false }))).await).await;
        assert_eq!((status, answer["code"].as_str(), answer["error"].as_str()), (StatusCode::FORBIDDEN, Some("not_allowed"), Some("not allowed there")));
        let (_, to_agent) = agent.await.unwrap();
        let agent = agent_answers(&state, to_agent, "h1", |_| (None, None, Some((common::SETTINGS_INVALID, "That port is the one for direct connections"))));
        let (status, _, _) = body(put_settings(admin(), Path("h1".into()), State(state.clone()), Json(serde_json::json!({ "rustdesk_port": 9000 }))).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        agent.await.unwrap();
    }

    #[tokio::test]
    async fn the_password_is_shown_only_when_asked_and_not_kept() {
        let state = state().await;
        let to_agent = connect(&state, "h1");
        let agent = agent_answers(&state, to_agent, "h1", |request| {
            assert_eq!(*request, SettingsRequest::ShowPassword);
            (Some(settings(true)), Some(SecretString("k7wq-3mzp-x9tr".into())), None)
        });
        let (status, answer, cache) = body(password(admin(), Path("h1".into()), State(state.clone()), Json(PasswordForm { action: "show".into(), password: None })).await).await;
        assert_eq!((status, answer["password"].as_str(), cache.as_deref()), (StatusCode::OK, Some("k7wq-3mzp-x9tr"), Some("no-store")));
        let (_, to_agent) = agent.await.unwrap();

        // A new one: the settings come back, not the password.
        let agent = agent_answers(&state, to_agent, "h1", |request| {
            assert_eq!(*request, SettingsRequest::NewPassword { password: Some(SecretString("my own password".into())) });
            (Some(settings(true)), None, None)
        });
        let form = PasswordForm { action: "set".into(), password: Some("my own password".into()) };
        let (status, answer, _) = body(password(admin(), Path("h1".into()), State(state.clone()), Json(form)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert!(answer.get("password").is_none() && answer["has_password"] == true);
        let (_, to_agent) = agent.await.unwrap();
        let agent = agent_answers(&state, to_agent, "h1", |request| {
            assert_eq!(*request, SettingsRequest::NewPassword { password: None });
            (Some(settings(true)), None, None)
        });
        let (status, _, _) = body(password(admin(), Path("h1".into()), State(state.clone()), Json(PasswordForm { action: "random".into(), password: None })).await).await;
        assert_eq!(status, StatusCode::OK);
        agent.await.unwrap();

        // Checked here before anything is sent.
        for form in [
            PasswordForm { action: "set".into(), password: Some("short".into()) },
            PasswordForm { action: "set".into(), password: None },
            PasswordForm { action: "show".into(), password: Some("x".into()) },
            PasswordForm { action: "steal".into(), password: None },
        ] {
            let (status, _, _) = body(password(admin(), Path("h1".into()), State(state.clone()), Json(form)).await).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn an_agent_that_does_not_answer_is_too_old() {
        let state = state().await;
        // An agent from before this: it drops the message it does not know.
        let _to_agent = connect(&state, "h1");
        let (status, answer, _) = body(get_settings(admin(), Path("h1".into()), State(state.clone())).await).await;
        assert_eq!((status, answer["code"].as_str()), (StatusCode::GATEWAY_TIMEOUT, Some("agent_too_old")));
        assert!(state.host_settings.0.lock().unwrap().is_empty());
        // One that went away meanwhile is offline, not old.
        let state2 = state.clone();
        let asking = tokio::spawn(async move { body(get_settings(admin(), Path("h1".into()), State(state2)).await).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        state.agents.write().unwrap().remove("h1");
        let (status, answer, _) = asking.await.unwrap();
        assert_eq!((status, answer["code"].as_str()), (StatusCode::CONFLICT, Some("offline")));
    }

    /// One that answered before was only slow: not called too old.
    #[tokio::test]
    async fn an_agent_that_answered_before_is_only_late() {
        let state = state().await;
        let to_agent = connect(&state, "h1");
        let agent = agent_answers(&state, to_agent, "h1", |_| (Some(settings(true)), None, None));
        assert_eq!(body(get_settings(admin(), Path("h1".into()), State(state.clone())).await).await.0, StatusCode::OK);
        let (_, _to_agent) = agent.await.unwrap();
        let (status, answer, _) = body(get_settings(admin(), Path("h1".into()), State(state.clone())).await).await;
        assert_eq!((status, answer["code"].as_str()), (StatusCode::GATEWAY_TIMEOUT, Some("timeout")));
        assert!(!answer["error"].as_str().unwrap().contains("too old"));
    }

    /// A request given up while it waits (the browser went away) leaves
    /// nothing behind.
    #[tokio::test]
    async fn a_dropped_request_is_not_kept() {
        let state = state().await;
        let _to_agent = connect(&state, "h1");
        let state2 = state.clone();
        let asking = tokio::spawn(async move { get_settings(admin(), Path("h1".into()), State(state2)).await.into_response() });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(state.host_settings.0.lock().unwrap().len(), 1);
        asking.abort();
        let _ = asking.await;
        assert!(state.host_settings.0.lock().unwrap().is_empty());
    }
}
