//! IO shell around [`super::tokens`]: persisted login store + HTTP flows.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use tokio::sync::Mutex;

use super::OAUTH_CLIENT_ID;
use super::OAUTH_ISSUER;
use super::error::missing_login_error;
use super::error::relogin_error;
use super::tokens;
use super::tokens::AuthRecord;
use super::tokens::AuthStatus;
use super::tokens::DeviceCodeAuthorization;
use super::tokens::DeviceCodePoll;
use super::tokens::DeviceCodeResponse;
use super::tokens::DeviceCodeStart;
use super::tokens::TokenResponse;
use crate::config::DIRS;
use crate::utils::now;

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

impl AuthStore {
    pub fn new(
        provider_id: &str,
        issuer: String,
        client_id: String,
    ) -> Result<Self> {
        let dir = DIRS
            .get_data_home()
            .context("missing data home")?
            .join("auth");
        Ok(Self::new_in(dir, provider_id, issuer, client_id))
    }

    pub fn new_in(
        dir: PathBuf,
        provider_id: &str,
        issuer: String,
        client_id: String,
    ) -> Self {
        Self {
            path: dir.join(format!("{}.json", tokens::escape_provider_id(provider_id))),
            provider_id: provider_id.to_string(),
            issuer,
            client_id,
        }
    }

    pub fn load(&self) -> Result<Option<AuthRecord>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let corrupt = || {
            format!(
                "provider '{}' has a corrupt ChatGPT login; run: vc chatgpt logout {} && vc chatgpt login {}",
                self.provider_id, self.provider_id, self.provider_id
            )
        };
        let record: AuthRecord = serde_json::from_str(&text).with_context(corrupt)?;
        record.validate(&self.provider_id).with_context(corrupt)?;
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
        Ok(Self::with_store(AuthStore::new(
            provider_id,
            OAUTH_ISSUER.into(),
            OAUTH_CLIENT_ID.into(),
        )?))
    }

    pub fn with_store(store: AuthStore) -> Self {
        Self {
            store,
            http: reqwest::Client::new(),
            refresh: Mutex::new(()),
            invalid: AtomicBool::new(false),
        }
    }

    pub fn status(&self) -> Result<AuthStatus> {
        let record = self.store.load()?;
        Ok(AuthStatus::from_record(
            &self.store.provider_id,
            record.as_ref(),
            now(),
        ))
    }

    pub fn logout(&self) -> Result<()> {
        self.store.delete()
    }

    pub async fn login_headless(&self) -> Result<()> {
        let start = self.start_device_flow().await?;
        println!(
            "verification_url: {}",
            tokens::verification_url(&self.store.issuer)?
        );
        println!("user_code: {}", start.user_code);
        println!("timeout: 15m");
        let auth = self
            .poll_device_flow(&start, Duration::from_secs(15 * 60))
            .await?;
        let redirect_uri = tokens::join_url(&self.store.issuer, "deviceauth/callback")?.to_string();
        let record = self
            .exchange_code(auth.authorization_code, auth.code_verifier, redirect_uri)
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
        tokens::request_headers(&record)
    }

    pub async fn refresh_if_needed(
        &self,
        force_refresh: bool,
    ) -> Result<AuthRecord> {
        let record = self.store.load_required()?;
        if !force_refresh && !record.needs_refresh(now()) {
            return Ok(record);
        }
        let _guard = self.refresh.lock().await;
        let record = self.store.load_required()?;
        if !force_refresh && !record.needs_refresh(now()) {
            return Ok(record);
        }
        self.refresh_record(record).await
    }

    pub(super) async fn start_device_flow(&self) -> Result<DeviceCodeResponse> {
        let url = tokens::join_url(&self.store.issuer, "api/accounts/deviceauth/usercode")?;
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
        let url = tokens::join_url(&self.store.issuer, "api/accounts/deviceauth/token")?;
        let interval = tokens::poll_interval(&start.interval)?;
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
    ) -> Result<AuthRecord> {
        let form = tokens::code_exchange_form(
            code,
            redirect_uri,
            self.store.client_id.clone(),
            code_verifier,
        );
        let response: TokenResponse = self
            .http
            .post(tokens::join_url(&self.store.issuer, "oauth/token")?)
            .form(&form)
            .send()
            .await?
            .error_for_status()
            .context("token exchange failed")?
            .json()
            .await?;
        tokens::record_from_login(&self.store.provider_id, response, now())
    }

    async fn refresh_record(
        &self,
        record: AuthRecord,
    ) -> Result<AuthRecord> {
        let response = self
            .http
            .post(tokens::join_url(&self.store.issuer, "oauth/token")?)
            .form(&tokens::refresh_form(
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
        let record = record.refreshed(response, now());
        self.store.save(&record)?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vicode-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_with_issuer(issuer: String) -> AuthStore {
        AuthStore::new_in(temp_dir(), "chatgpt", issuer, OAUTH_CLIENT_ID.into())
    }

    /// one-shot HTTP server: drains the request, replies with the JSON body
    async fn serve_token(
        status_line: &'static str,
        body: &'static str,
    ) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = sock.read(&mut tmp).await.unwrap();
                buf.extend_from_slice(&tmp[..n]);
                if let Some(headers_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&buf[..headers_end]).to_lowercase();
                    let content_length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .map(|v| v.trim().parse().unwrap())
                        .unwrap_or(0);
                    if buf.len() >= headers_end + 4 + content_length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len(),
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn persists_status_without_tokens() {
        let store = store_with_issuer(OAUTH_ISSUER.into());
        store
            .save(&AuthRecord {
                expires_at_unix_ms: now() + 1_000,
                ..AuthRecord::fake()
            })
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&store.path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let manager = ChatgptAuthManager::with_store(store);
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

    #[test]
    fn corrupt_record_errors() {
        let store = store_with_issuer(OAUTH_ISSUER.into());
        std::fs::write(&store.path, "not json").unwrap();
        let err = store.load().unwrap_err();
        assert!(err.to_string().contains("corrupt ChatGPT login"));
    }

    #[tokio::test]
    async fn refresh_persists_new_record() {
        let base = serve_token(
            "200 OK",
            r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#,
        )
        .await;
        let store = store_with_issuer(base);
        store
            .save(&AuthRecord {
                expires_at_unix_ms: now(),
                ..AuthRecord::fake()
            })
            .unwrap();
        let manager = ChatgptAuthManager::with_store(store.clone());
        let record = manager.refresh_if_needed(false).await.unwrap();
        assert_eq!(record.access_token, "new-access");
        assert_eq!(record.refresh_token, "new-refresh");
        assert_eq!(store.load().unwrap(), Some(record));
    }

    #[tokio::test]
    async fn refresh_unauthorized_marks_invalid() {
        let base = serve_token("401 Unauthorized", "{}").await;
        let store = store_with_issuer(base);
        store
            .save(&AuthRecord {
                expires_at_unix_ms: now(),
                ..AuthRecord::fake()
            })
            .unwrap();
        let manager = ChatgptAuthManager::with_store(store);
        let err = manager.refresh_if_needed(false).await.unwrap_err();
        assert!(err.to_string().contains("vc chatgpt login chatgpt"));
        // the one-shot server is consumed: this can only succeed via the invalid flag
        let err = manager.request_headers(false).await.unwrap_err();
        assert!(err.to_string().contains("vc chatgpt login chatgpt"));
    }

    #[tokio::test]
    async fn concurrent_requests_refresh_once() {
        // the server accepts exactly one connection; the second request can
        // only succeed by re-loading the refreshed record under the mutex
        let base = serve_token(
            "200 OK",
            r#"{"access_token":"new-access","expires_in":3600}"#,
        )
        .await;
        let store = store_with_issuer(base);
        store
            .save(&AuthRecord {
                expires_at_unix_ms: now() + 1_000,
                ..AuthRecord::fake()
            })
            .unwrap();
        let manager = ChatgptAuthManager::with_store(store);
        let (a, b) = tokio::join!(
            manager.request_headers(false),
            manager.request_headers(false)
        );
        a.unwrap();
        b.unwrap();
    }
}
