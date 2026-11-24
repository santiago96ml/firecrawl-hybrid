package main

import (
	"compress/flate"
	"compress/gzip"
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"strings"
	"sync/atomic"
	"time"

	"github.com/rs/zerolog/log"
)

const (
	defaultScrapeTimeoutMs = 30000
	maxResponseBodyBytes   = 50 * 1024 * 1024 // 50 MB max HTML response body
)

var userAgents = []string{
	"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
	"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
	"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
	"Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0",
	"Mozilla/5.0 (Macintosh; Intel Mac OS X 14.7; rv:133.0) Gecko/20100101 Firefox/133.0",
	"Mozilla/5.0 (Macintosh; Intel Mac OS X 14_7_1) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
}

// Scraper performs high-concurrency HTTP/2 + TLS fetching and DOM conversion
type Scraper struct {
	client    *http.Client
	converter *Converter
	uaCounter uint64
}

// NewScraper initializes a Scraper with a pooled HTTP/2 + TLS transport
func NewScraper(converter *Converter) *Scraper {
	transport := &http.Transport{
		Proxy: http.ProxyFromEnvironment,
		DialContext: (&net.Dialer{
			Timeout:   10 * time.Second,
			KeepAlive: 30 * time.Second,
		}).DialContext,
		ForceAttemptHTTP2:     true,
		MaxIdleConns:          100,
		MaxIdleConnsPerHost:   20,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ExpectContinueTimeout: 1 * time.Second,
		TLSClientConfig: &tls.Config{
			MinVersion: tls.VersionTLS12,
		},
	}

	return &Scraper{
		client: &http.Client{
			Transport: transport,
			Timeout:   30 * time.Second,
		},
		converter: converter,
	}
}

// NextUserAgent rotates through realistic browser User-Agent strings
func (s *Scraper) NextUserAgent() string {
	idx := atomic.AddUint64(&s.uaCounter, 1)
	return userAgents[idx%uint64(len(userAgents))]
}

// ScrapeRequest represents the JSON payload for POST /scrape
type ScrapeRequest struct {
	URL             string            `json:"url"`
	OnlyMainContent bool              `json:"onlyMainContent"`
	IncludeTags     []string          `json:"includeTags"`
	ExcludeTags     []string          `json:"excludeTags"`
	Headers         map[string]string `json:"headers"`
	Timeout         int               `json:"timeout"`
}

// ScrapeResponse represents the JSON response for POST /scrape
type ScrapeResponse struct {
	Success     bool     `json:"success"`
	URL         string   `json:"url"`
	StatusCode  int      `json:"statusCode"`
	Title       string   `json:"title"`
	Description string   `json:"description"`
	Markdown    string   `json:"markdown"`
	HTML        string   `json:"html"`
	RawHTML     string   `json:"rawHtml"`
	Links       []string `json:"links"`
	ElapsedMs   int64    `json:"elapsedMs"`
	Error       string   `json:"error,omitempty"`
}

// FetchRaw performs an HTTP GET with browser headers, timeout, and transparent gzip/deflate decompression
func (s *Scraper) FetchRaw(ctx context.Context, targetURL string, headers map[string]string, timeoutMs int) (string, int, string, error) {
	if timeoutMs <= 0 {
		timeoutMs = defaultScrapeTimeoutMs
	}
	reqCtx, cancel := context.WithTimeout(ctx, time.Duration(timeoutMs)*time.Millisecond)
	defer cancel()

	req, err := http.NewRequestWithContext(reqCtx, http.MethodGet, targetURL, nil)
	if err != nil {
		return "", 0, targetURL, err
	}

	req.Header.Set("User-Agent", s.NextUserAgent())
	req.Header.Set("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8")
	req.Header.Set("Accept-Language", "en-US,en;q=0.9")
	req.Header.Set("Accept-Encoding", "gzip, deflate")
	req.Header.Set("Cache-Control", "no-cache")
	req.Header.Set("Upgrade-Insecure-Requests", "1")

	for k, v := range headers {
		if strings.TrimSpace(k) != "" {
			req.Header.Set(k, v)
		}
	}

	resp, err := s.client.Do(req)
	if err != nil {
		return "", 0, targetURL, err
	}
	defer resp.Body.Close()

	var reader io.Reader = resp.Body
	encoding := strings.ToLower(strings.TrimSpace(resp.Header.Get("Content-Encoding")))
	switch encoding {
	case "gzip":
		gzReader, gzErr := gzip.NewReader(resp.Body)
		if gzErr != nil {
			return "", resp.StatusCode, targetURL, fmt.Errorf("gzip decompression failed: %w", gzErr)
		}
		defer gzReader.Close()
		reader = gzReader
	case "deflate":
		flateReader := flate.NewReader(resp.Body)
		defer flateReader.Close()
		reader = flateReader
	}

	bodyBytes, err := io.ReadAll(io.LimitReader(reader, maxResponseBodyBytes))
	if err != nil {
		return "", resp.StatusCode, targetURL, err
	}

	finalURL := targetURL
	if resp.Request != nil && resp.Request.URL != nil {
		finalURL = resp.Request.URL.String()
	}

	return string(bodyBytes), resp.StatusCode, finalURL, nil
}

// ScrapeURL executes a full fetch, clean, metadata extraction, and Markdown conversion
func (s *Scraper) ScrapeURL(ctx context.Context, req ScrapeRequest) (*ScrapeResponse, error) {
	start := time.Now()

	rawHTML, statusCode, finalURL, err := s.FetchRaw(ctx, req.URL, req.Headers, req.Timeout)
	if err != nil {
		return &ScrapeResponse{
			Success:    false,
			URL:        req.URL,
			StatusCode: statusCode,
			Links:      []string{},
			ElapsedMs:  time.Since(start).Milliseconds(),
			Error:      err.Error(),
		}, err
	}

	mdOut, cleanedHTML, title, description, links, convErr := s.converter.CleanAndConvertHTML(
		rawHTML,
		req.OnlyMainContent,
		req.IncludeTags,
		req.ExcludeTags,
		finalURL,
	)
	if convErr != nil {
		return &ScrapeResponse{
			Success:    false,
			URL:        finalURL,
			StatusCode: statusCode,
			RawHTML:    rawHTML,
			Links:      []string{},
			ElapsedMs:  time.Since(start).Milliseconds(),
			Error:      convErr.Error(),
		}, convErr
	}

	return &ScrapeResponse{
		Success:     statusCode >= 200 && statusCode < 400,
		URL:         finalURL,
		StatusCode:  statusCode,
		Title:       title,
		Description: description,
		Markdown:    mdOut,
		HTML:        cleanedHTML,
		RawHTML:     rawHTML,
		Links:       links,
		ElapsedMs:   time.Since(start).Milliseconds(),
	}, nil
}

// HandleScrape handles POST /scrape requests
func (s *Scraper) HandleScrape(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		writeJSONError(w, "Failed to read request body", err.Error(), http.StatusBadRequest)
		return
	}

	// Default OnlyMainContent to true and Timeout to 30000ms before unmarshaling
	req := ScrapeRequest{
		OnlyMainContent: true,
		Timeout:         defaultScrapeTimeoutMs,
	}
	if err := json.Unmarshal(body, &req); err != nil {
		writeJSONError(w, "Invalid JSON in request body", err.Error(), http.StatusBadRequest)
		return
	}

	req.URL = strings.TrimSpace(req.URL)
	if req.URL == "" {
		writeJSONError(w, "URL field is required", "The 'url' field cannot be empty", http.StatusBadRequest)
		return
	}
	if !strings.HasPrefix(req.URL, "http://") && !strings.HasPrefix(req.URL, "https://") {
		req.URL = "https://" + req.URL
	}

	result, err := s.ScrapeURL(r.Context(), req)
	if err != nil {
		log.Warn().Err(err).Str("url", req.URL).Msg("Scrape failed")
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadGateway)
		json.NewEncoder(w).Encode(result)
		return
	}

	log.Info().
		Str("url", result.URL).
		Int("status", result.StatusCode).
		Int64("elapsed_ms", result.ElapsedMs).
		Int("md_len", len(result.Markdown)).
		Int("links", len(result.Links)).
		Msg("Scrape completed")

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(result)
}
