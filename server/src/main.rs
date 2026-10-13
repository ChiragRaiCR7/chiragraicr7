//! Portfolio server: serves the static Astro build and a small JSON API backed by SQLite.
//!
//! Usage:
//!   portfolio-server            run the HTTP server
//!   portfolio-server messages   print stored contact messages as JSON lines

mod api;
mod db;

use std::net::SocketAddr;
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();

    let db_path = env_or("DATABASE_PATH", "data/portfolio.db");
    if let Some(parent) = PathBuf::from(&db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut conn =
        db::open(&db_path).map_err(|e| format!("cannot open database at {db_path}: {e}"))?;

    if std::env::args().nth(1).as_deref() == Some("messages") {
        for message in db::list_messages(&conn)? {
            println!("{}", serde_json::to_string(&message)?);
        }
        return Ok(());
    }

    let resume: db::Resume = serde_json::from_str(db::RESUME_JSON)?;
    let indexed = db::reindex(&mut conn, &resume)?;
    tracing::info!(indexed, db = %db_path, "search index rebuilt");

    let static_dir = PathBuf::from(env_or("STATIC_DIR", "../web/dist"));
    if !static_dir.join("index.html").exists() {
        tracing::warn!(dir = %static_dir.display(), "static site not built; only /api routes will work");
    }

    let addr: SocketAddr =
        format!("{}:{}", env_or("HOST", "127.0.0.1"), env_or("PORT", "8080")).parse()?;
    let app = api::router(api::AppState::new(conn), &static_dir);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("shutting down");
}
