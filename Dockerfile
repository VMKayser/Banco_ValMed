# syntax=docker/dockerfile:1
#
# Banco ValMed (Rust). Contexto de construcción: esta carpeta.
#
#   docker build -t banco-valmed .
#
# Las dos etapas usan Debian 13 (trixie): el binario enlaza contra la glibc de la
# etapa de construcción y la imagen final debe tener la misma o una más nueva.

FROM rust:1.90-slim AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY openapi ./openapi
# Las cachés de BuildKit guardan las dependencias entre construcciones.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
    && cp target/release/banco-valmed /usr/local/bin/banco-valmed

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin banco
COPY --from=build /usr/local/bin/banco-valmed /usr/local/bin/banco-valmed
COPY static /app/static
WORKDIR /app
ENV BANK_STATIC_DIR=/app/static \
    BANK_HTTP_PORT=8080
USER 10001
# 8080: API, Swagger (/docs) y web app (/)
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=3 \
    CMD ["banco-valmed", "healthcheck"]
ENTRYPOINT ["banco-valmed"]
