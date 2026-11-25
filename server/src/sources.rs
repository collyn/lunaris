//! Sources: computers of other types (RustDesk by IP address, ...) that a
//! Lunaris computer shows through a connector, listed as computers of their
//! own.
//!
//! What it takes to reach one (its address, its password) is kept by that
//! Lunaris computer's agent and nowhere else: when one is added or changed
//! here, the fields pass through once, in the message to the agent, and are
//! neither stored nor logged. The server keeps the id, the name, the type
//! and the Lunaris computer it is reached through, as the agent announces
//! them (`AgentMessage::Sources`), to list it while that computer is away.
//!
//! A source can have a group (`common::SourceInfoMsg::group`): an opaque
//! word its agent keeps and announces, which says that it is the same
//! computer as the other sources with that word, or (`host:<id>`) as the
//! Lunaris computer with that id. The clients then show one card for that
//! computer, with a way to connect for each. It changes nothing about who
//! may use what: a source is listed for those who may use the Lunaris
//! computer it is reached through, and one that names a Lunaris computer
//! the user may not use is listed without its group. A group is a word of
//! `common::valid_source_group`; anything else from an agent is dropped,
//! and from a form refused.
//!
//! Who may do what: listing and connecting need the right to use the
//! Lunaris computer (`security::host_access`, as for the computer itself);
//! adding, changing and removing are for administrators, as adding and
//! removing computers is. A computer that asks at its screen before each
//! connection does not take changes from here (its agent says so and
//! refuses them); connecting to its sources is asked for there like any
//! other connection.
//!
//! Adding one of a type that has a set-up (a pairing: the agent's connector
//! makes a PIN that the person types at the other computer) is answered
//! with 202 and what to show; the client then asks about the set-up once a
//! second until it is done or failed, and cancels it when the person closes
//! the window. The PIN passes through like the fields do: in the answer,
//! once, neither stored nor logged. Of a set-up the server remembers only
//! which computer it is on.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use common::{ConnectorInfoMsg, ServerToAgentMessage, SetupInfo, SourceChange, SourceInfoMsg};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::signaling::SignalingState;

/// How long an agent has to answer a change.
const CHANGE_TIMEOUT: Duration = Duration::from_secs(15);
/// The most sources and connectors taken from one agent's announcement.
const MAX_ANNOUNCED: usize = 200;

/// What a connected agent announced.
#[derive(Clone, Default)]
pub struct Announced {
    pub connectors: Vec<ConnectorInfoMsg>,
    pub manageable: bool,
    /// source id -> whether that computer answered its agent's last check
    /// (`SourceInfoMsg::online`). A source that is not in it is not known
    /// to be away: it is listed as online while its agent is.
    pub reachable: HashMap<String, bool>,
}

/// How long a set-up is known here: longer than an agent keeps one (5
/// minutes to finish it, 10 to remember its end).
const SETUP_KNOWN: Duration = Duration::from_secs(20 * 60);

/// What an agent answered that worked.
enum Changed {
    /// The source's id.
    Id(String),
    /// The source's set-up waits for the person: what to show them.
    Setup(SetupInfo),
}

/// The answer to a change, or why not.
type Answer = Result<Changed, String>;

#[derive(Default)]
pub struct Registry {
    /// agent id -> what it announced, while it is connected.
    announced: RwLock<HashMap<String, Announced>>,
    /// request id -> (the agent asked, who waits for its answer).
    pending: Mutex<HashMap<String, (String, oneshot::Sender<Answer>)>>,
    /// set-up id -> (the agent it runs on, since when): only these are
    /// asked about, and only on that agent.
    setups: Mutex<HashMap<String, (String, std::time::Instant)>>,
}

impl Registry {
    fn setup_started(&self, setup: &str, host_id: &str) {
        let mut setups = self.setups.lock().unwrap();
        setups.retain(|_, (_, since)| since.elapsed() < SETUP_KNOWN);
        setups.insert(setup.to_string(), (host_id.to_string(), std::time::Instant::now()));
    }

    fn setup_known(&self, setup: &str, host_id: &str) -> bool {
        self.setups.lock().unwrap().get(setup).is_some_and(|(host, since)| host == host_id && since.elapsed() < SETUP_KNOWN)
    }

    fn setup_over(&self, setup: &str) {
        self.setups.lock().unwrap().remove(setup);
    }
}

/// An agent told its sources and connectors: they are kept (the sources in
/// the database, to list them while it is away).
pub(crate) async fn announced(
    state: &Arc<SignalingState>,
    agent_id: &str,
    mut sources: Vec<SourceInfoMsg>,
    mut connectors: Vec<ConnectorInfoMsg>,
    manageable: bool,
) {
    sources.truncate(MAX_ANNOUNCED);
    connectors.truncate(MAX_ANNOUNCED);
    // Removed from the list while it was away: not listed again.
    let removals = removals_of(state, agent_id).await;
    let ids: Vec<String> = sources.iter().map(|source| source.id.clone()).collect();
    sources.retain(|source| !removals.contains(&source.id));
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            error!("Database error: {}", e);
            return;
        }
    };
    let saved: Result<(), sqlx::Error> = async {
        sqlx::query("DELETE FROM host_sources WHERE host_id = ?").bind(agent_id).execute(&mut *tx).await?;
        for source in &sources {
            let connector_name = connectors.iter().find(|c| c.id == source.connector).map(|c| c.name.as_str()).unwrap_or(&source.connector);
            // What an agent calls a group is kept only when it can be one.
            let group = source.group.as_deref().filter(|group| common::valid_source_group(group));
            let values = source.values.as_ref().map(|values| shown_values(values)).and_then(|v| serde_json::to_string(&v).ok());
            sqlx::query("INSERT OR REPLACE INTO host_sources (host_id, source_id, name, connector, connector_name, source_group, source_values) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .bind(agent_id)
                .bind(&source.id)
                .bind(&source.name)
                .bind(&source.connector)
                .bind(connector_name)
                .bind(group)
                .bind(values)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }
    .await;
    if let Err(e) = saved {
        // A host that is not in the list (yet) has no sources either.
        warn!("The sources of {} could not be saved: {}", agent_id, e);
    }
    let reachable = sources.iter().filter_map(|source| Some((source.id.clone(), source.online?))).collect();
    state.sources.announced.write().unwrap().insert(agent_id.to_string(), Announced { connectors, manageable, reachable });
    if !removals.is_empty() {
        // Not awaited: the answers come through this agent's messages.
        tokio::spawn(apply_removals(state.clone(), agent_id.to_string(), removals, ids));
    }
}

/// Most fields of one source kept, and the longest value (characters).
const MAX_VALUES: usize = 32;
const MAX_VALUE_CHARS: usize = 256;

/// What is kept of the field values an agent announced: bounded, and never
/// a field whose name says it is a secret (agents leave those out; this is
/// in case one does not).
fn shown_values(values: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    values
        .iter()
        .filter(|(key, _)| {
            let key = key.to_ascii_lowercase();
            !["password", "secret", "token", "key", "cert"].iter().any(|word| key.contains(word))
        })
        .take(MAX_VALUES)
        .map(|(key, value)| (key.chars().take(64).collect(), value.chars().take(MAX_VALUE_CHARS).collect()))
        .collect()
}

/// Where a source is, for its card: its `host` field, with the `port` when
/// one is set ("192.168.1.5:3389"). None when it has no host field.
fn address_of(values: &BTreeMap<String, String>) -> Option<String> {
    let host = values.get("host").map(|h| h.trim()).filter(|h| !h.is_empty())?;
    match values.get("port").map(|p| p.trim()).filter(|p| !p.is_empty()) {
        Some(port) if host.contains(':') && !host.starts_with('[') => Some(format!("[{}]:{}", host, port)),
        Some(port) => Some(format!("{}:{}", host, port)),
        None => Some(host.to_string()),
    }
}

/// An agent said its own addresses: kept with its host, to show while it is
/// away too. An agent from before this says none, and what was kept stays.
pub(crate) async fn addresses_announced(state: &Arc<SignalingState>, agent_id: &str, addresses: Vec<String>) {
    let addresses: Vec<String> = addresses
        .into_iter()
        .filter(|a| a.parse::<std::net::IpAddr>().is_ok())
        .take(16)
        .collect();
    if addresses.is_empty() {
        return;
    }
    let Ok(json) = serde_json::to_string(&addresses) else { return };
    if let Err(e) = sqlx::query("UPDATE hosts SET addresses = ? WHERE id = ?").bind(json).bind(agent_id).execute(&state.db).await {
        warn!("The addresses of {} could not be saved: {}", agent_id, e);
    }
}

/// An agent said what it is (version, OS, how it was installed): kept with
/// its host, to show and to say how to update it. Anything that is not one
/// of the known values is not kept.
pub(crate) async fn build_announced(state: &Arc<SignalingState>, agent_id: &str, build: common::AgentBuild) {
    let Some(build) = checked_build(build) else {
        warn!("{} said an agent version that is not one: not kept", agent_id);
        return;
    };
    if let Err(e) = sqlx::query("UPDATE hosts SET agent_version = ?, agent_os = ?, agent_install = ?, agent_game_mouse = ?, agent_remote_update = ? WHERE id = ?")
        .bind(&build.version)
        .bind(&build.os)
        .bind(&build.install)
        .bind(&build.game_mouse)
        .bind(build.remote_update)
        .bind(agent_id)
        .execute(&state.db)
        .await
    {
        warn!("The agent version of {} could not be saved: {}", agent_id, e);
    }
}

fn checked_build(build: common::AgentBuild) -> Option<common::AgentBuild> {
    let version_ok = !build.version.is_empty()
        && build.version.len() <= 32
        && build.version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    let os_ok = matches!(build.os.as_str(), "linux" | "windows" | "macos");
    let install_ok = [common::INSTALL_DEB, common::INSTALL_APPIMAGE, common::INSTALL_WINDOWS, common::INSTALL_PACKAGE, common::INSTALL_MANUAL]
        .contains(&build.install.as_str());
    // Only the values this server knows (shown on the computer's page).
    let game_mouse = build
        .game_mouse
        .filter(|g| [common::GAME_MOUSE_USBIP, common::GAME_MOUSE_NONE].contains(&g.as_str()));
    (version_ok && os_ok).then(|| common::AgentBuild {
        install: if install_ok { build.install } else { common::INSTALL_MANUAL.to_string() },
        game_mouse,
        ..build
    })
}

/// An agent answered a change. Only the agent that was asked is believed.
pub(crate) fn changed(
    state: &Arc<SignalingState>,
    agent_id: &str,
    request_id: &str,
    id: Option<String>,
    error: Option<String>,
    setup: Option<SetupInfo>,
) {
    let mut pending = state.sources.pending.lock().unwrap();
    if !pending.get(request_id).is_some_and(|(asked, _)| asked == agent_id) {
        return;
    }
    if let Some((_, waiting)) = pending.remove(request_id) {
        let answer = match (id, error, setup) {
            (_, Some(error), _) => Err(error),
            (_, None, Some(setup)) => Ok(Changed::Setup(setup)),
            (Some(id), None, None) => Ok(Changed::Id(id)),
            (None, None, None) => Err(NO_ANSWER.to_string()),
        };
        let _ = waiting.send(answer);
    }
}

/// The agent went away: its connectors are not offered any more. Its
/// sources stay listed (offline).
pub(crate) fn agent_gone(state: &Arc<SignalingState>, agent_id: &str) {
    state.sources.announced.write().unwrap().remove(agent_id);
}

#[derive(Serialize)]
struct SourceEntry {
    host_id: String,
    host_name: String,
    id: String,
    name: String,
    connector: String,
    connector_name: String,
    /// Whether it can be connected to now: its agent is connected and has
    /// not found the computer away (`reachable`).
    online: bool,
    /// Whether the computer itself answered its agent's last check. Left
    /// out when that is not known (the agent is away, has not checked yet,
    /// cannot check it, or is from before the checks).
    #[serde(skip_serializing_if = "Option::is_none")]
    reachable: Option<bool>,
    /// What its connector can do, as its agent says while it is connected.
    /// Left out when that is not known (the agent is away or from before
    /// this): a client then offers what it always offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    capabilities: Option<common::ConnectorCapabilitiesMsg>,
    /// Which computer it is, when that computer is listed in other ways
    /// too: the sources with the same group are one computer, and
    /// `host:<id>` is the Lunaris computer with that id. Left out when it
    /// has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    /// Where it is ("host" or "host:port"), for its card: administrators
    /// only. Left out when its agent did not say (from before this) or it
    /// has no host field.
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    /// The fields of its form that are not secrets, for an edit form:
    /// administrators only (they are the ones who edit).
    #[serde(skip_serializing_if = "Option::is_none")]
    values: Option<BTreeMap<String, String>>,
}

/// The group of a source as `access` may see it: a group that names a
/// Lunaris computer is for those who may use that computer (the others are
/// not told that it exists).
fn group_for(access: &crate::security::HostAccess, group: Option<String>) -> Option<String> {
    let group = group.filter(|group| common::valid_source_group(group))?;
    match group.strip_prefix(common::SOURCE_GROUP_HOST) {
        Some(host_id) if !access.allows(host_id) => None,
        _ => Some(group),
    }
}

/// GET /api/sources: the sources of the computers the user may use.
pub async fn list_sources(user: crate::auth::AuthenticatedUser, State(state): State<Arc<SignalingState>>) -> impl IntoResponse {
    let access = match crate::security::host_access(&state.db, &user.user_id).await {
        Ok(access) => access,
        Err(e) => {
            error!("Database error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Internal Database Error" }))).into_response();
        }
    };
    let admin = user.role == "admin";
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, String, String, String, String, Option<String>, Option<String>)> = match sqlx::query_as(concat!("SELECT s.host_id, ", crate::db::host_name_sql!(), ", s.source_id, s.name, s.connector, s.connector_name, s.source_group, s.source_values
         FROM host_sources s INNER JOIN hosts h ON h.id = s.host_id
         ORDER BY s.name COLLATE NOCASE, s.source_id"))
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            error!("Database error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Internal Database Error" }))).into_response();
        }
    };
    let online = state.agents.read().unwrap();
    let announced = state.sources.announced.read().unwrap();
    let entries: Vec<SourceEntry> = rows
        .into_iter()
        .filter(|row| access.allows(&row.0))
        .map(|(host_id, host_name, id, name, connector, connector_name, group, values)| {
            let values: Option<BTreeMap<String, String>> = values.and_then(|v| serde_json::from_str(&v).ok());
            let agent_online = online.contains_key(&host_id);
            let reachable = announced.get(&host_id).filter(|_| agent_online).and_then(|a| a.reachable.get(&id).copied());
            SourceEntry {
                group: group_for(&access, group),
                capabilities: announced.get(&host_id).and_then(|a| a.connectors.iter().find(|c| c.id == connector)).and_then(|c| c.capabilities),
                // The computer of the source, not only the one it is reached
                // through: one that is shut down is offline.
                online: agent_online && reachable != Some(false),
                reachable,
                host_id,
                host_name,
                id,
                name,
                connector,
                connector_name,
                // The internal address of an RDP/VNC/... target is for
                // administrators, as its other fields are.
                address: values.as_ref().and_then(address_of).filter(|_| admin),
                values: values.filter(|_| admin),
            }
        })
        .collect();
    Json(entries).into_response()
}

#[derive(Serialize)]
struct HostConnectors {
    host_id: String,
    host_name: String,
    connectors: Vec<ConnectorInfoMsg>,
}

/// GET /api/connectors: the kinds of computers that can be added, by the
/// Lunaris computer they would be reached through (connected ones that
/// take changes from here).
pub async fn list_connectors(user: crate::auth::AuthenticatedUser, State(state): State<Arc<SignalingState>>) -> impl IntoResponse {
    let access = match crate::security::host_access(&state.db, &user.user_id).await {
        Ok(access) => access,
        Err(e) => {
            error!("Database error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Internal Database Error" }))).into_response();
        }
    };
    let announced: Vec<(String, Announced)> = state
        .sources
        .announced
        .read()
        .unwrap()
        .iter()
        .filter(|(host_id, a)| a.manageable && !a.connectors.is_empty() && access.allows(host_id))
        .map(|(host_id, a)| (host_id.clone(), a.clone()))
        .collect();
    let mut hosts = Vec::new();
    for (host_id, a) in announced {
        if !state.agents.read().unwrap().contains_key(&host_id) {
            continue;
        }
        let host_name: Option<String> = sqlx::query_scalar(concat!("SELECT ", crate::db::host_name_sql!(), " FROM hosts h WHERE h.id = ?")).bind(&host_id).fetch_optional(&state.db).await.ok().flatten();
        let Some(host_name) = host_name else { continue };
        hosts.push(HostConnectors { host_id, host_name, connectors: a.connectors });
    }
    hosts.sort_by(|a, b| a.host_name.to_lowercase().cmp(&b.host_name.to_lowercase()).then(a.host_id.cmp(&b.host_id)));
    Json(hosts).into_response()
}

#[derive(Deserialize)]
pub struct SourceForm {
    name: String,
    #[serde(default)]
    connector: String,
    #[serde(default)]
    values: std::collections::BTreeMap<String, String>,
    /// Which computer it is (`common::SourceInfoMsg::group`). Left out:
    /// adding lets the agent group it with a source at the same address,
    /// and changing keeps its group; empty: no group.
    #[serde(default)]
    group: Option<String>,
}

/// The group of a form, checked here too (the agent checks it again): the
/// response that refuses it when it cannot be one.
fn form_group(group: Option<String>) -> Result<Option<String>, axum::response::Response> {
    match group.map(|group| group.trim().to_string()) {
        Some(group) if !group.is_empty() && !common::valid_source_group(&group) => {
            Err(refused(StatusCode::BAD_REQUEST, "That group of computers is not valid"))
        }
        group => Ok(group),
    }
}

fn refused(status: StatusCode, text: &str) -> axum::response::Response {
    (status, Json(serde_json::json!({ "error": text }))).into_response()
}

const NO_ANSWER: &str = "That computer did not say whether it worked";

/// For an answer that can carry a set-up's code (a PIN): it is for the
/// person's eyes now, and is not to be kept by a browser or anything
/// between it and the server.
fn not_stored(response: impl IntoResponse) -> axum::response::Response {
    let mut response = response.into_response();
    response.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    response
}

/// Asks the agent of `host_id` to make `change` and waits for its answer;
/// the error is the response that says why not.
async fn ask(state: &Arc<SignalingState>, host_id: &str, change: SourceChange, requester: &str) -> Result<Changed, axum::response::Response> {
    let agent = state.agents.read().unwrap().get(host_id).cloned();
    let Some(agent) = agent else {
        return Err(refused(StatusCode::CONFLICT, "That computer is offline"));
    };
    // An agent that did not announce its sources is from before this: the
    // message would be nothing it knows (and it would write it to its log).
    let announced = state.sources.announced.read().unwrap().get(host_id).cloned();
    let Some(announced) = announced else {
        return Err(refused(StatusCode::CONFLICT, "The Lunaris on that computer is too old for this. Update it first."));
    };
    if !announced.manageable {
        return Err(refused(
            StatusCode::CONFLICT,
            "That computer asks at its screen before each server session, so computers cannot be added to it from here. On that computer, run `lunaris-agent set server-sessions trusted` to manage it from here.",
        ));
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let (answer_tx, answer_rx) = oneshot::channel();
    state.sources.pending.lock().unwrap().insert(request_id.clone(), (host_id.to_string(), answer_tx));
    // Not each time a set-up is asked about (once a second).
    if !matches!(change, SourceChange::SetupState { .. }) {
        info!("Sources of {}: {:?} asked for by {}", host_id, change, requester);
    }
    let sent = agent.send(ServerToAgentMessage::ChangeSource { request_id: request_id.clone(), change, requester: Some(requester.to_string()) });
    let answer = match sent {
        Ok(()) => tokio::time::timeout(CHANGE_TIMEOUT, answer_rx).await,
        Err(()) => {
            state.sources.pending.lock().unwrap().remove(&request_id);
            return Err(refused(StatusCode::CONFLICT, "That computer is offline"));
        }
    };
    state.sources.pending.lock().unwrap().remove(&request_id);
    match answer {
        Ok(Ok(Ok(changed))) => Ok(changed),
        Ok(Ok(Err(text))) => Err(refused(StatusCode::UNPROCESSABLE_ENTITY, &text)),
        Ok(Err(_)) => Err(refused(StatusCode::CONFLICT, "That computer went offline")),
        Err(_) => Err(refused(StatusCode::GATEWAY_TIMEOUT, "That computer did not answer")),
    }
}

/// A change that has no set-up: the source's id, or why not.
async fn change(state: &Arc<SignalingState>, host_id: &str, change: SourceChange, requester: &str) -> axum::response::Response {
    match ask(state, host_id, change, requester).await {
        Ok(Changed::Id(id)) => Json(serde_json::json!({ "id": id })).into_response(),
        Ok(Changed::Setup(_)) => refused(StatusCode::BAD_GATEWAY, NO_ANSWER),
        Err(response) => response,
    }
}

/// POST /api/hosts/{host_id}/sources: 200 `{"id"}` when it is added, or 202
/// `{"setup", "message", "code"}` when its type has a set-up that waits for
/// the person (ask on with GET .../sources/setup/{setup}).
pub async fn add_source(
    admin: crate::auth::AdminUser,
    Path(host_id): Path<String>,
    State(state): State<Arc<SignalingState>>,
    Json(form): Json<SourceForm>,
) -> impl IntoResponse {
    let group = match form_group(form.group) {
        Ok(group) => group,
        Err(response) => return response,
    };
    let add = SourceChange::Add { name: form.name, connector: form.connector, values: form.values, group };
    match ask(&state, &host_id, add, &admin.username).await {
        Ok(Changed::Id(id)) => Json(serde_json::json!({ "id": id })).into_response(),
        Ok(Changed::Setup(setup)) => {
            state.sources.setup_started(&setup.setup, &host_id);
            not_stored((StatusCode::ACCEPTED, Json(serde_json::json!({ "setup": setup.setup, "message": setup.message, "code": setup.code }))))
        }
        Err(response) => response,
    }
}

/// GET /api/hosts/{host_id}/sources/setup/{setup}: where the set-up of a
/// source being added is. `{"state":"pending","message","code"}`,
/// `{"state":"done","id"}` or `{"state":"failed","error"}`; 404 for one that
/// was not started here on that computer (or long ago).
pub async fn setup_state(
    admin: crate::auth::AdminUser,
    Path((host_id, setup)): Path<(String, String)>,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    if !state.sources.setup_known(&setup, &host_id) {
        return refused(StatusCode::NOT_FOUND, "That set-up is not there any more. Add the computer again.");
    }
    match ask(&state, &host_id, SourceChange::SetupState { setup: setup.clone() }, &admin.username).await {
        Ok(Changed::Setup(info)) => not_stored(Json(serde_json::json!({ "state": "pending", "message": info.message, "code": info.code }))),
        Ok(Changed::Id(id)) => {
            state.sources.setup_over(&setup);
            Json(serde_json::json!({ "state": "done", "id": id })).into_response()
        }
        Err(response) if response.status() == StatusCode::UNPROCESSABLE_ENTITY => {
            // The agent's words: why the set-up failed.
            state.sources.setup_over(&setup);
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap_or_default();
            let error = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_default();
            Json(serde_json::json!({ "state": "failed", "error": error })).into_response()
        }
        // The computer cannot be asked now (offline, no answer).
        Err(response) => response,
    }
}

/// DELETE /api/hosts/{host_id}/sources/setup/{setup}: stops the set-up (the
/// person closed the window). Nothing to stop is not an error.
pub async fn cancel_setup(
    admin: crate::auth::AdminUser,
    Path((host_id, setup)): Path<(String, String)>,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    if !state.sources.setup_known(&setup, &host_id) {
        return Json(serde_json::json!({})).into_response();
    }
    state.sources.setup_over(&setup);
    match ask(&state, &host_id, SourceChange::SetupCancel { setup }, &admin.username).await {
        Ok(_) => Json(serde_json::json!({})).into_response(),
        Err(response) => response,
    }
}

/// PUT /api/hosts/{host_id}/sources/{id}
pub async fn update_source(
    admin: crate::auth::AdminUser,
    Path((host_id, id)): Path<(String, String)>,
    State(state): State<Arc<SignalingState>>,
    Json(form): Json<SourceForm>,
) -> impl IntoResponse {
    let group = match form_group(form.group) {
        Ok(group) => group,
        Err(response) => return response,
    };
    change(&state, &host_id, SourceChange::Update { id, name: form.name, values: form.values, group }, &admin.username).await
}

/// DELETE /api/hosts/{host_id}/sources/{id}
pub async fn remove_source(
    admin: crate::auth::AdminUser,
    Path((host_id, id)): Path<(String, String)>,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    // The agent is away: the card is taken off the list now, and the agent
    // (which still has the source) is asked to remove it when it is back.
    if !state.agents.read().unwrap().contains_key(&host_id) {
        return remove_while_away(&state, &host_id, &id, &admin.username).await;
    }
    let response = change(&state, &host_id, SourceChange::Remove { id }, &admin.username).await;
    if response.status().is_success() {
        return Json(serde_json::json!({})).into_response();
    }
    response
}

async fn remove_while_away(state: &Arc<SignalingState>, host_id: &str, id: &str, requester: &str) -> axum::response::Response {
    let removed: Result<u64, sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        let gone = sqlx::query("DELETE FROM host_sources WHERE host_id = ? AND source_id = ?")
            .bind(host_id)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if gone > 0 {
            sqlx::query("INSERT OR IGNORE INTO source_removals (host_id, source_id) VALUES (?, ?)")
                .bind(host_id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(gone)
    }
    .await;
    match removed {
        Ok(0) => refused(StatusCode::NOT_FOUND, "That computer is not in the list any more."),
        Ok(_) => {
            info!("Sources of {} (offline): {} removed from the list by {}; the agent is asked when it is back", host_id, id, requester);
            Json(serde_json::json!({})).into_response()
        }
        Err(e) => {
            error!("Database error: {}", e);
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Failed to remove it")
        }
    }
}

/// The sources of `agent_id` removed from the list while it was away.
async fn removals_of(state: &Arc<SignalingState>, agent_id: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT source_id FROM source_removals WHERE host_id = ?")
        .bind(agent_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_else(|e| {
            error!("Database error: {}", e);
            Vec::new()
        })
}

async fn forget_removal(state: &Arc<SignalingState>, agent_id: &str, id: &str) {
    let _ = sqlx::query("DELETE FROM source_removals WHERE host_id = ? AND source_id = ?")
        .bind(agent_id)
        .bind(id)
        .execute(&state.db)
        .await;
}

/// Asks an agent that is back to remove what was removed from the list while
/// it was away. What it no longer has is forgotten; a removal it refuses
/// (or that cannot be asked now) is kept, and the source stays off the list.
async fn apply_removals(state: Arc<SignalingState>, agent_id: String, removals: Vec<String>, announced: Vec<String>) {
    for id in removals {
        if !announced.contains(&id) {
            forget_removal(&state, &agent_id, &id).await;
            continue;
        }
        match ask(&state, &agent_id, SourceChange::Remove { id: id.clone() }, "the server (removed while offline)").await {
            Ok(_) => forget_removal(&state, &agent_id, &id).await,
            Err(response) => warn!("Sources of {}: {} could not be removed now ({})", agent_id, id, response.status()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{AgentMessage, ConnectorFieldMsg};

    async fn state() -> Arc<SignalingState> {
        let db = crate::security::tests::test_db().await;
        let state = Arc::new(SignalingState::new(db, "t".into(), None));
        for (id, role) in [("admin", "admin"), ("alice", "user"), ("bob", "user")] {
            sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES (?, ?, 'x', ?, 1)")
                .bind(id)
                .bind(format!("{}-test", id))
                .bind(role)
                .execute(&state.db)
                .await
                .unwrap();
        }
        for (id, name) in [("h1", "Office PC"), ("h2", "Lab PC")] {
            sqlx::query("INSERT INTO hosts (id, name, status) VALUES (?, ?, 'Online')").bind(id).bind(name).execute(&state.db).await.unwrap();
        }
        // alice may use h1 (through a group); bob may use nothing.
        sqlx::query("INSERT INTO groups (id, name) VALUES ('g', 'G')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ('alice', 'g')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO host_groups (host_id, group_id) VALUES ('h1', 'g')").execute(&state.db).await.unwrap();
        state
    }

    fn user(id: &str) -> crate::auth::AuthenticatedUser {
        crate::auth::AuthenticatedUser { user_id: id.into(), username: id.into(), role: if id == "admin" { "admin".into() } else { "user".into() } }
    }

    fn admin() -> crate::auth::AdminUser {
        crate::auth::AdminUser { user_id: "admin".into(), username: "admin".into() }
    }

    fn rustdesk() -> ConnectorInfoMsg {
        let field = |key: &str, kind: &str, required: bool| ConnectorFieldMsg { key: key.into(), label: key.into(), kind: kind.into(), default: String::new(), required, options: Vec::new() };
        ConnectorInfoMsg { id: "rustdesk".into(), name: "RustDesk (by IP address)".into(), fields: vec![field("host", "text", true), field("password", "secret", true)], note: String::new(), capabilities: None }
    }

    fn source(id: &str, name: &str) -> SourceInfoMsg {
        SourceInfoMsg { id: id.into(), name: name.into(), connector: "rustdesk".into(), group: None, online: None, values: None }
    }

    async fn body(response: impl IntoResponse) -> (StatusCode, serde_json::Value) {
        let response = response.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    /// Connects a pretend agent for `host`; returns what the server sends it.
    fn connect(state: &Arc<SignalingState>, host: &str) -> crate::queue::Inbox<ServerToAgentMessage> {
        let (tx, rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert(host.into(), tx);
        rx
    }

    #[tokio::test]
    async fn sources_are_listed_for_those_who_may_use_the_computer() {
        let state = state().await;
        let _h1 = connect(&state, "h1");
        announced(&state, "h1", vec![source("lab-1", "Lab server"), source("den-2", "Den PC")], vec![rustdesk()], true).await;
        announced(&state, "h2", vec![source("x-3", "Other")], vec![rustdesk()], true).await;

        // The administrator sees all of them; h2's agent is not connected.
        let (status, list) = body(list_sources(user("admin"), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        let names: Vec<(&str, &str, bool)> =
            list.as_array().unwrap().iter().map(|s| (s["name"].as_str().unwrap(), s["host_name"].as_str().unwrap(), s["online"].as_bool().unwrap())).collect();
        assert_eq!(names, [("Den PC", "Office PC", true), ("Lab server", "Office PC", true), ("Other", "Lab PC", false)]);
        let first = &list[0];
        assert_eq!((first["host_id"].as_str(), first["id"].as_str(), first["connector"].as_str()), (Some("h1"), Some("den-2"), Some("rustdesk")));
        assert_eq!(first["connector_name"], "RustDesk (by IP address)");

        // alice may use h1 only; bob nothing.
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(list.as_array().unwrap().len(), 2);
        assert!(list.as_array().unwrap().iter().all(|s| s["host_id"] == "h1"));
        let (_, list) = body(list_sources(user("bob"), State(state.clone())).await).await;
        assert!(list.as_array().unwrap().is_empty());

        // A new announcement replaces the list; the agent going away keeps
        // it (offline) and takes its connectors off the offer.
        announced(&state, "h1", vec![source("lab-1", "Lab server renamed")], vec![rustdesk()], true).await;
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["name"], "Lab server renamed");
        let (_, offer) = body(list_connectors(user("alice"), State(state.clone())).await).await;
        assert_eq!(offer.as_array().unwrap().len(), 1);
        assert_eq!((offer[0]["host_id"].as_str(), offer[0]["connectors"][0]["id"].as_str()), (Some("h1"), Some("rustdesk")));
        assert_eq!(offer[0]["connectors"][0]["fields"][1]["kind"], "secret");
        // A connector's note is offered with it; one without a note has no "note" (as before notes).
        assert!(offer[0]["connectors"][0].get("note").is_none());
        let mut noted = rustdesk();
        noted.note = "It must be running on that computer.".into();
        announced(&state, "h1", vec![], vec![noted], true).await;
        let (_, offer) = body(list_connectors(user("alice"), State(state.clone())).await).await;
        assert_eq!(offer[0]["connectors"][0]["note"], "It must be running on that computer.");
        // What a connector can do goes with the type and with each source of
        // it; an agent from before this says nothing, and nothing is made up.
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert!(list[0].get("capabilities").is_none());
        let mut plain = rustdesk();
        plain.capabilities = Some(common::ConnectorCapabilitiesMsg { input: true, audio: true, ..Default::default() });
        announced(&state, "h1", vec![source("lab-1", "Lab server renamed")], vec![plain], true).await;
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(list[0]["capabilities"]["files"], false);
        assert_eq!(list[0]["capabilities"]["input"], true);
        let (_, offer) = body(list_connectors(user("alice"), State(state.clone())).await).await;
        assert_eq!(offer[0]["connectors"][0]["capabilities"]["files"], false);
        let old: ConnectorInfoMsg = serde_json::from_str(r#"{"id":"rustdesk","name":"RustDesk"}"#).unwrap();
        assert_eq!(old.capabilities, None);
        let new: ConnectorInfoMsg = serde_json::from_str(r#"{"id":"rdp","name":"RDP","capabilities":{"files":true,"later":1}}"#).unwrap();
        assert!(new.capabilities.is_some_and(|c| c.files && !c.clipboard));
        // An agent from before notes announces none.
        let old: ConnectorInfoMsg = serde_json::from_str(r#"{"id":"rustdesk","name":"RustDesk"}"#).unwrap();
        assert_eq!(old.note, "");
        announced(&state, "h1", vec![source("lab-1", "Lab server renamed")], vec![rustdesk()], true).await;
        state.agents.write().unwrap().remove("h1");
        agent_gone(&state, "h1");
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(list[0]["online"], false);
        let (_, offer) = body(list_connectors(user("alice"), State(state.clone())).await).await;
        assert!(offer.as_array().unwrap().is_empty());

        // Removing the Lunaris computer removes its sources from the list.
        sqlx::query("DELETE FROM hosts WHERE id = 'h1'").execute(&state.db).await.unwrap();
        let (_, list) = body(list_sources(user("admin"), State(state.clone())).await).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_change_goes_to_the_agent_once_and_nothing_of_it_is_kept() {
        let state = state().await;
        let mut to_agent = connect(&state, "h1");
        announced(&state, "h1", vec![], vec![rustdesk()], true).await;

        // The pretend agent: answers as a new agent does (its new list
        // first, then the answer).
        let agent_state = state.clone();
        let agent = tokio::spawn(async move {
            let Some(ServerToAgentMessage::ChangeSource { request_id, change, requester }) = to_agent.recv().await else { panic!("no ChangeSource") };
            assert_eq!(requester.as_deref(), Some("admin"));
            let SourceChange::Add { name, connector, values, group } = &change else { panic!("not an add") };
            assert_eq!(group, &None, "a form without a group says nothing about it");
            assert_eq!((name.as_str(), connector.as_str()), ("Den PC", "rustdesk"));
            assert_eq!(values.get("password").map(String::as_str), Some("s3cret-pw"));
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::Sources { sources: vec![source("den-pc-ab12", "Den PC")], connectors: vec![rustdesk()], manageable: true, addresses: vec![], build: None }).await;
            // An answer from another agent with the same request id is not believed.
            crate::signaling::agent_message(&agent_state, "h2", AgentMessage::SourceChanged { request_id: request_id.clone(), id: None, error: Some("forged".into()), setup: None }).await;
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: Some("den-pc-ab12".into()), error: None, setup: None }).await;
            to_agent
        });
        let form = SourceForm {
            name: "Den PC".into(),
            connector: "rustdesk".into(),
            values: [("host".to_string(), "192.168.1.50".to_string()), ("password".to_string(), "s3cret-pw".to_string())].into_iter().collect(),
            group: None,
        };
        let (status, answer) = body(add_source(admin(), Path("h1".into()), State(state.clone()), Json(form)).await).await;
        assert_eq!((status, answer["id"].as_str()), (StatusCode::OK, Some("den-pc-ab12")));
        let mut to_agent = agent.await.unwrap();
        assert!(to_agent.rx_try().is_none(), "one message, once");
        assert!(state.sources.pending.lock().unwrap().is_empty());

        // The list has it at once; the database has the name and the type and
        // nothing of the address or the password, in any table.
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(list[0]["id"], "den-pc-ab12");
        let tables: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'").fetch_all(&state.db).await.unwrap();
        for table in tables {
            let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT * FROM \"{}\"", table))).fetch_all(&state.db).await.unwrap();
            for row in rows {
                use sqlx::Row;
                for i in 0..row.len() {
                    let text: Option<String> = row.try_get(i).ok().flatten();
                    assert!(!text.as_deref().unwrap_or("").contains("s3cret-pw") && !text.as_deref().unwrap_or("").contains("192.168.1.50"), "{}", table);
                }
            }
        }

        // What the agent refuses comes back as its words.
        let agent_state = state.clone();
        let agent = tokio::spawn(async move {
            let Some(ServerToAgentMessage::ChangeSource { request_id, change, .. }) = to_agent.recv().await else { panic!("no ChangeSource") };
            assert!(matches!(change, SourceChange::Update { ref id, .. } if id == "den-pc-ab12"));
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: None, error: Some("host is required".into()), setup: None }).await;
            to_agent
        });
        let form = SourceForm { name: "Den".into(), connector: String::new(), values: Default::default(), group: None };
        let (status, answer) = body(update_source(admin(), Path(("h1".into(), "den-pc-ab12".into())), State(state.clone()), Json(form)).await).await;
        assert_eq!((status, answer["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("host is required")));
        let mut to_agent = agent.await.unwrap();

        // Removing.
        let agent_state = state.clone();
        let agent = tokio::spawn(async move {
            let Some(ServerToAgentMessage::ChangeSource { request_id, change, .. }) = to_agent.recv().await else { panic!("no ChangeSource") };
            assert_eq!(change, SourceChange::Remove { id: "den-pc-ab12".into() });
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::Sources { sources: vec![], connectors: vec![rustdesk()], manageable: true, addresses: vec![], build: None }).await;
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: Some("den-pc-ab12".into()), error: None, setup: None }).await;
        });
        let (status, _) = body(remove_source(admin(), Path(("h1".into(), "den-pc-ab12".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        agent.await.unwrap();
        let (_, list) = body(list_sources(user("admin"), State(state.clone())).await).await;
        assert!(list.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn what_an_agent_is_is_kept_with_its_computer() {
        let state = state().await;
        let _h1 = connect(&state, "h1");
        let said = |version: &str, os: &str, install: &str| AgentMessage::Sources {
            sources: vec![],
            connectors: vec![],
            manageable: true,
            addresses: vec![],
            build: Some(common::AgentBuild { version: version.into(), os: os.into(), install: install.into(), game_mouse: None, remote_update: None }),
        };
        let kept = || async {
            sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>("SELECT agent_version, agent_os, agent_install FROM hosts WHERE id = 'h1'")
                .fetch_one(&state.db)
                .await
                .unwrap()
        };
        crate::signaling::agent_message(&state, "h1", said("0.7.6", "linux", "deb")).await;
        assert_eq!(kept().await, (Some("0.7.6".into()), Some("linux".into()), Some("deb".into())));
        // An install kind this server does not know is shown as "by hand".
        crate::signaling::agent_message(&state, "h1", said("0.8.0-rc.1", "windows", "msix")).await;
        assert_eq!(kept().await, (Some("0.8.0-rc.1".into()), Some("windows".into()), Some("manual".into())));
        // Not a version, or not an OS: not kept.
        crate::signaling::agent_message(&state, "h1", said("1.0 <script>", "linux", "deb")).await;
        crate::signaling::agent_message(&state, "h1", said("1.0.0", "plan9", "deb")).await;
        assert_eq!(kept().await.0.as_deref(), Some("0.8.0-rc.1"));
        // A Windows agent says whether its game mouse can be plugged in; a
        // value this server does not know is not kept.
        let with_mouse = |game_mouse: &str| AgentMessage::Sources {
            sources: vec![],
            connectors: vec![],
            manageable: true,
            addresses: vec![],
            build: Some(common::AgentBuild { version: "0.7.8".into(), os: "windows".into(), install: "windows".into(), game_mouse: Some(game_mouse.into()), remote_update: Some(true) }),
        };
        let mouse = || async {
            sqlx::query_scalar::<_, Option<String>>("SELECT agent_game_mouse FROM hosts WHERE id = 'h1'")
                .fetch_one(&state.db)
                .await
                .unwrap()
        };
        crate::signaling::agent_message(&state, "h1", with_mouse("usbip")).await;
        assert_eq!(mouse().await.as_deref(), Some("usbip"));
        crate::signaling::agent_message(&state, "h1", with_mouse("<b>")).await;
        assert_eq!(mouse().await, None);
        // An older agent says nothing: what was kept stays.
        crate::signaling::agent_message(&state, "h1", AgentMessage::Sources { sources: vec![], connectors: vec![], manageable: true, addresses: vec![], build: None }).await;
        assert_eq!(kept().await.0.as_deref(), Some("0.7.8"));
    }

    #[tokio::test]
    async fn a_source_shows_its_address_and_its_fields_to_administrators_only() {
        let state = state().await;
        let _h1 = connect(&state, "h1");
        let values: BTreeMap<String, String> = [("host", "192.168.1.5"), ("port", "3390"), ("username", "huy"), ("password", "leaked-pw")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let den = SourceInfoMsg { values: Some(values), ..source("den-1", "Den PC") };
        crate::signaling::agent_message(
            &state,
            "h1",
            AgentMessage::Sources { sources: vec![den, source("old-2", "Old agent's")], connectors: vec![rustdesk()], manageable: true, addresses: vec!["192.168.1.20".into(), "not an ip".into()], build: None },
        )
        .await;

        let (_, list) = body(list_sources(user("admin"), State(state.clone())).await).await;
        let den = list.as_array().unwrap().iter().find(|s| s["id"] == "den-1").unwrap().clone();
        assert_eq!(den["address"], "192.168.1.5:3390");
        assert_eq!(den["values"]["username"], "huy");
        // A secret is not kept even when an agent sends one.
        assert!(!list.to_string().contains("leaked-pw"));
        let old = list.as_array().unwrap().iter().find(|s| s["id"] == "old-2").unwrap().clone();
        assert!(old.get("address").is_none() && old.get("values").is_none());

        // A user who may use the computer sees neither its fields nor its
        // internal address.
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        let den = list.as_array().unwrap().iter().find(|s| s["id"] == "den-1").unwrap().clone();
        assert!(den.get("address").is_none());
        assert!(den.get("values").is_none());
        assert!(!list.to_string().contains("192.168.1.5"));

        // The computer's own addresses (only real ones), kept with its host;
        // an agent that says none leaves them.
        let stored: Option<String> = sqlx::query_scalar("SELECT addresses FROM hosts WHERE id = 'h1'").fetch_one(&state.db).await.unwrap();
        assert_eq!(stored.as_deref(), Some(r#"["192.168.1.20"]"#));
        addresses_announced(&state, "h1", vec![]).await;
        let stored: Option<String> = sqlx::query_scalar("SELECT addresses FROM hosts WHERE id = 'h1'").fetch_one(&state.db).await.unwrap();
        assert_eq!(stored.as_deref(), Some(r#"["192.168.1.20"]"#));
    }

    #[test]
    fn an_address_is_the_host_with_its_port() {
        let v = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();
        assert_eq!(address_of(&v(&[("host", " pc.lan ")])).as_deref(), Some("pc.lan"));
        assert_eq!(address_of(&v(&[("host", "10.0.0.2"), ("port", "")])).as_deref(), Some("10.0.0.2"));
        assert_eq!(address_of(&v(&[("host", "fe80::1"), ("port", "5900")])).as_deref(), Some("[fe80::1]:5900"));
        assert_eq!(address_of(&v(&[("port", "5900")])), None);
    }

    #[tokio::test]
    async fn a_source_removed_while_its_agent_is_away_goes_now_and_is_removed_there_when_it_is_back() {
        let state = state().await;
        // h1 announced two sources, then went away.
        announced(&state, "h1", vec![source("den-1", "Den PC"), source("lab-2", "Lab")], vec![rustdesk()], true).await;
        agent_gone(&state, "h1");

        let (status, _) = body(remove_source(admin(), Path(("h1".into(), "den-1".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        let (_, list) = body(list_sources(user("admin"), State(state.clone())).await).await;
        let ids: Vec<&str> = list.as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["lab-2"]);
        // Not there (any more): said so.
        let (status, _) = body(remove_source(admin(), Path(("h1".into(), "den-1".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Back, still with the source: it stays off the list and the agent
        // is asked to remove it.
        let mut to_agent = connect(&state, "h1");
        announced(&state, "h1", vec![source("den-1", "Den PC"), source("lab-2", "Lab")], vec![rustdesk()], true).await;
        let (_, list) = body(list_sources(user("admin"), State(state.clone())).await).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        let (request_id, change) = asked(&mut to_agent).await;
        assert_eq!(change, SourceChange::Remove { id: "den-1".into() });
        crate::signaling::agent_message(&state, "h1", AgentMessage::SourceChanged { request_id, id: Some("den-1".into()), error: None, setup: None }).await;
        for _ in 0..100 {
            if removals_of(&state, "h1").await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(removals_of(&state, "h1").await.is_empty());

        // Should it add a source with that id again later, it is listed.
        announced(&state, "h1", vec![source("den-1", "Den PC"), source("lab-2", "Lab")], vec![rustdesk()], true).await;
        let (_, list) = body(list_sources(user("admin"), State(state.clone())).await).await;
        assert_eq!(list.as_array().unwrap().len(), 2);
        assert!(to_agent.rx_try().is_none());
    }

    #[tokio::test]
    async fn nothing_is_sent_to_an_agent_that_is_old_away_or_asks_at_its_screen() {
        let state = state().await;
        let form = || SourceForm {
            name: "Den PC".into(),
            connector: "rustdesk".into(),
            values: [("password".to_string(), "s3cret-pw".to_string())].into_iter().collect(),
            group: None,
        };
        // Not connected.
        let (status, answer) = body(add_source(admin(), Path("h1".into()), State(state.clone()), Json(form())).await).await;
        assert_eq!((status, answer["error"].as_str()), (StatusCode::CONFLICT, Some("That computer is offline")));

        // Connected, but it never announced sources: an agent from before
        // them. It is sent nothing (it would write the message to its log).
        let mut old_agent = connect(&state, "h1");
        let (status, answer) = body(add_source(admin(), Path("h1".into()), State(state.clone()), Json(form())).await).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(answer["error"].as_str().unwrap().contains("too old"));
        assert!(old_agent.rx_try().is_none());
        // And its messages as it sends them still parse here.
        assert!(serde_json::from_str::<AgentMessage>(r#"{"event":"StatusUpdate","data":{"status":"Online"}}"#).is_ok());

        // An agent that asks at its screen before each connection says so,
        // and is not asked to change anything.
        announced(&state, "h1", vec![source("a-1", "A")], vec![rustdesk()], false).await;
        let (status, answer) = body(add_source(admin(), Path("h1".into()), State(state.clone()), Json(form())).await).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(answer["error"].as_str().unwrap().contains("asks at its screen"));
        let (status, _) = body(remove_source(admin(), Path(("h1".into(), "a-1".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(old_agent.rx_try().is_none());
        // Its sources are listed all the same (using one is asked for there
        // like any connection), and its connectors are not on offer.
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        let (_, offer) = body(list_connectors(user("admin"), State(state.clone())).await).await;
        assert!(offer.as_array().unwrap().is_empty());

        // The messages as they are on the wire; a change is never printed with its values.
        let wire = serde_json::to_string(&ServerToAgentMessage::ChangeSource {
            request_id: "r".into(),
            change: SourceChange::Add { name: "n".into(), connector: "rustdesk".into(), values: form().values, group: None },
            requester: None,
        })
        .unwrap();
        assert!(wire.starts_with(r#"{"event":"ChangeSource","data":{"request_id":"r","change":{"op":"add""#), "{}", wire);
        let printed = format!("{:?}", serde_json::from_str::<ServerToAgentMessage>(&wire).unwrap());
        assert!(!printed.contains("s3cret-pw"), "{}", printed);
        // An agent from before sources announces nothing; one from after says `Sources`.
        let announce: AgentMessage =
            serde_json::from_str(r#"{"event":"Sources","data":{"sources":[{"id":"a","name":"A","connector":"rustdesk"}],"connectors":[{"id":"rustdesk","name":"RustDesk"}]}}"#).unwrap();
        assert!(matches!(announce, AgentMessage::Sources { manageable: false, ref connectors, .. } if connectors[0].fields.is_empty()));
    }

    fn grouped(id: &str, name: &str, group: &str) -> SourceInfoMsg {
        SourceInfoMsg { group: Some(group.into()), ..source(id, name) }
    }

    #[tokio::test]
    async fn a_group_is_listed_as_announced_and_only_to_those_it_is_for() {
        let state = state().await;
        let _h1 = connect(&state, "h1");
        // Two that are one computer, one that is the Lunaris computer h2, one
        // that is h1 itself, one with no group, and one whose "group" is not one.
        announced(
            &state,
            "h1",
            vec![
                grouped("den-rd", "Den PC", "g-0a1b2c3d4e5f"),
                grouped("den-sun", "Den PC (Sunshine)", "g-0a1b2c3d4e5f"),
                grouped("lab-rd", "Lab", "host:h2"),
                grouped("office-sun", "Office", "host:h1"),
                source("alone", "Alone"),
                grouped("bad", "Bad", "<script>alert(1)</script>"),
                grouped("bad2", "Bad 2", "x'; DROP TABLE host_sources; --"),
            ],
            vec![rustdesk()],
            true,
        )
        .await;
        let groups = |list: &serde_json::Value| -> Vec<(String, Option<String>)> {
            let mut all: Vec<(String, Option<String>)> =
                list.as_array().unwrap().iter().map(|s| (s["id"].as_str().unwrap().to_string(), s["group"].as_str().map(str::to_string))).collect();
            all.sort();
            all
        };
        let some = |id: &str, group: &str| (id.to_string(), Some(group.to_string()));
        let none = |id: &str| (id.to_string(), None);

        // The administrator may use every computer: every group that is one.
        let (_, list) = body(list_sources(user("admin"), State(state.clone())).await).await;
        assert_eq!(
            groups(&list),
            [none("alone"), none("bad"), none("bad2"), some("den-rd", "g-0a1b2c3d4e5f"), some("den-sun", "g-0a1b2c3d4e5f"), some("lab-rd", "host:h2"), some("office-sun", "host:h1")]
        );
        // A source without a group is listed as it always was: no "group" at all.
        let alone = list.as_array().unwrap().iter().find(|s| s["id"] == "alone").unwrap();
        assert!(alone.get("group").is_none());
        // What was not fit to be a group is not in the database either; the table is whole.
        let stored: Vec<Option<String>> = sqlx::query_scalar("SELECT source_group FROM host_sources WHERE source_id LIKE 'bad%'").fetch_all(&state.db).await.unwrap();
        assert_eq!(stored, [None, None]);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM host_sources").fetch_one(&state.db).await.unwrap();
        assert_eq!(count, 7);

        // alice may use h1 and not h2: she sees h1's sources as before, the
        // groups among them and the one that is h1; the one that names h2 is
        // listed without its group (it is a card of its own for her).
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(
            groups(&list),
            [none("alone"), none("bad"), none("bad2"), some("den-rd", "g-0a1b2c3d4e5f"), some("den-sun", "g-0a1b2c3d4e5f"), none("lab-rd"), some("office-sun", "host:h1")]
        );
        assert!(!list.to_string().contains("h2"), "nothing of a computer she may not use: {}", list);
        // bob may use nothing: a group gives him nothing.
        let (_, list) = body(list_sources(user("bob"), State(state.clone())).await).await;
        assert!(list.as_array().unwrap().is_empty());
        // A source of h2 that says it is h1 is not listed for alice at all:
        // she may not use the computer it is reached through.
        announced(&state, "h2", vec![grouped("spy", "Spy", "host:h1")], vec![rustdesk()], true).await;
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert!(list.as_array().unwrap().iter().all(|s| s["host_id"] == "h1"));

        // The next announcement replaces the groups with the rest.
        announced(&state, "h1", vec![source("den-rd", "Den PC"), grouped("den-sun", "Den PC (Sunshine)", "g-new")], vec![rustdesk()], true).await;
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(groups(&list), [none("den-rd"), some("den-sun", "g-new")]);
    }

    #[tokio::test]
    async fn a_group_passes_to_the_agent_when_it_can_be_one() {
        let state = state().await;
        let mut to_agent = connect(&state, "h1");
        announced(&state, "h1", vec![], vec![rustdesk()], true).await;
        let form = |group: Option<&str>| SourceForm {
            name: "Den PC".into(),
            connector: "rustdesk".into(),
            values: [("host".to_string(), "192.168.1.50".to_string())].into_iter().collect(),
            group: group.map(str::to_string),
        };
        // What cannot be a group is refused here; the agent is sent nothing.
        for bad in ["a b", "<b>", "x'; DROP TABLE hosts; --", &"x".repeat(65)] {
            let (status, answer) = body(add_source(admin(), Path("h1".into()), State(state.clone()), Json(form(Some(bad)))).await).await;
            assert_eq!((status, answer["error"].as_str()), (StatusCode::BAD_REQUEST, Some("That group of computers is not valid")));
            let (status, _) = body(update_source(admin(), Path(("h1".into(), "a".into())), State(state.clone()), Json(form(Some(bad)))).await).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        assert!(to_agent.rx_try().is_none());

        // Adding with a group, changing it, and taking the source out of it
        // (an empty group): each reaches the agent as it was asked.
        for (update, group) in [(false, Some("host:h2")), (true, Some("g-0a1b2c3d4e5f")), (true, Some("")), (true, None)] {
            let agent_state = state.clone();
            let expected = group.map(str::to_string);
            let agent = tokio::spawn(async move {
                let (request_id, change) = asked(&mut to_agent).await;
                match &change {
                    SourceChange::Add { group, .. } | SourceChange::Update { group, .. } => assert_eq!(group, &expected),
                    other => panic!("{:?}", other),
                }
                // A group is not printed with the change either way (it is no secret, but the log names fields only).
                crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: Some("den-pc-ab12".into()), error: None, setup: None }).await;
                to_agent
            });
            let (status, _) = if update {
                body(update_source(admin(), Path(("h1".into(), "den-pc-ab12".into())), State(state.clone()), Json(form(group))).await).await
            } else {
                body(add_source(admin(), Path("h1".into()), State(state.clone()), Json(form(group))).await).await
            };
            assert_eq!(status, StatusCode::OK);
            to_agent = agent.await.unwrap();
        }

        // On the wire: a change without a group is what it always was, and an
        // agent from before groups reads one with a group (it skips the field).
        let plain = SourceChange::Update { id: "a".into(), name: "A".into(), values: Default::default(), group: None };
        assert_eq!(serde_json::to_string(&plain).unwrap(), r#"{"op":"update","id":"a","name":"A","values":{}}"#);
        #[derive(Deserialize, Debug)]
        #[serde(tag = "op", rename_all = "snake_case")]
        enum OldChange {
            Add {
                name: String,
                #[allow(dead_code)]
                connector: String,
                #[allow(dead_code)]
                values: std::collections::BTreeMap<String, String>,
            },
        }
        let wire = serde_json::to_string(&SourceChange::Add { name: "A".into(), connector: "rustdesk".into(), values: Default::default(), group: Some("host:h1".into()) }).unwrap();
        let OldChange::Add { name, .. } = serde_json::from_str::<OldChange>(&wire).unwrap();
        assert_eq!(name, "A");
        // A server from before groups reads a new agent's announcement, and a
        // new one an old agent's.
        #[derive(Deserialize)]
        struct OldSource {
            id: String,
        }
        let told = serde_json::to_string(&grouped("a", "A", "g-1")).unwrap();
        assert_eq!(told, r#"{"id":"a","name":"A","connector":"rustdesk","group":"g-1"}"#);
        assert_eq!(serde_json::from_str::<OldSource>(&told).unwrap().id, "a");
        let old: SourceInfoMsg = serde_json::from_str(r#"{"id":"a","name":"A","connector":"rustdesk"}"#).unwrap();
        assert_eq!(old.group, None);
        assert_eq!(serde_json::to_string(&old).unwrap(), r#"{"id":"a","name":"A","connector":"rustdesk"}"#);
        // A form from an older client has no "group".
        let old_form: SourceForm = serde_json::from_str(r#"{"name":"A","values":{}}"#).unwrap();
        assert_eq!(old_form.group, None);
    }

    fn checked(id: &str, name: &str, online: Option<bool>) -> SourceInfoMsg {
        SourceInfoMsg { online, ..source(id, name) }
    }

    /// A source whose computer is shut down is offline although the
    /// Lunaris computer it is reached through is online.
    #[tokio::test]
    async fn a_source_is_online_only_when_its_computer_answers() {
        let state = state().await;
        let _h1 = connect(&state, "h1");
        announced(&state, "h1", vec![checked("a-off", "A off", Some(false)), checked("b-on", "B on", Some(true)), checked("c-new", "C unknown", None)], vec![rustdesk()], true).await;
        let status = |list: &serde_json::Value| -> Vec<(String, bool, Option<bool>)> {
            list.as_array().unwrap().iter().map(|s| (s["id"].as_str().unwrap().to_string(), s["online"].as_bool().unwrap(), s["reachable"].as_bool())).collect()
        };
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(status(&list), [("a-off".into(), false, Some(false)), ("b-on".into(), true, Some(true)), ("c-new".into(), true, None)]);
        // Not known is not said at all (as before the checks).
        assert!(list[2].get("reachable").is_none());

        // The next announcement (the agent's next check) changes it.
        announced(&state, "h1", vec![checked("a-off", "A off", Some(true)), checked("b-on", "B on", Some(false)), checked("c-new", "C unknown", None)], vec![rustdesk()], true).await;
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(status(&list), [("a-off".into(), true, Some(true)), ("b-on".into(), false, Some(false)), ("c-new".into(), true, None)]);

        // Through the agent's connection, as the agent sends it.
        crate::signaling::agent_message(&state, "h1", AgentMessage::Sources { sources: vec![checked("a-off", "A off", Some(false))], connectors: vec![rustdesk()], manageable: true, addresses: vec![], build: None }).await;
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(status(&list), [("a-off".into(), false, Some(false))]);

        // The agent goes away: all of its sources are offline, and what it
        // last found is not told.
        announced(&state, "h1", vec![checked("a-off", "A off", Some(true))], vec![rustdesk()], true).await;
        state.agents.write().unwrap().remove("h1");
        agent_gone(&state, "h1");
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(status(&list), [("a-off".into(), false, None)]);

        // On the wire: an agent from before the checks says nothing (and is
        // read as unknown), a new one says it only when it knows, and a
        // server from before the checks reads a new agent's announcement.
        let old: SourceInfoMsg = serde_json::from_str(r#"{"id":"a","name":"A","connector":"rustdesk"}"#).unwrap();
        assert_eq!(old.online, None);
        assert_eq!(serde_json::to_string(&checked("a", "A", None)).unwrap(), r#"{"id":"a","name":"A","connector":"rustdesk"}"#);
        let told = serde_json::to_string(&checked("a", "A", Some(false))).unwrap();
        assert_eq!(told, r#"{"id":"a","name":"A","connector":"rustdesk","online":false}"#);
        #[derive(Deserialize)]
        struct OldSource {
            id: String,
        }
        assert_eq!(serde_json::from_str::<OldSource>(&told).unwrap().id, "a");
        let announce: AgentMessage =
            serde_json::from_str(r#"{"event":"Sources","data":{"sources":[{"id":"a","name":"A","connector":"rustdesk","online":true}],"connectors":[]}}"#).unwrap();
        assert!(matches!(announce, AgentMessage::Sources { ref sources, .. } if sources[0].online == Some(true)));
    }

    /// A database from before groups gets the column, and what it has stays.
    #[tokio::test]
    async fn an_older_database_gets_the_group_column() {
        let dir = std::env::temp_dir().join(format!("lunaris-sources-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.join("old.db").display());
        // As a server from before groups left it: the table without the column, with a source in it.
        {
            let pool = crate::db::init_db(&url).await.unwrap();
            sqlx::query("DROP TABLE host_sources").execute(&pool).await.unwrap();
            sqlx::query(
                "CREATE TABLE host_sources (host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE, source_id TEXT NOT NULL, name TEXT NOT NULL,
                 connector TEXT NOT NULL, connector_name TEXT NOT NULL DEFAULT '', PRIMARY KEY (host_id, source_id))",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', 'Office PC', 'Offline')").execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO host_sources (host_id, source_id, name, connector, connector_name) VALUES ('h1', 'den-1', 'Den PC', 'rustdesk', 'RustDesk')").execute(&pool).await.unwrap();
            let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('host_sources')").fetch_all(&pool).await.unwrap();
            assert!(!columns.iter().any(|c| c == "source_group"));
            pool.close().await;
        }
        let pool = crate::db::init_db(&url).await.unwrap();
        let row: (String, Option<String>) = sqlx::query_as("SELECT name, source_group FROM host_sources WHERE source_id = 'den-1'").fetch_one(&pool).await.unwrap();
        assert_eq!(row, ("Den PC".to_string(), None));
        // Opening it again changes nothing.
        pool.close().await;
        let pool = crate::db::init_db(&url).await.unwrap();
        let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('host_sources')").fetch_all(&pool).await.unwrap();
        assert_eq!(columns.iter().filter(|c| c.as_str() == "source_group").count(), 1);
        pool.close().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The pretend agent's next request; `None` when nothing was sent.
    async fn asked(to_agent: &mut crate::queue::Inbox<ServerToAgentMessage>) -> (String, SourceChange) {
        match to_agent.recv().await {
            Some(ServerToAgentMessage::ChangeSource { request_id, change, .. }) => (request_id, change),
            other => panic!("no ChangeSource: {:?}", other),
        }
    }

    fn step(setup: &str, message: &str, code: Option<&str>) -> Option<SetupInfo> {
        Some(SetupInfo { setup: setup.into(), message: message.into(), code: code.map(str::to_string) })
    }

    #[tokio::test]
    async fn adding_with_a_set_up_is_answered_202_then_asked_about_until_it_is_done() {
        let state = state().await;
        let mut to_agent = connect(&state, "h1");
        announced(&state, "h1", vec![], vec![rustdesk()], true).await;
        let _other = connect(&state, "h2");
        announced(&state, "h2", vec![], vec![rustdesk()], true).await;
        let form = || SourceForm { name: "Den PC".into(), connector: "sunshine".into(), values: [("host".to_string(), "192.168.1.50".to_string())].into_iter().collect(), group: None };
        const MESSAGE: &str = "Open Sunshine on the other computer (its web page, PIN) and enter this PIN.";

        // The agent starts the pairing and says what to show.
        let agent_state = state.clone();
        let agent = tokio::spawn(async move {
            let (request_id, change) = asked(&mut to_agent).await;
            assert!(matches!(change, SourceChange::Add { ref connector, .. } if connector == "sunshine"));
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: None, error: None, setup: step("su-1", MESSAGE, Some("4711")) }).await;
            to_agent
        });
        let response = add_source(admin(), Path("h1".into()), State(state.clone()), Json(form())).await.into_response();
        // The answer has the PIN: it is not to be kept anywhere on its way.
        assert_eq!(response.headers().get(axum::http::header::CACHE_CONTROL).map(|v| v.as_bytes()), Some(&b"no-store"[..]));
        let (status, answer) = body(response).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(answer, serde_json::json!({ "setup": "su-1", "message": MESSAGE, "code": "4711" }));
        let mut to_agent = agent.await.unwrap();

        // Only a set-up started here is asked about, and only on its computer:
        // anything else is 404 and nothing is sent to an agent.
        let (status, _) = body(setup_state(admin(), Path(("h1".into(), "su-other".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = body(setup_state(admin(), Path(("h2".into(), "su-1".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, answer) = body(cancel_setup(admin(), Path(("h2".into(), "su-1".into())), State(state.clone())).await).await;
        assert_eq!((status, answer), (StatusCode::OK, serde_json::json!({})));
        assert!(to_agent.rx_try().is_none());

        // Still waiting: the step as it is now (a step without a code has none).
        let agent_state = state.clone();
        let agent = tokio::spawn(async move {
            let (request_id, change) = asked(&mut to_agent).await;
            assert_eq!(change, SourceChange::SetupState { setup: "su-1".into() });
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: None, error: None, setup: step("su-1", "Almost there", None) }).await;
            to_agent
        });
        let response = setup_state(admin(), Path(("h1".into(), "su-1".into())), State(state.clone())).await.into_response();
        assert_eq!(response.headers().get(axum::http::header::CACHE_CONTROL).map(|v| v.as_bytes()), Some(&b"no-store"[..]));
        let (status, answer) = body(response).await;
        assert_eq!((status, answer), (StatusCode::OK, serde_json::json!({ "state": "pending", "message": "Almost there", "code": null })));
        let mut to_agent = agent.await.unwrap();

        // Done: the agent's new list first, then the id. The card is there at once.
        let agent_state = state.clone();
        let agent = tokio::spawn(async move {
            let (request_id, _) = asked(&mut to_agent).await;
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::Sources { sources: vec![SourceInfoMsg { id: "den-pc-ab12".into(), name: "Den PC".into(), connector: "sunshine".into(), group: None, online: None, values: None }], connectors: vec![rustdesk()], manageable: true, addresses: vec![], build: None }).await;
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: Some("den-pc-ab12".into()), error: None, setup: None }).await;
            to_agent
        });
        let (status, answer) = body(setup_state(admin(), Path(("h1".into(), "su-1".into())), State(state.clone())).await).await;
        assert_eq!((status, answer), (StatusCode::OK, serde_json::json!({ "state": "done", "id": "den-pc-ab12" })));
        let mut to_agent = agent.await.unwrap();
        let (_, list) = body(list_sources(user("alice"), State(state.clone())).await).await;
        assert_eq!(list[0]["id"], "den-pc-ab12");
        // It is over: not known any more, and the agent is not asked again.
        let (status, _) = body(setup_state(admin(), Path(("h1".into(), "su-1".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(to_agent.rx_try().is_none());
        assert!(state.sources.pending.lock().unwrap().is_empty() && state.sources.setups.lock().unwrap().is_empty());

        // Nothing of the PIN or the address is kept, in any table.
        let tables: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'").fetch_all(&state.db).await.unwrap();
        for table in tables {
            let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT * FROM \"{}\"", table))).fetch_all(&state.db).await.unwrap();
            for row in rows {
                use sqlx::Row;
                for i in 0..row.len() {
                    let text: Option<String> = row.try_get(i).ok().flatten();
                    assert!(!text.as_deref().unwrap_or("").contains("4711") && !text.as_deref().unwrap_or("").contains("192.168.1.50"), "{}", table);
                }
            }
        }
    }

    #[tokio::test]
    async fn a_set_up_that_fails_or_is_cancelled() {
        let state = state().await;
        let to_agent = connect(&state, "h1");
        announced(&state, "h1", vec![], vec![rustdesk()], true).await;
        let form = || SourceForm { name: "Den PC".into(), connector: "sunshine".into(), values: Default::default(), group: None };

        /// Adds with the pretend agent answering "set-up `setup` waits".
        async fn add(state: &Arc<SignalingState>, mut to_agent: crate::queue::Inbox<ServerToAgentMessage>, form: SourceForm, setup: &'static str) -> crate::queue::Inbox<ServerToAgentMessage> {
            let agent_state = state.clone();
            let agent = tokio::spawn(async move {
                let (request_id, _) = asked(&mut to_agent).await;
                crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: None, error: None, setup: step(setup, "Enter this PIN", Some("0000")) }).await;
                to_agent
            });
            let (status, _) = body(add_source(admin(), Path("h1".into()), State(state.clone()), Json(form)).await).await;
            assert_eq!(status, StatusCode::ACCEPTED);
            agent.await.unwrap()
        }

        // Failed: the agent's words, with 200 (the question was answered).
        let mut to_agent = add(&state, to_agent, form(), "su-1").await;
        let agent_state = state.clone();
        let agent = tokio::spawn(async move {
            let (request_id, _) = asked(&mut to_agent).await;
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: None, error: Some("Pairing was not completed in time".into()), setup: None }).await;
            to_agent
        });
        let (status, answer) = body(setup_state(admin(), Path(("h1".into(), "su-1".into())), State(state.clone())).await).await;
        assert_eq!((status, answer), (StatusCode::OK, serde_json::json!({ "state": "failed", "error": "Pairing was not completed in time" })));
        let to_agent = agent.await.unwrap();
        let (status, _) = body(setup_state(admin(), Path(("h1".into(), "su-1".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Cancelled: the agent is told once; cancelling again is not an error and tells nobody.
        let mut to_agent = add(&state, to_agent, form(), "su-2").await;
        let agent_state = state.clone();
        let agent = tokio::spawn(async move {
            let (request_id, change) = asked(&mut to_agent).await;
            assert_eq!(change, SourceChange::SetupCancel { setup: "su-2".into() });
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: Some("su-2".into()), error: None, setup: None }).await;
            to_agent
        });
        let (status, answer) = body(cancel_setup(admin(), Path(("h1".into(), "su-2".into())), State(state.clone())).await).await;
        assert_eq!((status, answer), (StatusCode::OK, serde_json::json!({})));
        let mut to_agent = agent.await.unwrap();
        let (status, _) = body(cancel_setup(admin(), Path(("h1".into(), "su-2".into())), State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        assert!(to_agent.rx_try().is_none());

        // The computer goes offline while its set-up waits: asking says so
        // (409), and the set-up is still known for when it is back.
        let _to_agent = add(&state, to_agent, form(), "su-3").await;
        state.agents.write().unwrap().remove("h1");
        agent_gone(&state, "h1");
        let (status, answer) = body(setup_state(admin(), Path(("h1".into(), "su-3".into())), State(state.clone())).await).await;
        assert_eq!((status, answer["error"].as_str()), (StatusCode::CONFLICT, Some("That computer is offline")));
        assert!(state.sources.setup_known("su-3", "h1"));

        // An answer with a set-up to a change that has none is not taken for success.
        let mut to_agent = connect(&state, "h1");
        announced(&state, "h1", vec![], vec![rustdesk()], true).await;
        let agent_state = state.clone();
        tokio::spawn(async move {
            let (request_id, _) = asked(&mut to_agent).await;
            crate::signaling::agent_message(&agent_state, "h1", AgentMessage::SourceChanged { request_id, id: None, error: None, setup: step("x", "m", None) }).await;
        });
        let (status, _) = body(update_source(admin(), Path(("h1".into(), "a".into())), State(state.clone()), Json(form())).await).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn set_up_messages_on_the_wire_and_with_older_peers() {
        use serde::Deserialize;
        // Asking about a set-up and stopping it are changes like the others.
        let ask = ServerToAgentMessage::ChangeSource { request_id: "r".into(), change: SourceChange::SetupState { setup: "s1".into() }, requester: None };
        assert_eq!(serde_json::to_string(&ask).unwrap(), r#"{"event":"ChangeSource","data":{"request_id":"r","change":{"op":"setup_state","setup":"s1"}}}"#);
        let cancel: SourceChange = serde_json::from_str(r#"{"op":"setup_cancel","setup":"s1"}"#).unwrap();
        assert_eq!(cancel, SourceChange::SetupCancel { setup: "s1".into() });

        // The answer with a set-up; a step without a code has no "code".
        let info = SetupInfo { setup: "s1".into(), message: "Enter this PIN".into(), code: Some("4711".into()) };
        let answer = AgentMessage::SourceChanged { request_id: "r".into(), id: None, error: None, setup: Some(info.clone()) };
        let wire = serde_json::to_string(&answer).unwrap();
        assert_eq!(wire, r#"{"event":"SourceChanged","data":{"request_id":"r","setup":{"setup":"s1","message":"Enter this PIN","code":"4711"}}}"#);
        assert!(matches!(serde_json::from_str::<AgentMessage>(&wire).unwrap(), AgentMessage::SourceChanged { setup: Some(read), .. } if read == info));
        let plain = serde_json::to_string(&SetupInfo { code: None, ..info.clone() }).unwrap();
        assert_eq!(plain, r#"{"setup":"s1","message":"Enter this PIN"}"#);
        // What is printed of them never has the code or the message.
        let printed = format!("{:?} {:?}", answer, ask);
        assert!(!printed.contains("4711") && !printed.contains("Enter this PIN"), "{}", printed);

        // Peers from before set-ups: their answer has no `setup`, and an
        // answer without one is on the wire as it always was.
        let old: AgentMessage = serde_json::from_str(r#"{"event":"SourceChanged","data":{"request_id":"r","id":"den-1"}}"#).unwrap();
        assert!(matches!(old, AgentMessage::SourceChanged { id: Some(_), error: None, setup: None, .. }));
        let same = serde_json::to_string(&AgentMessage::SourceChanged { request_id: "r".into(), id: None, error: Some("no".into()), setup: None }).unwrap();
        assert_eq!(same, r#"{"event":"SourceChanged","data":{"request_id":"r","error":"no"}}"#);
        // An older peer that derives the same way skips the field it does not know.
        #[derive(Deserialize)]
        struct OldAnswer {
            #[allow(dead_code)]
            request_id: String,
            id: Option<String>,
            error: Option<String>,
        }
        let read: OldAnswer = serde_json::from_str(r#"{"request_id":"r","setup":{"setup":"s1","message":"m","code":"1"}}"#).unwrap();
        assert!(read.id.is_none() && read.error.is_none());
        // An older agent does not know the new changes: the message fails to
        // parse there as a whole (and is dropped), which is why they are only
        // ever sent about a set-up that agent started itself.
        #[derive(Deserialize, Debug)]
        #[serde(tag = "op", rename_all = "snake_case")]
        enum OldChange {
            #[allow(dead_code)]
            Remove { id: String },
        }
        assert!(serde_json::from_str::<OldChange>(r#"{"op":"setup_state","setup":"s1"}"#).is_err());
    }
}
