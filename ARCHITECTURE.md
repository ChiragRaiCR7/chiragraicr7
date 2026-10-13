# Architecture

Interactive diagram: open [`docs/architecture.html`](docs/architecture.html)
(source: `docs/architecture.json`, rendered with archify).

## Request flow

1. The browser loads static HTML/CSS/JS produced by Astro (`web/dist`), served by the Rust binary
   (`tower_http::ServeDir`) or by GitHub Pages.
2. `main.ts` probes `GET /api/health`. On success it reveals the search panel and the contact form.
   On a static host the probe 404s and the page stays fully usable without them.
3. `GET /api/search?q=` → FTS5 `MATCH` → bm25-ranked hits with snippets. Matched terms are wrapped
   in control characters (`\u0002`/`\u0003`), which the client turns into `<mark>` with `textContent`,
   so no server HTML is ever injected.
4. `POST /api/contact` (JSON) → validation → per-IP limiter → honeypot → `INSERT` into `messages`.

## Data

`content/resume.json` is the only content source. Astro imports it at build time; the server embeds
it with `include_str!` and rebuilds the `docs` FTS5 table in one transaction on every boot. The index
is derived data, so there are no content migrations. `messages` is the only durable table; its
`CHECK` constraints repeat the API's length rules at the lowest layer.

Schema changes: `CREATE ... IF NOT EXISTS` in `server/src/db.rs`. A change to `messages` needs an
explicit migration (add `PRAGMA user_version` bookkeeping when that first happens).

## API

| Method | Path | Success | Errors |
|---|---|---|---|
| GET | `/api/health` | `200 {"status":"ok"}` | `500` if SQLite is unreachable |
| GET | `/api/search?q=` | `200 {"query","hits":[{anchor,section,title,snippet}]}` | `422` query > 200 chars |
| POST | `/api/contact` | `201 {"ok":true}` | `422 {"error","field"}` invalid field, `400`/`415`/`422` unparseable body, `413` body > 16 KiB, `429` rate limited; all errors are `{"error": ...}` JSON |

## Security

- CSP `default-src 'self'` with no `unsafe-inline`: Astro is configured to emit only external
  assets, and fonts are self-hosted. Plus `nosniff`, `frame-ancestors 'none'`, a referrer policy and
  a permissions policy.
- FTS5 input: every term is quoted, so FTS5 operators typed by visitors have no effect (tested).
  All SQL is parameterized.
- Contact: a JSON-only endpoint (cross-site HTML forms cannot send `application/json` without a CORS
  preflight, and no CORS is enabled), a 16 KiB body limit, 5 messages/hour/IP, a honeypot, trimmed
  and length-checked fields, and DB `CHECK` constraints. No IPs are stored.
- Container runs as a non-root user; the database lives on a volume.

## Known limits (deliberate)

- **Rate limiter is per process and keyed on the TCP peer.** Behind a reverse proxy every visitor
  shares the proxy's IP. Before deploying behind a proxy, read the client IP from a trusted
  `X-Forwarded-For` hop.
- One SQLite connection behind a mutex. That is enough for portfolio traffic; switch to a pool if
  writes contend.
- No email notification for new messages; read them with `portfolio-server messages`.
- No LLM generation: "Ask this résumé" is retrieval only, which needs no API key, has no per-query
  cost, and has no hallucination risk. Adding generation would need a provider key, a spend cap and
  abuse controls.
