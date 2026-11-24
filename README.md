# Firecrawl Hybrid (`rust-core` + `go-engine`)

Ultra-lightweight, Firecrawl **v1/v2 API-compatible** web scraping, crawling, mapping, web search, and AI extraction server written in **Rust (Axum + Tokio)** and **Go**.

Designed as a drop-in replacement for self-hosted automation workflows (n8n, AI agents, RAG pipelines, LLM tools) that only need fast HTTP scraping, GitHub-Flavored Markdown conversion, DuckDuckGo metasearch, and structured OpenAI-compatible JSON extraction **without** running Playwright/Chromium, Redis, RabbitMQ, PostgreSQL, or SearXNG.

---

## Measured Memory & Footprint (Windows x64)

Instead of ~4.5–5.5 GB across multiple Node.js, Playwright, Redis, RabbitMQ, PostgreSQL, and SearXNG containers, the hybrid stack runs as two native statically-linked binaries:

| Metric | `go-engine` (Go 1.26) | `firecrawl-rs` (Rust 2021) | Combined Stack | Legacy Multi-Container Stack |
| --- | --- | --- | --- | --- |
| **Binary Size** (stripped release) | `7.36 MB` | `7.11 MB` | **`14.47 MB`** | Multi-GB Docker images |
| **Idle Working Set (WS)** | `9.67 MB` (`0.60 MB` heap) | `7.30 MB` | **`16.97 MB`** | ~4.5–5.5 GB |
| **Idle Private Bytes** | `46.07 MB` | `1.92 MB` | **`47.99 MB`** | ~4.5–5.5 GB |
| **Peak Working Set** (24 concurrent `/v1/scrape`) | `28.23 MB` | `16.16 MB` | **`44.39 MB`** | Multi-GB + Chromium spikes |
| **Peak Private Bytes** (24 concurrent `/v1/scrape`) | — | — | **`69.21 MB`** | Multi-GB |

> Measured locally against compiled release binaries (`go build -ldflags="-s -w"` and `cargo build --release`) serving a 24-way concurrent `/v1/scrape` burst (`24/24` HTTP 200 OK), plus live `/v1/map`, `/v1/crawl`, `/v1/search`, and `/convert` checks. Even under heavy concurrent burst load, total memory stays **well below the 100 MB ceiling**.

---

## Architecture

```
Client / n8n / AI Agent
         │
         ▼  HTTP :3002
┌──────────────────────────────────────────────────────────┐
│  firecrawl-rs (Rust — Axum + Tokio + Reqwest + DashMap)  │
│  • Firecrawl v1/v2 API compatibility & Bearer Auth guard │
│  • Async BFS crawler (Semaphore=10, depth & regex filter)│
│  • Native Reqwest/Rustls fallback scraper                │
│  • OpenAI-compatible LLM /extract & /agent (100k chars)  │
│  • Deterministic heuristic extractor fallback (no API key) │
└───────────────────────────┬──────────────────────────────┘
                            │  HTTP :8081 (loopback / docker network)
                            ▼
┌──────────────────────────────────────────────────────────┐
│  go-engine (Go — net/http HTTP/2 + goquery + GFM)        │
│  • High-throughput HTTP/2 + TLS 1.2+ fetcher (pool=100)  │
│  • 6-profile Desktop User-Agent rotation & gzip/deflate  │
│  • goquery DOM noise stripping + GFM Markdown converter  │
│  • Concurrent DuckDuckGo metasearch + 5-worker scraper   │
└──────────────────────────────────────────────────────────┘
                            │
                            ▼  (Optional, for /v1/extract & /v1/agent)
             OpenAI-Compatible LLM Endpoint
       (NVIDIA NIM default or any OpenAI-format API)
```

### Components Included

- [`rust-core/`](./rust-core): Axum + Tokio API gateway (`v2.0.0`), job state store (`DashMap`), async BFS crawler, native `rustls` fallback scraper, optional Bearer auth middleware, and LLM/heuristic extractor.
- [`go-engine/`](./go-engine): Go HTTP/2 fetcher, `goquery` HTML cleaner, GitHub-Flavored Markdown converter, and concurrent DuckDuckGo search + worker pool.
- `start_hybrid_firecrawl.bat` & `start.ps1`: Local one-command launchers (Windows).
- [`docker-compose.yml`](./docker-compose.yml): Minimal two-container deployment with hard memory limits (`128M` for Go, `256M` for Rust).

*(Legacy Node.js, Playwright, RabbitMQ, Redis, PostgreSQL, and SearXNG services are intentionally omitted from this repository.)*

---

## Quick Start

### Option 1: Windows Portable Release (Zero Toolchain Required)

1. Download `firecrawl-hybrid-v1.0.0-win-x64.zip` from [GitHub Releases](../../releases).
2. Extract it to any folder.
3. *(Optional)* Copy `.env.example` to `.env` and add your `OPENAI_API_KEY` if you want LLM-powered `/v1/extract` and `/v1/agent`.
4. Run **`start_hybrid_firecrawl.bat`** (or `.\start.ps1` in PowerShell).
5. Verify health:
   ```powershell
   curl http://127.0.0.1:3002/health
   ```

### Option 2: Docker Compose

```bash
cp .env.example .env
docker compose up -d --build
```

The API gateway listens on **`http://localhost:3002`** (`go-engine` stays internal on port `8081`).

### Option 3: Build from Source

Prerequisites: **Go 1.26+** and **Rust (stable)** with Cargo.

```bash
# 1. Build Go engine
cd go-engine
go vet ./...
go build -ldflags="-s -w" -o go-engine .   # Windows: go-engine.exe

# 2. Build Rust API gateway
cd ../rust-core
cargo build --release                     # Output: target/release/firecrawl-rs(.exe)
```

Run both binaries:

```bash
# Terminal 1
./go-engine/go-engine

# Terminal 2
GO_ENGINE_URL=http://127.0.0.1:8081 ./rust-core/target/release/firecrawl-rs
```

---

## Configuration (`.env`)

Copy [`.env.example`](./.env.example) to `.env` in either the repository root or `rust-core/`:

| Variable | Default | Description |
| --- | --- | --- |
| `PORT` | `3002` | Public HTTP port for `firecrawl-rs` |
| `GO_ENGINE_PORT` | `8081` | Internal HTTP port for `go-engine` |
| `GO_ENGINE_URL` | `http://127.0.0.1:8081` | Base URL used by `rust-core` to reach `go-engine` |
| `OPENAI_API_KEY` | *(empty)* | Optional API key for `/extract` and `/agent` (falls back to deterministic heuristic extraction when unset) |
| `OPENAI_BASE_URL` | `https://integrate.api.nvidia.com/v1` | OpenAI-compatible chat completions endpoint |
| `MODEL_NAME` | `nvidia/nemotron-3-nano-30b` | Model identifier sent in `/chat/completions` requests |
| `TEST_API_KEY` | *(empty)* | Expected Bearer token when `ENFORCE_AUTH=true` |
| `ENFORCE_AUTH` | `false` | Require `Authorization: Bearer <token>` on all API routes except `/` and `/health` |

---

## API Reference

All Firecrawl-compatible endpoints are exposed under both **`/v1/...`** and **`/v2/...`**.

### Rust API Gateway (`:3002`)

| Method & Path | Description |
| --- | --- |
| `GET /` & `GET /health` | Service status (unauthenticated even when `ENFORCE_AUTH=true`) |
| `POST /v1/scrape` (`POST /v2/scrape`) | Scrape a single URL to `markdown`, `html`, `rawHtml`, `links`, and metadata |
| `POST /v1/map` (`POST /v2/map`) | Discover site URLs via sitemap + page link extraction |
| `POST /v1/crawl` (`POST /v2/crawl`) | Start an asynchronous BFS crawl (`limit` default `25`, `1..500`; `maxDepth` default `3`, `0..10`; `includePaths`, `excludePaths`, `allowExternalLinks`) |
| `GET /v1/crawl/:id` (`GET /v2/crawl/:id`) | Poll crawl job status and collected pages |
| `POST /v1/search` (`POST /v2/search`) | DuckDuckGo metasearch (limit up to `25`) with optional full-page Markdown scraping (`scrapeOptions`) |
| `POST /v1/extract` & `GET /v1/extract/:id` | Prompt + JSON-schema structured extraction (LLM up to `100,000` chars or heuristic fallback) |
| `POST /v1/agent` & `GET /v1/agent/:id` | Autonomous search + scrape + synthesis task with job status polling |

#### Example Requests

```bash
# Scrape a URL to clean Markdown
curl -X POST http://127.0.0.1:3002/v1/scrape \
  -H "Content-Type: application/json" \
  -d '{"url":"https://example.com","formats":["markdown","links"],"onlyMainContent":true}'

# Live Web Search + Optional Result Scraping
curl -X POST http://127.0.0.1:3002/v1/search \
  -H "Content-Type: application/json" \
  -d '{"query":"rust async web framework","limit":5}'

# Start Async BFS Crawl
curl -X POST http://127.0.0.1:3002/v1/crawl \
  -H "Content-Type: application/json" \
  -d '{"url":"https://example.com","limit":10,"maxDepth":2}'
```

### Internal Go Engine (`:8081`)

| Method & Path | Description |
| --- | --- |
| `GET /` & `GET /health` | Runtime health + Go heap `allocMB`, `sysMB`, and `numGoroutines` |
| `POST /convert` | Fast raw HTML -> GitHub-Flavored Markdown conversion (`goquery` + `html-to-markdown`, up to `150 MB` payload) |
| `POST /scrape` | HTTP/2 fetch (`50 MB` response cap) + clean Markdown, HTML, links, and metadata |
| `POST /search` | Concurrent DuckDuckGo search (`5` worker goroutines, `1..25` results) |

---

## Trade-offs vs Full Firecrawl

Because this implementation focuses strictly on lightweight footprint and native speed:
- **No headless Chromium / Playwright:** Pages requiring client-side JavaScript execution (SPAs rendered purely in browser JS) return their static HTML shell only.
- **In-memory job state:** Async `/crawl`, `/extract`, and `/agent` job IDs are stored in memory (`DashMap`), not persisted to PostgreSQL/Redis across restarts.

---

## License

Licensed under the **GNU Affero General Public License v3.0 (AGPL-3.0)** — see [LICENSE](./LICENSE).
