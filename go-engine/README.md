# Firecrawl Go Engine (`firecrawl-go-engine`)

Ultra-lightweight (~15 MB RAM, < 50 ms cold start) standalone Go engine that consolidates:
1. **Fast HTTP/2 + TLS Web Scraping (`POST /scrape`)**: Connection pooling, User-Agent rotation, transparent gzip/deflate decompression, DOM cleaning (`onlyMainContent`, `includeTags`, `excludeTags`), metadata extraction (`title`, `description`, `og:*`), deduplicated link resolution, and GitHub-Flavored Markdown generation.
2. **HTML-to-Markdown Conversion (`POST /convert`)**: Drop-in backward-compatible replacement for `go-html-to-md-service`.
3. **Concurrent Web Metasearch (`POST /search`)**: Native Go replacement for heavy Python SearXNG containers, with optional worker-pool parallel scraping of search results.
4. **Live Runtime Telemetry (`GET /health`)**: Reports memory allocation (`allocMB`, `sysMB`) and active goroutines (`numGoroutines`).

## Endpoints

### `GET /health`
Returns service status and live Go runtime memory telemetry.

### `POST /scrape`
```json
{
  "url": "https://example.com",
  "onlyMainContent": true,
  "includeTags": [],
  "excludeTags": [],
  "headers": {},
  "timeout": 30000
}
```

### `POST /search`
```json
{
  "query": "golang concurrency patterns",
  "limit": 5,
  "scrapeResults": true
}
```

### `POST /convert`
```json
{
  "html": "<h1>Hello</h1><p>World</p>"
}
```

## Build & Run

```bash
go build -ldflags="-s -w" -o go-engine.exe .
./go-engine.exe
```
