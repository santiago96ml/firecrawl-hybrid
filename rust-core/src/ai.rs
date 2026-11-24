use crate::models::{ScrapeRequest, SearchRequest};
use crate::scraper::{scrape_url_base, search_web};
use crate::AppState;
use regex::Regex;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, warn};

/// Maximum markdown context sent to NVIDIA NIM (100,000 characters, matching the n8n 100k fix).
pub const MAX_AI_CONTEXT_CHARS: usize = 100_000;

/// Truncate a string safely at a UTF-8 character boundary up to `max_chars`.
pub fn truncate_utf8(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    input.chars().take(max_chars).collect()
}

/// Call NVIDIA NIM (`OPENAI_BASE_URL/chat/completions`) to extract structured JSON from Markdown.
pub async fn extract_from_markdown(
    state: &AppState,
    markdown: &str,
    prompt: Option<&str>,
    system_prompt: Option<&str>,
    schema: Option<&Value>,
) -> Value {
    let context = truncate_utf8(markdown, MAX_AI_CONTEXT_CHARS);
    let user_goal = prompt.unwrap_or("Extract the key structured information from the provided web page content as JSON.");
    let sys_msg = system_prompt.unwrap_or(
        "You are Firecrawl AI Extractor powered by NVIDIA NIM. Always respond with valid JSON only. Do not include markdown fences or explanations outside the JSON object.",
    );

    let schema_instruction = if let Some(s) = schema {
        format!(
            "\n\nYou MUST strictly follow this JSON Schema:\n{}",
            serde_json::to_string_pretty(s).unwrap_or_default()
        )
    } else {
        String::new()
    };

    if state.openai_api_key.trim().is_empty() {
        warn!("OPENAI_API_KEY is not configured; returning heuristic structured extraction");
        return heuristic_extraction(&context, user_goal, schema);
    }

    let endpoint = format!(
        "{}/chat/completions",
        state.openai_base_url.trim_end_matches('/')
    );

    let user_content = format!(
        "Task: {}{}\n\n--- BEGIN WEB PAGE CONTENT ({} chars) ---\n{}\n--- END WEB PAGE CONTENT ---\n\nReturn ONLY a valid JSON object.",
        user_goal,
        schema_instruction,
        context.len(),
        context
    );

    let payload = json!({
        "model": state.model_name,
        "messages": [
            {
                "role": "system",
                "content": sys_msg
            },
            {
                "role": "user",
                "content": user_content
            }
        ],
        "temperature": 0.1,
        "max_tokens": 4096
    });

    match state
        .http_client
        .post(&endpoint)
        .timeout(Duration::from_secs(90))
        .bearer_auth(&state.openai_api_key)
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<Value>().await {
                if let Some(raw_content) = body
                    .pointer("/choices/0/message/content")
                    .and_then(|v| v.as_str())
                {
                    debug!("Received AI response from {}", state.model_name);
                    return parse_llm_json_output(raw_content);
                }
            }
            warn!("Unexpected response structure from NVIDIA NIM, using fallback extraction");
            heuristic_extraction(&context, user_goal, schema)
        }
        Ok(resp) => {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            warn!(
                "NVIDIA NIM returned HTTP {} ({}), using fallback structured output",
                status,
                truncate_utf8(&err_text, 250)
            );
            heuristic_extraction(&context, user_goal, schema)
        }
        Err(err) => {
            warn!("Failed to reach NVIDIA NIM ({}): using fallback structured output", err);
            heuristic_extraction(&context, user_goal, schema)
        }
    }
}

/// Strip `<think>...</think>` blocks and ````json ... ```` code blocks, then parse JSON.
pub fn parse_llm_json_output(raw: &str) -> Value {
    let mut cleaned = raw.trim().to_string();

    // Remove <think>...</think> blocks emitted by reasoning models
    if let Ok(re_think) = Regex::new(r"(?is)<think>.*?</think>") {
        cleaned = re_think.replace_all(&cleaned, "").trim().to_string();
    }

    // Extract from ```json ... ``` fences if present
    if let Ok(re_fence) = Regex::new(r"(?is)```(?:json)?\s*(.*?)\s*```") {
        if let Some(caps) = re_fence.captures(&cleaned) {
            if let Some(m) = caps.get(1) {
                cleaned = m.as_str().trim().to_string();
            }
        }
    }

    // 1. Direct JSON parse
    if let Ok(val) = serde_json::from_str::<Value>(&cleaned) {
        return val;
    }

    // 2. Find first `{` to last `}` or first `[` to last `]`
    if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if start < end {
            if let Ok(val) = serde_json::from_str::<Value>(&cleaned[start..=end]) {
                return val;
            }
        }
    }

    json!({
        "result": cleaned
    })
}

fn heuristic_extraction(context: &str, prompt: &str, schema: Option<&Value>) -> Value {
    let preview = truncate_utf8(context, 2_000);
    let lines: Vec<&str> = context
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(15)
        .collect();

    if let Some(s) = schema {
        if let Some(props) = s.get("properties").and_then(|p| p.as_object()) {
            let mut obj = serde_json::Map::new();
            for (k, v) in props {
                let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                match typ {
                    "array" => {
                        obj.insert(k.clone(), json!(lines));
                    }
                    "number" | "integer" => {
                        obj.insert(k.clone(), json!(lines.len()));
                    }
                    "boolean" => {
                        obj.insert(k.clone(), json!(true));
                    }
                    _ => {
                        obj.insert(
                            k.clone(),
                            json!(lines.first().copied().unwrap_or(prompt)),
                        );
                    }
                }
            }
            return Value::Object(obj);
        }
    }

    json!({
        "summary": lines.join(" "),
        "prompt": prompt,
        "contentPreview": preview,
        "charCount": context.len()
    })
}

/// Autonomous multi-URL / web-research agent powered by NVIDIA NIM.
pub async fn execute_agent(
    state: &AppState,
    prompt: Option<String>,
    urls: Vec<String>,
    schema: Option<Value>,
) -> (Value, Vec<String>) {
    let prompt_text = prompt.unwrap_or_else(|| {
        "Analyze the target web pages and synthesize a comprehensive structured report.".to_string()
    });

    // Discover target URLs from explicit list, URLs embedded in prompt, or live search
    let mut target_urls = urls;
    if target_urls.is_empty() {
        if let Ok(re_url) = Regex::new(r#"https?://[^\s"'<>)]+"#) {
            for m in re_url.find_iter(&prompt_text) {
                target_urls.push(m.as_str().trim_end_matches(&['.', ',', ';'][..]).to_string());
            }
        }
    }

    if target_urls.is_empty() {
        let search_req = SearchRequest {
            query: prompt_text.clone(),
            limit: Some(3),
            lang: Some("en".to_string()),
            country: Some("us".to_string()),
            scrape_options: None,
        };
        let search_hits = search_web(state, &search_req).await;
        for hit in search_hits.into_iter().take(3) {
            target_urls.push(hit.url);
        }
    }

    // Scrape discovered URLs concurrently (up to 5)
    let mut handles = Vec::new();
    for u in target_urls.iter().take(5).cloned() {
        let st = state.clone();
        handles.push(tokio::spawn(async move {
            let sreq = ScrapeRequest {
                url: u.clone(),
                only_main_content: Some(true),
                timeout: Some(25_000),
                ..Default::default()
            };
            let data = scrape_url_base(&st, &sreq).await;
            (u, data)
        }));
    }

    let mut combined_md = String::new();
    let mut sources = Vec::new();
    for h in handles {
        if let Ok((u, data)) = h.await {
            sources.push(u.clone());
            combined_md.push_str(&format!(
                "\n\n=== SOURCE: {} (Title: {}) ===\n{}\n",
                u, data.metadata.title, data.markdown
            ));
        }
    }

    let capped_md = truncate_utf8(&combined_md, MAX_AI_CONTEXT_CHARS);
    let extracted = extract_from_markdown(
        state,
        &capped_md,
        Some(&prompt_text),
        Some("You are Firecrawl Autonomous Research Agent powered by NVIDIA NIM. Synthesize findings from all scraped sources and return valid JSON."),
        schema.as_ref(),
    )
    .await;

    (extracted, sources)
}
