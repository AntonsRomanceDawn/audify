# Build stage — compiles the single `audify` binary.
# SQLX_OFFLINE makes the build use the committed .sqlx cache (no DB needed).
FROM rust:1-bookworm AS builder
WORKDIR /app
ENV SQLX_OFFLINE=true
COPY . .
RUN cargo build --release --bin audify

# Runtime stage — slim image with ffmpeg (audio assembly) and CA certs (HTTPS).
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/audify /usr/local/bin/audify
ENV AUDIFY_OUTPUT_DIR=/data/output
EXPOSE 8080
# `serve` by default; compose overrides with `worker` / `migrate`.
ENTRYPOINT ["audify"]
CMD ["serve"]
