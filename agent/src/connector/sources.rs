//! The sources of this computer's agent, for the settings: other computers
//! it shows besides its own screen, each through a connector
//! (agent/src/connector). Listing, adding and removing them in the agent's
//! config; the callers load and save it.
//!
//! What a source's fields hold (address, password, ...) never goes back to
//! the window and is never logged: a listing has the names only. A source
//! can also hold values that are no field of its connector's form: what the
//! connector's set-up returned (a pairing's certificate and key). They are
//! never listed to anyone and stay when the source is edited.
//!
//! Adding a source of a connector that has a set-up ([`begin_add`]) runs it
//! in the background; what it wants shown (a PIN to type at the other
//! computer) is asked for with [`setup_state`], and the source is saved when
//! it ends well.
//!
//! Used by both places the agent is configured (the Lunaris app's settings
//! and the stand-alone agent's window).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::connector::{Connector, Description, SetupStep, SourceConfig, Target};
use crate::pairing::AgentConfig;
use serde::Serialize;
use tracing::{info, warn};

/// Keys of a source in the config that are not a connector's field.
const RESERVED_KEYS: [&str; 4] = ["id", "name", "connector", "group"];
const MAX_NAME_CHARS: usize = 64;

#[derive(Serialize)]
struct FieldView<'a> {
    key: &'a str,
    label: &'a str,
    kind: &'a str,
    default: &'a str,
    required: bool,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    options: &'a [common::FieldOptionMsg],
}

#[derive(Serialize)]
struct ConnectorView<'a> {
    id: &'a str,
    name: &'a str,
    note: &'a str,
    fields: Vec<FieldView<'a>>,
}

#[derive(Serialize)]
struct SourceView<'a> {
    id: &'a str,
    name: &'a str,
    connector: &'a str,
    /// The connector's name, or its id when it is not installed (any more).
    connector_name: &'a str,
    /// Which computer it is, when it is listed in other ways too.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<&'a str>,
}

#[derive(Serialize)]
struct Listing<'a> {
    connectors: Vec<ConnectorView<'a>>,
    sources: Vec<SourceView<'a>>,
}

/// The connectors to choose from and the sources there are, as JSON for
/// the settings. Of a source only its id, its name and its connector.
pub fn listing_json(sources: &[SourceConfig], connectors: &[Description]) -> String {
    let listing = Listing {
        connectors: connectors
            .iter()
            .map(|c| ConnectorView {
                id: &c.id,
                name: &c.name,
                note: &c.note,
                fields: c
                    .fields
                    .iter()
                    .map(|f| FieldView { key: &f.key, label: &f.label, kind: &f.kind, default: &f.default, required: f.required, options: &f.options })
                    .collect(),
            })
            .collect(),
        sources: sources
            .iter()
            .map(|s| SourceView {
                id: &s.id,
                name: &s.name,
                connector: &s.target.connector,
                connector_name: connectors
                    .iter()
                    .find(|c| c.id == s.target.connector)
                    .map(|c| c.name.as_str())
                    .unwrap_or(&s.target.connector),
                group: group_of(s),
            })
            .collect(),
    };
    serde_json::to_string(&listing).unwrap_or_else(|_| "{\"connectors\":[],\"sources\":[]}".to_string())
}

/// The fields of a form, checked: only the connector's own fields are
/// kept. `stored` is what a source being edited has: a field left out, or a
/// secret or required field left empty, keeps its stored value; an optional
/// field sent empty is cleared. The error is a text for the person and names
/// fields, never what was typed in them.
fn read_values(description: &Description, values_json: &str, stored: Option<&BTreeMap<String, String>>) -> Result<BTreeMap<String, String>, String> {
    let typed: BTreeMap<String, String> =
        serde_json::from_str(values_json).map_err(|_| "The form could not be read".to_string())?;
    let mut values = BTreeMap::new();
    for field in &description.fields {
        if RESERVED_KEYS.contains(&field.key.as_str()) {
            continue;
        }
        let sent = typed.get(&field.key);
        let value = sent.map(String::as_str).unwrap_or("");
        // A password is kept as typed; anything else without the spaces around it.
        let value = if field.kind == "secret" { value } else { value.trim() };
        if value.is_empty() {
            // Sent empty on purpose: an optional field that is no secret is
            // cleared (a form that shows the stored values sends it so).
            if sent.is_some() && stored.is_some() && field.kind != "secret" && !field.required {
                continue;
            }
            if let Some(kept) = stored.and_then(|stored| stored.get(&field.key)) {
                values.insert(field.key.clone(), kept.clone());
            } else if field.required {
                return Err(format!("{} is required", field.label));
            }
            continue;
        }
        if field.kind == "port" && !value.parse::<u16>().is_ok_and(|port| port != 0) {
            return Err(format!("{} must be a port from 1 to 65535", field.label));
        }
        values.insert(field.key.clone(), value.to_string());
    }
    Ok(values)
}

fn checked_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the computer a name".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("The name can have {} characters at most", MAX_NAME_CHARS));
    }
    Ok(name)
}

// ---- Groups: the sources that are one computer ----

/// A group that cannot be one (`common::valid_source_group`).
pub const GROUP_NOT_VALID: &str = "That group of computers is not valid";

/// The fields that say where a computer is, by their key: what the
/// connectors there are call it ("host": RustDesk, Sunshine).
const ADDRESS_KEYS: [&str; 2] = ["host", "address"];

/// Where the computer with these values is, to tell whether two sources are
/// the same computer: its address field, without the spaces around it and
/// in lower case. `None` when it has no such field or it is empty. It is
/// compared here and goes nowhere.
fn target_address(values: &BTreeMap<String, String>) -> Option<String> {
    values
        .iter()
        .find(|(key, _)| ADDRESS_KEYS.iter().any(|known| key.eq_ignore_ascii_case(known)))
        .map(|(_, value)| value.trim().to_lowercase())
        .filter(|address| !address.is_empty())
}

/// The group of a source as it is told: none when what is in the config is
/// not fit to be one.
fn group_of(source: &SourceConfig) -> Option<&str> {
    source.group.as_deref().filter(|group| common::valid_source_group(group))
}

/// A group as it was asked for, checked: `None` says nothing about it,
/// `Some("")` asks for none.
fn checked_group(group: Option<&str>) -> Result<Option<&str>, String> {
    match group.map(str::trim) {
        Some(group) if !group.is_empty() && !common::valid_source_group(group) => Err(GROUP_NOT_VALID.to_string()),
        group => Ok(group),
    }
}

/// A group no source has yet.
fn new_group(existing: &[SourceConfig]) -> String {
    loop {
        let group = format!("g-{}", crate::pairing::to_hex(&crate::pairing::random_bytes(6)));
        if !existing.iter().any(|source| source.group.as_deref() == Some(group.as_str())) {
            return group;
        }
    }
}

/// The group of a source that is being added with these `values`. `asked`
/// is the group it was asked to have (checked): `Some("")` none, and `None`
/// the group of the first source at the same address, which gets one now
/// if it had none (then the two are one computer from here on).
fn group_for_new(existing: &mut [SourceConfig], values: &BTreeMap<String, String>, asked: Option<&str>) -> Option<String> {
    match asked {
        Some("") => None,
        Some(group) => Some(group.to_string()),
        None => {
            let address = target_address(values)?;
            let same = existing.iter().position(|source| target_address(&source.target.values).as_deref() == Some(address.as_str()))?;
            if group_of(&existing[same]).is_none() {
                existing[same].group = Some(new_group(existing));
            }
            existing[same].group.clone()
        }
    }
}

/// Adds a source to `config` and returns its new id. `values_json` is
/// `{"<field key>": "<value>"}` from the form. It is grouped with a source
/// at the same address, if there is one ([`add_in_group`] says otherwise).
pub fn add(
    config: &mut AgentConfig,
    connectors: &[Description],
    name: &str,
    connector: &str,
    values_json: &str,
) -> Result<String, String> {
    add_in_group(config, connectors, name, connector, values_json, None)
}

/// [`add`], saying which computer it is: `group` is its group
/// (`common::SourceInfoMsg::group`), `Some("")` for none, `None` to group
/// it with a source at the same address.
pub fn add_in_group(
    config: &mut AgentConfig,
    connectors: &[Description],
    name: &str,
    connector: &str,
    values_json: &str,
    group: Option<&str>,
) -> Result<String, String> {
    let name = checked_name(name)?;
    let group = checked_group(group)?;
    let description = connectors
        .iter()
        .find(|c| c.id == connector)
        .ok_or_else(|| "That connector is not installed".to_string())?;
    let values = read_values(description, values_json, None)?;
    let id = new_id(name, &config.sources);
    let group = group_for_new(&mut config.sources, &values, group);
    config.sources.push(SourceConfig {
        id: id.clone(),
        name: name.to_string(),
        group,
        target: Target { connector: description.id.clone(), values },
    });
    Ok(id)
}

/// Changes the source `id`: its name and its fields. A field left empty in
/// `values_json` keeps its value (so a password need not be typed again);
/// its id and its connector stay, and so does what its set-up returned.
pub fn update(
    config: &mut AgentConfig,
    connectors: &[Description],
    id: &str,
    name: &str,
    values_json: &str,
) -> Result<(), String> {
    update_in_group(config, connectors, id, name, values_json, None)
}

/// [`update`], and its group: `Some(group)` puts it in that group,
/// `Some("")` takes it out of its group (a computer of its own again),
/// `None` leaves it where it is.
pub fn update_in_group(
    config: &mut AgentConfig,
    connectors: &[Description],
    id: &str,
    name: &str,
    values_json: &str,
    group: Option<&str>,
) -> Result<(), String> {
    let name = checked_name(name)?;
    let group = checked_group(group)?;
    let source = config.sources.iter_mut().find(|source| source.id == id).ok_or_else(|| "That computer is not there any more".to_string())?;
    let description = connectors
        .iter()
        .find(|c| c.id == source.target.connector)
        .ok_or_else(|| "That connector is not installed".to_string())?;
    let mut values = read_values(description, values_json, Some(&source.target.values))?;
    // What is no field of the form (a set-up's certificate and key) cannot
    // be typed again: it stays as it is.
    for (key, value) in &source.target.values {
        if !description.fields.iter().any(|field| &field.key == key) {
            values.insert(key.clone(), value.clone());
        }
    }
    source.target.values = values;
    source.name = name.to_string();
    match group {
        None => {}
        Some("") => source.group = None,
        Some(group) => source.group = Some(group.to_string()),
    }
    Ok(())
}

/// Removes the source `id`; whether there was one.
pub fn remove(config: &mut AgentConfig, id: &str) -> bool {
    let before = config.sources.len();
    config.sources.retain(|source| source.id != id);
    config.sources.len() != before
}

/// The connectors to choose from, as JSON: `[{id, name, fields}]`.
pub fn connectors_json(connectors: &[Description]) -> String {
    serde_json::to_string(&connector_infos(connectors)).unwrap_or_else(|_| "[]".to_string())
}

/// The sources as JSON for a list and an edit form:
/// `[{id, name, connector, connector_name, values}]`, where `values` has the
/// connector's fields that are not secrets (a password never leaves the
/// config, and neither does a value that is no field of the form); and
/// `group` for one that has a group.
pub fn views_json(sources: &[SourceConfig], connectors: &[Description]) -> String {
    let views: Vec<serde_json::Value> = sources
        .iter()
        .map(|source| {
            let description = connectors.iter().find(|c| c.id == source.target.connector);
            let values = description.map(|d| form_values(source, d)).unwrap_or_default();
            let mut view = serde_json::json!({
                "id": source.id,
                "name": source.name,
                "connector": source.target.connector,
                "connector_name": description.map(|d| d.name.as_str()).unwrap_or(&source.target.connector),
                "values": values,
            });
            if let Some(group) = group_of(source) {
                view["group"] = serde_json::Value::String(group.to_string());
            }
            view
        })
        .collect();
    serde_json::to_string(&views).unwrap_or_else(|_| "[]".to_string())
}

/// Why a computer's sources cannot be changed from the server.
pub const NOT_MANAGEABLE: &str = "That computer asks at its screen before each connection, so computers cannot be added to it from here. Add it in the Lunaris app on that computer.";

/// Does what a server asked with this agent's sources, in its config file,
/// and returns the id of the source it was about.
pub fn apply(change: &common::SourceChange) -> Result<String, String> {
    let connectors = crate::connector::available(&crate::connector::connectors_dir());
    apply_at(crate::pairing::config_path(), &connectors, change)
}

/// `apply`, on the config file at `path`. Adding here runs no set-up
/// ([`answer_at`] does); asking about a set-up is not a change.
pub fn apply_at(path: &str, connectors: &[Description], change: &common::SourceChange) -> Result<String, String> {
    let _writing = CONFIG_WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let mut config = crate::pairing::load_config(path).map_err(|_| CONFIG_UNREADABLE.to_string())?;
    let json = |values: &BTreeMap<String, String>| serde_json::to_string(values).unwrap_or_else(|_| "{}".to_string());
    let id = match change {
        common::SourceChange::Add { name, connector, values, group } => add_in_group(&mut config, connectors, name, connector, &json(values), group.as_deref())?,
        common::SourceChange::Update { id, name, values, group } => {
            update_in_group(&mut config, connectors, id, name, &json(values), group.as_deref())?;
            id.clone()
        }
        common::SourceChange::Remove { id } => {
            if !remove(&mut config, id) {
                return Err("That computer is not there any more".into());
            }
            id.clone()
        }
        common::SourceChange::SetupState { .. } | common::SourceChange::SetupCancel { .. } => {
            return Err(SETUP_GONE.to_string());
        }
    };
    crate::pairing::save_config(&config, path).map_err(|_| CONFIG_UNSAVED.to_string())?;
    Ok(id)
}

/// What a server is answered (`AgentMessage::SourceChanged`): the source's
/// id when it is done, why not when it failed, what to show while a set-up
/// waits for the person.
#[derive(Default)]
pub struct Answer {
    pub id: Option<String>,
    pub error: Option<String>,
    pub setup: Option<common::SetupInfo>,
}

impl Answer {
    fn of(result: Result<String, String>) -> Self {
        match result {
            Ok(id) => Answer { id: Some(id), ..Default::default() },
            Err(error) => Answer { error: Some(error), ..Default::default() },
        }
    }

    fn waiting(setup: String, step: SetupStep) -> Self {
        Answer { setup: Some(common::SetupInfo { setup, message: step.message, code: step.code }), ..Default::default() }
    }

    /// Why this computer's sources are not changed from a server.
    pub fn not_manageable() -> Self {
        Answer::of(Err(NOT_MANAGEABLE.to_string()))
    }
}

/// Does what a server asked with this agent's sources and says what to
/// answer. Adding can wait a few seconds for a set-up's first step: not
/// for a thread that must not wait.
pub fn answer(change: &common::SourceChange) -> Answer {
    answer_at(crate::pairing::config_path(), &crate::connector::connectors_dir(), change)
}

/// `answer`, on the config file at `path` with the connectors in `connectors_dir`.
pub fn answer_at(path: &str, connectors_dir: &std::path::Path, change: &common::SourceChange) -> Answer {
    match change {
        common::SourceChange::Add { name, connector, values, group } => {
            let values_json = serde_json::to_string(values).unwrap_or_else(|_| "{}".to_string());
            match begin_add_in_group(path, connectors_dir, name, connector, &values_json, group.as_deref()) {
                Ok(AddOutcome::Added { id }) => Answer::of(Ok(id)),
                Ok(AddOutcome::Setup { setup, step }) => Answer::waiting(setup, step),
                Err(error) => Answer::of(Err(error)),
            }
        }
        common::SourceChange::SetupState { setup } => match setup_state(setup) {
            Some(SetupState::Pending { step }) => Answer::waiting(setup.clone(), step),
            Some(SetupState::Done { id }) => Answer::of(Ok(id)),
            Some(SetupState::Failed { error }) => Answer::of(Err(error)),
            None => Answer::of(Err(SETUP_GONE.to_string())),
        },
        common::SourceChange::SetupCancel { setup } => {
            cancel_setup(setup);
            Answer::of(Ok(setup.clone()))
        }
        common::SourceChange::Update { .. } | common::SourceChange::Remove { .. } => {
            Answer::of(apply_at(path, &crate::connector::available(connectors_dir), change))
        }
    }
}

/// The messages a server gets for the change it asked for: this agent's
/// new list first (the server has it when the answer is out), then the
/// answer. `via` names the way the server is connected, for the log. Can
/// wait a few seconds (`answer`).
pub fn reply(request_id: String, change: &common::SourceChange, requester: Option<&str>, via: &str, manageable: bool) -> Vec<common::AgentMessage> {
    let answer = if manageable {
        // Not each time a set-up is asked about (once a second).
        if !matches!(change, common::SourceChange::SetupState { .. }) {
            info!("Sources: {:?} asked for by {} through {}", change, requester.unwrap_or("someone"), via);
        }
        answer(change)
    } else {
        Answer::not_manageable()
    };
    let mut messages = Vec::new();
    // While a set-up waits nothing has changed.
    if answer.setup.is_none() {
        messages.push(announcement(manageable));
    }
    messages.push(common::AgentMessage::SourceChanged { request_id, id: answer.id, error: answer.error, setup: answer.setup });
    messages
}

// ---- Set-up: what a connector does once when a target is added ----

const CONFIG_UNREADABLE: &str = "That computer's settings could not be read";
const CONFIG_UNSAVED: &str = "That computer's settings could not be saved";
/// A set-up that is asked about and is not known (any more).
pub const SETUP_GONE: &str = "That set-up is not there any more. Add the computer again.";
/// A set-up nobody finished.
pub const SETUP_TIMED_OUT: &str = "Pairing was not completed in time";
/// Shown while a set-up has not said anything yet.
const SETUP_STARTING: &str = "Getting ready...";
/// The most set-ups running at one time.
const MAX_SETUPS: usize = 8;
const TOO_MANY_SETUPS: &str = "Too many computers are being set up at the moment. Finish or close one first.";

/// One writer of a config file at a time (a set-up ends when it ends;
/// settings.rs takes it too).
pub(crate) static CONFIG_WRITES: Mutex<()> = Mutex::new(());

/// How long the parts of a set-up may take.
#[derive(Clone, Copy)]
struct Limits {
    /// How long adding waits for the first step (or the end).
    first_step: Duration,
    /// After this a set-up nobody finished ends by itself.
    whole: Duration,
    /// How long its end is remembered.
    kept: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { first_step: Duration::from_secs(3), whole: Duration::from_secs(300), kept: Duration::from_secs(600) }
    }
}

/// What adding a source came to.
#[derive(Clone, PartialEq, Eq)]
pub enum AddOutcome {
    /// It is saved.
    Added { id: String },
    /// Its connector's set-up runs and wants this shown; ask on with
    /// [`setup_state`].
    Setup { setup: String, step: SetupStep },
}

/// Where a set-up is.
#[derive(Clone, PartialEq, Eq)]
pub enum SetupState {
    Pending { step: SetupStep },
    /// The source is saved.
    Done { id: String },
    Failed { error: String },
}

// A step can hold a PIN: it is never printed.
impl std::fmt::Debug for AddOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddOutcome::Added { id } => write!(f, "Added {{ id: {:?} }}", id),
            AddOutcome::Setup { setup, .. } => write!(f, "Setup {{ setup: {:?} }}", setup),
        }
    }
}

impl std::fmt::Debug for SetupState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupState::Pending { .. } => f.write_str("Pending"),
            SetupState::Done { id } => write!(f, "Done {{ id: {:?} }}", id),
            SetupState::Failed { error } => write!(f, "Failed {{ error: {:?} }}", error),
        }
    }
}

struct Running {
    state: SetupState,
    /// When it ended and how long that is remembered.
    ended: Option<Instant>,
    kept: Duration,
    task: Option<tokio::task::AbortHandle>,
    /// The config file its source goes to.
    config_path: String,
}

static SETUPS: Mutex<BTreeMap<String, Running>> = Mutex::new(BTreeMap::new());

fn setups() -> std::sync::MutexGuard<'static, BTreeMap<String, Running>> {
    let mut all = SETUPS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|_, running| !running.ended.is_some_and(|ended| ended.elapsed() >= running.kept));
    all
}

/// Set-ups run on a thread of their own, so that adding can be asked for
/// from anywhere (the agent's connections, the Lunaris app's window).
fn setup_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread().worker_threads(1).thread_name("lunaris-setup").enable_all().build().ok()
        })
        .as_ref()
        .ok_or_else(|| "The set-up could not be started".to_string())
}

/// Adds a source to the config file at `config_path`. `values_json` is
/// `{"<field key>": "<value>"}` from the form; the connector is looked for
/// among the built-in ones and in `connectors_dir`.
///
/// When the connector has a set-up it is started in the background, after
/// the form was checked, and this waits up to 3 seconds for its first step
/// or its end. The source is saved when the set-up ends well, with what it
/// returned. A set-up nobody finishes ends by itself after 5 minutes.
pub fn begin_add(
    config_path: &str,
    connectors_dir: &std::path::Path,
    name: &str,
    connector: &str,
    values_json: &str,
) -> Result<AddOutcome, String> {
    begin_add_in_group(config_path, connectors_dir, name, connector, values_json, None)
}

/// [`begin_add`], saying which computer it is (as [`add_in_group`]). The
/// group is the one it has when it is saved: with a set-up, when that ends.
pub fn begin_add_in_group(
    config_path: &str,
    connectors_dir: &std::path::Path,
    name: &str,
    connector: &str,
    values_json: &str,
    group: Option<&str>,
) -> Result<AddOutcome, String> {
    let connectors = crate::connector::available(connectors_dir);
    let description = connectors.iter().find(|c| c.id == connector).ok_or_else(|| "That connector is not installed".to_string())?;
    if !description.capabilities.setup {
        let checked = checked_name(name)?;
        let group = checked_group(group)?;
        let values = read_values(description, values_json, None)?;
        return save_new(config_path, checked, &description.id, values, group).map(|id| AddOutcome::Added { id });
    }
    let found = crate::connector::find(connector, connectors_dir).map_err(|_| "That connector is not installed".to_string())?;
    begin_with(found, config_path, name, values_json, group, Limits::default())
}

/// Saves a new source in the config file and returns its id. `group` as
/// [`group_for_new`] takes it; `values` may hold what a set-up returned.
fn save_new(config_path: &str, name: &str, connector: &str, values: BTreeMap<String, String>, group: Option<&str>) -> Result<String, String> {
    let _writing = CONFIG_WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let mut config = crate::pairing::load_config(config_path).map_err(|_| CONFIG_UNREADABLE.to_string())?;
    let id = new_id(name, &config.sources);
    let group = group_for_new(&mut config.sources, &values, group);
    config.sources.push(SourceConfig { id: id.clone(), name: name.to_string(), group, target: Target { connector: connector.to_string(), values } });
    crate::pairing::save_config(&config, config_path).map_err(|_| CONFIG_UNSAVED.to_string())?;
    Ok(id)
}

/// `begin_add` for a connector that has a set-up.
fn begin_with(connector: Arc<dyn Connector>, config_path: &str, name: &str, values_json: &str, group: Option<&str>, limits: Limits) -> Result<AddOutcome, String> {
    let description = connector.describe();
    let name = checked_name(name)?.to_string();
    let group = checked_group(group)?.map(str::to_string);
    let values = read_values(&description, values_json, None)?;
    // Not after the person typed a PIN somewhere.
    crate::pairing::load_config(config_path).map_err(|_| CONFIG_UNREADABLE.to_string())?;
    let runtime = setup_runtime()?;

    let setup = crate::pairing::to_hex(&crate::pairing::random_bytes(12));
    {
        let mut all = setups();
        if all.values().filter(|running| running.ended.is_none()).count() >= MAX_SETUPS {
            return Err(TOO_MANY_SETUPS.to_string());
        }
        let step = SetupStep { message: SETUP_STARTING.to_string(), code: None };
        all.insert(setup.clone(), Running { state: SetupState::Pending { step }, ended: None, kept: limits.kept, task: None, config_path: config_path.to_string() });
    }
    // Only the names of the fields: never what is in them.
    info!("Sources: set-up of a \"{}\" computer started (fields: {:?})", description.id, values.keys().collect::<Vec<_>>());
    let (said_tx, said) = std::sync::mpsc::channel::<()>();
    let task = runtime.spawn(run_setup(setup.clone(), connector, config_path.to_string(), name, description.id, values, group, limits.whole, said_tx));
    if let Some(running) = setups().get_mut(&setup) {
        if running.ended.is_none() {
            running.task = Some(task.abort_handle());
        }
    }

    // Its first step, or its end, or neither in time.
    let _ = said.recv_timeout(limits.first_step);
    let mut all = setups();
    match all.get(&setup).map(|running| running.state.clone()) {
        Some(SetupState::Pending { step }) => Ok(AddOutcome::Setup { setup, step }),
        Some(SetupState::Done { id }) => {
            all.remove(&setup);
            Ok(AddOutcome::Added { id })
        }
        Some(SetupState::Failed { error }) => {
            all.remove(&setup);
            Err(error)
        }
        None => Err(SETUP_GONE.to_string()),
    }
}

/// One set-up, from its start to the saved source (or the reason why not).
#[allow(clippy::too_many_arguments)]
async fn run_setup(
    setup: String,
    connector: Arc<dyn Connector>,
    config_path: String,
    name: String,
    connector_id: String,
    mut values: BTreeMap<String, String>,
    group: Option<String>,
    whole: Duration,
    said: std::sync::mpsc::Sender<()>,
) {
    let target = Target { connector: connector_id.clone(), values: values.clone() };
    let (steps_tx, mut steps) = tokio::sync::mpsc::channel::<SetupStep>(8);
    let mut work = connector.setup(&target, steps_tx);
    let deadline = tokio::time::sleep(whole);
    tokio::pin!(deadline);
    let mut steps_open = true;
    let result = loop {
        tokio::select! {
            result = &mut work => break result.map_err(|e| e.to_string()),
            step = steps.recv(), if steps_open => match step {
                Some(step) => {
                    if let Some(running) = setups().get_mut(&setup) {
                        running.state = SetupState::Pending { step };
                    }
                    let _ = said.send(());
                }
                None => steps_open = false,
            },
            _ = &mut deadline => break Err(SETUP_TIMED_OUT.to_string()),
        }
    };
    // Ended or given up on: the connector's work is dropped (its program stopped).
    drop(work);

    // No waiting from here on: a set-up that is cancelled is cancelled
    // before the source is saved, or not at all.
    let mut all = setups();
    let Some(running) = all.get_mut(&setup) else { return };
    let state = match result {
        Ok(kept) => {
            // What the set-up returned is kept with the form's values.
            values.extend(kept.into_iter().filter(|(key, _)| !key.is_empty() && !RESERVED_KEYS.contains(&key.as_str())));
            match save_new(&config_path, &name, &connector_id, values, group.as_deref()) {
                Ok(id) => SetupState::Done { id },
                Err(error) => SetupState::Failed { error },
            }
        }
        Err(error) => SetupState::Failed { error },
    };
    let done = matches!(state, SetupState::Done { .. });
    match &state {
        SetupState::Done { id } => info!("Sources: set-up of a \"{}\" computer is done: {} added", connector_id, id),
        // The connector's words are for the person, not for the log.
        _ => warn!("Sources: set-up of a \"{}\" computer did not succeed", connector_id),
    }
    running.state = state;
    running.ended = Some(Instant::now());
    running.task = None;
    drop(all);
    let _ = said.send(());
    // The servers hear of the new source now, whether or not anybody still
    // asks about the set-up.
    if done {
        sources_changed().send_modify(|count| *count += 1);
    }
}

/// Counts the sources that were added without a server having asked just
/// then (a set-up that ended well).
fn sources_changed() -> &'static tokio::sync::watch::Sender<u64> {
    static CHANGED: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();
    CHANGED.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// The servers are told the sources again (whether each is there changed:
/// `reachability`).
pub(crate) fn note_sources_changed() {
    sources_changed().send_modify(|count| *count += 1);
}

/// For one connection to a server: sends this agent's sources to `tx` again
/// whenever a set-up has added one, and when whether they are there
/// changed (`reachability`; at most once per round of checks, and a burst
/// of changes is one announcement). Ends when the returned guard is dropped
/// (the connection ended).
pub fn announce_changes(tx: tokio::sync::mpsc::UnboundedSender<common::AgentMessage>, manageable: bool) -> AnnounceChanges {
    let mut changed = sources_changed().subscribe();
    AnnounceChanges(tokio::spawn(async move {
        while changed.changed().await.is_ok() {
            if tx.send(announcement(manageable)).is_err() {
                break;
            }
        }
    }))
}

pub struct AnnounceChanges(tokio::task::JoinHandle<()>);

impl Drop for AnnounceChanges {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Where the set-up `setup` is; `None` when it is not known or forgotten
/// (its end is remembered for 10 minutes).
pub fn setup_state(setup: &str) -> Option<SetupState> {
    setups().get(setup).map(|running| running.state.clone())
}

/// Stops the set-up `setup` (the connector's work is dropped) and forgets
/// it. A source it has saved already stays.
pub fn cancel_setup(setup: &str) {
    let removed = setups().remove(setup);
    if let Some(task) = removed.and_then(|running| running.task) {
        task.abort();
        info!("Sources: a set-up was cancelled");
    }
}

/// Stops every set-up that runs.
pub fn cancel_all_setups() {
    let all = std::mem::take(&mut *setups());
    for task in all.into_values().filter_map(|running| running.task) {
        task.abort();
    }
}

/// Stops the set-ups whose source would go to the config file at
/// `config_path`, and forgets them (the agent or the app of that file ends).
pub fn cancel_setups_at(config_path: &str) {
    let mut all = setups();
    let ids: Vec<String> = all.iter().filter(|(_, running)| running.config_path == config_path).map(|(id, _)| id.clone()).collect();
    for task in ids.iter().filter_map(|id| all.remove(id)).filter_map(|running| running.task) {
        task.abort();
    }
}

/// Held while an agent runs: when it stops, so do the set-ups it started.
pub struct StopSetupsOnDrop(pub String);

impl Drop for StopSetupsOnDrop {
    fn drop(&mut self) {
        cancel_setups_at(&self.0);
    }
}

/// What a server is told when the agent connects and when its sources
/// change: the sources (id, name, connector, group, fields that are not
/// secrets), the connectors there are and this computer's addresses.
pub fn announcement(manageable: bool) -> common::AgentMessage {
    let configured = crate::connector::configured_sources();
    let available = crate::connector::available(&crate::connector::connectors_dir());
    let mut sources = source_infos(&configured, &available);
    // Whether each is there, as far as it is known (`reachability`).
    if let Some(monitor) = crate::connector::reachability::agent_monitor() {
        for (info, source) in sources.iter_mut().zip(&configured) {
            info.online = monitor.status(&source.target, available.iter().find(|c| c.id == source.target.connector));
        }
    }
    common::AgentMessage::Sources {
        sources,
        connectors: connector_infos(&available),
        manageable,
        addresses: crate::direct::known_local_addresses(),
    }
}

/// The connectors as the server and the viewers are told them.
pub fn connector_infos(connectors: &[Description]) -> Vec<common::ConnectorInfoMsg> {
    connectors
        .iter()
        .map(|c| common::ConnectorInfoMsg {
            id: c.id.clone(),
            name: c.name.clone(),
            note: c.note.clone(),
            capabilities: Some(common::ConnectorCapabilitiesMsg {
                audio: c.capabilities.audio,
                input: c.capabilities.input,
                clipboard: c.capabilities.clipboard,
                cursor: c.capabilities.cursor,
                files: c.capabilities.files,
                setup: c.capabilities.setup,
            }),
            fields: c
                .fields
                .iter()
                .map(|f| common::ConnectorFieldMsg {
                    key: f.key.clone(),
                    label: f.label.clone(),
                    kind: f.kind.clone(),
                    default: f.default.clone(),
                    required: f.required,
                    options: f.options.clone(),
                })
                .collect(),
        })
        .collect()
}

/// The values of `source`'s form fields that are not secrets: a password
/// never leaves the config, and neither does a value that is no field of
/// the form (what a set-up stored).
fn form_values(source: &SourceConfig, description: &Description) -> BTreeMap<String, String> {
    description
        .fields
        .iter()
        .filter(|field| field.kind != "secret")
        .filter_map(|field| source.target.values.get(&field.key).map(|value| (field.key.clone(), value.clone())))
        .collect()
}

/// The sources as the server is told them: id, name, connector, group and
/// the fields that are not secrets (`form_values`; none for a connector
/// that is not in `connectors`).
pub fn source_infos(sources: &[SourceConfig], connectors: &[Description]) -> Vec<common::SourceInfoMsg> {
    sources
        .iter()
        .map(|s| common::SourceInfoMsg {
            id: s.id.clone(),
            name: s.name.clone(),
            connector: s.target.connector.clone(),
            group: group_of(s).map(str::to_string),
            online: None,
            values: connectors.iter().find(|c| c.id == s.target.connector).map(|d| form_values(s, d)),
        })
        .collect()
}

/// Where the Lunaris app keeps the computers of other types it added for
/// itself: beside the agent's config, in a file of its own. The agent that
/// shares this computer never reads it, so they are not offered to the
/// people who view this computer.
pub fn bridge_config_path() -> std::path::PathBuf {
    std::path::Path::new(crate::pairing::config_path()).with_file_name("bridge_config.json")
}

/// The name as an id: lower case letters, digits and `-`.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 24 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() { "source".to_string() } else { slug.to_string() }
}

/// An id no other source has: the name's slug and a few random characters
/// (so that a source removed and added again under the same name is not
/// taken for the old one by a viewer that remembered it).
fn new_id(name: &str, existing: &[SourceConfig]) -> String {
    let slug = slug(name);
    loop {
        // The access password's generator: lower case letters and digits in groups.
        let suffix: String = crate::pairing::generate_direct_password()
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            .take(4)
            .collect();
        let id = format!("{}-{}", slug, suffix);
        if !existing.iter().any(|source| source.id == id) {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::{Capabilities, Field};

    fn connectors() -> Vec<Description> {
        let field = |key: &str, label: &str, kind: &str, default: &str, required: bool| Field {
            key: key.into(),
            label: label.into(),
            kind: kind.into(),
            default: default.into(),
            required,
            options: Vec::new(),
        };
        vec![Description {
            id: "demo".into(),
            name: "Demo".into(),
            fields: vec![
                field("host", "Address", "text", "", true),
                field("port", "Port", "port", "21118", false),
                field("password", "Password", "secret", "", true),
            ],
            capabilities: Capabilities::default(),
            note: "Demo must be running on that computer.".into(),
        }]
    }

    #[test]
    fn adds_lists_and_removes_without_showing_values() {
        let mut config = AgentConfig::default();
        let values = r#"{"host": " 192.168.1.20 ", "port": "", "password": " s3cret ", "id": "x", "other": "y"}"#;
        add(&mut config, &connectors(), "  Living room PC ", "demo", values).unwrap();
        add(&mut config, &connectors(), "Living room PC", "demo", values).unwrap();

        let first = &config.sources[0];
        assert_eq!(first.name, "Living room PC");
        assert_eq!(first.target.connector, "demo");
        assert_eq!(first.target.get("host"), "192.168.1.20");
        // The password as typed; an empty port and keys the connector does not have are left out.
        assert_eq!(first.target.get("password"), " s3cret ");
        assert_eq!(first.target.values.len(), 2);

        // Ids: of the name, in the characters allowed, and no two alike.
        for source in &config.sources {
            assert!(source.id.starts_with("living-room-pc-"), "{}", source.id);
            assert!(source.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        }
        assert_ne!(config.sources[0].id, config.sources[1].id);

        let listing = listing_json(&config.sources, &connectors());
        assert!(listing.contains("\"connector_name\":\"Demo\""));
        assert!(listing.contains("\"label\":\"Password\""));
        assert!(!listing.contains("s3cret") && !listing.contains("192.168.1.20"));

        // For a list and an edit form: what is not a secret, and no more.
        let views = views_json(&config.sources, &connectors());
        assert!(views.contains("\"values\":{\"host\":\"192.168.1.20\"}"), "{}", views);
        assert!(!views.contains("s3cret"));
        assert!(connectors_json(&connectors()).starts_with("[{\"id\":\"demo\""));
        // A connector's note goes with it to every place a type is chosen.
        for chosen_from in [connectors_json(&connectors()), listing] {
            assert!(chosen_from.contains("\"note\":\"Demo must be running on that computer.\""), "{}", chosen_from);
        }
        assert_eq!(source_infos(&config.sources, &[])[0].name, "Living room PC");
        // What the connector can do goes with it, so that a client offers only that.
        let mut with_files = connectors();
        with_files[0].capabilities.files = true;
        let told = connector_infos(&with_files)[0].capabilities.expect("capabilities");
        assert!(told.files && !told.clipboard && !told.setup);
        assert_eq!(connector_infos(&connectors())[0].capabilities.map(|c| c.files), Some(false));
        assert!(connectors_json(&with_files).contains("\"files\":true"));

        let id = config.sources[0].id.clone();
        assert!(remove(&mut config, &id));
        assert!(!remove(&mut config, &id));
        assert_eq!(config.sources.len(), 1);
    }

    /// A "choice" field goes out with what it is chosen from; a field
    /// without options is on the wire as it always was, and its value is
    /// text like any other (a client from before choices types it).
    #[test]
    fn a_choice_field_is_told_with_its_options() {
        let mut all = connectors();
        let plain = serde_json::to_string(&connector_infos(&all)[0].fields[0]).unwrap();
        assert_eq!(plain, r#"{"key":"host","label":"Address","kind":"text","default":"","required":true}"#);
        all[0].fields.push(Field {
            key: "pointer".into(),
            label: "Pointer".into(),
            kind: "choice".into(),
            default: "picture".into(),
            required: false,
            options: vec![
                common::FieldOptionMsg { value: "picture".into(), label: "In the picture".into() },
                common::FieldOptionMsg { value: "own".into(), label: "Drawn here".into() },
            ],
        });
        let told = serde_json::to_string(connector_infos(&all)[0].fields.last().unwrap()).unwrap();
        assert_eq!(told, r#"{"key":"pointer","label":"Pointer","kind":"choice","default":"picture","required":false,"options":[{"value":"picture","label":"In the picture"},{"value":"own","label":"Drawn here"}]}"#);
        for shown in [listing_json(&[], &all), connectors_json(&all)] {
            assert!(shown.contains(r#""options":[{"value":"picture""#), "{}", shown);
            assert_eq!(shown.matches("\"options\"").count(), 1, "only where there is something to choose from");
        }
        // A manifest names them the same way, and one from before has none.
        let read: Field = serde_json::from_str(r#"{"key":"mode","label":"Mode","kind":"choice","options":[{"value":"a","label":"A"}]}"#).unwrap();
        assert_eq!(read.options.len(), 1);
        let old: Field = serde_json::from_str(r#"{"key":"file","label":"File","kind":"text"}"#).unwrap();
        assert!(old.options.is_empty());
        let old: common::ConnectorFieldMsg = serde_json::from_str(r#"{"key":"file","label":"File","kind":"text"}"#).unwrap();
        assert!(old.options.is_empty());
        // The value is kept as text, whatever the kind: what it may be is the connector's to say.
        let mut config = AgentConfig::default();
        add(&mut config, &all, "A", "demo", r#"{"host": "h", "password": "p", "pointer": "own"}"#).unwrap();
        add(&mut config, &all, "B", "demo", r#"{"host": "h2", "password": "p", "pointer": "viewer"}"#).unwrap();
        assert_eq!((config.sources[0].target.get("pointer"), config.sources[1].target.get("pointer")), ("own", "viewer"));
    }

    #[test]
    fn a_servers_changes_are_made_in_the_config_file() {
        use common::SourceChange;
        let dir = std::env::temp_dir().join(format!("lunaris-sources-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent_config.json").to_string_lossy().into_owned();
        crate::pairing::save_config(&AgentConfig::default(), &path).unwrap();
        let all = connectors();
        let values = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();

        let add = SourceChange::Add { name: "Lab".into(), connector: "demo".into(), values: values(&[("host", "10.1.1.1"), ("password", "pw-one")]), group: None };
        // What is printed of a change has the names of its fields, not what is in them.
        let printed = format!("{:?}", add);
        assert!(printed.contains("host") && !printed.contains("10.1.1.1") && !printed.contains("pw-one"), "{}", printed);
        let id = apply_at(&path, &all, &add).unwrap();
        let saved = crate::pairing::load_config(&path).unwrap();
        assert_eq!(saved.sources.len(), 1);
        assert_eq!((saved.sources[0].id.as_str(), saved.sources[0].target.get("password")), (id.as_str(), "pw-one"));
        // The file is the owner's alone.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        let update = SourceChange::Update { id: id.clone(), name: "Lab 2".into(), values: values(&[("host", "10.1.1.2")]), group: None };
        assert_eq!(apply_at(&path, &all, &update).unwrap(), id);
        let saved = crate::pairing::load_config(&path).unwrap();
        assert_eq!((saved.sources[0].name.as_str(), saved.sources[0].target.get("host"), saved.sources[0].target.get("password")), ("Lab 2", "10.1.1.2", "pw-one"));

        // What is refused changes nothing.
        let bad = SourceChange::Add { name: "X".into(), connector: "nope".into(), values: values(&[]), group: None };
        assert_eq!(apply_at(&path, &all, &bad).unwrap_err(), "That connector is not installed");
        assert_eq!(crate::pairing::load_config(&path).unwrap().sources.len(), 1);

        assert_eq!(apply_at(&path, &all, &SourceChange::Remove { id: id.clone() }).unwrap(), id);
        assert!(crate::pairing::load_config(&path).unwrap().sources.is_empty());
        assert_eq!(apply_at(&path, &all, &SourceChange::Remove { id }).unwrap_err(), "That computer is not there any more");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn editing_keeps_what_is_left_empty() {
        let mut config = AgentConfig::default();
        let all = connectors();
        let id = add(&mut config, &all, "Office", "demo", r#"{"host": "10.0.0.5", "port": "5000", "password": "first"}"#).unwrap();
        assert_eq!(config.sources[0].id, id);
        // A new name and address; the password and the port are not typed again.
        update(&mut config, &all, &id, " Office 2 ", r#"{"host": "10.0.0.6", "password": ""}"#).unwrap();
        let source = &config.sources[0];
        assert_eq!((source.id.as_str(), source.name.as_str()), (id.as_str(), "Office 2"));
        assert_eq!((source.target.get("host"), source.target.get("port"), source.target.get("password")), ("10.0.0.6", "5000", "first"));
        // A new password replaces the old one; a wrong port is refused and nothing changes.
        update(&mut config, &all, &id, "Office 2", r#"{"password": "second"}"#).unwrap();
        assert_eq!(config.sources[0].target.get("password"), "second");
        assert_eq!(update(&mut config, &all, &id, "Office 2", r#"{"port": "0"}"#).unwrap_err(), "Port must be a port from 1 to 65535");
        assert_eq!(config.sources[0].target.get("port"), "5000");
        // A form that shows the stored values sends a cleared field empty:
        // an optional one is cleared, a required one or a secret is kept.
        update(&mut config, &all, &id, "Office 2", r#"{"host": "", "port": "", "password": ""}"#).unwrap();
        let source = &config.sources[0];
        assert_eq!((source.target.get("host"), source.target.get("port"), source.target.get("password")), ("10.0.0.6", "", "second"));
        assert!(!source.target.values.contains_key("port"));
        assert_eq!(update(&mut config, &all, &id, " ", "{}").unwrap_err(), "Give the computer a name");
        assert_eq!(update(&mut config, &all, "nope", "X", "{}").unwrap_err(), "That computer is not there any more");
    }

    #[test]
    fn refuses_what_is_missing_or_wrong() {
        let mut config = AgentConfig::default();
        let all = connectors();
        let error = |name: &str, connector: &str, values: &str| {
            add(&mut AgentConfig::default(), &all, name, connector, values).unwrap_err()
        };
        assert_eq!(error(" ", "demo", "{}"), "Give the computer a name");
        assert_eq!(error("A", "nope", "{}"), "That connector is not installed");
        assert_eq!(error("A", "demo", r#"{"host": "h"}"#), "Password is required");
        assert_eq!(error("A", "demo", r#"{"host": "  ", "password": "p"}"#), "Address is required");
        let bad_port = error("A", "demo", r#"{"host": "h", "password": "hunter2", "port": "70000"}"#);
        assert_eq!(bad_port, "Port must be a port from 1 to 65535");
        assert!(config.sources.is_empty());
        add(&mut config, &all, "Ünïcødé!!", "demo", r#"{"host": "h", "password": "p", "port": "5900"}"#).unwrap();
        assert!(config.sources[0].id.starts_with("n-c-d-"), "{}", config.sources[0].id);
        add(&mut config, &all, "日本", "demo", r#"{"host": "h", "password": "p"}"#).unwrap();
        assert!(config.sources[1].id.starts_with("source-"), "{}", config.sources[1].id);
    }

    // ---- Groups ----

    fn group(config: &AgentConfig, id: &str) -> Option<String> {
        config.sources.iter().find(|source| source.id == id).and_then(|source| source.group.clone())
    }

    /// Two connectors of other kinds whose address field is called "host"
    /// and "address".
    fn two_kinds() -> Vec<Description> {
        let mut all = connectors();
        let mut other = all[0].clone();
        other.id = "other".into();
        other.name = "Other".into();
        other.fields[0].key = "Address".into();
        all.push(other);
        all
    }

    #[test]
    fn sources_at_the_same_address_are_one_computer() {
        let mut config = AgentConfig::default();
        let all = two_kinds();
        let first = add(&mut config, &all, "Den PC", "demo", r#"{"host": "Den-PC.lan", "password": "p"}"#).unwrap();
        // The only one: no group, and nothing new on the wire or in the file.
        assert_eq!(group(&config, &first), None);
        assert_eq!(serde_json::to_string(&source_infos(&config.sources, &[])).unwrap(), format!(r#"[{{"id":"{}","name":"Den PC","connector":"demo"}}]"#, first));
        assert!(!serde_json::to_string(&config.sources).unwrap().contains("group"));

        // Another address: a computer of its own.
        let elsewhere = add(&mut config, &all, "Lab", "demo", r#"{"host": "10.0.0.9", "password": "p"}"#).unwrap();
        assert_eq!((group(&config, &first), group(&config, &elsewhere)), (None, None));

        // The same address through another connector (other letter case,
        // spaces around it, a field called "Address"): both get one group.
        let second = add(&mut config, &all, "Den PC (other)", "other", r#"{"Address": "  den-pc.LAN ", "password": "p"}"#).unwrap();
        let shared = group(&config, &first).expect("a group");
        assert_eq!(group(&config, &second).as_deref(), Some(shared.as_str()));
        assert!(common::valid_source_group(&shared) && shared.starts_with("g-") && !shared.starts_with(common::SOURCE_GROUP_HOST), "{}", shared);
        assert_eq!(group(&config, &elsewhere), None);
        // A third joins the group there is.
        let third = add(&mut config, &all, "Den again", "demo", r#"{"host": "den-pc.lan", "password": "p"}"#).unwrap();
        assert_eq!(group(&config, &third).as_deref(), Some(shared.as_str()));

        // The group is told with the id and the name, and it is a word of
        // its own: nothing of the address is in it or beside it.
        let told = serde_json::to_string(&source_infos(&config.sources, &[])).unwrap();
        assert!(told.contains(&format!(r#""group":"{}""#, shared)) && !told.to_lowercase().contains("den-pc.lan"), "{}", told);
        for listed in [listing_json(&config.sources, &all), views_json(&config.sources, &all)] {
            assert!(listed.contains(&format!(r#""group":"{}""#, shared)), "{}", listed);
        }
        assert!(!listing_json(&config.sources, &all).to_lowercase().contains("den-pc.lan"));

        // In the file it is a key beside the name; it is no field's value.
        let stored = serde_json::to_string(&config.sources).unwrap();
        let read: Vec<SourceConfig> = serde_json::from_str(&stored).unwrap();
        assert_eq!(read, config.sources);
        assert!(read.iter().all(|source| !source.target.values.contains_key("group")));
        // A file from before groups reads as it did.
        let old: SourceConfig = serde_json::from_str(r#"{"id":"a-1","name":"A","connector":"demo","host":"h"}"#).unwrap();
        assert_eq!((old.group, old.target.get("host")), (None, "h"));
    }

    #[test]
    fn a_group_can_be_named_changed_and_left() {
        let mut config = AgentConfig::default();
        let all = connectors();
        let host_group = "host:0b7c1f0e-3d7a-4c57-9a0e-6f0f4c1d2e3f";
        let values = r#"{"host": "10.0.0.5", "password": "p"}"#;
        // Named when it is added: that group, whatever the address.
        let named = add_in_group(&mut config, &all, "Office", "demo", values, Some(host_group)).unwrap();
        assert_eq!(group(&config, &named).as_deref(), Some(host_group));
        // Asked to have none: none, although one at its address has a group.
        let alone = add_in_group(&mut config, &all, "Office 2", "demo", values, Some("")).unwrap();
        assert_eq!(group(&config, &alone), None);
        // Nothing said: the group of the first one at its address.
        let auto = add(&mut config, &all, "Office 3", "demo", values).unwrap();
        assert_eq!(group(&config, &auto).as_deref(), Some(host_group));

        // What cannot be a group is refused and nothing is added or changed.
        for bad in ["<script>", "a b", "x'; DROP TABLE hosts; --", &"x".repeat(65)] {
            assert_eq!(add_in_group(&mut config, &all, "Bad", "demo", values, Some(bad)).unwrap_err(), GROUP_NOT_VALID);
            assert_eq!(update_in_group(&mut config, &all, &auto, "Office 3", "{}", Some(bad)).unwrap_err(), GROUP_NOT_VALID);
        }
        assert_eq!(config.sources.len(), 3);
        assert_eq!(config.sources[2].name, "Office 3");

        // An edit that says nothing of the group keeps it (an older client).
        update(&mut config, &all, &auto, "Office 3b", "{}").unwrap();
        assert_eq!(group(&config, &auto).as_deref(), Some(host_group));
        // Moved to another group; then out of it: a computer of its own.
        update_in_group(&mut config, &all, &auto, "Office 3b", "{}", Some("g-other")).unwrap();
        assert_eq!(group(&config, &auto).as_deref(), Some("g-other"));
        update_in_group(&mut config, &all, &auto, "Office 3b", "{}", Some("")).unwrap();
        assert_eq!(group(&config, &auto), None);
        assert_eq!(config.sources[2].target.get("password"), "p", "its fields stay");
        assert_eq!(group(&config, &named).as_deref(), Some(host_group), "the others stay where they are");

        // A group in the file that is not fit to be one is not told to anyone.
        config.sources[1].group = Some("<img src=x>".into());
        assert_eq!(source_infos(&config.sources, &[])[1].group, None);
        assert!(!views_json(&config.sources, &all).contains("img") && !listing_json(&config.sources, &all).contains("img"));
        // ... and a new one at its address gives both a real one.
        let fresh = add(&mut AgentConfig { sources: vec![config.sources[1].clone()], ..AgentConfig::default() }, &all, "New", "demo", values);
        assert!(fresh.is_ok());
    }

    #[test]
    fn a_server_names_the_group_and_an_older_one_says_nothing() {
        use common::SourceChange;
        let (dir, path) = config_file("groups");
        let all = connectors();
        let values = |host: &str| [("host".to_string(), host.to_string()), ("password".to_string(), "p".to_string())].into_iter().collect::<BTreeMap<_, _>>();
        // An older server's message has no "group": the agent groups by address.
        let old: SourceChange = serde_json::from_str(r#"{"op":"add","name":"A","connector":"demo","values":{"host":"10.2.2.2","password":"p"}}"#).unwrap();
        assert!(matches!(&old, SourceChange::Add { group: None, .. }));
        let a = apply_at(&path, &all, &old).unwrap();
        let b = apply_at(&path, &all, &old).unwrap();
        let sources = saved(&path);
        assert!(sources[0].group.is_some() && sources[0].group == sources[1].group);
        assert_eq!((sources[0].id.as_str(), sources[1].id.as_str()), (a.as_str(), b.as_str()));
        // And a change without a group is on the wire as it always was.
        assert_eq!(serde_json::to_string(&old).unwrap(), r#"{"op":"add","name":"A","connector":"demo","values":{"host":"10.2.2.2","password":"p"}}"#);

        // Named by the server.
        let add = SourceChange::Add { name: "C".into(), connector: "demo".into(), values: values("10.3.3.3"), group: Some("host:h1".into()) };
        assert!(serde_json::to_string(&add).unwrap().ends_with(r#""group":"host:h1"}"#));
        let c = apply_at(&path, &all, &add).unwrap();
        assert_eq!(saved(&path)[2].group.as_deref(), Some("host:h1"));
        // Taken out of its group by an edit; an edit from an older server keeps it.
        let keep: SourceChange = serde_json::from_str(&format!(r#"{{"op":"update","id":"{}","name":"C2","values":{{}}}}"#, c)).unwrap();
        apply_at(&path, &all, &keep).unwrap();
        assert_eq!(saved(&path)[2].group.as_deref(), Some("host:h1"));
        let leave = SourceChange::Update { id: c.clone(), name: "C2".into(), values: BTreeMap::new(), group: Some(String::new()) };
        apply_at(&path, &all, &leave).unwrap();
        assert_eq!(saved(&path)[2].group, None);
        // Refused: in the agent's words, and nothing changes.
        let bad = SourceChange::Update { id: c, name: "C2".into(), values: BTreeMap::new(), group: Some("no good".into()) };
        assert_eq!(apply_at(&path, &all, &bad).unwrap_err(), GROUP_NOT_VALID);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A source whose connector has a set-up gets its group when it is saved.
    #[test]
    fn a_set_up_keeps_the_group_it_was_asked_with() {
        let (dir, path) = config_file("setup-group");
        let pairing = Pairing::new(End::WhenLetGo(Ok(())));
        let AddOutcome::Setup { setup, .. } = begin_with(pairing.clone(), &path, "Den PC", FORM, Some("host:h1"), quick()).unwrap() else { panic!("no set-up") };
        pairing.let_go.notify_one();
        assert!(matches!(ended(&setup), Some(SetupState::Done { .. })));
        assert_eq!(saved(&path)[0].group.as_deref(), Some("host:h1"));
        // The next one says nothing: grouped by its address, when it is done.
        let outcome = begin_with(Pairing::new(End::AtOnce(Ok(()))), &path, "Den again", FORM, None, quick()).unwrap();
        assert!(matches!(outcome, AddOutcome::Added { .. }));
        assert_eq!(saved(&path)[1].group.as_deref(), Some("host:h1"));
        // A group that cannot be one is refused before any set-up.
        let never = Pairing::new(End::Never);
        assert_eq!(begin_with(never.clone(), &path, "X", FORM, Some("a b"), quick()).unwrap_err(), GROUP_NOT_VALID);
        assert!(!never.asked.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---- Set-up ----

    use crate::connector::{Session, Viewer};
    use std::sync::atomic::{AtomicBool, Ordering};

    const CERT: &str = "-----BEGIN CERTIFICATE----- c3VwZXI= -----END CERTIFICATE-----";
    const KEY: &str = "PRIVATE-KEY-b3BlbnNlc2FtZQ";
    const PIN: &str = "4711";

    /// How the pretend pairing ends.
    #[derive(Clone)]
    enum End {
        /// At once, without a step.
        AtOnce(Result<(), &'static str>),
        /// After its step, when the test lets it ("the person typed the PIN").
        WhenLetGo(Result<(), &'static str>),
        Never,
    }

    /// A connector with a set-up: a pairing that shows a PIN.
    struct Pairing {
        end: End,
        /// Quiet for this long before its first step.
        quiet: Duration,
        let_go: Arc<tokio::sync::Notify>,
        asked: Arc<AtomicBool>,
        /// Its work was dropped (ended or given up on).
        dropped: Arc<AtomicBool>,
    }

    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl Pairing {
        fn new(end: End) -> Arc<Self> {
            Arc::new(Pairing { end, quiet: Duration::ZERO, let_go: Default::default(), asked: Default::default(), dropped: Default::default() })
        }
    }

    #[async_trait::async_trait]
    impl Connector for Pairing {
        fn describe(&self) -> Description {
            let mut description = connectors().remove(0);
            description.id = "pairing".into();
            description.capabilities.setup = true;
            description
        }

        async fn connect(&self, _target: &Target, _viewer: &Viewer) -> anyhow::Result<Session> {
            anyhow::bail!("not in this test")
        }

        async fn setup(&self, target: &Target, steps: tokio::sync::mpsc::Sender<SetupStep>) -> anyhow::Result<BTreeMap<String, String>> {
            self.asked.store(true, Ordering::SeqCst);
            let _dropped = Dropped(self.dropped.clone());
            // The form's values are there, checked.
            assert_eq!(target.get("host"), "10.0.0.9");
            let result = match &self.end {
                End::AtOnce(result) => *result,
                End::WhenLetGo(result) => {
                    tokio::time::sleep(self.quiet).await;
                    let _ = steps.send(SetupStep { message: "Enter this PIN at the other computer.".into(), code: Some(PIN.into()) }).await;
                    self.let_go.notified().await;
                    *result
                }
                End::Never => {
                    let _ = steps.send(SetupStep { message: "Enter this PIN at the other computer.".into(), code: Some(PIN.into()) }).await;
                    std::future::pending::<()>().await;
                    Ok(())
                }
            };
            result.map_err(|text| anyhow::anyhow!(text))?;
            // "name" is not a value: the source's name is the person's.
            Ok([("client_cert", CERT), ("client_key", KEY), ("name", "x")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
        }
    }

    /// An agent's config file of its own, with no sources.
    fn config_file(name: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("lunaris-setup-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent_config.json").to_string_lossy().into_owned();
        crate::pairing::save_config(&AgentConfig::default(), &path).unwrap();
        (dir, path)
    }

    fn quick() -> Limits {
        Limits { first_step: Duration::from_secs(3), whole: Duration::from_secs(60), kept: Duration::from_secs(60) }
    }

    const FORM: &str = r#"{"host": "10.0.0.9", "password": "pw-typed", "client_key": "typed-by-someone"}"#;

    fn saved(path: &str) -> Vec<SourceConfig> {
        crate::pairing::load_config(path).unwrap().sources
    }

    /// Waits until the set-up is not pending any more.
    fn ended(setup: &str) -> Option<SetupState> {
        for _ in 0..500 {
            match setup_state(setup) {
                Some(SetupState::Pending { .. }) => std::thread::sleep(Duration::from_millis(10)),
                other => return other,
            }
        }
        panic!("the set-up did not end");
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("{}", what);
    }

    #[test]
    fn a_set_up_shows_its_step_and_then_the_source_is_saved_with_what_it_returned() {
        let (dir, path) = config_file("success");
        let pairing = Pairing::new(End::WhenLetGo(Ok(())));
        let outcome = begin_with(pairing.clone(), &path, " Den PC ", FORM, None, quick()).unwrap();
        let AddOutcome::Setup { setup, step } = outcome.clone() else { panic!("no set-up: {:?}", outcome) };
        assert_eq!(step, SetupStep { message: "Enter this PIN at the other computer.".into(), code: Some(PIN.into()) });
        // Nothing is saved while it waits; asking again says the same.
        assert!(saved(&path).is_empty());
        assert_eq!(setup_state(&setup), Some(SetupState::Pending { step }));
        // What is printed of it never has the PIN.
        let printed = format!("{:?} {:?}", outcome, setup_state(&setup));
        assert!(!printed.contains(PIN), "{}", printed);

        // The person typed the PIN at the other computer.
        pairing.let_go.notify_one();
        let Some(SetupState::Done { id }) = ended(&setup) else { panic!("not done") };
        assert!(pairing.dropped.load(Ordering::SeqCst));
        let sources = saved(&path);
        assert_eq!(sources.len(), 1);
        let source = &sources[0];
        assert_eq!((source.id.as_str(), source.name.as_str(), source.target.connector.as_str()), (id.as_str(), "Den PC", "pairing"));
        assert!(id.starts_with("den-pc-"));
        // The form's fields, and what the set-up returned (not what someone typed under its name).
        assert_eq!((source.target.get("host"), source.target.get("password")), ("10.0.0.9", "pw-typed"));
        assert_eq!((source.target.get("client_cert"), source.target.get("client_key")), (CERT, KEY));
        assert_eq!(source.name, "Den PC");
        assert_eq!(source.target.values.len(), 4);
        // Its end is remembered for whoever asks next.
        assert_eq!(setup_state(&setup), Some(SetupState::Done { id: id.clone() }));

        // Nobody is listed what the set-up returned, or the password: not the
        // settings, not an edit form, not a server, not a log.
        let all = vec![pairing.describe()];
        let shown = [
            listing_json(&sources, &all),
            views_json(&sources, &all),
            // The connector is not installed any more: still nothing.
            views_json(&sources, &[]),
            listing_json(&sources, &[]),
            serde_json::to_string(&source_infos(&sources, &[])).unwrap(),
            serde_json::to_string(&source_infos(&sources, &all)).unwrap(),
            serde_json::to_string(&connector_infos(&all)).unwrap(),
            format!("{:?}", sources),
            format!("{:?}", source.target),
        ];
        for text in &shown {
            for hidden in [CERT, KEY, "pw-typed", "c3VwZXI", "b3BlbnNlc2FtZQ"] {
                assert!(!text.contains(hidden), "{}", text);
            }
        }
        assert!(views_json(&sources, &all).contains(r#""values":{"host":"10.0.0.9"}"#));
        // The names of the hidden values are not listed either.
        assert!(!views_json(&sources, &all).contains("client_"));

        // Editing keeps them: through the settings and through a server.
        let mut config = crate::pairing::load_config(&path).unwrap();
        update(&mut config, &all, &id, "Den", r#"{"host": "10.0.0.10", "client_cert": "", "client_key": "overwritten"}"#).unwrap();
        let edited = &config.sources[0].target;
        assert_eq!((edited.get("host"), edited.get("password"), edited.get("client_cert"), edited.get("client_key")), ("10.0.0.10", "pw-typed", CERT, KEY));
        let change = common::SourceChange::Update { id: id.clone(), name: "Den 2".into(), values: [("host".to_string(), "10.0.0.11".to_string())].into(), group: None };
        apply_at(&path, &all, &change).unwrap();
        let edited = &saved(&path)[0];
        assert_eq!((edited.name.as_str(), edited.target.get("host"), edited.target.get("client_cert"), edited.target.get("client_key")), ("Den 2", "10.0.0.11", CERT, KEY));
        assert_eq!(edited.target.values.len(), 4);

        cancel_setup(&setup);
        assert_eq!(setup_state(&setup), None);
        assert_eq!(saved(&path).len(), 1, "forgetting a set-up that is done removes nothing");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_set_up_that_ends_at_once_is_an_ordinary_answer() {
        let (dir, path) = config_file("at-once");
        // Done without a step: the source is there, nothing to ask about.
        let outcome = begin_with(Pairing::new(End::AtOnce(Ok(()))), &path, "A", FORM, None, quick()).unwrap();
        let AddOutcome::Added { id } = outcome else { panic!("{:?}", outcome) };
        assert_eq!(saved(&path)[0].id, id);
        assert_eq!(saved(&path)[0].target.get("client_key"), KEY);
        // Failed without a step: its reason, and nothing saved.
        let error = begin_with(Pairing::new(End::AtOnce(Err("That computer cannot be reached"))), &path, "B", FORM, None, quick()).unwrap_err();
        assert_eq!(error, "That computer cannot be reached");
        assert_eq!(saved(&path).len(), 1);

        // A form that is wrong is refused before any set-up.
        let pairing = Pairing::new(End::Never);
        assert_eq!(begin_with(pairing.clone(), &path, "C", r#"{"host": "10.0.0.9"}"#, None, quick()).unwrap_err(), "Password is required");
        assert_eq!(begin_with(pairing.clone(), &path, " ", FORM, None, quick()).unwrap_err(), "Give the computer a name");
        assert_eq!(begin_with(pairing.clone(), "/nonexistent-lunaris/agent_config.json", "C", FORM, None, quick()).unwrap_err(), CONFIG_UNREADABLE);
        assert!(!pairing.asked.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_set_up_that_fails_saves_nothing_and_says_why() {
        let (dir, path) = config_file("failure");
        let pairing = Pairing::new(End::WhenLetGo(Err("Wrong PIN")));
        let AddOutcome::Setup { setup, .. } = begin_with(pairing.clone(), &path, "Den PC", FORM, None, quick()).unwrap() else { panic!("no set-up") };
        pairing.let_go.notify_one();
        assert_eq!(ended(&setup), Some(SetupState::Failed { error: "Wrong PIN".into() }));
        assert!(saved(&path).is_empty());
        // Said again to whoever asks again, until it is forgotten.
        assert_eq!(setup_state(&setup), Some(SetupState::Failed { error: "Wrong PIN".into() }));
        assert_eq!(setup_state("no-such-set-up"), None);
        cancel_setup("no-such-set-up");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_set_up_nobody_finishes_ends_by_itself_and_is_forgotten_later() {
        let (dir, path) = config_file("time-out");
        let pairing = Pairing::new(End::Never);
        let limits = Limits { first_step: Duration::from_secs(3), whole: Duration::from_millis(300), kept: Duration::from_millis(400) };
        let AddOutcome::Setup { setup, .. } = begin_with(pairing.clone(), &path, "Den PC", FORM, None, limits).unwrap() else { panic!("no set-up") };
        assert!(!pairing.dropped.load(Ordering::SeqCst));
        assert_eq!(ended(&setup), Some(SetupState::Failed { error: "Pairing was not completed in time".into() }));
        // The connector's work is dropped, not left waiting.
        assert!(pairing.dropped.load(Ordering::SeqCst));
        assert!(saved(&path).is_empty());
        until("its end is forgotten", || setup_state(&setup).is_none());
        // The real limits: 3 seconds for the first step, 5 minutes in all, remembered for 10.
        let real = Limits::default();
        assert_eq!((real.first_step, real.whole, real.kept), (Duration::from_secs(3), Duration::from_secs(300), Duration::from_secs(600)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cancelled_set_up_is_stopped_and_saves_nothing() {
        let (dir, path) = config_file("cancel");
        let pairing = Pairing::new(End::WhenLetGo(Ok(())));
        let AddOutcome::Setup { setup, .. } = begin_with(pairing.clone(), &path, "Den PC", FORM, None, quick()).unwrap() else { panic!("no set-up") };
        cancel_setup(&setup);
        assert_eq!(setup_state(&setup), None);
        until("the connector's work is dropped", || pairing.dropped.load(Ordering::SeqCst));
        // Even if the person types the PIN now: nothing is added.
        pairing.let_go.notify_one();
        std::thread::sleep(Duration::from_millis(100));
        assert!(saved(&path).is_empty());

        // The agent of this config file stops: its set-ups do, others do not.
        let (other_dir, other_path) = config_file("cancel-other");
        let (mine, other) = (Pairing::new(End::Never), Pairing::new(End::WhenLetGo(Ok(()))));
        let AddOutcome::Setup { setup: my_setup, .. } = begin_with(mine.clone(), &path, "Mine", FORM, None, quick()).unwrap() else { panic!("no set-up") };
        let AddOutcome::Setup { setup: other_setup, .. } = begin_with(other.clone(), &other_path, "Other", FORM, None, quick()).unwrap() else { panic!("no set-up") };
        drop(StopSetupsOnDrop(path.clone()));
        assert_eq!(setup_state(&my_setup), None);
        until("the connector's work is dropped", || mine.dropped.load(Ordering::SeqCst));
        assert!(matches!(setup_state(&other_setup), Some(SetupState::Pending { .. })));
        other.let_go.notify_one();
        assert!(matches!(ended(&other_setup), Some(SetupState::Done { .. })));
        let _ = std::fs::remove_dir_all(other_dir);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_set_up_that_is_slow_to_say_something_is_waited_for_a_moment_only() {
        let (dir, path) = config_file("slow");
        let pairing = Arc::new(Pairing { quiet: Duration::from_millis(400), ..Arc::try_unwrap(Pairing::new(End::WhenLetGo(Ok(())))).ok().unwrap() });
        let limits = Limits { first_step: Duration::from_millis(100), ..quick() };
        let started = Instant::now();
        let AddOutcome::Setup { setup, step } = begin_with(pairing.clone(), &path, "Den PC", FORM, None, limits).unwrap() else { panic!("no set-up") };
        assert!(started.elapsed() < Duration::from_millis(350));
        assert_eq!(step, SetupStep { message: "Getting ready...".into(), code: None });
        // Its step is there for whoever asks a moment later.
        until("the step", || matches!(setup_state(&setup), Some(SetupState::Pending { step }) if step.code.as_deref() == Some(PIN)));
        pairing.let_go.notify_one();
        assert!(matches!(ended(&setup), Some(SetupState::Done { .. })));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_server_is_answered_with_the_step_then_the_id() {
        use common::{AgentMessage, SourceChange};
        // Asking about a set-up that is not there, and cancelling one, through `answer_at`.
        let nowhere = std::path::Path::new("/nonexistent-lunaris-connectors");
        let gone = answer_at("/nonexistent-lunaris/agent_config.json", nowhere, &SourceChange::SetupState { setup: "nope".into() });
        assert_eq!((gone.id, gone.error.as_deref(), gone.setup), (None, Some(SETUP_GONE), None));
        let cancelled = answer_at("/nonexistent-lunaris/agent_config.json", nowhere, &SourceChange::SetupCancel { setup: "nope".into() });
        assert_eq!((cancelled.id.as_deref(), cancelled.error), (Some("nope"), None));
        // These are not changes of the config.
        assert_eq!(apply_at("/nonexistent-lunaris/agent_config.json", &[], &SourceChange::SetupState { setup: "nope".into() }).unwrap_err(), CONFIG_UNREADABLE);

        // A running one, as a server is told.
        let (dir, path) = config_file("server");
        let pairing = Pairing::new(End::WhenLetGo(Ok(())));
        let AddOutcome::Setup { setup, .. } = begin_with(pairing.clone(), &path, "Den PC", FORM, None, quick()).unwrap() else { panic!("no set-up") };
        let waiting = answer_at(&path, nowhere, &SourceChange::SetupState { setup: setup.clone() });
        assert_eq!((waiting.id, waiting.error), (None, None));
        let info = waiting.setup.expect("what to show");
        assert_eq!((info.setup.as_str(), info.code.as_deref()), (setup.as_str(), Some(PIN)));
        // The message as it goes out has the code, what is printed of it has not.
        let message = AgentMessage::SourceChanged { request_id: "r".into(), id: None, error: None, setup: Some(info) };
        assert!(serde_json::to_string(&message).unwrap().contains(r#""code":"4711""#));
        assert!(!format!("{:?}", message).contains(PIN));
        pairing.let_go.notify_one();
        let Some(SetupState::Done { id }) = ended(&setup) else { panic!("not done") };
        let done = answer_at(&path, nowhere, &SourceChange::SetupState { setup: setup.clone() });
        assert_eq!((done.id.as_deref(), done.error, done.setup), (Some(id.as_str()), None, None));

        // A computer that asks at its screen is not changed, whatever is asked.
        let refused = reply("r1".into(), &SourceChange::SetupState { setup }, Some("admin"), "the server", false);
        assert!(matches!(&refused[..], [AgentMessage::Sources { manageable: false, .. }, AgentMessage::SourceChanged { id: None, error: Some(error), setup: None, .. }] if error == NOT_MANAGEABLE));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A set-up that ends well tells the servers at once, also when nobody
    /// asks about it any more (the window was closed elsewhere).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_set_up_that_is_done_is_announced_without_being_asked_about() {
        let (dir, path) = config_file("announce");
        let (tx, mut to_server) = tokio::sync::mpsc::unbounded_channel::<common::AgentMessage>();
        let announcing = announce_changes(tx, true);
        tokio::time::sleep(Duration::from_millis(150)).await;
        // (Other tests' set-ups end too: only what comes after ours counts.)
        while to_server.try_recv().is_ok() {}
        let pairing = Pairing::new(End::WhenLetGo(Ok(())));
        let begun = {
            let (pairing, path) = (pairing.clone(), path.clone());
            tokio::task::spawn_blocking(move || begin_with(pairing, &path, "Den PC", FORM, None, quick())).await.unwrap().unwrap()
        };
        let AddOutcome::Setup { setup, .. } = begun else { panic!("no set-up") };
        // The person typed the PIN; nobody asks `setup_state`.
        pairing.let_go.notify_one();
        let message = tokio::time::timeout(Duration::from_secs(5), to_server.recv()).await.expect("an announcement in time");
        assert!(matches!(message, Some(common::AgentMessage::Sources { manageable: true, .. })), "{:?}", message);
        assert_eq!(saved(&path).len(), 1);
        assert!(matches!(setup_state(&setup), Some(SetupState::Done { .. })));

        // A set-up that fails announces nothing; the connection ending ends the announcing.
        while to_server.try_recv().is_ok() {}
        drop(announcing);
        let ended = tokio::time::timeout(Duration::from_secs(5), async { while to_server.recv().await.is_some() {} }).await;
        assert!(ended.is_ok(), "the task is gone with its connection");
        let _ = std::fs::remove_dir_all(dir);
    }
}
