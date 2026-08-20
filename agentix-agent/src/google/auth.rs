pub struct GoogleAuth;

impl GoogleAuth {
    #[allow(dead_code)]
    pub async fn init(_credentials_path: &str, _token_cache_path: &str) -> anyhow::Result<Self> {
        anyhow::bail!("Google auth not yet implemented")
    }

    #[allow(dead_code)]
    pub async fn token(&self) -> anyhow::Result<String> {
        anyhow::bail!("Google auth not yet implemented")
    }
}
