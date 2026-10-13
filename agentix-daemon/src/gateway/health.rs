use super::AppState;
use axum::{extract::State, response::IntoResponse};

pub async fn handler(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = &state.config;
    let anthropic_auth = if cfg.anthropic_api_key.is_some() {
        "api_key"
    } else {
        "passthrough"
    };

    axum::Json(serde_json::json!({
        "status": "ok",
        "llama_socket": cfg.llama_socket,
        "qwen_socket": cfg.qwen_socket,
        "qwen_model": cfg.qwen_model,
        "whisper_socket": cfg.whisper_socket,
        "ollama_url": cfg.ollama_base_url,
        "anthropic_auth": anthropic_auth,
        "openai_proxy": cfg.openai_api_key.is_some(),
        "openrouter": cfg.openrouter_api_key.is_some(),
    }))
}
