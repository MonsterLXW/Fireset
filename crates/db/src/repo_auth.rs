use crate::Db;
use shared::Result;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

/// SHA-256 hex of a session token — what we store and look up. The raw
/// token exists only in the caller's hands and in this process's memory.
pub fn token_hash(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    h.finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub pass_hash: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct SessionUser {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
}

/// How long a login stays valid without activity.
const SESSION_DAYS: i32 = 30;

impl Db {
    /// Insert an account; `None` when lower(email) is taken.
    pub async fn create_user(
        &self,
        email: &str,
        name: &str,
        pass_hash: &str,
    ) -> Result<Option<UserRow>> {
        let row = sqlx::query_as::<_, UserRow>(
            "INSERT INTO users (email, name, pass_hash)
             VALUES ($1, $2, $3)
             ON CONFLICT (lower(email)) DO NOTHING
             RETURNING id, email, name, pass_hash",
        )
        .bind(email)
        .bind(name)
        .bind(pass_hash)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn find_user_by_email(&self, email: &str) -> Result<Option<UserRow>> {
        Ok(sqlx::query_as::<_, UserRow>(
            "SELECT id, email, name, pass_hash FROM users WHERE lower(email) = lower($1)",
        )
        .bind(email)
        .fetch_optional(self.pool())
        .await?)
    }

    /// Open a session for the raw token; expired sessions are pruned alongside.
    pub async fn create_session(&self, raw_token: &str, user_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE expires_at < now()")
            .execute(self.pool())
            .await?;
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, expires_at)
             VALUES ($1, $2, now() + make_interval(days => $3))",
        )
        .bind(token_hash(raw_token))
        .bind(user_id)
        .bind(SESSION_DAYS)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn session_user(&self, raw_token: &str) -> Result<Option<SessionUser>> {
        Ok(sqlx::query_as::<_, SessionUser>(
            "SELECT s.user_id, u.email, u.name
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > now()",
        )
        .bind(token_hash(raw_token))
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_session(&self, raw_token: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash(raw_token))
            .execute(self.pool())
            .await?;
        Ok(())
    }
}
