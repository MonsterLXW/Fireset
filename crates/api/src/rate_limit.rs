use crate::AppState;
use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

/// Fixed-window per-token rate limit for the (expensive) vision route.
/// Only the hashed bearer token is kept in memory — never the token itself.
#[derive(Clone)]
pub struct ScanLimiter {
    windows: std::sync::Arc<Mutex<HashMap<u64, (u64, u32)>>>, // hash -> (minute slot, count)
    limit: u32,
}

impl ScanLimiter {
    pub fn new(limit_per_minute: u32) -> Self {
        Self {
            windows: std::sync::Arc::new(Mutex::new(HashMap::new())),
            limit: limit_per_minute.max(1),
        }
    }

    /// Ok(()) when allowed; Err(retry_after_seconds) when over the limit.
    pub fn allow(&self, token: &[u8]) -> Result<(), u64> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut h);
        let key = h.finish();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let minute = now / 60;

        let mut g = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        if g.len() > 10_000 {
            g.retain(|_, (m, _)| *m == minute);
        }
        let entry = g.entry(key).or_insert((minute, 0));
        if entry.0 != minute {
            *entry = (minute, 1);
            Ok(())
        } else if entry.1 < self.limit {
            entry.1 += 1;
            Ok(())
        } else {
            Err(60 - (now % 60))
        }
    }
}

pub async fn scan_rate_limit(
    State(st): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().as_bytes().to_vec())
        .unwrap_or_default();

    match st.scan_limiter.allow(&token) {
        Ok(()) => next.run(req).await,
        Err(retry_after) => {
            let mut resp = (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(json!({
                    "error": format!(
                        "rate limit exceeded: at most {} scans per minute",
                        st.config.scan_rate_limit_per_minute
                    ),
                    "status": 429
                })),
            )
                .into_response();
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
            resp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ScanLimiter;

    #[test]
    fn blocks_after_limit() {
        let l = ScanLimiter::new(3);
        for _ in 0..3 {
            assert!(l.allow(b"tok-a").is_ok());
        }
        assert!(l.allow(b"tok-a").is_err());
        // Other tokens are unaffected.
        assert!(l.allow(b"tok-b").is_ok());
    }
}
