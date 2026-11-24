# Stage 1: Build static Go binary
FROM golang:1.26-alpine AS builder

WORKDIR /app

RUN apk add --no-cache git ca-certificates

COPY go.mod go.sum ./
RUN go mod download

COPY *.go ./
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o /go-engine .

# Stage 2: Minimal Alpine runtime (< 15 MB image)
FROM alpine:3.20

RUN apk add --no-cache ca-certificates tzdata && \
    adduser -D -g '' appuser

WORKDIR /app
COPY --from=builder /go-engine /app/go-engine

USER appuser

ENV GO_ENGINE_PORT=8081
EXPOSE 8081

HEALTHCHECK --interval=15s --timeout=3s --start-period=2s --retries=3 \
  CMD wget -qO- http://127.0.0.1:8081/health || exit 1

ENTRYPOINT ["/app/go-engine"]
