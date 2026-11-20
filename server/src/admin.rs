use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use common::{
    CreateGroupRequest, CreateUserRequest, GroupBrief, GroupInfo, HostBrief, UpdateGroupRequest,
    UpdateUserRequest, UserBrief, UserInfo,
};
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    auth::{hash_password_blocking, AdminUser, AuthenticatedUser},
    signaling::SignalingState,
};

// Helper: fetch UserInfo with groups for a given user id
async fn fetch_user_info(
    db: &sqlx::SqlitePool,
    user_id: &str,
) -> Result<Option<UserInfo>, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(String, String, String, i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, username, COALESCE(role, 'user'), is_active, created_at,
                EXISTS(SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled = 1)
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal Database Error" })),
        )
    })?;

    match row {
        Some((id, username, role, is_active, created_at, mfa)) => {
            let groups: Vec<(String, String)> = sqlx::query_as(
                "SELECT g.id, g.name FROM groups g INNER JOIN user_groups ug ON g.id = ug.group_id WHERE ug.user_id = ?",
            )
            .bind(&id)
            .fetch_all(db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Internal Database Error" })),
                )
            })?;

            Ok(Some(UserInfo {
                id,
                username,
                role,
                is_active: is_active != 0,
                groups: groups
                    .into_iter()
                    .map(|(id, name)| GroupBrief { id, name })
                    .collect(),
                created_at,
                mfa_enabled: mfa != 0,
            }))
        }
        None => Ok(None),
    }
}

// GET /api/admin/users
pub async fn list_users(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<(String, String, String, i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, username, COALESCE(role, 'user'), is_active, created_at,
                EXISTS(SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled = 1)
         FROM users",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal Database Error" })),
        )
    })?;

    let mut users = Vec::new();
    for (id, username, role, is_active, created_at, mfa) in rows {
        let groups: Vec<(String, String)> = sqlx::query_as(
            "SELECT g.id, g.name FROM groups g INNER JOIN user_groups ug ON g.id = ug.group_id WHERE ug.user_id = ?",
        )
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

        users.push(UserInfo {
            id,
            username,
            role,
            is_active: is_active != 0,
            groups: groups
                .into_iter()
                .map(|(id, name)| GroupBrief { id, name })
                .collect(),
            created_at,
            mfa_enabled: mfa != 0,
        });
    }

    Ok((StatusCode::OK, Json(users)))
}

// POST /api/admin/users
pub async fn create_user(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Json(payload): Json<CreateUserRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if payload.username.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Username cannot be empty" })),
        ));
    }
    if payload.password.len() < 6 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Password must be at least 6 characters" })),
        ));
    }
    let role = payload.role.unwrap_or_else(|| "user".to_string());
    if role != "admin" && role != "user" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Role must be 'admin' or 'user'" })),
        ));
    }

    // Check if user already exists
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(&payload.username)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

    if existing.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "Username already taken" })),
        ));
    }

    let hashed = hash_password_blocking(payload.password.clone()).await.map_err(|e| {
        error!("Hashing error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal Hashing Error" })),
        )
    })?;

    let user_id = Uuid::new_v4().to_string();

    sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active, created_at) VALUES (?, ?, ?, ?, 1, datetime('now'))")
        .bind(&user_id)
        .bind(&payload.username)
        .bind(&hashed)
        .bind(&role)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to create user" })),
            )
        })?;

    info!("Admin created user: {}", payload.username);

    let user_info = fetch_user_info(&state.db, &user_id).await?.unwrap();

    Ok((StatusCode::CREATED, Json(user_info)))
}

// PUT /api/admin/users/:id
pub async fn update_user(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(user_id): Path<String>,
    Json(payload): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // Check user exists
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

    if existing.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "User not found" })),
        ));
    }

    if let Some(ref role) = payload.role {
        if role != "admin" && role != "user" {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Role must be 'admin' or 'user'" })),
            ));
        }
    }
    if payload.role.is_some() || payload.is_active.is_some() {
        let losing_admin = payload.role.as_deref() == Some("user") || payload.is_active == Some(false);
        if losing_admin && user_id == _admin.user_id {
            return Err(own_account_refused());
        }
        set_role_and_active(&state.db, &user_id, payload.role.as_deref(), payload.is_active).await?;
        if payload.is_active == Some(false) {
            // A disabled account loses its open sessions now, not at next sign-in.
            state.close_user_connections(&user_id);
        }
    }

    if let Some(ref password) = payload.password {
        if password.len() < 6 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Password must be at least 6 characters" })),
            ));
        }
        let hashed = hash_password_blocking(password.clone()).await.map_err(|e| {
            error!("Hashing error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Hashing Error" })),
            )
        })?;
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(&hashed)
            .bind(&user_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Failed to update user" })),
                )
            })?;
        // A new password ends every session signed in with the old one.
        crate::auth::revoke_sessions(&state, &user_id).await.map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to update user" })),
            )
        })?;
        info!("Admin {} set a new password for user {}; their sessions were ended", _admin.username, user_id);
    }

    if let Some(ref group_ids) = payload.group_ids {
        // Sync user_groups: delete all, insert new
        sqlx::query("DELETE FROM user_groups WHERE user_id = ?")
            .bind(&user_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Failed to update user groups" })),
                )
            })?;

        for gid in group_ids {
            sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES (?, ?)")
                .bind(&user_id)
                .bind(gid)
                .execute(&state.db)
                .await
                .map_err(|e| {
                    error!("Database error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": "Failed to update user groups" })),
                    )
                })?;
        }
    }

    info!("Admin updated user: {}", user_id);

    // Fewer computers now (other groups, no longer an admin): what the user
    // may no longer use ends now.
    if payload.role.is_some() || payload.group_ids.is_some() {
        crate::signaling::end_sessions_without_access(&state).await;
    }

    let user_info = fetch_user_info(&state.db, &user_id).await?.unwrap();

    Ok((StatusCode::OK, Json(user_info)))
}

fn own_account_refused() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "You can't remove your own administrator rights, or disable or delete your own account. Another administrator can."
        })),
    )
}

fn no_admin_left() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": "This would leave no active administrator. Make another account an active administrator first."
        })),
    )
}

fn db_failed(e: sqlx::Error, what: &str) -> (StatusCode, Json<serde_json::Value>) {
    error!("Database error: {}", e);
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": what })))
}

/// Active administrators (role admin, account enabled).
async fn active_admins(conn: &mut sqlx::SqliteConnection) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin' AND is_active = 1")
        .fetch_one(conn)
        .await
}

/// Sets a user's role and/or active flag in one transaction that is undone
/// when it would leave no active administrator. The write comes first, so
/// concurrent changes are serialized and each sees the other's result.
async fn set_role_and_active(
    db: &sqlx::SqlitePool,
    user_id: &str,
    role: Option<&str>,
    is_active: Option<bool>,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let fail = |e| db_failed(e, "Failed to update user");
    let mut tx = db.begin().await.map_err(fail)?;
    sqlx::query("UPDATE users SET role = COALESCE(?, role), is_active = COALESCE(?, is_active) WHERE id = ?")
        .bind(role)
        .bind(is_active.map(i64::from))
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(fail)?;
    if active_admins(&mut tx).await.map_err(fail)? == 0 {
        let _ = tx.rollback().await;
        return Err(no_admin_left());
    }
    tx.commit().await.map_err(fail)
}

// DELETE /api/admin/users/:id
pub async fn delete_user(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if user_id == _admin.user_id {
        return Err(own_account_refused());
    }
    // In one transaction, undone when no active administrator would be left
    // (writes first: concurrent deletions are serialized).
    let fail = |e| db_failed(e, "Failed to delete user");
    let mut tx = state.db.begin().await.map_err(fail)?;
    sqlx::query("DELETE FROM user_groups WHERE user_id = ?")
        .bind(&user_id)
        .execute(&mut *tx)
        .await
        .map_err(fail)?;
    let deleted = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&user_id)
        .execute(&mut *tx)
        .await
        .map_err(fail)?;
    if deleted.rows_affected() == 0 {
        let _ = tx.rollback().await;
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "User not found" })),
        ));
    }
    if active_admins(&mut tx).await.map_err(fail)? == 0 {
        let _ = tx.rollback().await;
        return Err(no_admin_left());
    }
    tx.commit().await.map_err(fail)?;

    state.close_user_connections(&user_id);
    info!("Admin deleted user: {}", user_id);

    Ok(StatusCode::OK)
}

// POST /api/admin/hosts/:id/reset-key
/// Forgets a host's agent key (e.g. the agent lost its config): the next
/// agent registering with this id and a key stores a new one. The host keeps
/// its groups.
pub async fn reset_host_key(
    admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(host_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // A host joined with an install token joins again with one (install.rs).
    let done = sqlx::query("UPDATE hosts SET agent_key_hash = NULL, enrolled = 0 WHERE id = ?")
        .bind(&host_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;
    if done.rows_affected() == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Host not found" })),
        ));
    }
    // The agent connected now proved the old key: it goes too.
    crate::signaling::disconnect_agent(&state, &host_id).await;
    info!("Admin {} reset the agent key of host {}", admin.username, host_id);
    Ok(StatusCode::OK)
}

// GET /api/admin/groups
pub async fn list_groups(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, name, COALESCE(note, '') FROM groups")
            .fetch_all(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Internal Database Error" })),
                )
            })?;

    let mut groups = Vec::new();
    for (id, name, note) in rows {
        let user_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM user_groups WHERE group_id = ?")
                .bind(&id)
                .fetch_one(&state.db)
                .await
                .map_err(|e| {
                    error!("Database error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": "Internal Database Error" })),
                    )
                })?;

        let host_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM host_groups WHERE group_id = ?")
                .bind(&id)
                .fetch_one(&state.db)
                .await
                .map_err(|e| {
                    error!("Database error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": "Internal Database Error" })),
                    )
                })?;

        let db_err = |e: sqlx::Error| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        };
        let users: Vec<(String, String)> = sqlx::query_as(
            "SELECT u.id, u.username FROM users u INNER JOIN user_groups ug ON u.id = ug.user_id WHERE ug.group_id = ?",
        )
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?;
        let hosts: Vec<(String, String)> = sqlx::query_as(concat!("SELECT h.id, ", crate::db::host_name_sql!(), " FROM hosts h INNER JOIN host_groups hg ON h.id = hg.host_id WHERE hg.group_id = ?"))
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?;

        groups.push(GroupInfo {
            id,
            name,
            note,
            user_count: user_count.0,
            host_count: host_count.0,
            users: users
                .into_iter()
                .map(|(id, username)| UserBrief { id, username })
                .collect(),
            hosts: hosts
                .into_iter()
                .map(|(id, name)| HostBrief { id, name })
                .collect(),
        });
    }

    Ok((StatusCode::OK, Json(groups)))
}

// POST /api/admin/groups
pub async fn create_group(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Json(payload): Json<CreateGroupRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if payload.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Group name cannot be empty" })),
        ));
    }

    let group_id = Uuid::new_v4().to_string();
    let note = payload.note.unwrap_or_default();

    sqlx::query(
        "INSERT INTO groups (id, name, note, created_at) VALUES (?, ?, ?, datetime('now'))",
    )
    .bind(&group_id)
    .bind(&payload.name)
    .bind(&note)
    .execute(&state.db)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to create group" })),
        )
    })?;

    info!("Admin created group: {}", payload.name);

    Ok((
        StatusCode::CREATED,
        Json(GroupInfo {
            id: group_id,
            name: payload.name,
            note,
            user_count: 0,
            host_count: 0,
            users: Vec::new(),
            hosts: Vec::new(),
        }),
    ))
}

// PUT /api/admin/groups/:id
pub async fn update_group(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(group_id): Path<String>,
    Json(payload): Json<UpdateGroupRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // Check group exists
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM groups WHERE id = ?")
        .bind(&group_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

    if existing.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Group not found" })),
        ));
    }

    if let Some(ref name) = payload.name {
        if name.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Group name cannot be empty" })),
            ));
        }
        sqlx::query("UPDATE groups SET name = ? WHERE id = ?")
            .bind(name)
            .bind(&group_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Failed to update group" })),
                )
            })?;
    }

    if let Some(ref note) = payload.note {
        sqlx::query("UPDATE groups SET note = ? WHERE id = ?")
            .bind(note)
            .bind(&group_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Failed to update group" })),
                )
            })?;
    }

    if let Some(ref user_ids) = payload.user_ids {
        sqlx::query("DELETE FROM user_groups WHERE group_id = ?")
            .bind(&group_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Failed to update group users" })),
                )
            })?;

        for uid in user_ids {
            sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES (?, ?)")
                .bind(uid)
                .bind(&group_id)
                .execute(&state.db)
                .await
                .map_err(|e| {
                    error!("Database error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": "Failed to update group users" })),
                    )
                })?;
        }
    }

    if let Some(ref host_ids) = payload.host_ids {
        sqlx::query("DELETE FROM host_groups WHERE group_id = ?")
            .bind(&group_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Failed to update group hosts" })),
                )
            })?;

        for hid in host_ids {
            sqlx::query("INSERT INTO host_groups (host_id, group_id) VALUES (?, ?)")
                .bind(hid)
                .bind(&group_id)
                .execute(&state.db)
                .await
                .map_err(|e| {
                    error!("Database error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": "Failed to update group hosts" })),
                    )
                })?;
        }
    }

    info!("Admin updated group: {}", group_id);
    if payload.user_ids.is_some() || payload.host_ids.is_some() {
        // Members or computers left the group: what they lost ends now.
        crate::signaling::end_sessions_without_access(&state).await;
    }

    // Fetch updated group info
    let row: (String, String, String) =
        sqlx::query_as("SELECT id, name, COALESCE(note, '') FROM groups WHERE id = ?")
            .bind(&group_id)
            .fetch_one(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Internal Database Error" })),
                )
            })?;

    let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_groups WHERE group_id = ?")
        .bind(&group_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

    let host_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM host_groups WHERE group_id = ?")
        .bind(&group_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

    Ok((
        StatusCode::OK,
        Json(GroupInfo {
            id: row.0,
            name: row.1,
            note: row.2,
            user_count: user_count.0,
            host_count: host_count.0,
            users: Vec::new(),
            hosts: Vec::new(),
        }),
    ))
}

// DELETE /api/admin/groups/:id
pub async fn delete_group(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(group_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // Check group exists
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM groups WHERE id = ?")
        .bind(&group_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

    if existing.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Group not found" })),
        ));
    }

    // Delete junction table entries first
    sqlx::query("DELETE FROM user_groups WHERE group_id = ?")
        .bind(&group_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to delete group" })),
            )
        })?;

    sqlx::query("DELETE FROM host_groups WHERE group_id = ?")
        .bind(&group_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to delete group" })),
            )
        })?;

    sqlx::query("DELETE FROM groups WHERE id = ?")
        .bind(&group_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to delete group" })),
            )
        })?;

    info!("Admin deleted group: {}", group_id);
    crate::signaling::end_sessions_without_access(&state).await;

    Ok(StatusCode::OK)
}

// GET /api/auth/me
pub async fn get_current_user(
    user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let user_info = fetch_user_info(&state.db, &user.user_id).await?;

    match user_info {
        Some(info) => Ok((StatusCode::OK, Json(info))),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "User not found" })),
        )),
    }
}

// --- TURN SERVER API ---

// GET /api/admin/turn-servers
pub async fn list_turn_servers(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<(String, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, urls, username, credential, created_at FROM turn_servers ORDER BY created_at DESC"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal Database Error" })),
        )
    })?;

    let servers: Vec<common::TurnServer> = rows
        .into_iter()
        .map(
            |(id, urls, username, credential, created_at)| common::TurnServer {
                id,
                urls,
                username,
                credential,
                created_at,
            },
        )
        .collect();

    Ok((StatusCode::OK, Json(servers)))
}

// POST /api/admin/turn-servers
pub async fn create_turn_server(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Json(payload): Json<common::CreateTurnServerRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if payload.urls.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "URL cannot be empty" })),
        ));
    }

    let server_id = Uuid::new_v4().to_string();

    sqlx::query("INSERT INTO turn_servers (id, urls, username, credential) VALUES (?, ?, ?, ?)")
        .bind(&server_id)
        .bind(&payload.urls)
        .bind(&payload.username)
        .bind(&payload.credential)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to create TURN server" })),
            )
        })?;

    // Fetch the newly created record to get the default created_at value
    let created: (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT id, urls, username, credential, created_at FROM turn_servers WHERE id = ?",
    )
    .bind(&server_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to retrieve created TURN server" })),
        )
    })?;

    info!("Admin added TURN server: {}", payload.urls);

    Ok((
        StatusCode::CREATED,
        Json(common::TurnServer {
            id: created.0,
            urls: created.1,
            username: created.2,
            credential: created.3,
            created_at: created.4,
        }),
    ))
}

// DELETE /api/admin/turn-servers/:id
pub async fn delete_turn_server(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(server_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // Check exists
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM turn_servers WHERE id = ?")
        .bind(&server_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

    if existing.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "TURN server not found" })),
        ));
    }

    sqlx::query("DELETE FROM turn_servers WHERE id = ?")
        .bind(&server_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to delete TURN server" })),
            )
        })?;

    info!("Admin deleted TURN server: {}", server_id);

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod authz_tests {
    use super::*;
    use common::ServerToAgentMessage;

    async fn state() -> Arc<SignalingState> {
        let db = crate::security::tests::test_db().await;
        // test_db's own first-start admin is not part of these tests.
        sqlx::query("DELETE FROM users").execute(&db).await.unwrap();
        for (id, role, active) in [("a1", "admin", 1), ("a2", "admin", 1), ("a3", "admin", 0), ("u1", "user", 1)] {
            sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES (?, ?, 'x', ?, ?)")
                .bind(id)
                .bind(id)
                .bind(role)
                .bind(active)
                .execute(&db)
                .await
                .unwrap();
        }
        Arc::new(SignalingState::new(db, "t".into(), None))
    }

    fn as_admin(id: &str) -> AdminUser {
        AdminUser { user_id: id.into(), username: id.into() }
    }

    fn change(role: Option<&str>, is_active: Option<bool>) -> Json<UpdateUserRequest> {
        Json(UpdateUserRequest { role: role.map(String::from), is_active, password: None, group_ids: None })
    }

    async fn update(state: &Arc<SignalingState>, by: &str, id: &str, req: Json<UpdateUserRequest>) -> StatusCode {
        match update_user(as_admin(by), State(state.clone()), Path(id.into()), req).await {
            Ok(r) => r.into_response().status(),
            Err((code, _)) => code,
        }
    }

    async fn delete(state: &Arc<SignalingState>, by: &str, id: &str) -> StatusCode {
        match delete_user(as_admin(by), State(state.clone()), Path(id.into())).await {
            Ok(r) => r.into_response().status(),
            Err((code, _)) => code,
        }
    }

    async fn role_active(state: &SignalingState, id: &str) -> (String, i64) {
        sqlx::query_as("SELECT role, is_active FROM users WHERE id = ?").bind(id).fetch_one(&state.db).await.unwrap()
    }

    #[tokio::test]
    async fn admins_cannot_remove_themselves_or_the_last_active_admin() {
        let state = state().await;
        // Not on one's own account.
        assert_eq!(update(&state, "a1", "a1", change(Some("user"), None)).await, StatusCode::BAD_REQUEST);
        assert_eq!(update(&state, "a1", "a1", change(None, Some(false))).await, StatusCode::BAD_REQUEST);
        assert_eq!(delete(&state, "a1", "a1").await, StatusCode::BAD_REQUEST);
        assert_eq!(role_active(&state, "a1").await, ("admin".to_string(), 1));
        // Harmless own changes still work.
        assert_eq!(update(&state, "a1", "a1", change(Some("admin"), Some(true))).await, StatusCode::OK);

        // a1 demotes a2: a1 is the only active admin left (a3 is disabled).
        assert_eq!(update(&state, "a1", "a2", change(Some("user"), None)).await, StatusCode::OK);
        // The disabled admin doesn't count: removing a1 now (as another
        // admin acting at the same moment could) is refused, by update or delete.
        assert_eq!(update(&state, "a2", "a1", change(Some("user"), None)).await, StatusCode::CONFLICT);
        assert_eq!(update(&state, "a2", "a1", change(None, Some(false))).await, StatusCode::CONFLICT);
        assert_eq!(delete(&state, "a2", "a1").await, StatusCode::CONFLICT);
        assert_eq!(role_active(&state, "a1").await, ("admin".to_string(), 1));
        // Deleting the disabled admin or an ordinary user is fine.
        assert_eq!(delete(&state, "a1", "a3").await, StatusCode::OK);
        assert_eq!(delete(&state, "a1", "u1").await, StatusCode::OK);
        assert_eq!(delete(&state, "a1", "nobody").await, StatusCode::NOT_FOUND);
        // Re-enabling and promoting keep working.
        assert_eq!(update(&state, "a1", "a2", change(Some("admin"), None)).await, StatusCode::OK);
        assert_eq!(update(&state, "a1", "a2", change(None, Some(false))).await, StatusCode::OK);
        assert_eq!(update(&state, "a1", "a2", change(None, Some(true))).await, StatusCode::OK);
    }

    /// Group changes and demotions end what the user may no longer use.
    #[tokio::test]
    async fn access_changes_end_live_sessions() {
        let state = state().await;
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', 'H1', 'Busy')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO groups (id, name) VALUES ('g', 'G')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO host_groups VALUES ('h1', 'g')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO user_groups VALUES ('u1', 'g')").execute(&state.db).await.unwrap();
        let (agent_tx, mut agent_rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert("h1".into(), agent_tx);
        let mut conns = Vec::new();
        for (conn, user) in [("c-u1", "u1"), ("c-a2", "a2")] {
            let (tx, rx) = crate::queue::channel();
            state.clients.write().unwrap().insert(conn.into(), tx);
            state.client_connections.write().unwrap().insert(conn.into(), user.into());
            state.client_to_agent.write().unwrap().insert(conn.into(), "h1".into());
            conns.push(rx);
        }
        let ended = |rx: &mut crate::queue::Inbox<ServerToAgentMessage>| match rx.rx_try() {
            Some(ServerToAgentMessage::Signaling(common::SignalingMessage::EndSession { target_id })) => Some(target_id),
            _ => None,
        };

        // u1 leaves the group (a rename alone changes nothing).
        let rename = UpdateGroupRequest { name: Some("G2".into()), note: None, user_ids: None, host_ids: None };
        assert!(update_group(as_admin("a1"), State(state.clone()), Path("g".into()), Json(rename)).await.is_ok());
        assert!(agent_rx.rx_try().is_none());
        let members = UpdateGroupRequest { name: None, note: None, user_ids: Some(vec![]), host_ids: None };
        assert!(update_group(as_admin("a1"), State(state.clone()), Path("g".into()), Json(members)).await.is_ok());
        assert_eq!(ended(&mut agent_rx).as_deref(), Some("c-u1"));
        assert!(agent_rx.rx_try().is_none(), "the admin's session goes on");

        // a2 demoted (no groups): its session ends too.
        assert_eq!(update(&state, "a1", "a2", change(Some("user"), None)).await, StatusCode::OK);
        assert_eq!(ended(&mut agent_rx).as_deref(), Some("c-a2"));
        assert!(state.client_to_agent.read().unwrap().is_empty());
    }
}
