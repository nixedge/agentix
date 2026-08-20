use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub gateway_host: String,
    pub gateway_port: u16,
    pub ollama_base_url: String,
    pub llama_socket: PathBuf,
    pub whisper_socket: PathBuf,

    #[allow(dead_code)] // reserved for future gateway auth
    pub agentix_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub openai_base_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            gateway_host: std::env::var("AGENTIX_GATEWAY_HOST").unwrap_or_else(|_| "[::]".into()),
            gateway_port: std::env::var("AGENTIX_GATEWAY_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(11434),
            ollama_base_url: std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
            llama_socket: std::env::var("AGENTIX_LLAMA_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/run/agentix/llama.sock")),
            whisper_socket: std::env::var("AGENTIX_WHISPER_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/run/agentix/whisper.sock")),
            agentix_api_key: std::env::var("AGENTIX_API_KEY").ok(),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            openrouter_api_key: std::env::var("OPENROUTER_API_KEY").ok(),
            anthropic_base_url: std::env::var("ANTHROPIC_BASE_URL_UPSTREAM").ok(),
            openai_base_url: std::env::var("OPENAI_BASE_URL_UPSTREAM").ok(),
        }
    }
}
