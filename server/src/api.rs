//! HTTP layer: JSON API, static site, security headers.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::db;

const MAX_BODY_BYTES: usize = 16 * 1024;
const MAX_QUERY_CHARS: usize = 200;
const CONTACT_LIMIT: usize = 5;
const CONTACT_WINDOW: Duration = Duration::from_secs(60 * 60);
const LIMITER_MAX_KEYS: usize = 10_000;

/// No inline scripts or third-party origins: Astro emits external, hashed
/// assets, and fonts are self-hosted.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
font-src 'self'; connect-src 'self'; form-action 'self'; base-uri 'none'; \
object-src 'none'; frame-ancestors 'none'";

#[derive(Clone)]
pub struct AppState {
    // ponytail: one connection behind a mutex; switch to a pool (r2d2) if
    // concurrent writes ever show up as latency. Portfolio traffic will not.
    db: Arc<Mutex<Connection>>,
    limiter: Arc<Mutex<RateLimiter>>,
}

impl AppState {
    pub fn new(conn: Connection) -> Self {
        Self {
            db: Arc::new(Mutex::new(conn)),
            limiter: Arc::new(Mutex::new(RateLimiter::default())),
        }
    }
}

/// Sliding-window limiter keyed by client IP.
// ponytail: process-local; a multi-instance deploy needs a shared store.
#[derive(Default)]
struct RateLimiter {
    hits: HashMap<IpAddr, VecDeque<Instant>>,
}

impl RateLimiter {
    fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self.hits.len() > LIMITER_MAX_KEYS {
            self.hits.retain(|_, q| {
                q.back()
                    .is_some_and(|t| now.duration_since(*t) < CONTACT_WINDOW)
            });
        }
        let q = self.hits.entry(ip).or_default();
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= CONTACT_WINDOW)
        {
            q.pop_front();
        }
        if q.len() >= CONTACT_LIMIT {
            return false;
        }
        q.push_back(now);
        true
    }
}

pub fn router(state: AppState, static_dir: &Path) -> Router {
    let site = ServeDir::new(static_dir)
        .append_index_html_on_directories(true)
        .not_found_service(ServeFile::new(static_dir.join("404.html")));

    let header = |name: HeaderName, value: &'static str| {
        SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
    };

    Router::new()
        .route("/api/health", get(health))
        .route("/api/search", get(search))
        .route("/api/contact", post(contact))
        .fallback_service(site)
        .with_state(state)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(CompressionLayer::new())
        .layer(header(header::CONTENT_SECURITY_POLICY, CSP))
        .layer(header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ))
        .layer(header(
            HeaderName::from_static("permissions-policy"),
            "camera=(), microphone=(), geolocation=(), interest-cohort=()",
        ))
        .layer(TraceLayer::new_for_http())
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// A failure already logged server-side; renders as a generic 500 so no
/// internal detail reaches the client.
struct InternalError;

impl IntoResponse for InternalError {
    fn into_response(self) -> Response {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong on our side. Please try again.",
        )
    }
}

fn internal(err: impl std::fmt::Display) -> InternalError {
    tracing::error!(%err, "request failed");
    InternalError
}

/// Runs a closure against the database off the async executor.
async fn with_db<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
) -> Result<T, InternalError> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&conn)
    })
    .await
    .map_err(internal)?
    .map_err(internal)
}

async fn health(State(state): State<AppState>) -> Response {
    match with_db(&state, |c| {
        c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
    })
    .await
    {
        Ok(_) => Json(json!({ "status": "ok" })).into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Deserialize)]
struct SearchParams {
    #[serde(default)]
    q: String,
}

async fn search(State(state): State<AppState>, Query(params): Query<SearchParams>) -> Response {
    if params.q.chars().count() > MAX_QUERY_CHARS {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Keep the question under 200 characters.",
        );
    }
    let q = params.q.clone();
    match with_db(&state, move |c| db::search(c, &q)).await {
        Ok(hits) => Json(json!({ "query": params.q, "hits": hits })).into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Deserialize)]
struct ContactForm {
    name: String,
    email: String,
    message: String,
    /// Honeypot: hidden from people, filled in by naive bots.
    #[serde(default)]
    website: String,
}

/// Returns (field, message) for the first invalid field.
fn validate(form: &ContactForm) -> Result<(), (&'static str, &'static str)> {
    let name = form.name.trim().chars().count();
    if !(1..=100).contains(&name) {
        return Err(("name", "Enter your name (up to 100 characters)."));
    }
    let email = form.email.trim();
    let plausible = (3..=254).contains(&email.chars().count())
        && !email.chars().any(char::is_whitespace)
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
        });
    if !plausible {
        return Err(("email", "Enter an email address like name@company.com."));
    }
    let body = form.message.trim().chars().count();
    if !(10..=5000).contains(&body) {
        return Err((
            "message",
            "Write a message between 10 and 5,000 characters.",
        ));
    }
    Ok(())
}

async fn contact(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Result<Json<ContactForm>, JsonRejection>,
) -> Response {
    // Keep the JSON error contract even for bodies axum cannot parse.
    let Json(form) = match body {
        Ok(json) => json,
        Err(rejection) => {
            return error(
                rejection.status(),
                "Send name, email and message as a JSON object.",
            );
        }
    };
    if let Err((field, message)) = validate(&form) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": message, "field": field })),
        )
            .into_response();
    }
    let allowed = state
        .limiter
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .allow(peer.ip(), Instant::now());
    if !allowed {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many messages from your network. Please email directly instead.",
        );
    }
    if !form.website.is_empty() {
        // Answer like a success so bots learn nothing; store nothing.
        tracing::info!("contact honeypot triggered");
        return (StatusCode::CREATED, Json(json!({ "ok": true }))).into_response();
    }
    let (name, email, body) = (
        form.name.trim().to_owned(),
        form.email.trim().to_owned(),
        form.message.trim().to_owned(),
    );
    match with_db(&state, move |c| db::insert_message(c, &name, &email, &body)).await {
        Ok(id) => {
            tracing::info!(message_id = id, "contact message stored");
            (StatusCode::CREATED, Json(json!({ "ok": true }))).into_response()
        }
        Err(err) => err.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> (Router, AppState) {
        let mut conn = db::open(":memory:").unwrap();
        let resume: db::Resume = serde_json::from_str(db::RESUME_JSON).unwrap();
        db::reindex(&mut conn, &resume).unwrap();
        let state = AppState::new(conn);
        let dir = std::env::temp_dir();
        let router = router(state.clone(), &dir)
            .layer(MockConnectInfo(SocketAddr::from(([203, 0, 113, 7], 4000))));
        (router, state)
    }

    async fn send(
        router: &Router,
        req: Request<Body>,
    ) -> (StatusCode, serde_json::Value, Response<()>) {
        let resp = router.clone().oneshot(req).await.unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (parts.status, value, Response::from_parts(parts, ()))
    }

    fn post_json(body: serde_json::Value) -> Request<Body> {
        Request::post("/api/contact")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn stored(state: &AppState) -> usize {
        db::list_messages(&state.db.lock().unwrap()).unwrap().len()
    }

    #[tokio::test]
    async fn health_and_security_headers() {
        let (router, _) = app();
        let (status, body, resp) = send(
            &router,
            Request::get("/api/health").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert!(
            resp.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
        assert_eq!(resp.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }

    #[tokio::test]
    async fn search_returns_cited_hits() {
        let (router, _) = app();
        let (status, body, _) = send(
            &router,
            Request::get("/api/search?q=MCP%20tenancy")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["hits"][0]["anchor"], "project-autoresolve");
    }

    #[tokio::test]
    async fn search_rejects_long_query() {
        let (router, _) = app();
        let uri = format!("/api/search?q={}", "a".repeat(201));
        let (status, _, _) = send(&router, Request::get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn contact_stores_valid_message() {
        let (router, state) = app();
        let (status, _, _) = send(
            &router,
            post_json(json!({"name": " Ana ", "email": "ana@corp.io", "message": "Let's talk about a role."})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let msgs = db::list_messages(&state.db.lock().unwrap()).unwrap();
        assert_eq!(msgs[0].name, "Ana");
    }

    #[tokio::test]
    async fn contact_names_invalid_field() {
        let (router, state) = app();
        let (status, body, _) = send(
            &router,
            post_json(json!({"name": "Ana", "email": "ana@corp", "message": "Long enough text"})),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "email");
        assert_eq!(stored(&state), 0);
    }

    #[tokio::test]
    async fn contact_honeypot_is_silently_dropped() {
        let (router, state) = app();
        let (status, _, _) = send(
            &router,
            post_json(json!({"name": "Bot", "email": "b@spam.io", "message": "Buy cheap things now", "website": "x"})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(stored(&state), 0);
    }

    #[tokio::test]
    async fn contact_is_rate_limited_per_ip() {
        let (router, state) = app();
        let msg =
            json!({"name": "Ana", "email": "ana@corp.io", "message": "Let's talk about a role."});
        for _ in 0..CONTACT_LIMIT {
            assert_eq!(
                send(&router, post_json(msg.clone())).await.0,
                StatusCode::CREATED
            );
        }
        assert_eq!(
            send(&router, post_json(msg)).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(stored(&state), CONTACT_LIMIT);
    }

    #[tokio::test]
    async fn contact_malformed_json_keeps_json_errors() {
        let (router, _) = app();
        let req = Request::post("/api/contact")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{bad"))
            .unwrap();
        let (status, body, _) = send(&router, req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].is_string());
        let (status, body, _) = send(&router, post_json(json!({"name": "Ana"}))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn contact_rejects_oversized_body() {
        let (router, _) = app();
        let big =
            json!({"name": "Ana", "email": "ana@corp.io", "message": "x".repeat(MAX_BODY_BYTES)});
        let (status, _, _) = send(&router, post_json(big)).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn limiter_window_slides() {
        let mut limiter = RateLimiter::default();
        let ip: IpAddr = [10, 0, 0, 1].into();
        let t0 = Instant::now();
        for _ in 0..CONTACT_LIMIT {
            assert!(limiter.allow(ip, t0));
        }
        assert!(!limiter.allow(ip, t0));
        assert!(limiter.allow(ip, t0 + CONTACT_WINDOW));
    }
}
