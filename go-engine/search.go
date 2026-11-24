package main

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/PuerkitoBio/goquery"
	"github.com/rs/zerolog/log"
)

const (
	defaultSearchLimit      = 5
	maxSearchLimit          = 25
	searchWorkerConcurrency = 5
	searchScrapeTimeoutMs   = 15000
)

// SearchEngine performs native concurrent DuckDuckGo metasearch and parallel page scraping
type SearchEngine struct {
	scraper *Scraper
}

// NewSearchEngine creates a new SearchEngine backed by Scraper
func NewSearchEngine(scraper *Scraper) *SearchEngine {
	return &SearchEngine{
		scraper: scraper,
	}
}

// SearchRequest represents the JSON payload for POST /search
type SearchRequest struct {
	Query         string `json:"query"`
	Limit         int    `json:"limit"`
	ScrapeResults bool   `json:"scrapeResults"`
}

// SearchResultItem represents a single web search result (with optional scraped Markdown)
type SearchResultItem struct {
	Title       string   `json:"title"`
	URL         string   `json:"url"`
	Snippet     string   `json:"snippet"`
	Description string   `json:"description"`
	Markdown    string   `json:"markdown,omitempty"`
	Links       []string `json:"links,omitempty"`
}

// SearchResponse represents the JSON response for POST /search
type SearchResponse struct {
	Success   bool               `json:"success"`
	Query     string             `json:"query"`
	Results   []SearchResultItem `json:"results"`
	Count     int                `json:"count"`
	ElapsedMs int64              `json:"elapsedMs"`
	Error     string             `json:"error,omitempty"`
}

// decodeDuckDuckGoURL extracts the destination URL from DuckDuckGo's redirect links
func decodeDuckDuckGoURL(rawHref string) string {
	rawHref = strings.TrimSpace(rawHref)
	if rawHref == "" {
		return ""
	}
	if strings.HasPrefix(rawHref, "//") {
		rawHref = "https:" + rawHref
	}
	parsed, err := url.Parse(rawHref)
	if err != nil {
		return rawHref
	}
	if strings.Contains(parsed.Host, "duckduckgo.com") && parsed.Path == "/l/" {
		if uddg := parsed.Query().Get("uddg"); uddg != "" {
			if decoded, err := url.QueryUnescape(uddg); err == nil {
				return decoded
			}
			return uddg
		}
	}
	return rawHref
}

// queryDuckDuckGoHTML fetches and parses DuckDuckGo HTML search results
func (se *SearchEngine) queryDuckDuckGoHTML(ctx context.Context, query string, limit int) ([]SearchResultItem, error) {
	ddgURL := fmt.Sprintf("https://html.duckduckgo.com/html/?q=%s", url.QueryEscape(query))

	headers := map[string]string{
		"Referer": "https://html.duckduckgo.com/",
	}

	rawHTML, statusCode, _, err := se.scraper.FetchRaw(ctx, ddgURL, headers, 15000)
	if err != nil {
		return nil, err
	}
	if statusCode >= 400 {
		return nil, fmt.Errorf("duckduckgo returned status %d", statusCode)
	}

	doc, err := goquery.NewDocumentFromReader(strings.NewReader(rawHTML))
	if err != nil {
		return nil, err
	}

	results := make([]SearchResultItem, 0, limit)
	seen := make(map[string]struct{})

	doc.Find(".result").EachWithBreak(func(_ int, s *goquery.Selection) bool {
		if s.HasClass("result--ad") {
			return true
		}

		titleSel := s.Find(".result__a").First()
		title := strings.TrimSpace(titleSel.Text())
		rawHref, exists := titleSel.Attr("href")
		if !exists || title == "" {
			return true
		}

		destURL := decodeDuckDuckGoURL(rawHref)
		if !strings.HasPrefix(destURL, "http://") && !strings.HasPrefix(destURL, "https://") {
			return true
		}
		if strings.Contains(destURL, "duckduckgo.com/y.js") {
			return true
		}
		if _, dup := seen[destURL]; dup {
			return true
		}
		seen[destURL] = struct{}{}

		snippet := strings.TrimSpace(s.Find(".result__snippet").First().Text())

		results = append(results, SearchResultItem{
			Title:       title,
			URL:         destURL,
			Snippet:     snippet,
			Description: snippet,
		})

		return len(results) < limit
	})

	return results, nil
}

// Search executes concurrent search and optional worker-pool scraping of top N URLs
func (se *SearchEngine) Search(ctx context.Context, req SearchRequest) (*SearchResponse, error) {
	start := time.Now()

	limit := req.Limit
	if limit <= 0 {
		limit = defaultSearchLimit
	}
	if limit > maxSearchLimit {
		limit = maxSearchLimit
	}

	// Execute search in a goroutine channel so context cancellation or multi-page expansion is seamless
	type ddgOut struct {
		items []SearchResultItem
		err   error
	}
	ch := make(chan ddgOut, 1)
	go func() {
		items, err := se.queryDuckDuckGoHTML(ctx, req.Query, limit)
		ch <- ddgOut{items: items, err: err}
	}()

	var results []SearchResultItem
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case out := <-ch:
		if out.err != nil {
			return &SearchResponse{
				Success:   false,
				Query:     req.Query,
				Results:   []SearchResultItem{},
				ElapsedMs: time.Since(start).Milliseconds(),
				Error:     out.err.Error(),
			}, out.err
		}
		results = out.items
	}

	if results == nil {
		results = []SearchResultItem{}
	}

	// If scrapeResults is true, concurrently scrape the top N URLs using a worker pool of 5
	if req.ScrapeResults && len(results) > 0 {
		var wg sync.WaitGroup
		sem := make(chan struct{}, searchWorkerConcurrency)

		for i := range results {
			wg.Add(1)
			go func(idx int) {
				defer wg.Done()
				sem <- struct{}{}
				defer func() { <-sem }()

				scrapeResp, err := se.scraper.ScrapeURL(ctx, ScrapeRequest{
					URL:             results[idx].URL,
					OnlyMainContent: true,
					Timeout:         searchScrapeTimeoutMs,
				})
				if err == nil && scrapeResp != nil && scrapeResp.Markdown != "" {
					results[idx].Markdown = scrapeResp.Markdown
					results[idx].Links = scrapeResp.Links
					if results[idx].Title == "" && scrapeResp.Title != "" {
						results[idx].Title = scrapeResp.Title
					}
					if results[idx].Description == "" && scrapeResp.Description != "" {
						results[idx].Description = scrapeResp.Description
						results[idx].Snippet = scrapeResp.Description
					}
				}
			}(i)
		}
		wg.Wait()
	}

	return &SearchResponse{
		Success:   true,
		Query:     req.Query,
		Results:   results,
		Count:     len(results),
		ElapsedMs: time.Since(start).Milliseconds(),
	}, nil
}

// HandleSearch handles POST /search requests
func (se *SearchEngine) HandleSearch(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		writeJSONError(w, "Failed to read request body", err.Error(), http.StatusBadRequest)
		return
	}

	req := SearchRequest{
		Limit: defaultSearchLimit,
	}
	if err := json.Unmarshal(body, &req); err != nil {
		writeJSONError(w, "Invalid JSON in request body", err.Error(), http.StatusBadRequest)
		return
	}

	req.Query = strings.TrimSpace(req.Query)
	if req.Query == "" {
		writeJSONError(w, "Query field is required", "The 'query' field cannot be empty", http.StatusBadRequest)
		return
	}

	resp, err := se.Search(r.Context(), req)
	if err != nil {
		log.Warn().Err(err).Str("query", req.Query).Msg("Search failed")
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadGateway)
		json.NewEncoder(w).Encode(resp)
		return
	}

	log.Info().
		Str("query", req.Query).
		Int("count", resp.Count).
		Bool("scraped", req.ScrapeResults).
		Int64("elapsed_ms", resp.ElapsedMs).
		Msg("Search completed")

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(resp)
}
