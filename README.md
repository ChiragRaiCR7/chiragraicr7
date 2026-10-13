# Chirag Rai — Portfolio

Personal site for **Chirag Rai**, Generative AI engineer (RAG, LangGraph agents, Python backends).

- Live (static): https://chiragraicr7.github.io/chiragraicr7/
- GitHub: https://github.com/ChiragRaiCR7 · LinkedIn: https://www.linkedin.com/in/chiragr7

## Stack

| Layer | Choice | Why |
|---|---|---|
| Content | `content/resume.json` | One source of truth for the page and the search index |
| Frontend | Astro 7 (Node 22), self-hosted fonts | Static HTML, no framework runtime; ~4 KB of progressive-enhancement JS |
| Backend | Rust, axum 0.8, tower-http | One small binary: serves the site and a JSON API |
| Database | SQLite (rusqlite, bundled) | FTS5 search index + contact inbox in one file; no server to run |

See [ARCHITECTURE.md](ARCHITECTURE.md) and the interactive diagram in [`docs/architecture.html`](docs/architecture.html).

## Features

- **Ask this résumé**: full-text search (SQLite FTS5, bm25) over every line of the résumé. Each result
  cites its section and jumps to the source passage.
- **Contact form**: validated client- and server-side, stored in SQLite, rate limited per IP, honeypot
  for bots. Read messages with `portfolio-server messages`.
- **Works without the backend**: on GitHub Pages the search box and form stay hidden, and email/LinkedIn
  links remain.

## Run locally

Requires Rust (stable), Node 22.

```bash
# 1. Build the site
cd web && npm ci && npm run build && cd ..

# 2. Run the server (serves web/dist + /api on http://127.0.0.1:8080)
cd server && cargo run
```

Environment variables (server): `HOST` (default `127.0.0.1`), `PORT` (`8080`),
`DATABASE_PATH` (`data/portfolio.db`), `STATIC_DIR` (`../web/dist`), `RUST_LOG`.

Frontend dev server with hot reload: `cd web && npm run dev` (API features stay off unless the Rust
server is reachable on the same origin).

### Docker

```bash
docker build -t portfolio .
docker run -p 8080:8080 -v portfolio-data:/data portfolio
docker run --rm -v portfolio-data:/data portfolio portfolio-server messages   # read the inbox
```

## Tests

```bash
cd server && cargo test && cargo clippy --all-targets -- -D warnings
cd web && npm run check && npm run build

# End-to-end (Playwright, desktop + mobile, full stack and static-only)
python3 -m venv .venv && .venv/bin/pip install -r tests/e2e/requirements.txt
.venv/bin/playwright install chromium
(cd server && cargo build) && tests/e2e/run.sh
```

## Editing content

Edit `content/resume.json`, then rebuild the site and the server. The server rebuilds the search index
on every start, so the page and search can never disagree.

## Deploy

- `main` → GitHub Pages (static build, `.github/workflows/static.yml`).
- Full stack: run the Docker image on any container host with a persistent volume at `/data`, behind
  HTTPS. See ARCHITECTURE.md for the reverse-proxy caveat.
