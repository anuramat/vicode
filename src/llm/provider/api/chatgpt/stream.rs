use anyhow::Result;
use async_openai::types::responses;
use futures::StreamExt;
use futures::stream;

use super::ChatgptAuthManager;
use super::error;

pub async fn run<F>(
    auth: &ChatgptAuthManager,
    api_base: &str,
    build_body: F,
) -> Result<responses::ResponseStream>
where
    F: Fn() -> Result<responses::CreateResponse>,
{
    match send_once(auth, api_base, build_body()?).await {
        Ok(result) => Ok(result),
        Err(err) if error::is_auth_error(&err) => {
            auth.refresh_if_needed(true).await?;
            send_once(auth, api_base, build_body()?)
                .await
                .map_err(error::map_backend_error)
        }
        Err(err) => Err(error::map_backend_error(err)),
    }
}

async fn send_once(
    auth: &ChatgptAuthManager,
    api_base: &str,
    body: responses::CreateResponse,
) -> std::result::Result<responses::ResponseStream, async_openai::error::OpenAIError> {
    let headers = auth
        .request_headers(false)
        .await
        .map_err(|err| async_openai::error::OpenAIError::InvalidArgument(err.to_string()))?;
    let url = format!("{api_base}/responses");
    let resp = reqwest::Client::new()
        .post(&url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(async_openai::error::OpenAIError::Reqwest)?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(async_openai::error::OpenAIError::ApiError(
                async_openai::error::ApiError {
                    message: text,
                    r#type: Some("invalid_request_error".into()),
                    param: None,
                    code: Some("token_expired".into()),
                },
            ));
        }
        return Err(async_openai::error::OpenAIError::ApiError(
            serde_json::from_str::<async_openai::error::WrappedError>(&text)
                .map(|w| w.error)
                .unwrap_or(async_openai::error::ApiError {
                    message: text,
                    r#type: None,
                    param: None,
                    code: None,
                }),
        ));
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        use eventsource_stream::Eventsource;
        let mut stream = resp
            .bytes_stream()
            .map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)))
            .eventsource();
        while let Some(ev) = stream.next().await {
            match ev {
                Ok(event) => {
                    if event.data == "[DONE]" {
                        break;
                    }
                    let item = serde_json::from_str::<responses::ResponseStreamEvent>(&event.data)
                        .map_err(|e| {
                            async_openai::error::OpenAIError::JSONDeserialize(e, event.data.clone())
                        });
                    if tx.send(item).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    drop(tx.send(Err(async_openai::error::OpenAIError::StreamError(
                        Box::new(async_openai::error::StreamError::EventStream(e.to_string())),
                    ))));
                    break;
                }
            }
        }
    });
    let mut inner: responses::ResponseStream =
        Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));
    match inner.next().await {
        Some(Err(err)) => Err(err),
        Some(Ok(ev)) => Ok(Box::pin(stream::iter(std::iter::once(Ok(ev))).chain(inner))),
        None => Ok(inner),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use base64::Engine as _;

    use super::super::CHATGPT_AUTH_TYPE;
    use super::super::OAUTH_CLIENT_ID;
    use super::super::auth::AuthRecord;
    use super::super::auth::AuthStore;
    use super::super::test_support;
    use super::super::test_support::RecordedRequest;
    use super::*;
    use crate::utils::now;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vicode-stream-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn token(account_id: &str) -> String {
        format!(
            "a.{}.c",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&serde_json::json!({
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": account_id
                    }
                }))
                .unwrap(),
            )
        )
    }

    fn body() -> responses::CreateResponse {
        responses::CreateResponseArgs::default()
            .model("gpt-test")
            .stream(true)
            .input(responses::InputParam::Items(Vec::new()))
            .build()
            .unwrap()
    }

    fn summarize(requests: &[RecordedRequest]) -> serde_json::Value {
        let keep = ["authorization", "chatgpt-account-id", "originator"];
        requests
            .iter()
            .map(|r| {
                serde_json::json!({
                    "path": r.path,
                    "headers": r.headers.iter()
                        .filter(|(k, _)| keep.contains(&k.as_str()))
                        .collect::<BTreeMap<_, _>>(),
                    "body": r.body,
                })
            })
            .collect()
    }

    #[tokio::test]
    async fn refreshes_before_request_and_sets_headers() {
        let server = test_support::mock_server(vec![
            test_support::MockResponse {
                status: 200,
                content_type: "application/json",
                body: serde_json::json!({
                    "access_token": "fresh_access",
                    "refresh_token": "fresh_refresh",
                    "id_token": token("org_123"),
                    "expires_in": 3600
                })
                .to_string(),
            },
            test_support::MockResponse {
                status: 200,
                content_type: "text/event-stream",
                body: String::new(),
            },
        ])
        .await;
        let store = AuthStore::new_in(
            temp_dir(),
            "chatgpt",
            server.base_url.clone(),
            OAUTH_CLIENT_ID.into(),
        );
        store
            .save(&AuthRecord {
                version: 1,
                kind: CHATGPT_AUTH_TYPE.into(),
                provider_id: "chatgpt".into(),
                issuer: server.base_url.clone(),
                client_id: OAUTH_CLIENT_ID.into(),
                access_token: "old_access".into(),
                refresh_token: "old_refresh".into(),
                expires_at_unix_ms: now() + 1_000,
                account_id: None,
                plan_type: None,
                email: None,
            })
            .unwrap();
        let auth = ChatgptAuthManager::with_store(store).unwrap();

        let _ = run(&auth, &server.base_url, || Ok(body())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        insta::assert_json_snapshot!(summarize(&server.requests.lock().unwrap()), @r#"
        [
          {
            "body": "grant_type=refresh_token&refresh_token=old_refresh&client_id=app_EMoamEEZ73f0CkXaXp7hrann",
            "headers": {},
            "path": "/oauth/token"
          },
          {
            "body": "{\"input\":[],\"model\":\"gpt-test\",\"stream\":true}",
            "headers": {
              "authorization": "Bearer fresh_access",
              "chatgpt-account-id": "org_123",
              "originator": "vicode"
            },
            "path": "/responses"
          }
        ]
        "#);
    }

    #[tokio::test]
    async fn retries_once_after_auth_failure() {
        let server = test_support::mock_server(vec![
            test_support::MockResponse {
                status: 401,
                content_type: "application/json",
                body: serde_json::json!({
                    "error": {
                        "message": "token expired",
                        "type": "invalid_request_error",
                        "param": null,
                        "code": "token_expired"
                    }
                })
                .to_string(),
            },
            test_support::MockResponse {
                status: 200,
                content_type: "application/json",
                body: serde_json::json!({
                    "access_token": "fresh_access",
                    "refresh_token": "fresh_refresh",
                    "expires_in": 3600
                })
                .to_string(),
            },
            test_support::MockResponse {
                status: 200,
                content_type: "text/event-stream",
                body: String::new(),
            },
        ])
        .await;
        let store = AuthStore::new_in(
            temp_dir(),
            "chatgpt",
            server.base_url.clone(),
            OAUTH_CLIENT_ID.into(),
        );
        store
            .save(&AuthRecord {
                version: 1,
                kind: CHATGPT_AUTH_TYPE.into(),
                provider_id: "chatgpt".into(),
                issuer: server.base_url.clone(),
                client_id: OAUTH_CLIENT_ID.into(),
                access_token: "stale_access".into(),
                refresh_token: "old_refresh".into(),
                expires_at_unix_ms: now() + 10 * 60_000,
                account_id: None,
                plan_type: None,
                email: None,
            })
            .unwrap();
        let auth = ChatgptAuthManager::with_store(store).unwrap();

        let _ = run(&auth, &server.base_url, || Ok(body())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        insta::assert_json_snapshot!(summarize(&server.requests.lock().unwrap()), @r#"
        [
          {
            "body": "{\"input\":[],\"model\":\"gpt-test\",\"stream\":true}",
            "headers": {
              "authorization": "Bearer stale_access",
              "originator": "vicode"
            },
            "path": "/responses"
          },
          {
            "body": "grant_type=refresh_token&refresh_token=old_refresh&client_id=app_EMoamEEZ73f0CkXaXp7hrann",
            "headers": {},
            "path": "/oauth/token"
          },
          {
            "body": "{\"input\":[],\"model\":\"gpt-test\",\"stream\":true}",
            "headers": {
              "authorization": "Bearer fresh_access",
              "originator": "vicode"
            },
            "path": "/responses"
          }
        ]
        "#);
    }

    #[tokio::test]
    async fn missing_login_errors_before_network() {
        let store = AuthStore::new_in(
            temp_dir(),
            "chatgpt",
            "http://127.0.0.1:1".into(),
            OAUTH_CLIENT_ID.into(),
        );
        let auth = ChatgptAuthManager::with_store(store).unwrap();
        let err = run(&auth, "http://127.0.0.1:1", || Ok(body())).await;
        let err = match err {
            Ok(_) => panic!("expected missing-login error"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("vc chatgpt login chatgpt"));
    }
}
