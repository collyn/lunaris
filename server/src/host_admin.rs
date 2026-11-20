//! What administrators change about a Lunaris computer in the list: the
//! name it is shown with (`PUT /api/admin/hosts/{id}/name`), and for a
//! computer added by address its address, password and name
//! (`PUT /api/admin/hosts/{id}/address`, which restarts the connection).
//!
//! `{id}` is the host's id, or `address:<address>` for a computer added by
//! address that never answered yet (it has no id of its own then).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use tracing::{error, info};

use crate::{auth::AdminUser, links, signaling::SignalingState};

/// The longest name, in characters.
pub const MAX_NAME: usize = 100;

/// A name as typed: trimmed; `Ok(None)` when empty (the name its agent
/// announces is shown then). Err: why it can't be a name.
pub fn clean_name(text: &str) -> Result<Option<String>, &'static str> {
    let name = text.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > MAX_NAME {
        return Err("The name is too long (100 characters at most)");
    }
    if name.chars().any(char::is_control) {
        return Err("The name cannot contain control characters");
    }
    Ok(Some(name.to_string()))
}

fn bad(code: StatusCode, message: &str) -> axum::response::Response {
    (code, Json(serde_json::json!({ "error": message }))).into_response()
}

fn db_failed(e: sqlx::Error) -> axum::response::Response {
    error!("Database error: {}", e);
    bad(StatusCode::INTERNAL_SERVER_ERROR, "Internal Database Error")
}

#[derive(Deserialize)]
pub struct RenameRequest {
    #[serde(default)]
    pub name: String,
}

/// PUT /api/admin/hosts/{id}/name: `{"name": "..."}`; an empty name goes
/// back to the one the computer's agent announces.
pub async fn rename_host(
    admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(id): Path<String>,
    Json(req): Json<RenameRequest>,
) -> axum::response::Response {
    let name = match clean_name(&req.name) {
        Ok(name) => name,
        Err(why) => return bad(StatusCode::BAD_REQUEST, why),
    };
    // Not answered yet: only the name it was added with.
    if let Some(address) = id.strip_prefix("address:") {
        let done = sqlx::query("UPDATE address_hosts SET name = ? WHERE address = ?")
            .bind(name.as_deref().unwrap_or(""))
            .bind(address)
            .execute(&state.db)
            .await;
        return match done {
            Ok(r) if r.rows_affected() == 0 => bad(StatusCode::NOT_FOUND, "Host not found"),
            Ok(_) => Json(serde_json::json!({ "name": name.unwrap_or_else(|| address.to_string()) })).into_response(),
            Err(e) => db_failed(e),
        };
    }
    let done = sqlx::query("UPDATE hosts SET display_name = ? WHERE id = ?")
        .bind(name.as_deref())
        .bind(&id)
        .execute(&state.db)
        .await;
    match done {
        Ok(r) if r.rows_affected() == 0 => return bad(StatusCode::NOT_FOUND, "Host not found"),
        Ok(_) => {}
        Err(e) => return db_failed(e),
    }
    // The name it was added by address with is replaced too: an empty name
    // shows the agent's own.
    let _ = sqlx::query("UPDATE address_hosts SET name = '' WHERE host_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    let shown: Option<String> = sqlx::query_scalar(concat!("SELECT ", crate::db::host_name_sql!(), " FROM hosts h WHERE h.id = ?"))
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    info!("Admin {} renamed host {} ({})", admin.username, id, name.as_deref().unwrap_or("its own name"));
    Json(serde_json::json!({ "name": shown.unwrap_or_default() })).into_response()
}

#[derive(Deserialize)]
pub struct AddressHostChange {
    /// Where it is now ("host" or "host:port").
    pub address: String,
    /// None or empty: the saved password stays.
    #[serde(default)]
    pub password: Option<String>,
    /// None: the name stays; empty: the one its agent announces.
    #[serde(default)]
    pub name: Option<String>,
}

/// The address a host (or `address:<address>`) was added with.
async fn address_of(state: &SignalingState, id: &str) -> Result<Option<String>, sqlx::Error> {
    if let Some(address) = id.strip_prefix("address:") {
        let known: Option<String> = sqlx::query_scalar("SELECT address FROM address_hosts WHERE address = ?")
            .bind(address)
            .fetch_optional(&state.db)
            .await?;
        return Ok(known);
    }
    if let Some(address) = state.links.address_of(id) {
        return Ok(Some(address));
    }
    sqlx::query_scalar("SELECT address FROM address_hosts WHERE host_id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
}

/// PUT /api/admin/hosts/{id}/address: a computer added by address gets a new
/// address, password or name; its connection starts again with them. It
/// stays the same computer: should another one answer at the new address,
/// the connection is refused (links.rs) and the card says why.
pub async fn update_address_host(
    admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(id): Path<String>,
    Json(req): Json<AddressHostChange>,
) -> axum::response::Response {
    let Some(new_address) = links::normalize_address(&req.address) else {
        return bad(StatusCode::BAD_REQUEST, "Not a valid address");
    };
    let name = match req.name.as_deref().map(clean_name).transpose() {
        Ok(name) => name,
        Err(why) => return bad(StatusCode::BAD_REQUEST, why),
    };
    let old_address = match address_of(&state, &id).await {
        Ok(Some(a)) => a,
        Ok(None) => return bad(StatusCode::NOT_FOUND, "This computer was not added by address"),
        Err(e) => return db_failed(e),
    };
    let row: Option<(String, Option<String>, String)> =
        match sqlx::query_as("SELECT password, host_id, name FROM address_hosts WHERE address = ?")
            .bind(&old_address)
            .fetch_optional(&state.db)
            .await
        {
            Ok(row) => row,
            Err(e) => return db_failed(e),
        };
    let Some((old_password, host_id, old_name)) = row else {
        return bad(StatusCode::NOT_FOUND, "This computer was not added by address");
    };
    if new_address != old_address {
        let taken: Result<Option<String>, _> = sqlx::query_scalar("SELECT address FROM address_hosts WHERE address = ?")
            .bind(&new_address)
            .fetch_optional(&state.db)
            .await;
        match taken {
            Ok(Some(_)) => return bad(StatusCode::CONFLICT, "Another computer is already added with that address"),
            Ok(None) => {}
            Err(e) => return db_failed(e),
        }
    }
    let password = match req.password.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => old_password,
    };
    let stored_name = match &name {
        Some(n) => n.clone().unwrap_or_default(),
        None => old_name,
    };
    if let Err(e) = sqlx::query("UPDATE address_hosts SET address = ?, password = ?, name = ? WHERE address = ?")
        .bind(&new_address)
        .bind(&password)
        .bind(&stored_name)
        .bind(&old_address)
        .execute(&state.db)
        .await
    {
        return db_failed(e);
    }
    // A computer that has answered: the name is its shown name.
    if let (Some(name), Some(host_id)) = (&name, &host_id) {
        let _ = sqlx::query("UPDATE hosts SET display_name = ? WHERE id = ?")
            .bind(name.as_deref())
            .bind(host_id)
            .execute(&state.db)
            .await;
    }
    state.links.stop(&old_address);
    links::start(&state, new_address.clone(), password, host_id.clone());
    info!("Admin {} changed the computer added by address {} (now {})", admin.username, old_address, new_address);
    Json(serde_json::json!({
        "address": new_address,
        "id": host_id.unwrap_or_else(|| format!("address:{new_address}")),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn state() -> Arc<SignalingState> {
        let db = crate::security::tests::test_db().await;
        Arc::new(SignalingState::new(db, "t".into(), None))
    }

    fn admin() -> AdminUser {
        AdminUser { user_id: "u1".into(), username: "alice".into() }
    }

    async fn exec(state: &SignalingState, sql: &'static str) {
        sqlx::query(sql).execute(&state.db).await.unwrap();
    }

    #[test]
    fn names() {
        assert_eq!(clean_name("  Office PC "), Ok(Some("Office PC".into())));
        assert_eq!(clean_name("   "), Ok(None));
        assert!(clean_name("a\u{7}b").is_err());
        assert!(clean_name(&"x".repeat(MAX_NAME + 1)).is_err());
        assert_eq!(clean_name(&"é".repeat(MAX_NAME)).unwrap().unwrap().chars().count(), MAX_NAME);
    }

    #[tokio::test]
    async fn rename_and_back() {
        let state = state().await;
        exec(&state, "INSERT INTO hosts (id, name, status) VALUES ('h1', 'DESKTOP-1', 'Offline')").await;
        let r = rename_host(admin(), State(state.clone()), Path("h1".into()), Json(RenameRequest { name: " Office ".into() })).await;
        assert_eq!(r.status(), StatusCode::OK);
        let row: (String, Option<String>) = sqlx::query_as("SELECT name, display_name FROM hosts WHERE id = 'h1'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(row, ("DESKTOP-1".into(), Some("Office".into())));
        // The agent registering again keeps the admin's name.
        state.register_host_db("h1", "DESKTOP-1-renamed-by-os", None).await;
        let shown: String = sqlx::query_scalar(concat!("SELECT ", crate::db::host_name_sql!(), " FROM hosts h WHERE h.id = 'h1'"))
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(shown, "Office");
        // Empty: the agent's own name again.
        let r = rename_host(admin(), State(state.clone()), Path("h1".into()), Json(RenameRequest { name: String::new() })).await;
        assert_eq!(r.status(), StatusCode::OK);
        let display: Option<String> = sqlx::query_scalar("SELECT display_name FROM hosts WHERE id = 'h1'").fetch_one(&state.db).await.unwrap();
        assert!(display.is_none());
        let r = rename_host(admin(), State(state.clone()), Path("nope".into()), Json(RenameRequest { name: "x".into() })).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = rename_host(admin(), State(state.clone()), Path("h1".into()), Json(RenameRequest { name: "a\nb".into() })).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn display_name_in_groups_and_sources() {
        let state = state().await;
        exec(&state, "INSERT INTO hosts (id, name, status, display_name) VALUES ('h1', 'DESKTOP-1', 'Offline', 'Office')").await;
        exec(&state, "INSERT INTO host_sources (host_id, source_id, name, connector, connector_name) VALUES ('h1', 's1', 'Den', 'rdp', 'RDP')").await;
        let host_name: String = sqlx::query_scalar(concat!("SELECT ", crate::db::host_name_sql!(), " FROM host_sources s INNER JOIN hosts h ON h.id = s.host_id"))
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(host_name, "Office");
    }

    #[tokio::test]
    async fn edit_a_computer_added_by_address() {
        let state = state().await;
        exec(&state, "INSERT INTO address_hosts (address, password, host_id, name) VALUES ('192.0.2.10:8091', 'secret', NULL, 'Den')").await;
        exec(&state, "INSERT INTO address_hosts (address, password, host_id) VALUES ('192.0.2.11:8091', '', NULL)").await;
        // Another one's address is refused.
        let change = |address: &str, password: Option<&str>, name: Option<&str>| AddressHostChange {
            address: address.into(),
            password: password.map(Into::into),
            name: name.map(Into::into),
        };
        let r = update_address_host(admin(), State(state.clone()), Path("address:192.0.2.10:8091".into()), Json(change("192.0.2.11", None, None))).await;
        assert_eq!(r.status(), StatusCode::CONFLICT);
        let r = update_address_host(admin(), State(state.clone()), Path("address:192.0.2.10:8091".into()), Json(change("not an address", None, None))).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = update_address_host(admin(), State(state.clone()), Path("address:192.0.2.99:8091".into()), Json(change("192.0.2.12", None, None))).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        // New address, empty password keeps the saved one, new name.
        let r = update_address_host(admin(), State(state.clone()), Path("address:192.0.2.10:8091".into()), Json(change("192.0.2.12", Some(""), Some("Lab")))).await;
        assert_eq!(r.status(), StatusCode::OK);
        let row: (String, String) = sqlx::query_as("SELECT password, name FROM address_hosts WHERE address = '192.0.2.12:8091'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(row, ("secret".into(), "Lab".into()));
        let old: Option<String> = sqlx::query_scalar("SELECT address FROM address_hosts WHERE address = '192.0.2.10:8091'")
            .fetch_optional(&state.db)
            .await
            .unwrap();
        assert!(old.is_none());
        // The connection was started again at the new address.
        let links = state.links.list();
        assert!(links.iter().any(|l| l.address == "192.0.2.12:8091" && !l.needs_approval));
        assert!(!links.iter().any(|l| l.address == "192.0.2.10:8091"));
        // A new password; the name stays when none is sent.
        let r = update_address_host(admin(), State(state.clone()), Path("address:192.0.2.12:8091".into()), Json(change("192.0.2.12:8091", Some("other"), None))).await;
        assert_eq!(r.status(), StatusCode::OK);
        let row: (String, String) = sqlx::query_as("SELECT password, name FROM address_hosts WHERE address = '192.0.2.12:8091'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(row, ("other".into(), "Lab".into()));
        state.links.stop("192.0.2.12:8091");
    }

    #[tokio::test]
    async fn edit_names_a_known_computer() {
        let state = state().await;
        exec(&state, "INSERT INTO hosts (id, name, status) VALUES ('h7', 'DESKTOP-7', 'Offline')").await;
        exec(&state, "INSERT INTO address_hosts (address, password, host_id) VALUES ('192.0.2.20:8091', 'pw', 'h7')").await;
        let r = update_address_host(
            admin(),
            State(state.clone()),
            Path("h7".into()),
            Json(AddressHostChange { address: "192.0.2.20".into(), password: None, name: Some("Kitchen".into()) }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let display: Option<String> = sqlx::query_scalar("SELECT display_name FROM hosts WHERE id = 'h7'").fetch_one(&state.db).await.unwrap();
        assert_eq!(display.as_deref(), Some("Kitchen"));
        state.links.stop("192.0.2.20:8091");
    }
}
