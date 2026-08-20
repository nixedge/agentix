use anyhow::Context;

pub struct GoogleAuth {
    auth: yup_oauth2::authenticator::Authenticator<
        yup_oauth2::hyper_rustls::HttpsConnector<
            hyper_util::client::legacy::connect::HttpConnector,
        >,
    >,
}

impl GoogleAuth {
    pub async fn init(credentials_path: &str, token_cache_path: &str) -> anyhow::Result<Self> {
        let secret = yup_oauth2::read_application_secret(credentials_path)
            .await
            .with_context(|| format!("reading credentials from {credentials_path}"))?;

        let auth = yup_oauth2::InstalledFlowAuthenticator::builder(
            secret,
            yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
        )
        .persist_tokens_to_disk(token_cache_path)
        .build()
        .await
        .context("building authenticator")?;

        Ok(Self { auth })
    }

    pub async fn token(&self) -> anyhow::Result<String> {
        let scopes = &[
            "https://www.googleapis.com/auth/calendar.readonly",
            "https://www.googleapis.com/auth/gmail.readonly",
        ];
        let token = self
            .auth
            .token(scopes)
            .await
            .context("fetching OAuth token")?;
        token
            .token()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("token is empty"))
    }
}
