//! `cargo xtask contributors`: write `contributors/contributors.json` from git history and
//! `contributors/people.toml` (docs/contributors.md).
//!
//! Offline: each commit author is credited by the `people.toml` entry whose `emails` list holds
//! their address, else by their git author name. `Co-Authored-By` trailers from an AI model's
//! address count under `models`. Lists are sorted by commits, most first.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

/// Co-author addresses that identify an AI model, and the company behind it.
pub const MODEL_EMAILS: &[(&str, &str)] = &[("noreply@anthropic.com", "Anthropic"), ("noreply@openai.com", "OpenAI")];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Commit {
    pub author: String,
    pub email: String,
    /// UTC, `YYYY-MM-DDTHH:MM:SSZ`.
    pub date: String,
    pub co_authors: Vec<String>,
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub binary_changes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct People {
    /// email → login
    pub by_email: BTreeMap<String, String>,
    pub logins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub company: String,
    pub model: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    pub repo: String,
    pub branch: String,
    pub sha: String,
    pub date: String,
}

/// Header fields separated by U+001F, trailers by U+001D, records start with U+001E.
pub const LOG_FORMAT: &str = "--format=%x1e%an%x1f%ae%x1f%ad%x1f%(trailers:key=Co-authored-by,valueonly,separator=%x1d)";

pub fn parse_log(text: &str) -> Vec<Commit> {
    let mut out = Vec::new();
    for record in text.split('\u{1e}').skip(1) {
        let mut lines = record.lines();
        let Some(header) = lines.next() else { continue };
        let fields: Vec<&str> = header.split('\u{1f}').collect();
        let [author, email, date, trailers] = fields[..] else { continue };
        let mut c = Commit {
            author: author.trim().into(),
            email: email.trim().to_ascii_lowercase(),
            date: date.trim().into(),
            co_authors: trailers.split('\u{1d}').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect(),
            ..Commit::default()
        };
        for line in lines {
            let mut parts = line.splitn(3, '\t');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("-"), Some("-"), Some(_)) => c.binary_changes += 1,
                (Some(a), Some(d), Some(_)) => {
                    if let (Ok(a), Ok(d)) = (a.parse::<u64>(), d.parse::<u64>()) {
                        c.lines_added += a;
                        c.lines_deleted += d;
                    }
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

pub fn parse_people(toml: &str) -> People {
    let mut people = People::default();
    let mut current: Option<String> = None;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(login) = line.strip_prefix("[people.").and_then(|r| r.strip_suffix(']')) {
            let login = login.trim_matches('"').to_owned();
            people.logins.push(login.clone());
            current = Some(login);
            continue;
        }
        if line.starts_with('[') {
            current = None;
            continue;
        }
        let (Some(login), Some(list)) = (&current, line.strip_prefix("emails").map(str::trim_start).and_then(|r| r.strip_prefix('='))) else {
            continue;
        };
        for email in list.split('"').skip(1).step_by(2) {
            people.by_email.insert(email.trim().to_ascii_lowercase(), login.clone());
        }
    }
    people
}

pub fn model_of(co_author: &str) -> Option<Model> {
    let (name, rest) = co_author.split_once('<')?;
    let email = rest.split_once('>')?.0.trim().to_ascii_lowercase();
    let company = MODEL_EMAILS.iter().find(|(e, _)| *e == email)?.1;
    let name = name.trim();
    let (model, version) = match name.rsplit_once(' ') {
        Some((m, v)) if v.starts_with(|ch: char| ch.is_ascii_digit()) => (m.trim(), v),
        _ => (name, ""),
    };
    let slug = model.to_ascii_lowercase().replace(' ', "-");
    let id = if version.is_empty() {
        format!("{}/{slug}", company.to_ascii_lowercase())
    } else {
        format!("{}/{slug}/{version}", company.to_ascii_lowercase())
    };
    Some(Model { id, company: company.into(), model: model.into(), version: version.into() })
}

#[derive(Default)]
struct Tally {
    commits: u64,
    added: u64,
    deleted: u64,
    binary: u64,
    first: String,
    last: String,
}

impl Tally {
    fn add(&mut self, c: &Commit) {
        self.commits += 1;
        self.added += c.lines_added;
        self.deleted += c.lines_deleted;
        self.binary += c.binary_changes;
        if self.first.is_empty() || c.date < self.first {
            self.first = c.date.clone();
        }
        if c.date > self.last {
            self.last = c.date.clone();
        }
    }
}

pub fn report(commits: &[Commit], people: &People, head: &Head) -> Value {
    let mut by_login: BTreeMap<String, Tally> = BTreeMap::new();
    let mut by_model: BTreeMap<String, (Model, Tally)> = BTreeMap::new();
    for c in commits {
        let login = people.by_email.get(&c.email).cloned().unwrap_or_else(|| c.author.clone());
        by_login.entry(login).or_default().add(c);
        for m in c.co_authors.iter().filter_map(|a| model_of(a)) {
            by_model.entry(m.id.clone()).or_insert_with(|| (m, Tally::default())).1.add(c);
        }
    }
    let mut contributors: Vec<(String, Tally)> = by_login.into_iter().collect();
    contributors.sort_by(|a, b| b.1.commits.cmp(&a.1.commits).then_with(|| a.0.cmp(&b.0)));
    let mut models: Vec<(Model, Tally)> = by_model.into_values().collect();
    models.sort_by(|a, b| b.1.commits.cmp(&a.1.commits).then_with(|| a.0.id.cmp(&b.0.id)));
    json!({
        "schema": 1,
        "repo": head.repo,
        "branch": head.branch,
        "head": head.sha,
        "head_date": head.date,
        "commits": commits.len(),
        "contributors": contributors.iter().map(|(login, t)| json!({
            "login": login,
            "people_entry": people.logins.contains(login),
            "commits": t.commits,
            "lines_added": t.added,
            "lines_deleted": t.deleted,
            "binary_changes": t.binary,
            "first_commit": t.first,
            "last_commit": t.last,
        })).collect::<Vec<_>>(),
        "models": models.iter().map(|(m, t)| json!({
            "id": m.id,
            "company": m.company,
            "model": m.model,
            "version": m.version,
            "commits": t.commits,
            "lines_added": t.added,
            "lines_deleted": t.deleted,
        })).collect::<Vec<_>>(),
    })
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").current_dir(root).env("TZ", "UTC").args(args).output().map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn run(root: &Path) -> Result<(), String> {
    const DATE: &str = "--date=format-local:%Y-%m-%dT%H:%M:%SZ";
    let sha = git(root, &["rev-parse", "HEAD"]).map_err(|_| "no commits yet: nothing to credit".to_string())?;
    let head = Head {
        repo: "KasparRosin/dbee".into(),
        branch: git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().into(),
        sha: sha.trim().into(),
        date: git(root, &["log", "-1", DATE, "--format=%ad"])?.trim().into(),
    };
    let log = git(root, &["log", "--no-merges", DATE, LOG_FORMAT, "--numstat"])?;
    let people = std::fs::read_to_string(root.join("contributors/people.toml")).unwrap_or_default();
    let value = report(&parse_log(&log), &parse_people(&people), &head);
    let text = serde_json::to_string_pretty(&value).map_err(|e| format!("contributors.json: {e}"))? + "\n";
    std::fs::write(root.join("contributors/contributors.json"), text).map_err(|e| format!("write contributors.json: {e}"))?;
    println!("contributors/contributors.json: {} commits", value["commits"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\u{1e}Kaspar Rosin\u{1f}kaspar@rosin.dev\u{1f}2026-10-16T09:00:00Z\u{1f}Claude Opus 5.5 <noreply@anthropic.com>\n\
\n\
10\t2\tsrc/a.rs\n\
-\t-\tassets/i.png\n\
\u{1e}Kaspar Rosin\u{1f}kaspar@rosin.dev\u{1f}2026-10-15T09:00:00Z\u{1f}\n\
\n\
5\t0\tREADME.md\n\
\u{1e}Ada\u{1f}ada@example.org\u{1f}2026-10-15T08:00:00Z\u{1f}Claude Opus 5.5 <noreply@anthropic.com>\u{1d}Bob <bob@example.org>\n\
\n\
1\t1\tdocs/x.md\n";

    #[test]
    fn parses_commits_numstat_and_trailers() {
        let c = parse_log(LOG);
        assert_eq!(c.len(), 3);
        assert_eq!(
            c[0],
            Commit {
                author: "Kaspar Rosin".into(),
                email: "kaspar@rosin.dev".into(),
                date: "2026-10-16T09:00:00Z".into(),
                co_authors: vec!["Claude Opus 5.5 <noreply@anthropic.com>".into()],
                lines_added: 10,
                lines_deleted: 2,
                binary_changes: 1,
            }
        );
        assert!(c[1].co_authors.is_empty());
        assert_eq!(c[2].co_authors.len(), 2);
    }

    #[test]
    fn malformed_log_lines_are_skipped_not_fatal() {
        let c = parse_log("garbage\n\u{1e}only-a-name\n\u{1e}A\u{1f}a@x\u{1f}2026-01-01T00:00:00Z\u{1f}\nx\ty\tz\n");
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].lines_added, c[0].binary_changes), (0, 0));
    }

    #[test]
    fn people_by_email() {
        let p = parse_people(
            "# comment\n[people.KasparRosin]\nreal_name = \"Kaspar Rosin\"\nemails = [\"kaspar@rosin.dev\", \"k@example.org\"]\n\n[people.ada]\nsync_github_name = true\n",
        );
        assert_eq!(p.by_email.get("kaspar@rosin.dev").map(String::as_str), Some("KasparRosin"));
        assert_eq!(p.by_email.get("k@example.org").map(String::as_str), Some("KasparRosin"));
        assert_eq!(p.logins, vec!["KasparRosin".to_string(), "ada".into()]);
    }

    #[test]
    fn models_from_known_addresses_only() {
        assert_eq!(
            model_of("Claude Opus 5.5 <noreply@anthropic.com>"),
            Some(Model { id: "anthropic/claude-opus/5.5".into(), company: "Anthropic".into(), model: "Claude Opus".into(), version: "5.5".into() })
        );
        assert_eq!(model_of("Codex <noreply@openai.com>").map(|m| m.id), Some("openai/codex".into()));
        assert_eq!(model_of("Bob <bob@example.org>"), None);
        assert_eq!(model_of("no address at all"), None);
    }

    #[test]
    fn report_aggregates_people_and_models() {
        let people = parse_people("[people.KasparRosin]\nemails = [\"kaspar@rosin.dev\"]\n");
        let head = Head { repo: "KasparRosin/dbee".into(), branch: "main".into(), sha: "abc".into(), date: "2026-10-16T09:00:00Z".into() };
        let r = report(&parse_log(LOG), &people, &head);
        assert_eq!(r["schema"], 1);
        assert_eq!(r["commits"], 3);
        let first = &r["contributors"][0];
        assert_eq!(first["login"], "KasparRosin");
        assert_eq!(first["people_entry"], true);
        assert_eq!(first["commits"], 2);
        assert_eq!(first["lines_added"], 15);
        assert_eq!(first["first_commit"], "2026-10-15T09:00:00Z");
        assert_eq!(first["last_commit"], "2026-10-16T09:00:00Z");
        assert_eq!(r["contributors"][1]["login"], "Ada");
        assert_eq!(r["contributors"][1]["people_entry"], false);
        assert_eq!(r["models"][0]["id"], "anthropic/claude-opus/5.5");
        assert_eq!(r["models"][0]["commits"], 2);
        assert_eq!(r["models"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn empty_history_gives_an_empty_report() {
        let r = report(&[], &People::default(), &Head::default());
        assert_eq!(r["commits"], 0);
        assert_eq!(r["contributors"], json!([]));
    }
}
