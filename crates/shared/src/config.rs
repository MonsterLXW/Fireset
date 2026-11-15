use std::env;
use std::time::Duration;

/// Application configuration, read from environment variables.
///
/// AI provider defaults are fixed to the self-hosted vLLM endpoint over
/// Tailscale (Anthropic Messages API protocol). We never fall back to a
/// cloud provider, localhost, or another inference server.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub database_url: String,
    pub bind_addr: String,

    pub ai_provider: String,
    pub vllm_base_url: String,
    pub vllm_api_key: String,
    pub vllm_model: String,
    pub vllm_timeout: Duration,
    pub vllm_max_retries: u32,
    pub ai_streaming: bool,

    /// Embeddings are generated against the same vLLM host at
    /// POST {base}/v1/embeddings. When the server does not serve that route
    /// yet, chunks stay `pending` — we never fake vectors.
    pub vllm_embed_model: String,
    pub vllm_embed_dim: usize,

    /// Vision model used by /ai/scan. Defaults to the same vLLM host/model
    /// when the configured VLLM_MODEL can take images; point VISION_BASE_URL
    /// + VISION_MODEL + VISION_API_KEY at a vision-capable server otherwise.
    /// Keys come from the environment only — never from the repo or logs.
    pub vision_base_url: String,
    pub vision_model: String,
    pub vision_api_key: String,

    /// Scan requests per authenticated token per minute (per-token rate limit).
    pub scan_rate_limit_per_minute: u32,

    pub pdf_store_dir: String,
    pub ntrs_base_url: String,
    pub max_concurrent_ingest: usize,

    pub api_auth_token: String,
    pub allowed_origins: Vec<String>,
}

fn var_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

impl AppConfig {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        let allowed_origins = var_or(
            "ALLOWED_ORIGINS",
            "http://localhost:5173,http://127.0.0.1:5173",
        )
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

        // Vision falls back to the primary (vLLM) provider when VISION_* is
        // not set — our vLLM model accepts image blocks. Set VISION_* to
        // route scans to a different vision-capable server.
        let vllm_base_url = var_or("VLLM_BASE_URL", "http://100.124.134.63:8890");
        let vllm_model = var_or("VLLM_MODEL", "qwen3.8-flash-next");
        let vllm_api_key = var_or("VLLM_API_KEY", "");
        let vision_base_url = {
            let v = var_or("VISION_BASE_URL", "").trim().to_string();
            if v.is_empty() { vllm_base_url.clone() } else { v }
        };
        let vision_model = {
            let v = var_or("VISION_MODEL", "").trim().to_string();
            if v.is_empty() { vllm_model.clone() } else { v }
        };
        let vision_api_key = {
            let v = var_or("VISION_API_KEY", "").trim().to_string();
            if v.is_empty() { vllm_api_key.clone() } else { v }
        };

        Self {
            database_url: var_or("DATABASE_URL", "postgres://fire:fire@localhost:5435/fire"),
            bind_addr: var_or("BIND_ADDR", "0.0.0.0:8080"),
            ai_provider: var_or("AI_PROVIDER", "vllm"),
            vllm_base_url,
            vllm_api_key,
            vllm_model,
            vllm_timeout: Duration::from_secs(
                var_or("VLLM_TIMEOUT_SECONDS", "180").parse().unwrap_or(180),
            ),
            vllm_max_retries: var_or("VLLM_MAX_RETRIES", "3").parse().unwrap_or(3),
            ai_streaming: var_or("AI_STREAMING", "true") == "true",
            vllm_embed_model: var_or("VLLM_EMBED_MODEL", "qwen3.8-flash-next"),
            vllm_embed_dim: var_or("VLLM_EMBED_DIM", "1024").parse().unwrap_or(1024),
            vision_base_url,
            vision_model,
            vision_api_key,
            scan_rate_limit_per_minute: var_or("SCAN_RATE_LIMIT_PER_MINUTE", "20")
                .parse()
                .unwrap_or(20),
            pdf_store_dir: var_or("PDF_STORE_DIR", "/data/pdfs"),
            ntrs_base_url: var_or("NTRS_BASE_URL", "https://ntrs.nasa.gov"),
            max_concurrent_ingest: var_or("MAX_CONCURRENT_INGEST", "2").parse().unwrap_or(2),
            api_auth_token: var_or("API_AUTH_TOKEN", ""),
            allowed_origins,
        }
    }

    /// Panic message used when someone configures a forbidden provider or insecure auth.
    pub fn assert_supported(&self) {
        assert_eq!(
            self.ai_provider, "vllm",
            "AI_PROVIDER must be 'vllm' — this system must not silently use cloud APIs, \
             localhost, or other inference servers"
        );
        assert!(
            self.api_auth_token.trim().len() >= 24,
            "API_AUTH_TOKEN must be set in environment and at least 24 characters long"
        );
    }
}
