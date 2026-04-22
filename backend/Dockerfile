# ---- Build Stage ----
FROM rust:slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies: download crates first without compiling
COPY Cargo.toml Cargo.lock ./
RUN cargo fetch

# Copy the real source code and build
COPY src/ src/
RUN cargo build --release

# ---- Runtime Stage ----
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary
COPY --from=builder /app/target/release/x-rust /app/x-rust

# Copy config and migrations (migrations run on startup via CARGO_MANIFEST_DIR)
COPY config/ /app/config/
COPY migrations/ /app/migrations/

# Create data directory for SQLite
RUN mkdir -p /app/data

EXPOSE 3000

# Config can be overridden via APP_ prefixed env vars.
# Nested keys use __ (double underscore) as separator:
#   APP_SERVER__HOST=0.0.0.0
#   APP_SERVER__PORT=3000
#   APP_DATABASE_URL=sqlite:data/users.db?mode=rwc
#   APP_REDIS__HOST=redis
#   APP_REDIS__PORT=6379
#   APP_REDIS__PASSWORD=123456
#   APP_JWT_SECRET=your-secret-key
ENV APP_SERVER__HOST=0.0.0.0
ENV APP_SERVER__PORT=3000
ENV APP_DATABASE_URL=sqlite:data/users.db?mode=rwc

CMD ["/app/x-rust"]
