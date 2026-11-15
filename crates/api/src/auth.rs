//! Accounts: sign up, log in, who-am-I, log out.
//!
//! These routes are public (no site bearer token) and rate-limited per
//! email. Passwords are argon2id-hashed; a login returns a random session
//! token that the caller (the dashboard server) keeps in an HttpOnly cookie
//! and replays as `Authorization: Bearer <token>` on /me and /logout.

use crate::{ApiError, ApiResult, AppState};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::extract::State;
use axum::http::header;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shared::{Error, Result as SResult};

const MIN_PASSWORD: usize = 10;

fn password_hash(pw: &str) -> SResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Config(format!("password hashing failed: {e}")))
}

fn password_ok(pw: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// 128 bits of randomness, twice over: the token a signed-in holder keeps.
fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().as_simple(),
        uuid::Uuid::new_v4().as_simple()
    )
}

fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
}

fn rate_limited(st: &AppState, key: &str) -> ApiResult<()> {
    st.auth_limiter
        .allow(key.as_bytes())
        .map_err(|retry| {
            let _ = retry;
            ApiError::from(Error::TooManyRequests(
                "Too many tries for this account lately. Wait a minute and try again.".into(),
            ))
        })
}

#[derive(Debug, Deserialize)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

fn norm_email(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// Deliberately permissive: proves shape, not deliverability.
fn valid_email(e: &str) -> bool {
    let (local, domain) = match e.split_once('@') {
        Some(p) => p,
        None => return false,
    };
    !local.is_empty()
        && local.len() <= 64
        && domain.len() >= 3
        && domain.contains('.')
        && e.len() <= 254
        && !e.contains(' ')
}

/// "ada.lovelace@x.org" -> "Ada Lovelace" (used when the form has no name).
fn derived_name(email: &str) -> String {
    let local = email.split('@').next().unwrap_or("explorer");
    local
        .split(|c| c == '.' || c == '_' || c == '-')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut ch = p.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn signup(
    State(st): State<AppState>,
    Json(req): Json<SignupRequest>,
) -> ApiResult<Json<Value>> {
    let email = norm_email(&req.email);
    if !valid_email(&email) {
        return Err(Error::BadRequest("That does not look like an email address.".into()).into());
    }
    if req.password.len() < MIN_PASSWORD {
        return Err(Error::BadRequest(format!(
            "The password needs at least {MIN_PASSWORD} characters."
        ))
        .into());
    }
    rate_limited(&st, &format!("signup:{email}"))?;

    let name = req
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty() && n.chars().count() <= 60)
        .unwrap_or_else(|| derived_name(&email));
    if name.is_empty() {
        return Err(Error::BadRequest("A name is required.".into()).into());
    }

    let hash = password_hash(&req.password)?;
    let Some(user) = st.db.create_user(&email, &name, &hash).await? else {
        return Err(Error::Conflict(
            "That email already has an account. Log in instead.".into(),
        )
        .into());
    };

    let token = new_token();
    st.db.create_session(&token, user.id).await?;
    Ok(Json(json!({ "ok": true, "name": user.name, "email": user.email, "token": token })))
}

pub async fn login(
    State(st): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Json<Value>> {
    let email = norm_email(&req.email);
    rate_limited(&st, &format!("login:{email}"))?;

    let user = st.db.find_user_by_email(&email).await?;
    // Same reply and no early return for missing users: no account probing.
    let good = match &user {
        Some(u) => password_ok(&req.password, &u.pass_hash),
        None => {
            // Burn comparable time so timing cannot reveal account existence.
            let _ = password_hash(&req.password);
            false
        }
    };
    let Some(user) = user.filter(|_| good) else {
        return Err(Error::Unauthorized("Wrong email or password.".into()).into());
    };

    let token = new_token();
    st.db.create_session(&token, user.id).await?;
    Ok(Json(json!({ "ok": true, "name": user.name, "email": user.email, "token": token })))
}

#[derive(Debug, Serialize)]
pub struct MeOut {
    pub ok: bool,
    pub name: String,
    pub email: String,
}

pub async fn me(State(st): State<AppState>, headers: axum::http::HeaderMap) -> ApiResult<Json<MeOut>> {
    let Some(raw) = bearer(&headers) else {
        return Err(Error::Unauthorized("Not signed in.".into()).into());
    };
    match st.db.session_user(raw).await? {
        Some(u) => Ok(Json(MeOut { ok: true, name: u.name, email: u.email })),
        None => Err(Error::Unauthorized("Not signed in.".into()).into()),
    }
}

pub async fn logout(State(st): State<AppState>, headers: axum::http::HeaderMap) -> ApiResult<Json<Value>> {
    if let Some(raw) = bearer(&headers) {
        st.db.delete_session(raw).await?;
    }
    Ok(Json(json!({ "ok": true })))
}
