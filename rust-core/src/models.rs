use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExtractOptions {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeRequest {
    pub url: String,
    #[serde(default)]
    pub formats: Option<Vec<String>>,
    #[serde(default)]
    pub only_main_content: Option<bool>,
    #[serde(default)]
    pub include_tags: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_tags: Option<Vec<String>>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub wait_for: Option<u64>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub extract: Option<ExtractOptions>,
    #[serde(default)]
    pub json_options: Option<ExtractOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeMetadata {
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(rename = "sourceURL")]
    pub source_url: String,
    pub status_code: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeData {
    pub markdown: String,
    pub html: String,
    pub raw_html: String,
    pub links: Vec<String>,
    #[serde(default)]
    pub extract: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    pub metadata: ScrapeMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapeResponse {
    pub success: bool,
    pub data: ScrapeData,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CrawlRequest {
    pub url: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub max_depth: Option<usize>,
    #[serde(default)]
    pub max_discovery_depth: Option<usize>,
    #[serde(default)]
    pub include_paths: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_paths: Option<Vec<String>>,
    #[serde(default)]
    pub allow_backward_links: Option<bool>,
    #[serde(default)]
    pub allow_external_links: Option<bool>,
    #[serde(default)]
    pub scrape_options: Option<ScrapeOptionsPartial>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeOptionsPartial {
    #[serde(default)]
    pub formats: Option<Vec<String>>,
    #[serde(default)]
    pub only_main_content: Option<bool>,
    #[serde(default)]
    pub include_tags: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_tags: Option<Vec<String>>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub extract: Option<ExtractOptions>,
    #[serde(default)]
    pub json_options: Option<ExtractOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlStartResponse {
    pub success: bool,
    pub id: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrawlJob {
    pub id: String,
    pub status: String, // "scraping", "completed", "failed", "cancelled"
    pub total: usize,
    pub completed: usize,
    pub credits_used: usize,
    pub expires_at: String,
    pub data: Vec<ScrapeData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrawlStatusResponse {
    pub success: bool,
    pub status: String,
    pub total: usize,
    pub completed: usize,
    pub credits_used: usize,
    pub expires_at: String,
    pub data: Vec<ScrapeData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MapRequest {
    pub url: String,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub ignore_sitemap: Option<bool>,
    #[serde(default)]
    pub include_subdomains: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapResponse {
    pub success: bool,
    pub links: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub scrape_options: Option<ScrapeOptionsPartial>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultItem {
    pub title: String,
    pub url: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ScrapeMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub success: bool,
    pub data: Vec<SearchResultItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExtractRequest {
    #[serde(default)]
    pub urls: Option<Vec<String>>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentRequest {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub urls: Option<Vec<String>>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
    #[serde(default)]
    pub async_mode: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentJob {
    pub id: String,
    pub status: String, // "processing", "completed", "failed"
    pub data: Value,
    pub sources: Vec<String>,
    pub model: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
