# Firecrawl Hybrid Rust Core (`firecrawl-rs`)

Standalone high-performance Rust Core API, Concurrent In-Memory BFS Crawler, Sitemap/Link Mapper, and NVIDIA NIM AI Extraction & Autonomous Agent service.

Replaces the heavy Node.js + RabbitMQ + Redis + PostgreSQL stack with a single native binary (`firecrawl-rs.exe`) running on port `3002`, paired with the Go Fast HTTP/TLS Engine (`go-engine` on port `8081`) and automatic native Rust fallback.

## Key Features

- **Zero External Infrastructure Required**: In-memory lock-free job store (`DashMap`) + `tokio::sync::Semaphore` replaces Redis, RabbitMQ, and PostgreSQL.
- **100% Firecrawl v1 & v2 API Compatible**: Drop-in replacement for n8n Firecrawl nodes, LangChain, LlamaIndex, and AI Agent workflows on `http://localhost:3002`.
- **Hybrid Rust + Go Engine with Automatic Fallback**: Delegates ultra-fast HTTP/2 + DOM/Markdown conversion to `go-engine` (`http://127.0.0.1:8081`) and automatically falls back to native Rust (`reqwest` + `rustls` + regex HTML-to-Markdown) if `go-engine` is offline.
- **NVIDIA NIM 100k Context AI Extraction & Agent**: Connects directly to `https://integrate.api.nvidia.com/v1` (`nvidia/nemotron-3-nano-30b`) with a 100,000-character markdown window (`MAX_AI_CONTEXT_CHARS = 100_000`) and automatic `<think>` tag + JSON fence parsing.

## Environment Variables

Automatically loads `.env` from `./.env` and `../.env`:

| Variable | Default | Description |
|---|---|---|
| `PORT` | `3002` | HTTP API listening port (matches n8n configuration) |
| `GO_ENGINE_URL` | `http://127.0.0.1:8081` | Go Fast Scraper & Search Engine base URL |
| `OPENAI_API_KEY` | *(from `../.env`)* | NVIDIA NIM Bearer API key (`nvapi-...`) |
| `OPENAI_BASE_URL` | `https://integrate.api.nvidia.com/v1` | OpenAI-compatible LLM base URL |
| `MODEL_NAME` | `nvidia/nemotron-3-nano-30b` | Default LLM model for `/scrape` extract & `/agent` |
| `TEST_API_KEY` | *(optional)* | Optional API key when `ENFORCE_AUTH=true` |

## API Endpoints

### Health & Status
- `GET /` and `GET /health` — Service status, active jobs, configured model, and Go engine URL.

### Scrape (`v1` & `v2`)
- `POST /v1/scrape`
- `POST /v2/scrape`
  - Supports `url`, `formats` (`["markdown", "html", "rawHtml", "links", "extract", "json"]`), `onlyMainContent`, `includeTags`, `excludeTags`, `headers`, `timeout`, and `extract` / `jsonOptions` (`prompt`, `systemPrompt`, `schema`).

### Crawl (`v1` & `v2`)
- `POST /v1/crawl` & `POST /v2/crawl` — Starts an async concurrent BFS crawl (`limit`, `maxDepth`, `includePaths`, `excludePaths`, `allowExternalLinks`, `scrapeOptions`).
- `GET /v1/crawl/:id` & `GET /v2/crawl/:id` — Returns crawl job progress (`scraping` | `completed` | `cancelled`) and scraped page array.
- `DELETE /v1/crawl/:id` & `DELETE /v2/crawl/:id` — Cancels an active crawl job.

### Map (`v1` & `v2`)
- `POST /v1/map` & `POST /v2/map` — Scrapes the root page and `/sitemap.xml` concurrently to return all site links, filtered by optional `search` keyword and `limit`.

### Search (`v1` & `v2`)
- `POST /v1/search` & `POST /v2/search` — Searches via `go-engine` (`POST /search`) with native Rust DuckDuckGo fallback and optional `scrapeOptions` enrichment.

### AI Extract & Autonomous Agent (`v1` & `v2`)
- `POST /v1/extract` & `POST /v2/extract` — Multi-URL structured JSON extraction powered by NVIDIA NIM (`100,000` char context window).
- `POST /v1/agent` & `POST /v2/agent` — Autonomous research & extraction agent supporting both synchronous execution and async job polling.
- `GET /v1/agent/:id` & `GET /v2/agent/:id` — Returns agent job status (`completed` | `processing`) and extracted `data`.

## Building & Running

```powershell
cd rust-core
cargo build --release
.\target\release\firecrawl-rs.exe
```
