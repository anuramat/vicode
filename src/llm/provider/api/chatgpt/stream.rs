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
    use super::super::OAUTH_CLIENT_ID;
    use super::super::auth::AuthStore;
    use super::*;

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
