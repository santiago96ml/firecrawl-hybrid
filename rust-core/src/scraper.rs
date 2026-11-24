use crate::models::{
    ScrapeData, ScrapeMetadata, ScrapeRequest, SearchRequest, SearchResultItem,
};
use crate::AppState;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;
use tracing::{debug, warn};
use url::Url;

/// Scrape a URL via Go Engine with automatic native Rust fallback.
pub async fn scrape_url_base(state: &AppState, req: &ScrapeRequest) -> ScrapeData {
    let timeout_ms = req.timeout.unwrap_or(30_000).clamp(1_000, 120_000);
    let only_main = req.only_main_content.unwrap_or(true);

    // 1. Try Go Engine first
    let go_endpoint = format!("{}/scrape", state.go_engine_url.trim_end_matches('/'));
    let go_payload = json!({
        "url": req.url,
        "onlyMainContent": only_main,
        "includeTags": req.include_tags.clone().unwrap_or_default(),
        "excludeTags": req.exclude_tags.clone().unwrap_or_default(),
        "headers": req.headers.clone().unwrap_or_default(),
        "timeout": timeout_ms,
        "waitFor": req.wait_for.unwrap_or(0)
    });

    match state
        .http_client
        .post(&go_endpoint)
        .timeout(Duration::from_millis(timeout_ms + 2_000))
        .json(&go_payload)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(val) = resp.json::<Value>().await {
                if let Some(parsed) = parse_go_scrape_value(&val, &req.url) {
                    debug!("Scraped {} via Go Engine", req.url);
                    return parsed;
                }
            }
        }
        Ok(resp) => {
            warn!(
                "Go engine returned status {} for {}, falling back to Rust native scraper",
                resp.status(),
                req.url
            );
        }
        Err(err) => {
            warn!(
                "Go engine unreachable ({}) for {}, falling back to Rust native scraper",
                err, req.url
            );
        }
    }

    // 2. Fallback to native Rust HTTP/TLS + HTML-to-Markdown scraper
    native_rust_scrape(&state.http_client, req, timeout_ms, only_main).await
}

fn parse_go_scrape_value(val: &Value, fallback_url: &str) -> Option<ScrapeData> {
    // Check if the Go engine returned an explicit failure
    if let Some(false) = val.get("success").and_then(|v| v.as_bool()) {
        return None;
    }

    let root = val.get("data").unwrap_or(val);
    let markdown = root
        .get("markdown")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let html = root
        .get("html")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let raw_html = root
        .get("rawHtml")
        .or_else(|| root.get("raw_html"))
        .and_then(|v| v.as_str())
        .unwrap_or(&html)
        .to_string();

    if markdown.is_empty() && html.is_empty() && raw_html.is_empty() {
        return None;
    }

    let links: Vec<String> = root
        .get("links")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let meta_obj = root.get("metadata").unwrap_or(root);
    let title = meta_obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let description = meta_obj
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let language = meta_obj
        .get("language")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let source_url = meta_obj
        .get("sourceURL")
        .or_else(|| meta_obj.get("sourceUrl"))
        .or_else(|| meta_obj.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or(fallback_url)
        .to_string();
    let status_code = meta_obj
        .get("statusCode")
        .or_else(|| meta_obj.get("status_code"))
        .and_then(|v| v.as_u64())
        .unwrap_or(200) as u16;

    Some(ScrapeData {
        markdown,
        html,
        raw_html,
        links,
        extract: None,
        json: None,
        metadata: ScrapeMetadata {
            title,
            description,
            language,
            source_url,
            status_code,
            error: None,
            engine: Some("go-engine".to_string()),
        },
    })
}

pub async fn native_rust_scrape(
    client: &reqwest::Client,
    req: &ScrapeRequest,
    timeout_ms: u64,
    only_main: bool,
) -> ScrapeData {
    let mut request_builder = client
        .get(&req.url)
        .timeout(Duration::from_millis(timeout_ms))
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9,es;q=0.8");

    if let Some(headers) = &req.headers {
        for (k, v) in headers {
            request_builder = request_builder.header(k, v);
        }
    }

    match request_builder.send().await {
        Ok(resp) => {
            let status_code = resp.status().as_u16();
            let final_url = resp.url().to_string();
            let raw_html = resp.text().await.unwrap_or_default();

            let title = extract_tag_text(&raw_html, "title");
            let description = extract_meta_description(&raw_html);
            let language = extract_html_lang(&raw_html);
            let links = extract_links(&raw_html, &final_url);
            let cleaned_html = clean_html_dom(&raw_html, only_main);
            let markdown = html_to_markdown(&cleaned_html, &title);

            ScrapeData {
                markdown,
                html: cleaned_html,
                raw_html,
                links,
                extract: None,
                json: None,
                metadata: ScrapeMetadata {
                    title,
                    description,
                    language,
                    source_url: final_url,
                    status_code,
                    error: None,
                    engine: Some("rust-native".to_string()),
                },
            }
        }
        Err(err) => ScrapeData {
            markdown: String::new(),
            html: String::new(),
            raw_html: String::new(),
            links: Vec::new(),
            extract: None,
            json: None,
            metadata: ScrapeMetadata {
                title: String::new(),
                description: String::new(),
                language: None,
                source_url: req.url.clone(),
                status_code: 502,
                error: Some(err.to_string()),
                engine: Some("rust-native".to_string()),
            },
        },
    }
}

fn extract_tag_text(html: &str, tag: &str) -> String {
    let pattern = format!(r"(?is)<{}\b[^>]*>(.*?)</{}>", tag, tag);
    if let Ok(re) = Regex::new(&pattern) {
        if let Some(caps) = re.captures(html) {
            if let Some(m) = caps.get(1) {
                return decode_html_entities(strip_all_tags(m.as_str()).trim());
            }
        }
    }
    String::new()
}

fn extract_meta_description(html: &str) -> String {
    let patterns = [
        r#"(?is)<meta\s+[^>]*name\s*=\s*["']description["'][^>]*content\s*=\s*["']([^"']*)["']"#,
        r#"(?is)<meta\s+[^>]*content\s*=\s*["']([^"']*)["'][^>]*name\s*=\s*["']description["']"#,
        r#"(?is)<meta\s+[^>]*property\s*=\s*["']og:description["'][^>]*content\s*=\s*["']([^"']*)["']"#,
    ];
    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(caps) = re.captures(html) {
                if let Some(m) = caps.get(1) {
                    return decode_html_entities(m.as_str().trim());
                }
            }
        }
    }
    String::new()
}

fn extract_html_lang(html: &str) -> Option<String> {
    if let Ok(re) = Regex::new(r#"(?is)<html\b[^>]*\blang\s*=\s*["']([^"']+)["']"#) {
        if let Some(caps) = re.captures(html) {
            return caps.get(1).map(|m| m.as_str().trim().to_string());
        }
    }
    None
}

pub fn extract_links(html: &str, base_url_str: &str) -> Vec<String> {
    let base_url = Url::parse(base_url_str).ok();
    let mut seen = HashSet::new();
    let mut links = Vec::new();

    if let Ok(re) = Regex::new(r#"(?is)<a\b[^>]*\bhref\s*=\s*["']([^"'#]+)["']"#) {
        for caps in re.captures_iter(html) {
            if let Some(m) = caps.get(1) {
                let raw = m.as_str().trim();
                if raw.is_empty()
                    || raw.starts_with("javascript:")
                    || raw.starts_with("mailto:")
                    || raw.starts_with("tel:")
                    || raw.starts_with("data:")
                {
                    continue;
                }
                let resolved = if let Some(ref base) = base_url {
                    base.join(raw)
                        .ok()
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| raw.to_string())
                } else {
                    raw.to_string()
                };
                if (resolved.starts_with("http://") || resolved.starts_with("https://"))
                    && seen.insert(resolved.clone())
                {
                    links.push(resolved);
                }
            }
        }
    }
    links
}

fn clean_html_dom(html: &str, only_main: bool) -> String {
    let mut out = html.to_string();
    let always_remove = [
        r"(?is)<!--.*?-->",
        r"(?is)<script\b[^>]*>.*?</script>",
        r"(?is)<style\b[^>]*>.*?</style>",
        r"(?is)<noscript\b[^>]*>.*?</noscript>",
        r"(?is)<svg\b[^>]*>.*?</svg>",
        r"(?is)<iframe\b[^>]*>.*?</iframe>",
    ];
    for pat in always_remove {
        if let Ok(re) = Regex::new(pat) {
            out = re.replace_all(&out, "").to_string();
        }
    }

    if only_main {
        let chrome_tags = [
            r"(?is)<nav\b[^>]*>.*?</nav>",
            r"(?is)<footer\b[^>]*>.*?</footer>",
            r"(?is)<aside\b[^>]*>.*?</aside>",
        ];
        for pat in chrome_tags {
            if let Ok(re) = Regex::new(pat) {
                out = re.replace_all(&out, "").to_string();
            }
        }
    }
    out
}

fn html_to_markdown(cleaned_html: &str, title: &str) -> String {
    let mut md = cleaned_html.to_string();

    // Headings h1..h6
    for level in 1..=6 {
        let hashes = "#".repeat(level);
        let pat = format!(r"(?is)<h{}\b[^>]*>(.*?)</h{}>", level, level);
        if let Ok(re) = Regex::new(&pat) {
            md = re
                .replace_all(&md, |caps: &regex::Captures| {
                    let inner = strip_all_tags(&caps[1]);
                    format!("\n\n{} {}\n\n", hashes, inner.trim())
                })
                .to_string();
        }
    }

    // Links <a href="...">text</a>
    if let Ok(re) = Regex::new(r#"(?is)<a\b[^>]*\bhref\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#) {
        md = re
            .replace_all(&md, |caps: &regex::Captures| {
                let href = caps[1].trim();
                let text = strip_all_tags(&caps[2]);
                let clean_text = text.trim();
                if clean_text.is_empty() {
                    href.to_string()
                } else {
                    format!("[{}]({})", clean_text, href)
                }
            })
            .to_string();
    }

    // List items
    if let Ok(re) = Regex::new(r"(?is)<li\b[^>]*>(.*?)</li>") {
        md = re
            .replace_all(&md, |caps: &regex::Captures| {
                let inner = strip_all_tags(&caps[1]);
                format!("\n- {}", inner.trim())
            })
            .to_string();
    }

    // Paragraphs & block breaks
    if let Ok(re) = Regex::new(r"(?is)</?(p|div|section|article|tr|br|hr|blockquote|pre)\b[^>]*>") {
        md = re.replace_all(&md, "\n\n").to_string();
    }

    // Strip remaining HTML tags
    md = strip_all_tags(&md);
    md = decode_html_entities(&md);

    // Normalize whitespace & blank lines
    let mut lines = Vec::new();
    let mut blank_count = 0;
    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                lines.push(String::new());
            }
        } else {
            blank_count = 0;
            lines.push(trimmed.to_string());
        }
    }

    let result = lines.join("\n").trim().to_string();
    if result.is_empty() && !title.is_empty() {
        format!("# {}", title)
    } else {
        result
    }
}

fn strip_all_tags(input: &str) -> String {
    if let Ok(re) = Regex::new(r"(?is)<[^>]+>") {
        re.replace_all(input, " ").to_string()
    } else {
        input.to_string()
    }
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Fetch `/sitemap.xml` concurrently to enrich `/v1/map` and `/v2/map` link discovery.
pub async fn fetch_sitemap_links(client: &reqwest::Client, base_url_str: &str) -> Vec<String> {
    let Ok(parsed) = Url::parse(base_url_str) else {
        return Vec::new();
    };
    let Ok(sitemap_url) = parsed.join("/sitemap.xml") else {
        return Vec::new();
    };

    let Ok(resp) = client
        .get(sitemap_url.as_str())
        .timeout(Duration::from_secs(8))
        .header("User-Agent", "Mozilla/5.0 (compatible; FirecrawlRust/2.0)")
        .send()
        .await
    else {
        return Vec::new();
    };

    if !resp.status().is_success() {
        return Vec::new();
    }

    let xml = resp.text().await.unwrap_or_default();
    let mut urls = Vec::new();
    if let Ok(re) = Regex::new(r"(?is)<loc>\s*(https?://[^<\s]+)\s*</loc>") {
        for caps in re.captures_iter(&xml) {
            if let Some(m) = caps.get(1) {
                urls.push(decode_html_entities(m.as_str().trim()));
            }
        }
    }
    urls
}

/// Search the web via Go Engine `POST /search` with native Rust DuckDuckGo fallback.
pub async fn search_web(state: &AppState, req: &SearchRequest) -> Vec<SearchResultItem> {
    let limit = req.limit.unwrap_or(5).clamp(1, 20);
    let go_search_url = format!("{}/search", state.go_engine_url.trim_end_matches('/'));
    let payload = json!({
        "query": req.query,
        "limit": limit,
        "lang": req.lang.clone().unwrap_or_else(|| "en".to_string()),
        "country": req.country.clone().unwrap_or_else(|| "us".to_string())
    });

    let mut results = Vec::new();

    if let Ok(resp) = state
        .http_client
        .post(&go_search_url)
        .timeout(Duration::from_secs(15))
        .json(&payload)
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(val) = resp.json::<Value>().await {
                let arr_opt = val
                    .get("data")
                    .or_else(|| val.get("results"))
                    .and_then(|v| v.as_array())
                    .or_else(|| val.as_array());
                if let Some(arr) = arr_opt {
                    for item in arr.iter().take(limit) {
                        let url = item
                            .get("url")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if url.is_empty() {
                            continue;
                        }
                        let title = item
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&url)
                            .to_string();
                        let description = item
                            .get("description")
                            .or_else(|| item.get("snippet"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let markdown = item
                            .get("markdown")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        results.push(SearchResultItem {
                            title,
                            url,
                            description,
                            markdown,
                            html: None,
                            links: None,
                            metadata: None,
                        });
                    }
                }
            }
        }
    }

    if results.is_empty() {
        results = native_duckduckgo_search(&state.http_client, &req.query, limit).await;
    }

    // If scrapeOptions is requested on search, scrape each result concurrently
    if let Some(ref opts) = req.scrape_options {
        let should_scrape = opts
            .formats
            .as_ref()
            .map(|f| !f.is_empty())
            .unwrap_or(true);
        if should_scrape {
            let mut handles = Vec::new();
            for item in results {
                let st = state.clone();
                let only_main = opts.only_main_content;
                handles.push(tokio::spawn(async move {
                    let sreq = ScrapeRequest {
                        url: item.url.clone(),
                        only_main_content: only_main,
                        timeout: Some(15_000),
                        ..Default::default()
                    };
                    let scraped = scrape_url_base(&st, &sreq).await;
                    SearchResultItem {
                        title: if item.title.is_empty() {
                            scraped.metadata.title.clone()
                        } else {
                            item.title
                        },
                        url: item.url,
                        description: if item.description.is_empty() {
                            scraped.metadata.description.clone()
                        } else {
                            item.description
                        },
                        markdown: Some(scraped.markdown),
                        html: Some(scraped.html),
                        links: Some(scraped.links),
                        metadata: Some(scraped.metadata),
                    }
                }));
            }
            let mut enriched = Vec::new();
            for h in handles {
                if let Ok(item) = h.await {
                    enriched.push(item);
                }
            }
            return enriched;
        }
    }

    results
}

async fn native_duckduckgo_search(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Vec<SearchResultItem> {
    let Ok(mut search_url) = Url::parse("https://html.duckduckgo.com/html/") else {
        return Vec::new();
    };
    search_url.query_pairs_mut().append_pair("q", query);

    let Ok(resp) = client
        .get(search_url.as_str())
        .timeout(Duration::from_secs(12))
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .send()
        .await
    else {
        return Vec::new();
    };

    let html = resp.text().await.unwrap_or_default();
    let mut items = Vec::new();

    let Ok(re_link) = Regex::new(
        r#"(?is)<a\b[^>]*class\s*=\s*["'][^"']*result__a[^"']*["'][^>]*href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#,
    ) else {
        return Vec::new();
    };

    for caps in re_link.captures_iter(&html).take(limit) {
        let raw_href = decode_html_entities(caps.get(1).map(|m| m.as_str()).unwrap_or_default());
        let clean_url = extract_ddg_target_url(&raw_href);
        if !clean_url.starts_with("http") {
            continue;
        }
        let title = decode_html_entities(&strip_all_tags(
            caps.get(2).map(|m| m.as_str()).unwrap_or_default(),
        ))
        .trim()
        .to_string();

        items.push(SearchResultItem {
            title,
            url: clean_url,
            description: String::new(),
            markdown: None,
            html: None,
            links: None,
            metadata: None,
        });
    }

    items
}

fn extract_ddg_target_url(raw_href: &str) -> String {
    let full = if raw_href.starts_with("//") {
        format!("https:{}", raw_href)
    } else {
        raw_href.to_string()
    };
    if let Ok(parsed) = Url::parse(&full) {
        for (k, v) in parsed.query_pairs() {
            if k == "uddg" {
                return v.to_string();
            }
        }
    }
    full
}
