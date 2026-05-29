use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use common::{
    CreateGroupRequest, CreateUserRequest, GroupBrief, GroupInfo, UpdateGroupRequest,
    UpdateUserRequest, UserInfo,
};
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    auth::{hash_password, AdminUser, AuthenticatedUser},
    signaling::SignalingState,
};

// Helper: fetch UserInfo with groups for a given user id
async fn fetch_user_info(
    db: &sqlx::SqlitePool,
    user_id: &str,
) -> Result<Option<UserInfo>, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(String, String, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, username, COALESCE(role, 'user'), is_active, created_at FROM users WHERE id = ?",
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
        Some((id, username, role, is_active, created_at)) => {
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
    let rows: Vec<(String, String, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, username, COALESCE(role, 'user'), is_active, created_at FROM users",
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
    for (id, username, role, is_active, created_at) in rows {
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

    let hashed = hash_password(&payload.password).map_err(|e| {
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
        sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(role)
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
    }

    if let Some(is_active) = payload.is_active {
        let active_int: i64 = if is_active { 1 } else { 0 };
        sqlx::query("UPDATE users SET is_active = ? WHERE id = ?")
            .bind(active_int)
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
    }

    if let Some(ref password) = payload.password {
        if password.len() < 6 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Password must be at least 6 characters" })),
            ));
        }
        let hashed = hash_password(password).map_err(|e| {
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

    let user_info = fetch_user_info(&state.db, &user_id).await?.unwrap();

    Ok((StatusCode::OK, Json(user_info)))
}

// DELETE /api/admin/users/:id
pub async fn delete_user(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // Check if user exists and is admin
    let row: Option<(String,)> =
        sqlx::query_as("SELECT COALESCE(role, 'user') FROM users WHERE id = ?")
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

    match row {
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "User not found" })),
            ));
        }
        Some((role,)) if role == "admin" => {
            // Prevent deleting last admin
            let admin_count: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM users WHERE role = 'admin'")
                    .fetch_one(&state.db)
                    .await
                    .map_err(|e| {
                        error!("Database error: {}", e);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({ "error": "Internal Database Error" })),
                        )
                    })?;
            if admin_count.0 <= 1 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "Cannot delete the last admin user" })),
                ));
            }
        }
        _ => {}
    }

    // Delete user (cascading will handle user_groups)
    sqlx::query("DELETE FROM user_groups WHERE user_id = ?")
        .bind(&user_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to delete user" })),
            )
        })?;

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&user_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to delete user" })),
            )
        })?;

    info!("Admin deleted user: {}", user_id);

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

        groups.push(GroupInfo {
            id,
            name,
            note,
            user_count: user_count.0,
            host_count: host_count.0,
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
