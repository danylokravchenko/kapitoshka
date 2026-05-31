use anyhow::Result;

/// Fetch the list of available model IDs from the configured inference provider.
pub async fn fetch_models(provider: &str) -> Result<Vec<String>> {
    match provider {
        "anthropic" => fetch_anthropic_models().await,
        _ => fetch_openai_models().await,
    }
}

async fn fetch_openai_models() -> Result<Vec<String>> {
    let base = std::env::var("OPENAI_BASE_URL")
        .or_else(|_| std::env::var("OPENAI_API_BASE"))
        .unwrap_or_else(|_| "http://localhost:8000/v1".to_string());
    let base = base.trim_end_matches('/');
    let url = format!("{base}/models");

    let client = reqwest::Client::new();
    let mut req = client.get(&url);

    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        req = req.bearer_auth(key);
    }

    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("models endpoint returned {}", resp.status());
    }

    let body: serde_json::Value = resp.json().await?;
    let models = body["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

async fn fetch_anthropic_models() -> Result<Vec<String>> {
    let url = "https://api.anthropic.com/v1/models";
    let key = std::env::var("ANTHROPIC_API_KEY")?;

    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header("x-api-key", &key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("anthropic models endpoint returned {}", resp.status());
    }

    let body: serde_json::Value = resp.json().await?;
    let models = body["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}
