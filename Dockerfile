# Full stack in one image: Astro static build + Rust API, SQLite on a volume.
#   docker build -t portfolio .
#   docker run -p 8080:8080 -v portfolio-data:/data portfolio

FROM node:22-bookworm-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY content /src/content
COPY web ./
RUN npm run build

FROM rust:1-bookworm AS server
WORKDIR /src/server
COPY content /src/content
COPY server ./
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home /app app && mkdir -p /data && chown app /data
WORKDIR /app
COPY --from=server /src/server/target/release/portfolio-server /usr/local/bin/portfolio-server
COPY --from=web /src/web/dist /app/dist
USER app
ENV HOST=0.0.0.0 PORT=8080 STATIC_DIR=/app/dist DATABASE_PATH=/data/portfolio.db RUST_LOG=info
EXPOSE 8080
VOLUME ["/data"]
CMD ["portfolio-server"]
