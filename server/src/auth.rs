use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

static JWT_SECRET: Lazy<String> = Lazy::new(|| {
    std::env::var("JWT_SECRET").unwrap_or_else(|_| "lunaris-dev-secret-change-in-production".to_string())
});

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // User ID
    pub username: String,
    pub exp: u64,
}

pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

pub fn create_jwt(user_id: &str, username: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let expiration = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(7))
        .expect("valid timestamp")
        .timestamp() as u64;

    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        exp: expiration,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
}

pub fn verify_jwt(token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(JWT_SECRET.as_bytes()),
        &Validation::default(),
    )?;
    Ok(token_data.claims)
}

use std::sync::Arc;
use crate::signaling::SignalingState;

#[allow(dead_code)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub username: String,
}

#[async_trait]
impl FromRequestParts<Arc<SignalingState>> for AuthenticatedUser {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &Arc<SignalingState>) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|value| value.to_str().ok())
            .ok_or((StatusCode::UNAUTHORIZED, "Missing Authorization Header"))?;

        if !auth_header.starts_with("Bearer ") {
            return Err((StatusCode::UNAUTHORIZED, "Invalid Authorization Header Format"));
        }

        let token = &auth_header[7..];
        let claims = verify_jwt(token).map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid Token"))?;

        // Verify that the user exists in the database to prevent foreign key errors
        let user_exists: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
            .bind(&claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("Database verification error: {:?}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Database Error")
            })?;

        if user_exists.is_none() {
            return Err((StatusCode::UNAUTHORIZED, "User does not exist in database"));
        }

        Ok(AuthenticatedUser {
            user_id: claims.sub,
            username: claims.username,
        })
    }
}
