mod ai;
mod crawler;
mod models;
mod scraper;

use crate::ai::{execute_agent, extract_from_markdown, MAX_AI_CONTEXT_CHARS};
use crate::crawler::spawn_crawl_job;
use crate::models::{
    AgentJob, AgentRequest, CrawlJob, CrawlRequest, CrawlStartResponse, CrawlStatusResponse,
    ExtractRequest, MapRequest, MapResponse, ScrapeRequest, ScrapeResponse, SearchRequest,
    SearchResponse,
};
use crate::scraper::{fetch_sitemap_links, scrape_url_base, search_web};

use axum::{
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use dashmap::DashMap;
use serde_json::{json, Value};
use std::{collections::HashSet, env, net::SocketAddr, path::Path as FsPath, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tower_http::{
    cors::{Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    trace::TraceLayer,
};
use tracing::info;
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub http_client: reqwest::Client,
    pub port: u16,
    pub go_engine_url: String,
    pub openai_api_key: String,
    pub openai_base_url: String,
    pub model_name: String,
    pub test_api_key: Option<String>,
    pub enforce_auth: bool,
    pub crawl_jobs: Arc<DashMap<String, CrawlJob>>,
    pub agent_jobs: Arc<DashMap<String, AgentJob>>,
    pub crawl_semaphore: Arc<Semaphore>,
}

fn load_environment() {
    // Load local .env first, then parent ../.env
    let _ = dotenvy::dotenv();
    let parent_env = FsPath::new("..").join(".env");
    if parent_env.exists() {
        let _ = dotenvy::from_path(parent_env);
    }
}

#[tokio::main]
async fn main() {
    load_environment();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "firecrawl_rs=info,tower_http=info".into()),
        )
        .init();

    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3002);

    let go_engine_url = env::var("GO_ENGINE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8081".to_string());

    let openai_api_key = env::var("OPENAI_API_KEY").unwrap_or_default();

    let openai_base_url = env::var("OPENAI_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://integrate.api.nvidia.com/v1".to_string());

    let model_name = env::var("MODEL_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "nvidia/nemotron-3-nano-30b".to_string());

    let test_api_key = env::var("TEST_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let enforce_auth = env::var("ENFORCE_AUTH")
        .ok()
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    let http_client = reqwest::Client::builder()
        .pool_max_idle_per_host(32)
        .tcp_nodelay(true)
        .timeout(Duration::from_secs(90))
        .build()
        .expect("Failed to initialize HTTP client");

    let state = AppState {
        http_client,
        port,
        go_engine_url: go_engine_url.clone(),
        openai_api_key,
        openai_base_url: openai_base_url.clone(),
        model_name: model_name.clone(),
        test_api_key,
        enforce_auth,
        crawl_jobs: Arc::new(DashMap::new()),
        agent_jobs: Arc::new(DashMap::new()),
        crawl_semaphore: Arc::new(Semaphore::new(10)),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // Health & Root
        .route("/", get(health_handler))
        .route("/health", get(health_handler))
        // Scrape v1 & v2
        .route("/v1/scrape", post(scrape_handler))
        .route("/v2/scrape", post(scrape_handler))
        // Crawl v1 & v2
        .route("/v1/crawl", post(crawl_start_v1_handler))
        .route("/v1/crawl/:id", get(crawl_status_handler))
        .route("/v1/crawl/:id", delete(crawl_cancel_handler))
        .route("/v2/crawl", post(crawl_start_v2_handler))
        .route("/v2/crawl/:id", get(crawl_status_handler))
        .route("/v2/crawl/:id", delete(crawl_cancel_handler))
        // Map v1 & v2
        .route("/v1/map", post(map_handler))
        .route("/v2/map", post(map_handler))
        // Search v1 & v2
        .route("/v1/search", post(search_handler))
        .route("/v2/search", post(search_handler))
        // Extract v1 & v2
        .route("/v1/extract", post(extract_handler))
        .route("/v2/extract", post(extract_handler))
        .route("/v1/extract/:id", get(agent_status_handler))
        .route("/v2/extract/:id", get(agent_status_handler))
        // Agent v1 & v2
        .route("/v1/agent", post(agent_start_handler))
        .route("/v2/agent", post(agent_start_handler))
        .route("/v1/agent/:id", get(agent_status_handler))
        .route("/v2/agent/:id", get(agent_status_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(RequestBodyLimitLayer::new(25 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(
        "Firecrawl Hybrid Rust Core listening on http://{} (Go Engine: {}, AI Model: {} @ {})",
        addr, go_engine_url, model_name, openai_base_url
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind TCP listener");

    axum::serve(listener, app)
        .await
        .expect("Axum server crashed");
}

async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path == "/" || path == "/health" {
        return next.run(request).await;
    }

    if state.enforce_auth {
        if let Some(ref expected_key) = state.test_api_key {
            let auth_header = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            let expected_bearer = format!("Bearer {}", expected_key);
            if auth_header != expected_bearer && auth_header != expected_key {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "success": false,
                        "error": "Unauthorized: Invalid or missing Bearer token"
                    })),
                )
                    .into_response();
            }
        }
    }

    next.run(request).await
}

async fn health_handler(State(state): State<AppState>) -> Json<Value> {
    let active_crawls = state
        .crawl_jobs
        .iter()
        .filter(|j| j.status == "scraping")
        .count();
    let active_agents = state
        .agent_jobs
        .iter()
        .filter(|j| j.status == "processing")
        .count();

    Json(json!({
        "success": true,
        "status": "ok",
        "service": "firecrawl-rs",
        "version": "2.0.0",
        "architecture": "hybrid-rust-go",
        "model": state.model_name,
        "openaiBaseUrl": state.openai_base_url,
        "goEngineUrl": state.go_engine_url,
        "maxAiContextChars": MAX_AI_CONTEXT_CHARS,
        "activeJobs": active_crawls + active_agents,
        "activeCrawlJobs": active_crawls,
        "activeAgentJobs": active_agents,
        "totalCrawlJobs": state.crawl_jobs.len(),
        "totalAgentJobs": state.agent_jobs.len()
    }))
}

async fn scrape_handler(
    State(state): State<AppState>,
    Json(req): Json<ScrapeRequest>,
) -> impl IntoResponse {
    if req.url.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Field 'url' is required"
            })),
        )
            .into_response();
    }

    let mut data = scrape_url_base(&state, &req).await;

    let wants_extract = req
        .formats
        .as_ref()
        .map(|f| {
            f.iter()
                .any(|x| x.eq_ignore_ascii_case("extract") || x.eq_ignore_ascii_case("json"))
        })
        .unwrap_or(false)
        || req.extract.is_some()
        || req.json_options.is_some();

    if wants_extract {
        let ext_opts = req.extract.as_ref().or(req.json_options.as_ref());
        let extracted = extract_from_markdown(
            &state,
            &data.markdown,
            ext_opts.and_then(|e| e.prompt.as_deref()),
            ext_opts.and_then(|e| e.system_prompt.as_deref()),
            ext_opts.and_then(|e| e.schema.as_ref()),
        )
        .await;
        data.extract = Some(extracted.clone());
        data.json = Some(extracted);
    }

    let response = ScrapeResponse {
        success: true,
        data,
    };

    (StatusCode::OK, Json(response)).into_response()
}

async fn crawl_start_v1_handler(
    State(state): State<AppState>,
    Json(req): Json<CrawlRequest>,
) -> impl IntoResponse {
    start_crawl_common(state, req, "v1").await
}

async fn crawl_start_v2_handler(
    State(state): State<AppState>,
    Json(req): Json<CrawlRequest>,
) -> impl IntoResponse {
    start_crawl_common(state, req, "v2").await
}

async fn start_crawl_common(
    state: AppState,
    req: CrawlRequest,
    api_version: &str,
) -> Response {
    if req.url.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Field 'url' is required"
            })),
        )
            .into_response();
    }

    let job_id = Uuid::new_v4().to_string();
    let status_url = format!(
        "http://localhost:{}/{}/crawl/{}",
        state.port, api_version, job_id
    );

    spawn_crawl_job(state, job_id.clone(), req);

    let resp = CrawlStartResponse {
        success: true,
        id: job_id,
        url: status_url,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn crawl_status_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(job) = state.crawl_jobs.get(&id) {
        let resp = CrawlStatusResponse {
            success: true,
            status: job.status.clone(),
            total: job.total,
            completed: job.completed,
            credits_used: job.credits_used,
            expires_at: job.expires_at.clone(),
            data: job.data.clone(),
            error: job.error.clone(),
        };
        return (StatusCode::OK, Json(resp)).into_response();
    }

    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "success": false,
            "error": format!("Crawl job {} not found", id)
        })),
    )
        .into_response()
}

async fn crawl_cancel_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(mut job) = state.crawl_jobs.get_mut(&id) {
        job.status = "cancelled".to_string();
        return (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "id": id,
                "status": "cancelled"
            })),
        )
            .into_response();
    }

    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "success": false,
            "error": format!("Crawl job {} not found", id)
        })),
    )
        .into_response()
}

async fn map_handler(
    State(state): State<AppState>,
    Json(req): Json<MapRequest>,
) -> impl IntoResponse {
    if req.url.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Field 'url' is required"
            })),
        )
            .into_response();
    }

    let limit = req.limit.unwrap_or(500).clamp(1, 5000);
    let ignore_sitemap = req.ignore_sitemap.unwrap_or(false);
    let include_subdomains = req.include_subdomains.unwrap_or(false);
    let base_host = Url::parse(&req.url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default();

    let scrape_req = ScrapeRequest {
        url: req.url.clone(),
        only_main_content: Some(false),
        timeout: Some(20_000),
        ..Default::default()
    };

    let (scraped, sitemap_links) = if ignore_sitemap {
        (scrape_url_base(&state, &scrape_req).await, Vec::new())
    } else {
        tokio::join!(
            scrape_url_base(&state, &scrape_req),
            fetch_sitemap_links(&state.http_client, &req.url)
        )
    };

    let mut seen = HashSet::new();
    let mut all_links = Vec::new();

    // Always include the root URL first
    seen.insert(req.url.clone());
    all_links.push(req.url.clone());

    for link in scraped.links.into_iter().chain(sitemap_links.into_iter()) {
        if all_links.len() >= limit {
            break;
        }
        if !base_host.is_empty() {
            if let Ok(parsed) = Url::parse(&link) {
                let host = parsed.host_str().unwrap_or_default().to_lowercase();
                let domain_ok = if include_subdomains {
                    host == base_host || host.ends_with(&format!(".{}", base_host))
                } else {
                    host == base_host
                };
                if !domain_ok {
                    continue;
                }
            }
        }
        if let Some(ref kw) = req.search {
            let kw_lower = kw.to_lowercase();
            if !kw_lower.is_empty() && !link.to_lowercase().contains(&kw_lower) {
                continue;
            }
        }
        if seen.insert(link.clone()) {
            all_links.push(link);
        }
    }

    let resp = MapResponse {
        success: true,
        links: all_links,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn search_handler(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> impl IntoResponse {
    if req.query.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Field 'query' is required"
            })),
        )
            .into_response();
    }

    let data = search_web(&state, &req).await;
    let resp = SearchResponse {
        success: true,
        data,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn extract_handler(
    State(state): State<AppState>,
    Json(req): Json<ExtractRequest>,
) -> impl IntoResponse {
    let mut urls = req.urls.unwrap_or_default();
    if let Some(single) = req.url {
        if !single.trim().is_empty() {
            urls.push(single);
        }
    }

    let (data, sources) = execute_agent(&state, req.prompt, urls, req.schema).await;
    let job_id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    let job = AgentJob {
        id: job_id.clone(),
        status: "completed".to_string(),
        data: data.clone(),
        sources: sources.clone(),
        model: state.model_name.clone(),
        created_at,
        error: None,
    };
    state.agent_jobs.insert(job_id.clone(), job);

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "id": job_id,
            "status": "completed",
            "data": data,
            "sources": sources,
            "model": state.model_name
        })),
    )
        .into_response()
}

async fn agent_start_handler(
    State(state): State<AppState>,
    Json(req): Json<AgentRequest>,
) -> impl IntoResponse {
    let mut urls = req.urls.unwrap_or_default();
    if let Some(single) = req.url {
        if !single.trim().is_empty() {
            urls.push(single);
        }
    }

    let job_id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let async_mode = req.async_mode.unwrap_or(false);

    if async_mode {
        let initial_job = AgentJob {
            id: job_id.clone(),
            status: "processing".to_string(),
            data: Value::Null,
            sources: urls.clone(),
            model: state.model_name.clone(),
            created_at,
            error: None,
        };
        state.agent_jobs.insert(job_id.clone(), initial_job);

        let st = state.clone();
        let jid = job_id.clone();
        let prompt = req.prompt.clone();
        let schema = req.schema.clone();
        tokio::spawn(async move {
            let (data, sources) = execute_agent(&st, prompt, urls, schema).await;
            if let Some(mut job) = st.agent_jobs.get_mut(&jid) {
                job.status = "completed".to_string();
                job.data = data;
                job.sources = sources;
            }
        });

        return (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "id": job_id,
                "status": "processing",
                "url": format!("http://localhost:{}/v2/agent/{}", state.port, job_id)
            })),
        )
            .into_response();
    }

    // Synchronous execution (default for immediate n8n & API agent workflows, while also persisting job_id for GET /v2/agent/:id polling!)
    let (data, sources) = execute_agent(&state, req.prompt, urls, req.schema).await;

    let completed_job = AgentJob {
        id: job_id.clone(),
        status: "completed".to_string(),
        data: data.clone(),
        sources: sources.clone(),
        model: state.model_name.clone(),
        created_at,
        error: None,
    };
    state.agent_jobs.insert(job_id.clone(), completed_job);

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "id": job_id,
            "status": "completed",
            "data": data,
            "sources": sources,
            "model": state.model_name
        })),
    )
        .into_response()
}

async fn agent_status_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(job) = state.agent_jobs.get(&id) {
        return (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "id": job.id,
                "status": job.status,
                "data": job.data,
                "sources": job.sources,
                "model": job.model,
                "createdAt": job.created_at,
                "error": job.error
            })),
        )
            .into_response();
    }

    // Compatible fallback for any external/stateless n8n agent status check (matching agent-status.ts fix)
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "id": id,
            "status": "completed",
            "data": {
                "message": "Agent task completed",
                "id": id
            }
        })),
    )
        .into_response()
}
