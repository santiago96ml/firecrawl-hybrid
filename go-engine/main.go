package main

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"os/signal"
	"runtime"
	"syscall"
	"time"

	"github.com/gorilla/mux"
	"github.com/rs/zerolog"
	"github.com/rs/zerolog/log"
)

const (
	defaultPort            = "8081"
	defaultShutdownTimeout = 30 * time.Second
	defaultReadTimeout     = 2 * time.Minute
	defaultWriteTimeout    = 2 * time.Minute
	maxUploadSize          = 150 * 1024 * 1024 // 150 MB
)

// EngineServer wires Converter, Scraper, and SearchEngine HTTP handlers
type EngineServer struct {
	converter    *Converter
	scraper      *Scraper
	searchEngine *SearchEngine
}

// HealthCheckResponse represents GET /health telemetry
type HealthCheckResponse struct {
	Status        string    `json:"status"`
	Service       string    `json:"service"`
	Timestamp     time.Time `json:"timestamp"`
	AllocMB       float64   `json:"allocMB"`
	SysMB         float64   `json:"sysMB"`
	NumGoroutines int       `json:"numGoroutines"`
}

// ConvertRequest represents POST /convert request body
type ConvertRequest struct {
	HTML            string   `json:"html"`
	OnlyMainContent bool     `json:"onlyMainContent"`
	IncludeTags     []string `json:"includeTags,omitempty"`
	ExcludeTags     []string `json:"excludeTags,omitempty"`
	BaseURL         string   `json:"baseUrl,omitempty"`
}

// ConvertResponse represents POST /convert response body (backward compatible with go-html-to-md-service)
type ConvertResponse struct {
	Markdown    string   `json:"markdown"`
	Title       string   `json:"title,omitempty"`
	Description string   `json:"description,omitempty"`
	Links       []string `json:"links,omitempty"`
	Success     bool     `json:"success"`
}

// ErrorResponse represents a standard JSON error response
type ErrorResponse struct {
	Error   string `json:"error"`
	Details string `json:"details,omitempty"`
	Success bool   `json:"success"`
}

func writeJSONError(w http.ResponseWriter, message string, details string, statusCode int) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(statusCode)
	json.NewEncoder(w).Encode(ErrorResponse{
		Error:   message,
		Details: details,
		Success: false,
	})
}

// HandleHealth serves GET /health with live Go runtime memory and goroutine stats
func (es *EngineServer) HandleHealth(w http.ResponseWriter, r *http.Request) {
	var mem runtime.MemStats
	runtime.ReadMemStats(&mem)

	resp := HealthCheckResponse{
		Status:        "healthy",
		Service:       "firecrawl-go-engine",
		Timestamp:     time.Now().UTC(),
		AllocMB:       float64(mem.Alloc) / (1024.0 * 1024.0),
		SysMB:         float64(mem.Sys) / (1024.0 * 1024.0),
		NumGoroutines: runtime.NumGoroutine(),
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(resp)
}

// HandleConvert serves POST /convert (backward-compatible HTML-to-Markdown endpoint)
func (es *EngineServer) HandleConvert(w http.ResponseWriter, r *http.Request) {
	start := time.Now()

	body, err := io.ReadAll(r.Body)
	if err != nil {
		writeJSONError(w, "Failed to read request body", err.Error(), http.StatusBadRequest)
		return
	}

	var req ConvertRequest
	if err := json.Unmarshal(body, &req); err != nil {
		writeJSONError(w, "Invalid JSON in request body", err.Error(), http.StatusBadRequest)
		return
	}

	if req.HTML == "" {
		writeJSONError(w, "HTML field is required", "The 'html' field cannot be empty", http.StatusBadRequest)
		return
	}

	var markdown, title, description string
	var links []string

	if req.OnlyMainContent || len(req.IncludeTags) > 0 || len(req.ExcludeTags) > 0 || req.BaseURL != "" {
		markdown, _, title, description, links, err = es.converter.CleanAndConvertHTML(
			req.HTML,
			req.OnlyMainContent,
			req.IncludeTags,
			req.ExcludeTags,
			req.BaseURL,
		)
	} else {
		markdown, err = es.converter.ConvertHTMLToMarkdown(req.HTML)
	}

	if err != nil {
		log.Error().Err(err).Msg("Failed to convert HTML to Markdown")
		writeJSONError(w, "Failed to convert HTML to Markdown", err.Error(), http.StatusInternalServerError)
		return
	}

	log.Info().
		Dur("duration_ms", time.Since(start)).
		Int("input_size", len(req.HTML)).
		Int("output_size", len(markdown)).
		Msg("HTML to Markdown conversion completed")

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(ConvertResponse{
		Markdown:    markdown,
		Title:       title,
		Description: description,
		Links:       links,
		Success:     true,
	})
}

// HandleIndex serves GET / service overview
func (es *EngineServer) HandleIndex(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(map[string]interface{}{
		"service":     "firecrawl-go-engine",
		"version":     "2.0.0",
		"description": "High-performance Go HTTP/2 scraper, DOM cleaner, GFM Markdown converter, and concurrent metasearch engine",
		"endpoints": []string{
			"GET  /health  - Runtime health, memory, and goroutine telemetry",
			"POST /convert - Convert HTML to GitHub-Flavored Markdown",
			"POST /scrape  - Fast HTTP/2 + TLS scrape, DOM clean, metadata, links & Markdown",
			"POST /search  - Concurrent web metasearch + optional parallel page scraping",
		},
	})
}

func main() {
	zerolog.TimeFieldFormat = zerolog.TimeFormatUnix
	env := os.Getenv("ENV")
	if env == "production" {
		zerolog.SetGlobalLevel(zerolog.InfoLevel)
	} else {
		log.Logger = log.Output(zerolog.ConsoleWriter{
			Out:        os.Stdout,
			TimeFormat: time.RFC3339,
		})
	}

	port := os.Getenv("GO_ENGINE_PORT")
	if port == "" {
		port = os.Getenv("PORT")
	}
	if port == "" {
		port = defaultPort
	}

	converter := NewConverter()
	scraper := NewScraper(converter)
	searchEngine := NewSearchEngine(scraper)

	server := &EngineServer{
		converter:    converter,
		scraper:      scraper,
		searchEngine: searchEngine,
	}

	router := mux.NewRouter()
	router.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			r.Body = http.MaxBytesReader(w, r.Body, maxUploadSize)
			next.ServeHTTP(w, r)
		})
	})

	router.HandleFunc("/", server.HandleIndex).Methods(http.MethodGet)
	router.HandleFunc("/health", server.HandleHealth).Methods(http.MethodGet)
	router.HandleFunc("/convert", server.HandleConvert).Methods(http.MethodPost)
	router.HandleFunc("/scrape", scraper.HandleScrape).Methods(http.MethodPost)
	router.HandleFunc("/search", searchEngine.HandleSearch).Methods(http.MethodPost)

	srv := &http.Server{
		Addr:         ":" + port,
		Handler:      router,
		ReadTimeout:  defaultReadTimeout,
		WriteTimeout: defaultWriteTimeout,
	}

	go func() {
		log.Info().
			Str("port", port).
			Str("service", "firecrawl-go-engine").
			Msg("Starting Firecrawl Go Engine")

		if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatal().Err(err).Msg("Failed to start Firecrawl Go Engine")
		}
	}()

	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)
	<-quit

	log.Info().Msg("Shutting down Firecrawl Go Engine...")

	ctx, cancel := context.WithTimeout(context.Background(), defaultShutdownTimeout)
	defer cancel()

	if err := srv.Shutdown(ctx); err != nil {
		log.Fatal().Err(err).Msg("Server forced to shutdown")
	}

	log.Info().Msg("Firecrawl Go Engine exited cleanly")
}
