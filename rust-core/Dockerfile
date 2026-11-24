# syntax=docker/dockerfile:1
FROM rust:1-slim-bookworm AS builder

WORKDIR /app

# Copy manifest files first for layer caching
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src

# Copy actual source and build release binary
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/firecrawl-rs /usr/local/bin/firecrawl-rs

ENV PORT=3002
ENV GO_ENGINE_URL=http://127.0.0.1:8081
ENV OPENAI_BASE_URL=https://integrate.api.nvidia.com/v1
ENV MODEL_NAME=nvidia/nemotron-3-nano-30b

EXPOSE 3002

CMD ["firecrawl-rs"]
