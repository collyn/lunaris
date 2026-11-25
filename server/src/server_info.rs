//! GET /api/server/info: what the web's About and server pages show.
//!
//! Everyone signed in: the server's version and the project's links. Only
//! administrators: whether a newer version exists (update_notice.rs), the
//! ports, the data folder and the server gateway (gateway.rs).

use axum::Json;

use crate::{auth::AuthenticatedUser, paths, tls, update_notice};

pub const WEBSITE: &str = "https://github.com/collyn/lunaris";
pub const ISSUES: &str = "https://github.com/collyn/lunaris/issues";
pub const RELEASES: &str = "https://github.com/collyn/lunaris/releases";

/// Where the database and keys are: LUNARIS_DATA_DIR, else the working directory.
fn data_dir_text() -> String {
    paths::data_dir()
        .or_else(|| std::env::current_dir().ok())
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn update_json(checks: bool, last: Option<update_notice::CheckResult>) -> serde_json::Value {
    let mut out = serde_json::json!({ "checks": checks });
    if let Some(last) = last {
        out["checked_at"] = last.checked_at.into();
        if let Some((version, url)) = last.latest {
            out["available"] = update_notice::is_newer(&version).into();
            out["latest"] = version.into();
            out["url"] = url.into();
        }
        if let Some(error) = last.error {
            out["error"] = error.into();
        }
    }
    out
}

pub fn info_json(admin: bool) -> serde_json::Value {
    let mut out = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "links": { "website": WEBSITE, "issues": ISSUES, "releases": RELEASES },
    });
    if admin {
        let http: u16 = std::env::var("SERVER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8090);
        out["update"] = update_json(update_notice::checks(), update_notice::last());
        out["ports"] = serde_json::json!({ "http": http, "https": tls::https_port() });
        out["data_dir"] = data_dir_text().into();
    }
    out
}

pub async fn info(
    user: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::signaling::SignalingState>>,
) -> Json<serde_json::Value> {
    let admin = user.role == "admin";
    if admin {
        update_notice::refresh_if_stale().await;
    }
    let mut out = info_json(admin);
    if admin {
        // The server gateway (gateway.rs): its switch and where it is.
        out["gateway"] = crate::gateway::status(&state);
    }
    Json(out)
}

/// POST /api/admin/server/update-check: asks GitHub now, answers what
/// GET /api/server/info answers (administrators only).
pub async fn check_update(
    user: AuthenticatedUser,
    state: axum::extract::State<std::sync::Arc<crate::signaling::SignalingState>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    if user.role != "admin" {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    update_notice::check_on_request().await;
    Ok(info(user, state).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_admins_see_the_server() {
        let user = info_json(false);
        assert_eq!(user["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(user["links"]["issues"], ISSUES);
        assert!(user.get("data_dir").is_none() && user.get("update").is_none() && user.get("ports").is_none());
        let admin = info_json(true);
        assert!(admin["data_dir"].is_string());
        assert!(admin["ports"]["http"].is_number());
        assert!(admin["update"]["checks"].is_boolean());
    }

    #[test]
    fn update_states() {
        assert_eq!(update_json(false, None), serde_json::json!({ "checks": false }));
        let newer = update_json(
            true,
            Some(update_notice::CheckResult {
                checked_at: 5,
                latest: Some(("999.0.0".into(), "https://example.invalid/r".into())),
                error: None,
            }),
        );
        assert_eq!(newer["available"], true);
        assert_eq!(newer["latest"], "999.0.0");
        assert_eq!(newer["checked_at"], 5);
        let same = update_json(
            true,
            Some(update_notice::CheckResult { checked_at: 6, latest: Some((env!("CARGO_PKG_VERSION").into(), String::new())), error: None }),
        );
        assert_eq!(same["available"], false);
        let failed = update_json(true, Some(update_notice::CheckResult { checked_at: 7, latest: None, error: Some("timed out".into()) }));
        assert_eq!(failed["error"], "timed out");
        assert!(failed.get("available").is_none());
    }
}
