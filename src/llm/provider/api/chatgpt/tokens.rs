//! Pure token/record logic: no IO, no clock — time enters as data.

use std::fmt::Write as _;

use anyhow::Context;
use anyhow::Result;
use base64::Engine as _;
use reqwest::Url;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use serde::Serialize;

use super::CHATGPT_AUTH_TYPE;
use super::CHATGPT_AUTH_VERSION;
use super::LOGIN_REFRESH_WINDOW_MS;
use super::OAUTH_CLIENT_ID;
use super::OAUTH_ISSUER;
use super::ORIGINATOR;
use super::POLL_SAFETY_MARGIN_SECS;
use super::USER_AGENT_VALUE;

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
        anyhow::ensure!(
            self.issuer == OAUTH_ISSUER,
            "unexpected issuer in ChatGPT auth record"
        );
        anyhow::ensure!(
            self.client_id == OAUTH_CLIENT_ID,
            "unexpected client id in ChatGPT auth record"
        );
        Ok(())
    }

    pub fn expired(
        &self,
        now: u64,
    ) -> bool {
        self.expires_at_unix_ms <= now
    }

    pub fn needs_refresh(
        &self,
        now: u64,
    ) -> bool {
        self.expires_at_unix_ms <= now + LOGIN_REFRESH_WINDOW_MS
    }

    /// records the outcome of a token refresh: refresh token falls back to the
    /// old one, metadata and identity carry over from `self`
    pub fn refreshed(
        self,
        response: TokenResponse,
        now: u64,
    ) -> AuthRecord {
        let refresh_token = response
            .refresh_token
            .unwrap_or_else(|| self.refresh_token.clone());
        build(
            &self.provider_id,
            response.access_token,
            refresh_token,
            response.id_token,
            response.expires_in,
            Some(&self),
            now,
        )
    }
}

impl AuthStatus {
    pub fn from_record(
        provider_id: &str,
        record: Option<&AuthRecord>,
        now: u64,
    ) -> Self {
        Self {
            provider_id: provider_id.to_string(),
            logged_in: record.is_some(),
            expired: record.map(|record| record.expired(now)),
            expires_at_unix_ms: record.map(|record| record.expires_at_unix_ms),
            account_id: record.and_then(|record| record.account_id.clone()),
        }
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

pub fn record_from_login(
    provider_id: &str,
    response: TokenResponse,
    now: u64,
) -> Result<AuthRecord> {
    let refresh_token = response
        .refresh_token
        .context("token exchange did not return a refresh token")?;
    Ok(build(
        provider_id,
        response.access_token,
        refresh_token,
        response.id_token,
        response.expires_in,
        None,
        now,
    ))
}

pub fn request_headers(record: &AuthRecord) -> Result<HeaderMap> {
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

pub fn poll_interval(raw: &str) -> Result<u64> {
    Ok(raw
        .parse::<u64>()
        .context("invalid device-code polling interval")?
        .max(1)
        + POLL_SAFETY_MARGIN_SECS)
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

fn build(
    provider_id: &str,
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    expires_in: Option<u64>,
    previous: Option<&AuthRecord>,
    now: u64,
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
        provider_id: provider_id.to_string(),
        issuer: OAUTH_ISSUER.into(),
        client_id: OAUTH_CLIENT_ID.into(),
        access_token,
        refresh_token,
        expires_at_unix_ms: now + expires_in.unwrap_or(3600) * 1000,
        account_id: metadata.account_id,
        plan_type: metadata.plan_type,
        email: metadata.email,
    }
}

pub fn join_url(
    base: &str,
    path: &str,
) -> Result<Url> {
    Ok(Url::parse(base)?.join(path)?)
}

#[cfg(test)]
impl AuthRecord {
    pub fn fake() -> Self {
        Self {
            version: CHATGPT_AUTH_VERSION,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    fn jwt(claims: serde_json::Value) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap());
        format!("a.{payload}.c")
    }

    #[test]
    fn auth_record_round_trips() {
        let record = AuthRecord::fake();
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
        let token = jwt(serde_json::json!({
            "https://api.openai.com/profile": {
                "email": "profile@example.com"
            },
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "org_123",
                "chatgpt_plan_type": "pro"
            }
        }));
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
        let token = jwt(serde_json::json!({
            "email": "primary@example.com",
            "organizations": [
                { "id": "org_123" }
            ]
        }));
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

    #[test]
    fn expiry_window_boundaries() {
        let t = 1_000_000_000;
        let record = AuthRecord {
            expires_at_unix_ms: t,
            ..AuthRecord::fake()
        };
        assert!(!record.expired(t - 1));
        assert!(record.expired(t));
        assert!(!record.needs_refresh(t - LOGIN_REFRESH_WINDOW_MS - 1));
        assert!(record.needs_refresh(t - LOGIN_REFRESH_WINDOW_MS));
    }

    #[test]
    fn builds_record_from_login() {
        let response = TokenResponse {
            access_token: "new-access".into(),
            refresh_token: Some("new-refresh".into()),
            id_token: Some(jwt(serde_json::json!({
                "email": "user@example.com",
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": "org_123",
                    "chatgpt_plan_type": "pro"
                }
            }))),
            expires_in: Some(60),
        };
        let record = record_from_login("chatgpt", response, 1_000).unwrap();
        insta::assert_yaml_snapshot!(record, @r#"
        version: 1
        type: chatgpt_oauth
        provider_id: chatgpt
        issuer: "https://auth.openai.com"
        client_id: app_EMoamEEZ73f0CkXaXp7hrann
        access_token: new-access
        refresh_token: new-refresh
        expires_at_unix_ms: 61000
        account_id: org_123
        plan_type: pro
        email: user@example.com
        "#);
    }

    #[test]
    fn login_without_refresh_token_errors() {
        let response = TokenResponse {
            access_token: "new-access".into(),
            refresh_token: None,
            id_token: None,
            expires_in: None,
        };
        assert!(record_from_login("chatgpt", response, 0).is_err());
    }

    #[test]
    fn refresh_keeps_old_refresh_token_and_metadata() {
        let response = TokenResponse {
            access_token: "new-access".into(),
            refresh_token: None,
            id_token: None,
            expires_in: Some(60),
        };
        let record = AuthRecord::fake().refreshed(response, 1_000);
        insta::assert_yaml_snapshot!(record, @r#"
        version: 1
        type: chatgpt_oauth
        provider_id: chatgpt
        issuer: "https://auth.openai.com"
        client_id: app_EMoamEEZ73f0CkXaXp7hrann
        access_token: new-access
        refresh_token: refresh
        expires_at_unix_ms: 61000
        account_id: org_123
        plan_type: pro
        email: user@example.com
        "#);
    }

    #[test]
    fn status_from_record() {
        let record = AuthRecord {
            expires_at_unix_ms: 1_000,
            ..AuthRecord::fake()
        };
        let statuses = [
            AuthStatus::from_record("chatgpt", None, 500),
            AuthStatus::from_record("chatgpt", Some(&record), 500),
            AuthStatus::from_record("chatgpt", Some(&record), 1_000),
        ];
        insta::assert_debug_snapshot!(statuses, @r#"
        [
            AuthStatus {
                provider_id: "chatgpt",
                logged_in: false,
                expired: None,
                expires_at_unix_ms: None,
                account_id: None,
            },
            AuthStatus {
                provider_id: "chatgpt",
                logged_in: true,
                expired: Some(
                    false,
                ),
                expires_at_unix_ms: Some(
                    1000,
                ),
                account_id: Some(
                    "org_123",
                ),
            },
            AuthStatus {
                provider_id: "chatgpt",
                logged_in: true,
                expired: Some(
                    true,
                ),
                expires_at_unix_ms: Some(
                    1000,
                ),
                account_id: Some(
                    "org_123",
                ),
            },
        ]
        "#);
    }

    #[test]
    fn builds_request_headers() {
        let with_account = request_headers(&AuthRecord::fake()).unwrap();
        let without_account = request_headers(&AuthRecord {
            account_id: None,
            ..AuthRecord::fake()
        })
        .unwrap();
        insta::assert_debug_snapshot!((with_account, without_account), @r#"
        (
            {
                "authorization": "Bearer access",
                "originator": "vicode",
                "user-agent": "vicode/0.0.0",
                "chatgpt-account-id": "org_123",
            },
            {
                "authorization": "Bearer access",
                "originator": "vicode",
                "user-agent": "vicode/0.0.0",
            },
        )
        "#);
    }

    #[test]
    fn validate_rejections() {
        assert!(AuthRecord::fake().validate("chatgpt").is_ok());
        assert!(
            AuthRecord {
                version: 2,
                ..AuthRecord::fake()
            }
            .validate("chatgpt")
            .is_err()
        );
        assert!(
            AuthRecord {
                kind: "other".into(),
                ..AuthRecord::fake()
            }
            .validate("chatgpt")
            .is_err()
        );
        assert!(AuthRecord::fake().validate("other").is_err());
        assert!(
            AuthRecord {
                issuer: "https://evil.example.com".into(),
                ..AuthRecord::fake()
            }
            .validate("chatgpt")
            .is_err()
        );
    }

    #[test]
    fn escapes_provider_id() {
        assert_eq!(
            escape_provider_id("my provider/1.0_x-y"),
            "my%20provider%2F1.0_x-y"
        );
    }

    #[test]
    fn parses_poll_interval() {
        assert_eq!(poll_interval("5").unwrap(), 6);
        assert_eq!(poll_interval("0").unwrap(), 2);
        assert!(poll_interval("garbage").is_err());
    }
}
