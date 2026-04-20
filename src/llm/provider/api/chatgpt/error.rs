use reqwest::StatusCode;

pub fn missing_login_error(provider_id: &str) -> String {
    format!("provider '{provider_id}' requires ChatGPT login; run: vc chatgpt login {provider_id}")
}

pub fn relogin_error(provider_id: &str) -> String {
    format!("provider '{provider_id}' requires ChatGPT login; run: vc chatgpt login {provider_id}")
}

pub fn is_auth_error(err: &async_openai::error::OpenAIError) -> bool {
    match err {
        async_openai::error::OpenAIError::Reqwest(err) => err.status().is_some_and(|status| {
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN
        }),
        async_openai::error::OpenAIError::ApiError(err) => {
            let code = err.code.as_deref().unwrap_or_default().to_ascii_lowercase();
            let message = err.message.to_ascii_lowercase();
            code.contains("auth")
                || code.contains("token")
                || message.contains("auth")
                || message.contains("token")
                || message.contains("credential")
                || message.contains("unauthorized")
                || message.contains("forbidden")
                || message.contains("expired")
        }
        _ => false,
    }
}

pub fn map_backend_error(err: async_openai::error::OpenAIError) -> anyhow::Error {
    if let async_openai::error::OpenAIError::ApiError(api_error) = &err {
        if api_error.code.as_deref() == Some("usage_not_included") {
            return anyhow::anyhow!("the current ChatGPT plan does not include this usage");
        }
    }
    err.into()
}
