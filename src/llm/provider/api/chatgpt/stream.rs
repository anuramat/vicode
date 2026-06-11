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
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    use super::super::CHATGPT_AUTH_TYPE;
    use super::super::CHATGPT_AUTH_VERSION;
    use super::super::OAUTH_CLIENT_ID;
    use super::super::OAUTH_ISSUER;
    use super::super::auth::AuthRecord;
    use super::super::auth::AuthStore;
    use super::*;
    use crate::utils::now;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vicode-stream-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn body() -> responses::CreateResponse {
        responses::CreateResponseArgs::default()
            .model("gpt-test")
            .stream(true)
            .input(responses::InputParam::Items(Vec::new()))
            .build()
            .unwrap()
    }

    fn logged_in_auth() -> ChatgptAuthManager {
        let store = AuthStore::new_in(
            temp_dir(),
            "chatgpt",
            OAUTH_ISSUER.into(),
            OAUTH_CLIENT_ID.into(),
        );
        store
            .save(&AuthRecord {
                version: CHATGPT_AUTH_VERSION,
                kind: CHATGPT_AUTH_TYPE.into(),
                provider_id: "chatgpt".into(),
                issuer: OAUTH_ISSUER.into(),
                client_id: OAUTH_CLIENT_ID.into(),
                access_token: "test-access-token".into(),
                refresh_token: "test-refresh-token".into(),
                expires_at_unix_ms: now() + 3_600_000,
                account_id: None,
                plan_type: None,
                email: None,
            })
            .unwrap();
        ChatgptAuthManager::with_store(store).unwrap()
    }

    /// one-shot HTTP server: drains the request, replies with the SSE body
    async fn serve_sse(body: String) -> String {
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
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn sse_stream_to_assistant_events_snapshot() {
        let fixture = concat!(
            r#"data: {"type":"response.created","response":{"id":"resp_1","object":"response","output":[],"status":"in_progress"}}"#,
            "\n\n",
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant","status":"in_progress","content":[]}}"#,
            "\n\n",
            r#"data: {"type":"response.output_text.delta","item_id":"msg_1","delta":"hel"}"#,
            "\n\n",
            r#"data: {"type":"response.output_text.delta","item_id":"msg_1","delta":"lo"}"#,
            "\n\n",
            r#"data: {"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","arguments":"{\"current\":\"testing\",\"entries\":[]}","call_id":"call_1","name":"todo"}}"#,
            "\n\n",
            r#"data: {"type":"response.completed","response":{"id":"resp_1","object":"response","output":[],"status":"completed"}}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let auth = logged_in_auth();
        let base = serve_sse(fixture.into()).await;

        let inner = run(&auth, &base, || Ok(body())).await.unwrap();
        let permit = std::sync::Arc::new(tokio::sync::Semaphore::new(1))
            .acquire_owned()
            .await
            .expect("semaphore closed");
        let events: Vec<_> = crate::llm::provider::api::responses::started_stream(permit, inner)
            .stream
            .map(|event| event.map_err(|err| err.to_string()))
            .collect()
            .await;

        insta::assert_yaml_snapshot!(events, {
            ".**.timestamp" => "[ts]",
            ".**.started_at" => "[ts]",
            ".**.ended_at" => "[ts]",
            ".**.ready_at" => "[ts]",
        }, @r#"
        - Ok:
            Item:
              Output:
                id: msg_1
                content: []
                token_count: 0
                started_at: "[ts]"
                ended_at: "[ts]"
        - Ok:
            Delta:
              id: msg_1
              delta:
                Output: hel
              timestamp: "[ts]"
        - Ok:
            Delta:
              id: msg_1
              delta:
                Output: lo
              timestamp: "[ts]"
        - Ok:
            Item:
              ToolCall:
                id: ~
                call_id: call_1
                name: todo
                arguments:
                  current: testing
                  entries: []
                meta: ~
                output: ~
                token_count: 0
                started_at: "[ts]"
                ended_at: "[ts]"
                ready_at: "[ts]"
        - Ok:
            Completed:
              ended_at: "[ts]"
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
