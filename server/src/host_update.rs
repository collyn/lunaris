//! Updating a Lunaris computer's agent from the web (administrators).
//!
//! POST /api/admin/hosts/{id}/update asks the computer's agent to become
//! this server's version (`ServerToAgentMessage::Update`). The server names
//! a version and nothing else: the agent downloads that release from the
//! project's GitHub releases itself and installs it only when the release's
//! signature (the key built into the agent) and the file's checksum are
//! right, so neither this server nor whoever holds an administrator's
//! account can make a computer run anything but a release of the project.
//! The person at the computer can turn this off there
//! (`lunaris-agent set server-update deny`).
//!
//! The agent answers once (`AgentMessage::UpdateAnswer`): the installer was
//! started, or why not. It then restarts as the new version and says so
//! when it connects again (`AgentBuild` of its `Sources`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use common::ServerToAgentMessage;
use tokio::sync::oneshot;
use tracing::info;

use crate::signaling::SignalingState;

/// How long the agent has to download and check the release and start its
/// installer (a hundred megabytes over a slow line).
#[cfg(not(test))]
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15 * 60);
#[cfg(test)]
const ANSWER_TIMEOUT: Duration = Duration::from_millis(300);

struct Answer {
    error: Option<String>,
    code: Option<String>,
}

/// Requests waiting for their agent: request id -> (the agent asked, who waits).
#[derive(Default)]
pub struct Pending(Mutex<HashMap<String, (String, oneshot::Sender<Answer>)>>);

struct Waiting<'a> {
    state: &'a SignalingState,
    request_id: String,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.state.host_updates.0.lock().unwrap().remove(&self.request_id);
    }
}

/// An agent answered. Only the agent that was asked is believed.
pub(crate) fn answered(state: &Arc<SignalingState>, agent_id: &str, request_id: &str, error: Option<String>, code: Option<String>) {
    let mut pending = state.host_updates.0.lock().unwrap();
    if !pending.get(request_id).is_some_and(|(asked, _)| asked == agent_id) {
        return;
    }
    if let Some((_, waiting)) = pending.remove(request_id) {
        let _ = waiting.send(Answer { error, code });
    }
}

fn refused(status: StatusCode, code: &str, text: &str) -> axum::response::Response {
    (status, Json(serde_json::json!({ "error": text, "code": code }))).into_response()
}

const OFFLINE: &str = "That computer is offline. Its agent can be updated while it is connected.";
const TOO_OLD: &str = "That computer's agent is from before updates from the web: update it once with the command, and from then on this works.";
const NO_ANSWER: &str = "That computer did not answer in time. The update may still be running there: look again in a few minutes.";
const ALREADY: &str = "That computer's agent already is this server's version.";
const NOT_SUPPORTED: &str = "That computer's agent cannot update itself (how it was installed), or the computer does not allow it (`lunaris-agent set server-update allow` there): run the update command there.";

/// What the stored build of `host_id` says: its version and whether it can
/// be updated from here.
async fn stored_build(state: &SignalingState, host_id: &str) -> Option<(Option<String>, Option<bool>)> {
    sqlx::query_as::<_, (Option<String>, Option<bool>)>("SELECT agent_version, agent_remote_update FROM hosts WHERE id = ?")
        .bind(host_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

/// POST /api/admin/hosts/{id}/update: the agent becomes this server's
/// version. Answers `{ "version": ... }` when the installer was started
/// there, else why not (`code`: "offline", "agent_too_old", "current",
/// "timeout", or the agent's `UPDATE_*`).
pub async fn update(admin: crate::auth::AdminUser, Path(host_id): Path<String>, State(state): State<Arc<SignalingState>>) -> axum::response::Response {
    update_to(&state, &host_id, env!("CARGO_PKG_VERSION"), &admin.username).await
}

/// Asks the agent of `host_id` to become `version`.
async fn update_to(state: &Arc<SignalingState>, host_id: &str, version: &str, requester: &str) -> axum::response::Response {
    let agent = state.agents.read().unwrap().get(host_id).cloned();
    let Some(agent) = agent else {
        return refused(StatusCode::CONFLICT, "offline", OFFLINE);
    };
    // What the agent said it is decides before anything is sent: an agent
    // from before this message would never answer.
    match stored_build(state, host_id).await {
        Some((Some(has), remote)) => {
            if !updater::version::is_newer(&has, version) {
                return refused(StatusCode::CONFLICT, "current", ALREADY);
            }
            match remote {
                Some(true) => {}
                // It says it cannot, or its computer does not allow it.
                Some(false) => return refused(StatusCode::CONFLICT, common::UPDATE_NOT_SUPPORTED, NOT_SUPPORTED),
                // It never said: an agent from before this message, which would not answer.
                None => return refused(StatusCode::CONFLICT, "agent_too_old", TOO_OLD),
            }
        }
        _ => return refused(StatusCode::CONFLICT, "agent_too_old", TOO_OLD),
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let (answer_tx, answer_rx) = oneshot::channel();
    state.host_updates.0.lock().unwrap().insert(request_id.clone(), (host_id.to_string(), answer_tx));
    let _waiting = Waiting { state, request_id: request_id.clone() };
    info!("Update of the agent of {} to {} asked for by {}", host_id, version, requester);
    let sent = agent.send(ServerToAgentMessage::Update { request_id, version: version.to_string(), requester: Some(requester.to_string()) });
    if sent.is_err() {
        return refused(StatusCode::CONFLICT, "offline", OFFLINE);
    }
    let answer = match tokio::time::timeout(ANSWER_TIMEOUT, answer_rx).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(_)) => return refused(StatusCode::CONFLICT, "offline", OFFLINE),
        Err(_) if !state.agents.read().unwrap().contains_key(host_id) => return refused(StatusCode::CONFLICT, "offline", OFFLINE),
        Err(_) => return refused(StatusCode::GATEWAY_TIMEOUT, "timeout", NO_ANSWER),
    };
    if let Some(error) = &answer.error {
        let code = answer.code.as_deref().unwrap_or(common::UPDATE_FAILED);
        let status = match code {
            common::UPDATE_NOT_ALLOWED => StatusCode::FORBIDDEN,
            common::UPDATE_INVALID => StatusCode::BAD_REQUEST,
            common::UPDATE_NOT_SUPPORTED | common::UPDATE_BUSY => StatusCode::CONFLICT,
            _ => StatusCode::BAD_GATEWAY,
        };
        // In the agent's words, cut to what a page shows.
        let text: String = error.chars().take(400).collect();
        info!("Update of the agent of {}: refused there ({}): {}", host_id, code, text);
        return refused(status, code, &text);
    }
    info!("Update of the agent of {} to {}: its installer was started", host_id, version);
    Json(serde_json::json!({ "version": version })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::AgentMessage;

    async fn state() -> Arc<SignalingState> {
        let db = crate::security::tests::test_db().await;
        Arc::new(SignalingState::new(db, "t".into(), None))
    }

    /// The version this server is in these tests.
    const SERVER: &str = "9.1.0";

    fn connect(state: &Arc<SignalingState>, host: &str) -> crate::queue::Inbox<ServerToAgentMessage> {
        let (tx, rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert(host.into(), tx);
        rx
    }

    /// The host `id` with what its agent said it is.
    async fn host(state: &Arc<SignalingState>, id: &str, version: Option<&str>, remote: Option<bool>) {
        sqlx::query("INSERT OR REPLACE INTO hosts (id, name, status, agent_version, agent_os, agent_install, agent_remote_update) VALUES (?, ?, 'Online', ?, 'windows', 'windows', ?)")
            .bind(id)
            .bind(id)
            .bind(version)
            .bind(remote)
            .execute(&state.db)
            .await
            .unwrap();
    }

    async fn body(response: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    async fn ask(state: &Arc<SignalingState>, id: &str) -> (StatusCode, serde_json::Value) {
        body(update_to(state, id, SERVER, "admin").await).await
    }

    #[tokio::test]
    async fn the_agent_is_asked_for_this_servers_version_and_its_answer_passed_on() {
        let state = state().await;
        host(&state, "h1", Some("0.8.3"), Some(true)).await;
        let mut to_agent = connect(&state, "h1");
        let pretend = {
            let state = state.clone();
            tokio::spawn(async move {
                let Some(ServerToAgentMessage::Update { request_id, version, requester }) = to_agent.recv().await else { panic!("no Update") };
                assert_eq!(version, SERVER);
                assert_eq!(requester.as_deref(), Some("admin"));
                // Another agent's answer is not believed.
                crate::signaling::agent_message(&state, "other", AgentMessage::UpdateAnswer { request_id: request_id.clone(), error: Some("forged".into()), code: None }).await;
                crate::signaling::agent_message(&state, "h1", AgentMessage::UpdateAnswer { request_id, error: None, code: None }).await;
            })
        };
        let (status, answer) = ask(&state, "h1").await;
        assert_eq!((status, answer["version"].as_str()), (StatusCode::OK, Some(SERVER)));
        pretend.await.unwrap();
        assert!(state.host_updates.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn nothing_is_sent_to_an_agent_that_cannot_or_need_not() {
        let state = state().await;
        // Offline.
        host(&state, "h1", Some("0.7.0"), None).await;
        let (status, answer) = ask(&state, "h1").await;
        assert_eq!((status, answer["code"].as_str()), (StatusCode::CONFLICT, Some("offline")));

        let mut to_agent = connect(&state, "h1");
        // From before the message: it would never answer.
        let (status, answer) = ask(&state, "h1").await;
        assert_eq!((status, answer["code"].as_str()), (StatusCode::CONFLICT, Some("agent_too_old")));
        // Never said what it is.
        host(&state, "h1", None, None).await;
        assert_eq!(ask(&state, "h1").await.1["code"].as_str(), Some("agent_too_old"));
        // Already this version (or newer).
        host(&state, "h1", Some(SERVER), Some(true)).await;
        assert_eq!(ask(&state, "h1").await.1["code"].as_str(), Some("current"));
        host(&state, "h1", Some("99.0.0"), Some(true)).await;
        assert_eq!(ask(&state, "h1").await.1["code"].as_str(), Some("current"));
        // Its agent says it cannot (a package, or the computer does not allow it).
        host(&state, "h1", Some("0.8.3"), Some(false)).await;
        assert_eq!(ask(&state, "h1").await.1["code"].as_str(), Some(common::UPDATE_NOT_SUPPORTED));
        host(&state, "h1", Some("0.8.3"), None).await;
        assert_eq!(ask(&state, "h1").await.1["code"].as_str(), Some("agent_too_old"));
        assert!(to_agent.rx_try().is_none(), "nothing was sent");
    }

    #[tokio::test]
    async fn what_the_agent_refuses_is_told_as_it_is() {
        let state = state().await;
        host(&state, "h1", Some("0.8.3"), Some(true)).await;
        for (code, status) in [
            (common::UPDATE_NOT_ALLOWED, StatusCode::FORBIDDEN),
            (common::UPDATE_BUSY, StatusCode::CONFLICT),
            (common::UPDATE_FAILED, StatusCode::BAD_GATEWAY),
        ] {
            let mut to_agent = connect(&state, "h1");
            let pretend = {
                let state = state.clone();
                tokio::spawn(async move {
                    let Some(ServerToAgentMessage::Update { request_id, .. }) = to_agent.recv().await else { panic!("no Update") };
                    crate::signaling::agent_message(&state, "h1", AgentMessage::UpdateAnswer { request_id, error: Some("why".into()), code: Some(code.into()) }).await;
                })
            };
            let (got, answer) = ask(&state, "h1").await;
            assert_eq!((got, answer["code"].as_str(), answer["error"].as_str()), (status, Some(code), Some("why")));
            pretend.await.unwrap();
        }
        // An agent that goes away while it is asked.
        let to_agent = connect(&state, "h1");
        let gone = {
            let state = state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                state.agents.write().unwrap().remove("h1");
                drop(to_agent);
            })
        };
        assert_eq!(ask(&state, "h1").await.1["code"].as_str(), Some("offline"));
        gone.await.unwrap();
    }
}
