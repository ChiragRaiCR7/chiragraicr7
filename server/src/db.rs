//! SQLite storage: the full-text index over the résumé and the contact inbox.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// Résumé content, compiled into the binary so the server and the static site
/// always ship the same facts.
pub const RESUME_JSON: &str = include_str!("../../content/resume.json");

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
CREATE TABLE IF NOT EXISTS messages (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL CHECK (length(name)  BETWEEN 1 AND 100),
    email      TEXT NOT NULL CHECK (length(email) BETWEEN 3 AND 254),
    body       TEXT NOT NULL CHECK (length(body)  BETWEEN 10 AND 5000),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE VIRTUAL TABLE IF NOT EXISTS docs USING fts5(
    anchor UNINDEXED, section UNINDEXED, title, body,
    tokenize = 'unicode61 remove_diacritics 2'
);
";

/// Markers wrapped around matched terms in search snippets. Control characters
/// cannot appear in résumé text, so the client can split on them and render
/// with textContent instead of trusting HTML.
pub const MARK_START: char = '\u{2}';
pub const MARK_END: char = '\u{3}';

const MAX_QUERY_TERMS: usize = 8;
const MAX_HITS: u32 = 6;

#[derive(Deserialize)]
pub struct Resume {
    pub profile: Profile,
    pub experience: Vec<Experience>,
    pub projects: Vec<Project>,
    pub skills: Vec<SkillGroup>,
    pub education: Vec<Education>,
    pub certifications: Vec<Certification>,
}

#[derive(Deserialize)]
pub struct Profile {
    pub name: String,
    pub summary: String,
}

#[derive(Deserialize)]
pub struct Experience {
    pub role: String,
    pub company: String,
    pub points: Vec<String>,
}

#[derive(Deserialize)]
pub struct Project {
    pub slug: String,
    pub name: String,
    pub tagline: String,
    pub description: String,
    pub stack: Vec<String>,
}

#[derive(Deserialize)]
pub struct SkillGroup {
    pub group: String,
    pub items: Vec<String>,
}

#[derive(Deserialize)]
pub struct Education {
    pub degree: String,
    pub school: String,
    pub detail: String,
}

#[derive(Deserialize)]
pub struct Certification {
    pub name: String,
    pub issuer: String,
}

#[derive(Serialize, Debug)]
pub struct Hit {
    pub anchor: String,
    pub section: String,
    pub title: String,
    pub snippet: String,
}

#[derive(Serialize)]
pub struct Message {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub body: String,
    pub created_at: String,
}

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Rebuilds the search index from the résumé in one transaction. The index is
/// derived data, so a full rebuild on every boot keeps it exactly in sync.
pub fn reindex(conn: &mut Connection, resume: &Resume) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM docs", [])?;
    let mut rows = 0;
    {
        let mut insert =
            tx.prepare("INSERT INTO docs (anchor, section, title, body) VALUES (?1, ?2, ?3, ?4)")?;
        let mut add = |anchor: &str, section: &str, title: &str, body: &str| {
            rows += 1;
            insert
                .execute(params![anchor, section, title, body])
                .map(|_| ())
        };

        add(
            "about",
            "About",
            &resume.profile.name,
            &resume.profile.summary,
        )?;
        for job in &resume.experience {
            let title = format!("{} · {}", job.role, job.company);
            for point in &job.points {
                add("experience", "Experience", &title, point)?;
            }
        }
        for project in &resume.projects {
            let anchor = format!("project-{}", project.slug);
            let title = format!("{} — {}", project.name, project.tagline);
            let body = format!(
                "{} Stack: {}.",
                project.description,
                project.stack.join(", ")
            );
            add(&anchor, "Projects", &title, &body)?;
        }
        for group in &resume.skills {
            add("skills", "Skills", &group.group, &group.items.join(", "))?;
        }
        for edu in &resume.education {
            add(
                "education",
                "Education",
                &edu.degree,
                &format!("{}. {}", edu.school, edu.detail),
            )?;
        }
        for cert in &resume.certifications {
            add("education", "Certification", &cert.name, &cert.issuer)?;
        }
    }
    tx.commit()?;
    Ok(rows)
}

/// Turns free text into a safe FTS5 query: every term is quoted (so FTS5
/// operators in user input are inert) and prefix-matched so search works while
/// the visitor is still typing (no stemmer: a stemmed index misses prefixes
/// like "orchestrat"), joined with OR so
/// bm25 ranks passages that match more terms higher.
fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(MAX_QUERY_TERMS)
        .map(|t| format!("\"{}\"*", t.to_lowercase()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

pub fn search(conn: &Connection, input: &str) -> rusqlite::Result<Vec<Hit>> {
    let Some(query) = fts_query(input) else {
        return Ok(Vec::new());
    };
    // Title matches weigh 4x body matches; the UNINDEXED columns get weight 0.
    let mut stmt = conn.prepare_cached(
        "SELECT anchor, section, title, snippet(docs, 3, ?2, ?3, '…', 18)
         FROM docs WHERE docs MATCH ?1
         ORDER BY bm25(docs, 0.0, 0.0, 4.0, 1.0) LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        params![
            query,
            MARK_START.to_string(),
            MARK_END.to_string(),
            MAX_HITS
        ],
        |r| {
            Ok(Hit {
                anchor: r.get(0)?,
                section: r.get(1)?,
                title: r.get(2)?,
                snippet: r.get(3)?,
            })
        },
    )?;
    rows.collect()
}

pub fn insert_message(
    conn: &Connection,
    name: &str,
    email: &str,
    body: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO messages (name, email, body) VALUES (?1, ?2, ?3)",
        params![name, email, body],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_messages(conn: &Connection) -> rusqlite::Result<Vec<Message>> {
    let mut stmt =
        conn.prepare("SELECT id, name, email, body, created_at FROM messages ORDER BY id DESC")?;
    let rows = stmt.query_map([], |r| {
        Ok(Message {
            id: r.get(0)?,
            name: r.get(1)?,
            email: r.get(2)?,
            body: r.get(3)?,
            created_at: r.get(4)?,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexed() -> Connection {
        let mut conn = open(":memory:").unwrap();
        let resume: Resume = serde_json::from_str(RESUME_JSON).unwrap();
        assert!(reindex(&mut conn, &resume).unwrap() > 10);
        conn
    }

    #[test]
    fn reindex_is_idempotent() {
        let mut conn = indexed();
        let resume: Resume = serde_json::from_str(RESUME_JSON).unwrap();
        let first: i64 = conn
            .query_row("SELECT count(*) FROM docs", [], |r| r.get(0))
            .unwrap();
        reindex(&mut conn, &resume).unwrap();
        let second: i64 = conn
            .query_row("SELECT count(*) FROM docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn search_finds_and_marks_terms() {
        let hits = search(&indexed(), "hallucination").unwrap();
        assert_eq!(hits[0].anchor, "experience");
        assert!(hits[0].snippet.contains(MARK_START));
    }

    #[test]
    fn search_matches_partial_words() {
        let hits = search(&indexed(), "orchestrat").unwrap();
        assert!(!hits.is_empty());
        let hits = search(&indexed(), "retr").unwrap();
        assert!(hits.iter().any(|h| h.anchor == "project-autoresolve"));
    }

    #[test]
    fn fts_operators_in_input_are_inert() {
        let conn = indexed();
        for input in [
            "\"", "NEAR(", "a AND", "*", "col:rag", "-rag", "^", "))((", "😀 rag",
        ] {
            assert!(search(&conn, input).is_ok(), "query {input:?} errored");
        }
        assert!(search(&conn, "   ").unwrap().is_empty());
    }

    #[test]
    fn schema_rejects_out_of_range_rows() {
        let conn = indexed();
        assert!(insert_message(&conn, "", "a@b.co", "long enough body").is_err());
        assert!(insert_message(&conn, "Ana", "a@b.co", "long enough body").is_ok());
        assert_eq!(list_messages(&conn).unwrap().len(), 1);
    }
}
