use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use base64::Engine as _;
use reqwest::StatusCode;
use reqwest::Url;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::Mutex;

use super::CHATGPT_AUTH_TYPE;
use super::CHATGPT_AUTH_VERSION;
use super::LOGIN_REFRESH_WINDOW_MS;
use super::OAUTH_CLIENT_ID;
use super::OAUTH_ISSUER;
use super::ORIGINATOR;
use super::POLL_SAFETY_MARGIN_SECS;
use super::USER_AGENT_VALUE;
use super::error::missing_login_error;
use super::error::relogin_error;
use crate::config::Config;
use crate::config::DIRS;
use crate::utils::now;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthRecord {
    pub version: usize,
    #[serde(rename = "type")]
    pub kind: String,
    pub provider_id: String,
    pub issuer: String,
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_unix_ms: u64,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub provider_id: String,
    pub logged_in: bool,
    pub expired: Option<bool>,
    pub expires_at_unix_ms: Option<u64>,
    pub account_id: Option<String>,
}

#[derive(Debug)]
pub struct ChatgptAuthManager {
    store: AuthStore,
    http: reqwest::Client,
    refresh: Mutex<()>,
    invalid: AtomicBool,
}

#[derive(Debug, Clone)]
pub struct AuthStore {
    path: PathBuf,
    provider_id: String,
    issuer: String,
    client_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenMetadata {
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodeExchangeForm {
    pub grant_type: String,
    pub code: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub code_verifier: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RefreshForm {
    pub grant_type: String,
    pub refresh_token: String,
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceCodeStart {
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceCodePoll {
    pub device_auth_id: String,
    pub user_code: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_auth_id: String,
    pub user_code: String,
    pub interval: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct DeviceCodeAuthorization {
    pub authorization_code: String,
    pub code_verifier: String,
}

impl AuthRecord {
    pub fn validate(
        &self,
        provider_id: &str,
    ) -> Result<()> {
        anyhow::ensure!(
            self.version == CHATGPT_AUTH_VERSION,
            "unsupported ChatGPT auth record version"
        );
        anyhow::ensure!(
            self.kind == CHATGPT_AUTH_TYPE,
            "unexpected ChatGPT auth record type"
        );
        anyhow::ensure!(
            self.provider_id == provider_id,
            "unexpected provider id in ChatGPT auth record"
        );
        anyhow::ensure!(!self.access_token.is_empty(), "missing access token");
        anyhow::ensure!(!self.refresh_token.is_empty(), "missing refresh token");
        anyhow::ensure!(!self.issuer.is_empty(), "missing issuer");
        anyhow::ensure!(!self.client_id.is_empty(), "missing client id");
        Ok(())
    }

    pub fn expired(&self) -> bool {
        self.expires_at_unix_ms <= now()
    }

    pub fn needs_refresh(&self) -> bool {
        self.expires_at_unix_ms <= now() + LOGIN_REFRESH_WINDOW_MS
    }
}

impl TokenMetadata {
    pub fn merged_with(
        self,
        record: Option<&AuthRecord>,
    ) -> Self {
        Self {
            account_id: self
                .account_id
                .or_else(|| record.and_then(|record| record.account_id.clone())),
            plan_type: self
                .plan_type
                .or_else(|| record.and_then(|record| record.plan_type.clone())),
            email: self
                .email
                .or_else(|| record.and_then(|record| record.email.clone())),
        }
    }
}

impl AuthStore {
    pub fn new(
        provider_id: &str,
        issuer: String,
        client_id: String,
    ) -> Result<Self> {
        let dir = DIRS.create_data_directory("auth")?;
        Ok(Self::new_in(dir, provider_id, issuer, client_id))
    }

    pub fn new_in(
        dir: PathBuf,
        provider_id: &str,
        issuer: String,
        client_id: String,
    ) -> Self {
        Self {
            path: dir.join(format!("{}.json", escape_provider_id(provider_id))),
            provider_id: provider_id.to_string(),
            issuer,
            client_id,
        }
    }

    pub fn load(&self) -> Result<Option<AuthRecord>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Ok(None),
        };
        let record: AuthRecord = serde_json::from_str(&text).with_context(|| {
            format!(
                "provider '{}' has a corrupt ChatGPT login; run: vc chatgpt logout {} && vc chatgpt login {}",
                self.provider_id, self.provider_id, self.provider_id
            )
        })?;
        record.validate(&self.provider_id).with_context(|| {
            format!(
                "provider '{}' has a corrupt ChatGPT login; run: vc chatgpt logout {} && vc chatgpt login {}",
                self.provider_id, self.provider_id, self.provider_id
            )
        })?;
        Ok(Some(record))
    }

    pub fn load_required(&self) -> Result<AuthRecord> {
        self.load()?
            .with_context(|| missing_login_error(&self.provider_id))
    }

    pub fn save(
        &self,
        record: &AuthRecord,
    ) -> Result<()> {
        std::fs::create_dir_all(self.path.parent().context("missing auth directory")?)?;
        let bytes = serde_json::to_vec_pretty(record)?;
        #[cfg(unix)]
        {
            use std::fs::OpenOptions;
            use std::os::unix::fs::OpenOptionsExt;

            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&self.path)?;
            file.write_all(&bytes)?;
        }
        #[cfg(not(unix))]
        std::fs::write(&self.path, &bytes)?;
        Ok(())
    }

    pub fn delete(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl ChatgptAuthManager {
    pub fn new(provider_id: &str) -> Result<Self> {
        Self::with_store(AuthStore::new(
            provider_id,
            OAUTH_ISSUER.into(),
            OAUTH_CLIENT_ID.into(),
        )?)
    }

    pub fn with_store(store: AuthStore) -> Result<Self> {
        Ok(Self {
            store,
            http: reqwest::Client::new(),
            refresh: Mutex::new(()),
            invalid: AtomicBool::new(false),
        })
    }

    pub fn status(&self) -> Result<AuthStatus> {
        let record = self.store.load()?;
        Ok(AuthStatus {
            provider_id: self.store.provider_id.clone(),
            logged_in: record.is_some(),
            expired: record.as_ref().map(AuthRecord::expired),
            expires_at_unix_ms: record.as_ref().map(|record| record.expires_at_unix_ms),
            account_id: record.and_then(|record| record.account_id),
        })
    }

    pub fn logout(&self) -> Result<()> {
        self.store.delete()
    }

    pub async fn login_headless(&self) -> Result<()> {
        let start = self.start_device_flow().await?;
        println!(
            "verification_url: {}",
            verification_url(&self.store.issuer)?
        );
        println!("user_code: {}", start.user_code);
        println!("timeout: 15m");
        let auth = self
            .poll_device_flow(&start, Duration::from_secs(15 * 60))
            .await?;
        let redirect_uri = join_url(&self.store.issuer, "deviceauth/callback")?.to_string();
        let record = self
            .exchange_code(
                auth.authorization_code,
                auth.code_verifier,
                redirect_uri,
                None,
            )
            .await?;
        self.store.save(&record)?;
        println!("logged in to provider '{}'", self.store.provider_id);
        Ok(())
    }

    pub async fn request_headers(
        &self,
        force_refresh: bool,
    ) -> Result<HeaderMap> {
        if self.invalid.load(Ordering::Relaxed) {
            anyhow::bail!(relogin_error(&self.store.provider_id));
        }
        let record = self.refresh_if_needed(force_refresh).await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", record.access_token))?,
        );
        headers.insert("originator", HeaderValue::from_static(ORIGINATOR));
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
        if let Some(account_id) = &record.account_id {
            headers.insert("ChatGPT-Account-ID", HeaderValue::from_str(account_id)?);
        }
        Ok(headers)
    }

    pub async fn refresh_if_needed(
        &self,
        force_refresh: bool,
    ) -> Result<AuthRecord> {
        let record = self.store.load_required()?;
        if !force_refresh && !record.needs_refresh() {
            return Ok(record);
        }
        let _guard = self.refresh.lock().await;
        let record = self.store.load_required()?;
        if !force_refresh && !record.needs_refresh() {
            return Ok(record);
        }
        self.refresh_record(record).await
    }

    pub(super) async fn start_device_flow(&self) -> Result<DeviceCodeResponse> {
        let url = join_url(&self.store.issuer, "api/accounts/deviceauth/usercode")?;
        Ok(self
            .http
            .post(url)
            .json(&DeviceCodeStart {
                client_id: self.store.client_id.clone(),
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub(super) async fn poll_device_flow(
        &self,
        start: &DeviceCodeResponse,
        timeout: Duration,
    ) -> Result<DeviceCodeAuthorization> {
        let url = join_url(&self.store.issuer, "api/accounts/deviceauth/token")?;
        let interval = start
            .interval
            .parse::<u64>()
            .context("invalid device-code polling interval")?
            .max(1)
            + POLL_SAFETY_MARGIN_SECS;
        let deadline = Instant::now() + timeout;
        loop {
            anyhow::ensure!(Instant::now() < deadline, "headless login timed out");
            let response = self
                .http
                .post(url.clone())
                .json(&DeviceCodePoll {
                    device_auth_id: start.device_auth_id.clone(),
                    user_code: start.user_code.clone(),
                })
                .send()
                .await?;
            match response.status() {
                StatusCode::OK => return Ok(response.json().await?),
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
                    tokio::time::sleep(Duration::from_secs(interval)).await;
                }
                status => {
                    anyhow::bail!("headless polling failed with status {status}");
                }
            }
        }
    }

    async fn exchange_code(
        &self,
        code: String,
        code_verifier: String,
        redirect_uri: String,
        previous: Option<&AuthRecord>,
    ) -> Result<AuthRecord> {
        let form = code_exchange_form(
            code,
            redirect_uri,
            self.store.client_id.clone(),
            code_verifier,
        );
        let response: TokenResponse = self
            .http
            .post(join_url(&self.store.issuer, "oauth/token")?)
            .form(&form)
            .send()
            .await?
            .error_for_status()
            .context("token exchange failed")?
            .json()
            .await?;
        let refresh_token = response
            .refresh_token
            .context("token exchange did not return a refresh token")?;
        Ok(build_record(
            &self.store,
            response.access_token,
            refresh_token,
            response.id_token,
            response.expires_in,
            previous,
        ))
    }

    async fn refresh_record(
        &self,
        record: AuthRecord,
    ) -> Result<AuthRecord> {
        let response = self
            .http
            .post(join_url(&self.store.issuer, "oauth/token")?)
            .form(&refresh_form(
                record.refresh_token.clone(),
                self.store.client_id.clone(),
            ))
            .send()
            .await
            .context("failed to refresh ChatGPT login")?;
        if response.status().is_client_error() {
            self.invalid.store(true, Ordering::Relaxed);
            anyhow::bail!(relogin_error(&self.store.provider_id));
        }
        let response: TokenResponse = response
            .error_for_status()
            .context("failed to refresh ChatGPT login")?
            .json()
            .await?;
        let record = build_record(
            &self.store,
            response.access_token,
            response
                .refresh_token
                .unwrap_or_else(|| record.refresh_token.clone()),
            response.id_token,
            response.expires_in,
            Some(&record),
        );
        self.store.save(&record)?;
        Ok(record)
    }
}

pub fn provider_auth(
    config: &Config,
    provider_id: &str,
) -> Result<()> {
    let provider = config
        .providers
        .get(provider_id)
        .with_context(|| format!("unknown provider '{provider_id}'"))?;
    anyhow::ensure!(
        provider.is_chatgpt(),
        "provider '{provider_id}' is not configured with auth = \"chatgpt\""
    );
    Ok(())
}

pub fn code_exchange_form(
    code: String,
    redirect_uri: String,
    client_id: String,
    code_verifier: String,
) -> CodeExchangeForm {
    CodeExchangeForm {
        grant_type: "authorization_code".into(),
        code,
        redirect_uri,
        client_id,
        code_verifier,
    }
}

pub fn refresh_form(
    refresh_token: String,
    client_id: String,
) -> RefreshForm {
    RefreshForm {
        grant_type: "refresh_token".into(),
        refresh_token,
        client_id,
    }
}

pub fn verification_url(issuer: &str) -> Result<Url> {
    join_url(issuer, "codex/device")
}

pub fn escape_provider_id(provider_id: &str) -> String {
    let mut escaped = String::new();
    for byte in provider_id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            escaped.push(char::from(byte));
        } else {
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    escaped
}

pub fn metadata_from_jwt(token: &str) -> Option<TokenMetadata> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let profile = claims.get("https://api.openai.com/profile");
    let auth = claims.get("https://api.openai.com/auth");
    Some(TokenMetadata {
        email: claims
            .get("email")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                profile
                    .and_then(|profile| profile.get("email"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            }),
        account_id: auth
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                claims
                    .get("chatgpt_account_id")
                    .and_then(serde_json::Value::as_str)
            })
            .or_else(|| {
                claims
                    .get("organizations")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|organizations| organizations.first())
                    .and_then(|organization| organization.get("id"))
                    .and_then(serde_json::Value::as_str)
            })
            .map(ToOwned::to_owned),
        plan_type: auth
            .and_then(|auth| auth.get("chatgpt_plan_type"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
    })
}

fn build_record(
    store: &AuthStore,
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    expires_in: Option<u64>,
    previous: Option<&AuthRecord>,
) -> AuthRecord {
    let metadata = id_token
        .as_deref()
        .and_then(metadata_from_jwt)
        .or_else(|| metadata_from_jwt(&access_token))
        .unwrap_or(TokenMetadata {
            account_id: None,
            plan_type: None,
            email: None,
        })
        .merged_with(previous);
    AuthRecord {
        version: CHATGPT_AUTH_VERSION,
        kind: CHATGPT_AUTH_TYPE.into(),
        provider_id: store.provider_id.clone(),
        issuer: store.issuer.clone(),
        client_id: store.client_id.clone(),
        access_token,
        refresh_token,
        expires_at_unix_ms: now() + expires_in.unwrap_or(3600) * 1000,
        account_id: metadata.account_id,
        plan_type: metadata.plan_type,
        email: metadata.email,
    }
}

pub(super) fn join_url(
    base: &str,
    path: &str,
) -> Result<Url> {
    Ok(Url::parse(base)?.join(path)?)
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vicode-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn auth_record_round_trips() {
        let record = AuthRecord {
            version: 1,
            kind: CHATGPT_AUTH_TYPE.into(),
            provider_id: "chatgpt".into(),
            issuer: OAUTH_ISSUER.into(),
            client_id: OAUTH_CLIENT_ID.into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at_unix_ms: 123,
            account_id: Some("org_123".into()),
            plan_type: Some("pro".into()),
            email: Some("user@example.com".into()),
        };
        let json = serde_json::to_value(&record).unwrap();
        insta::assert_json_snapshot!(json, @r#"
        {
          "access_token": "access",
          "account_id": "org_123",
          "client_id": "app_EMoamEEZ73f0CkXaXp7hrann",
          "email": "user@example.com",
          "expires_at_unix_ms": 123,
          "issuer": "https://auth.openai.com",
          "plan_type": "pro",
          "provider_id": "chatgpt",
          "refresh_token": "refresh",
          "type": "chatgpt_oauth",
          "version": 1
        }
        "#);
        let parsed: AuthRecord = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, record);
    }

    #[test]
    fn extracts_metadata_from_jwt() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "https://api.openai.com/profile": {
                    "email": "profile@example.com"
                },
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": "org_123",
                    "chatgpt_plan_type": "pro"
                }
            }))
            .unwrap(),
        );
        let token = format!("a.{payload}.c");
        let metadata = metadata_from_jwt(&token).unwrap();
        insta::assert_debug_snapshot!(metadata, @r#"
        TokenMetadata {
            account_id: Some(
                "org_123",
            ),
            plan_type: Some(
                "pro",
            ),
            email: Some(
                "profile@example.com",
            ),
        }
        "#);
    }

    #[test]
    fn falls_back_to_first_organization_for_account_id() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "email": "primary@example.com",
                "organizations": [
                    { "id": "org_123" }
                ]
            }))
            .unwrap(),
        );
        let token = format!("a.{payload}.c");
        let metadata = metadata_from_jwt(&token).unwrap();
        insta::assert_debug_snapshot!(metadata, @r#"
        TokenMetadata {
            account_id: Some(
                "org_123",
            ),
            plan_type: None,
            email: Some(
                "primary@example.com",
            ),
        }
        "#);
    }

    #[test]
    fn builds_code_exchange_form() {
        let form = code_exchange_form(
            "code".into(),
            "http://127.0.0.1/callback".into(),
            "client".into(),
            "verifier".into(),
        );
        insta::assert_debug_snapshot!(form, @r#"
        CodeExchangeForm {
            grant_type: "authorization_code",
            code: "code",
            redirect_uri: "http://127.0.0.1/callback",
            client_id: "client",
            code_verifier: "verifier",
        }
        "#);
    }

    #[test]
    fn builds_refresh_form() {
        let form = refresh_form("refresh".into(), "client".into());
        insta::assert_debug_snapshot!(form, @r#"
        RefreshForm {
            grant_type: "refresh_token",
            refresh_token: "refresh",
            client_id: "client",
        }
        "#);
    }

    #[tokio::test]
    async fn persists_status_without_tokens() {
        let store = AuthStore::new_in(
            temp_dir(),
            "chatgpt",
            OAUTH_ISSUER.into(),
            OAUTH_CLIENT_ID.into(),
        );
        store
            .save(&AuthRecord {
                version: 1,
                kind: CHATGPT_AUTH_TYPE.into(),
                provider_id: "chatgpt".into(),
                issuer: OAUTH_ISSUER.into(),
                client_id: OAUTH_CLIENT_ID.into(),
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                expires_at_unix_ms: now() + 1_000,
                account_id: Some("org_123".into()),
                plan_type: None,
                email: None,
            })
            .unwrap();
        let manager = ChatgptAuthManager::with_store(store).unwrap();
        let status = manager.status().unwrap();
        assert!(status.expires_at_unix_ms.is_some());
        insta::assert_debug_snapshot!(
            AuthStatus { expires_at_unix_ms: None, ..status },
            @r#"
        AuthStatus {
            provider_id: "chatgpt",
            logged_in: true,
            expired: Some(
                false,
            ),
            expires_at_unix_ms: None,
            account_id: Some(
                "org_123",
            ),
        }
        "#
        );
    }
}
