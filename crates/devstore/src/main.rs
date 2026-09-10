//! A disposable record store, for proving the live chain end to end.
//!
//! **This is a proof fixture, not a deployment.** It exists so the disposable stack can
//! demonstrate one thing: that a credential resolves to an organization *server-side*, and that
//! everything downstream is scoped by the answer rather than by anything a caller supplied.
//!
//! It deliberately has no database, no migrations and no persistence. Synthetic records,
//! synthetic credentials, in memory, gone when the process exits. Every feature it grows makes
//! the live gate less trustworthy, because the thing under test would be less like the thing in
//! production and more like this.
//!
//! **It is no longer write-free, and the distinction matters.** `POST /append_activity` mutates
//! the in-memory book so the promotion loop can be demonstrated end to end — proposal,
//! confirmation, append, read-back — without a database. That mutation is **disposable and
//! in-memory**: it survives no restart, is written to no file, and models the production
//! endpoint's *decisions* (created / replayed / conflict / org guard) rather than its storage.
//! What it therefore cannot establish is exactly what Postgres would establish: that a unique
//! index enforces those decisions under concurrency. A proof built here says the seam asks
//! correctly, never that the store answers correctly.
//!
//! ```text
//! ironworks-devstore --port 3996 --book eval=fixtures/book \
//!     --identity dev-token-eval=eval
//!     [--book other=DIR --identity dev-token-other=other]
//!     [--append-identity dev-token-append=eval]
//!     [--drift-after N] [--fail-after N]
//! ```
//!
//! **A book belongs to an org**, and that is not a convenience. The store this stands in for
//! partitions records by `org_id`; a fixture where every credential resolves to a different org
//! and then reads the *same* records models resolution without partitioning, so a separation
//! proof built on it would assert something the fixture cannot exhibit — the vacuous pass that
//! "an empty database passes every isolation check" warns about. Binding each book to an org is
//! therefore drift *reduction*, not the feature growth this file otherwise refuses.
//!
//! `--drift-after` and `--fail-after` exist for the negative checks: a credential re-pointed at
//! another org mid-flight, and a store that goes away *after* verification. Both are conditions
//! a real store can produce and neither can be provoked by asking nicely. Drift changes the org a
//! response *claims*, never which book it is served from — the credential's scope is what the
//! client must notice diverging.

use ironworks_devhttp::{respond, respond_text};
use std::collections::BTreeMap;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The org a drifted answer claims. Deliberately not a real identity in any book.
const DRIFT_ORG: &str = "drifted-org";

struct Config {
    port: u16,
    /// org -> where its records are read from. One book per org: see the module note on why an
    /// unscoped book is not a thing the modelled store has.
    books: BTreeMap<String, String>,
    /// token -> org, for READS. Synthetic throughout.
    identities: BTreeMap<String, String>,
    /// token -> org, for APPENDS. A separate map, because that is how the real service separates
    /// them: a read token is not refused here, it is absent. Reusing one map would model an
    /// authority split that production does not have.
    append_identities: BTreeMap<String, String>,
    drift_after: Option<usize>,
    fail_after: Option<usize>,
}

fn usage() -> ! {
    eprintln!(
        "usage: ironworks-devstore --port N --book ORG=PATH --identity TOKEN=ORG \
         [--book ...] [--identity ...] [--append-identity TOKEN=ORG] \
         [--drift-after N] [--fail-after N]"
    );
    std::process::exit(64)
}

fn main() {
    let mut cfg = Config {
        port: 0,
        books: BTreeMap::new(),
        identities: BTreeMap::new(),
        append_identities: BTreeMap::new(),
        drift_after: None,
        fail_after: None,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--port" => cfg.port = val().parse().unwrap_or_else(|_| usage()),
            "--book" => {
                let kv = val();
                let (o, p) = kv.split_once('=').unwrap_or_else(|| usage());
                cfg.books.insert(o.to_string(), p.to_string());
            }
            "--identity" => {
                let kv = val();
                let (t, o) = kv.split_once('=').unwrap_or_else(|| usage());
                cfg.identities.insert(t.to_string(), o.to_string());
            }
            // A separate flag for a separate map. Passing a read token here would be a
            // mis-invocation the proof should not be able to make by accident.
            "--append-identity" => {
                let kv = val();
                let (t, o) = kv.split_once('=').unwrap_or_else(|| usage());
                cfg.append_identities.insert(t.to_string(), o.to_string());
            }
            "--drift-after" => cfg.drift_after = Some(val().parse().unwrap_or_else(|_| usage())),
            "--fail-after" => cfg.fail_after = Some(val().parse().unwrap_or_else(|_| usage())),
            _ => usage(),
        }
    }
    if cfg.port == 0 || cfg.books.is_empty() || cfg.identities.is_empty() {
        usage();
    }

    // An org whose book is empty answers 200 with zero records, and zero records is the state in
    // which every isolation check passes without measuring anything. Refuse at startup instead.
    let books: BTreeMap<String, Vec<Record>> = cfg
        .books
        .iter()
        .map(|(org, path)| {
            let records = load_book(path).unwrap_or_else(|problems| {
                eprintln!("devstore: book for org {org:?} at {path} has unusable record(s):");
                for problem in &problems {
                    eprintln!("  {problem}");
                }
                std::process::exit(64);
            });
            assert!(!records.is_empty(), "devstore: book for org {org:?} at {path} is empty");
            (org.clone(), records)
        })
        .collect();

    // An identity resolving to an org with no book would serve nothing and look authorised. That
    // is a mis-invocation, so it dies here rather than at the first request.
    for (token, org) in cfg.identities.iter().chain(cfg.append_identities.iter()) {
        if !books.contains_key(org) {
            eprintln!("devstore: identity {token:?} resolves to org {org:?}, which has no --book");
            std::process::exit(64);
        }
    }
    // A token in both maps would be a credential that reads AND appends, which is the one shape
    // the production service cannot express. Refusing it here keeps the fixture honest about the
    // separation it exists to model.
    for token in cfg.identities.keys() {
        if cfg.append_identities.contains_key(token) {
            eprintln!(
                "devstore: token {token:?} is both a read and an append identity; the real \
                 service keeps those in separate maps and no token is in both"
            );
            std::process::exit(64);
        }
    }

    let inventory: Vec<String> =
        books.iter().map(|(org, r)| format!("{org}={}", r.len())).collect();
    eprintln!(
        "devstore: {} synthetic record(s) across {} org(s) [{}], {} identity(ies), on 127.0.0.1:{}",
        books.values().map(Vec::len).sum::<usize>(),
        books.len(),
        inventory.join(" "),
        cfg.identities.len(),
        cfg.port
    );
    if !cfg.append_identities.is_empty() {
        eprintln!(
            "devstore: {} append identity(ies) — POST /append_activity mutates the IN-MEMORY book \
             only; nothing is persisted and no index enforces anything",
            cfg.append_identities.len()
        );
    }
    if cfg.drift_after.is_some() || cfg.fail_after.is_some() {
        eprintln!(
            "devstore: NEGATIVE MODE — drift_after={:?} fail_after={:?}",
            cfg.drift_after, cfg.fail_after
        );
    }

    let listener = TcpListener::bind(("127.0.0.1", cfg.port)).expect("bind");
    let seen = AtomicUsize::new(0);
    // Behind a lock now, because the append route mutates it. The loop is single-threaded, so
    // this buys nothing today — it is here so a future concurrent fixture cannot quietly become
    // the thing that proves concurrency the real store has an index for.
    let books = std::sync::Mutex::new(books);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        handle(stream, &cfg, &books, &seen);
    }
}

/// `POST /append_activity` — the disposable stand-in for the production write.
///
/// # What it models, and what it cannot
///
/// It reproduces the endpoint's **decisions and its request contract**: append authority from its
/// own identity map, the same validation the production parser applies (real calendar dates, the
/// contributor grammar, byte bounds, the shared-note namespace, an object-shaped payload), the
/// `expected_org` guard compared against the credential-resolved org, the account having to
/// exist, and created / replayed / conflict keyed on `(org, activity_id)` with every immutable
/// field compared. A request this accepts is one production accepts, and the error codes match —
/// otherwise a proof run here would pass on calls the real service refuses.
///
/// It does **not** model the storage. There is no unique index here — a `BTreeMap` scan decides
/// what Postgres decides there — so nothing proved against this says the database enforces
/// anything, least of all under concurrency. The mutation is in memory and dies with the process.
fn append_activity(
    stream: &mut TcpStream,
    cfg: &Config,
    books: &std::sync::Mutex<BTreeMap<String, Vec<Record>>>,
    token: &str,
    body: &[u8],
) {
    // Its OWN map. A read token is not refused here — it is absent, which is the separation the
    // real service gets from keeping two files.
    let Some(org) = cfg.append_identities.get(token) else {
        return respond_text(stream, 401, "Invalid or missing auth token");
    };
    // A non-object payload is `invalid_body`, matching the production parser: it refuses
    // anything that is not a dict before it looks at a single field.
    let payload = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(v) if v.is_object() => v,
        _ => return respond(stream, 400, r#"{"error":"invalid_body"}"#),
    };
    let field = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
    let (activity_id, account_id, occurred_at, note, contributor, expected_org) = (
        field("activity_id"),
        field("account_id"),
        field("occurred_at"),
        field("body"),
        field("contributor"),
        field("expected_org"),
    );
    // The production parser's order, and its codes. Getting either wrong would let a proof pass
    // on a request the real service refuses, which is the one way this fixture can lie.
    if let Some(code) = refuse_append_request(
        &activity_id,
        &account_id,
        &occurred_at,
        &note,
        &contributor,
        &expected_org,
    ) {
        return respond(stream, 400, &serde_json::json!({"error": code}).to_string());
    }
    // The confused-deputy guard, before anything is touched. `expected_org` is what the caller
    // believed; `org` is what its credential actually authorises.
    if expected_org != *org {
        let body = serde_json::json!({"error": "org_mismatch", "org": org}).to_string();
        return respond(stream, 409, &body);
    }

    let mut guard = books.lock().expect("books");
    let book = guard.get_mut(org.as_str()).expect("startup refuses an identity with no book");
    let Some(record) = book.iter_mut().find(|r| r.id == account_id) else {
        let body =
            serde_json::json!({"error": "unknown_account", "org": org, "account_id": account_id})
                .to_string();
        return respond(stream, 404, &body);
    };

    let wanted = serde_json::json!({
        "activity_id": activity_id,
        "occurred_at": occurred_at,
        "kind": "shared-note",
        "body": note,
        "contributor": contributor,
    });
    // `load_book` refuses a record with no activities array at startup, so this holds for every
    // record in every book this process is serving.
    let activities = record
        .body
        .get_mut("activities")
        .and_then(serde_json::Value::as_array_mut)
        .expect("startup refuses a record with no activities array");
    if let Some(existing) = activities
        .iter()
        .find(|a| a.get("activity_id").and_then(|v| v.as_str()) == Some(activity_id.as_str()))
    {
        // The row exists. Replay or conflict is decided by comparing every immutable field, and
        // nothing is written either way — there is no update branch to reach.
        let differing = ["occurred_at", "kind", "body", "contributor"]
            .into_iter()
            .find(|k| existing.get(*k) != wanted.get(*k));
        return match differing {
            Some(field) => {
                let body = serde_json::json!({
                    "error": "conflict", "org": org, "activity_id": activity_id,
                    "differing_field": field,
                })
                .to_string();
                respond(stream, 409, &body)
            }
            None => {
                let body = serde_json::json!({
                    "status": "replayed", "org": org, "activity_id": activity_id,
                    "account_id": account_id, "kind": "shared-note",
                })
                .to_string();
                respond(stream, 200, &body)
            }
        };
    }
    activities.push(wanted);
    let body = serde_json::json!({
        "status": "created", "org": org, "activity_id": activity_id,
        "account_id": account_id, "kind": "shared-note",
    })
    .to_string();
    respond(stream, 201, &body)
}

/// The production request contract, in the production order, returning the production code.
///
/// Mirrors `service_guards.parse_append_request`. It is a second implementation rather than a
/// shared schema, deliberately: a cross-language schema layer would be a third thing to keep
/// true, and the contract is six rules. Where a rule is already a Rust domain type — the
/// contributor grammar is `ContributorId`'s — that type is what decides, so the two languages
/// agree because they enforce the same grammar rather than because someone copied it.
fn refuse_append_request(
    activity_id: &str,
    account_id: &str,
    occurred_at: &str,
    body: &str,
    contributor: &str,
    expected_org: &str,
) -> Option<&'static str> {
    /// `MAX_ID_BYTES` upstream.
    const MAX_ID: usize = 128;
    /// `MAX_SHARED_BODY_BYTES` upstream. A transport bound, not a content rule.
    const MAX_BODY: usize = 8000;

    let bounded = |v: &str, limit: usize| !v.is_empty() && v.len() <= limit;

    if !bounded(activity_id, MAX_ID) {
        return Some("invalid_activity_id");
    }
    // The namespace guard: this writer may only ever touch ids it minted, so a caller-side
    // derivation bug cannot land on a team-authored activity.
    if !activity_id.starts_with("share-") {
        return Some("activity_id_outside_shared_namespace");
    }
    if !bounded(account_id, MAX_ID) {
        return Some("invalid_account_id");
    }
    if !valid_date(occurred_at) {
        return Some("invalid_occurred_at");
    }
    if !bounded(body, MAX_BODY) {
        return Some("invalid_body");
    }
    // The contributor grammar is `ContributorId`'s, asked of the type rather than restated. That
    // is what makes "the same grammar as Rust" true by construction on this side.
    if ironworks_domain::ContributorId::parse(contributor).is_err() {
        return Some("invalid_contributor");
    }
    if !bounded(expected_org, MAX_ID) {
        return Some("invalid_expected_org");
    }
    None
}

/// `YYYY-MM-DD`, and a real calendar date — `2026-02-31` is refused, as upstream refuses it.
///
/// Hand-rolled because `domain` carries no date library and this is the only place that needs one:
/// four rules and a leap-year test are smaller than a dependency, and the production side is
/// `datetime.date.fromisoformat`, which enforces exactly these.
fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let num = |from: usize, to: usize| value.get(from..to).and_then(|v| v.parse::<u32>().ok());
    let (Some(year), Some(month), Some(day)) = (num(0, 4), num(5, 7), num(8, 10)) else {
        return false;
    };
    // Year zero is not a date `datetime.date` will construct: Python's calendar runs 1..=9999,
    // and the fixed-width format already bounds the top. Permitting it here accepted
    // `0000-01-01`, which production refuses — measured, and the whole reason this bound is
    // explicit rather than implied by the arithmetic below.
    if year == 0 || !(1..=12).contains(&month) || day == 0 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    day <= last
}

/// One synthetic record, as the store serves it.
struct Record {
    id: String,
    name: String,
    updated_at: String,
    body: serde_json::Value,
}

/// Read one org's records: every `*.json` in a directory, or a single JSON file.
///
/// A lone file is allowed because a book is a set of records and some are a set of one. The
/// tenant fixtures sit beside each other in `fixtures/tenants/`, so pointing an org at that
/// directory would serve both tenants to both — the exact confusion the org key exists to stop.
///
/// # Every unusable file is named, and none is skipped
///
/// This used to be a `filter_map` whose body was a chain of `?` over `Option`, so a record that
/// was unreadable, unparseable, or missing a field simply was not in the book — and only a book
/// where *every* file failed would trip the emptiness refusal below. A proof then ran against a
/// book quietly short of records, which is the state in which a scope or isolation check passes
/// without measuring what it names.
///
/// `activities` is checked here rather than at the first `POST /append_activity`, because that is
/// what turns a mid-run panic into a startup refusal an operator can read.
fn load_book(path: &str) -> Result<Vec<Record>, Vec<String>> {
    let meta = std::fs::metadata(path).map_err(|e| vec![format!("cannot read {path}: {e}")])?;
    let mut paths: Vec<std::path::PathBuf> = if meta.is_dir() {
        std::fs::read_dir(path)
            .map_err(|e| vec![format!("cannot read {path}: {e}")])?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect()
    } else {
        vec![std::path::PathBuf::from(path)]
    };
    paths.sort();
    let (mut records, mut problems) = (Vec::new(), Vec::new());
    for p in &paths {
        let where_ = p.display().to_string();
        match record_from(p) {
            Ok(record) => records.push(record),
            Err(why) => problems.push(format!("{where_}: {why}")),
        }
    }
    if problems.is_empty() { Ok(records) } else { Err(problems) }
}

/// One record file, or what is wrong with it.
fn record_from(path: &std::path::Path) -> Result<Record, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let id = v
        .get("record_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("no string record_id")?
        .to_owned();
    let name = v
        .get("account")
        .and_then(|a| a.get("name"))
        .and_then(serde_json::Value::as_str)
        .ok_or("no string account.name")?
        .to_owned();
    if !v.get("activities").is_some_and(serde_json::Value::is_array) {
        // `append_activity` appends into this array and has nowhere to put a note without it.
        return Err("no activities array".to_owned());
    }
    // A stable synthetic version, so freshness comparisons have something to compare.
    let updated_at = format!("2026-08-01T00:00:{:02}+00:00", id.len() % 60);
    Ok(Record { id, name, updated_at, body: v })
}

fn handle(
    mut stream: TcpStream,
    cfg: &Config,
    books: &std::sync::Mutex<BTreeMap<String, Vec<Record>>>,
    seen: &AtomicUsize,
) {
    ironworks_devhttp::set_timeouts(&stream);
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let request = match ironworks_devhttp::read(&mut reader) {
        Ok(request) => request,
        Err(why) => return ironworks_devhttp::respond_refusal(&mut stream, why),
    };
    // The name is folded and the value is not — `Request::header` carries that rule and the
    // incident behind it.
    let token = request.header("x-service-token").unwrap_or("").to_owned();
    let body_bytes = request.body;
    let (method, path, query) = (request.method, request.path.as_str(), request.query.as_str());

    // Liveness is NOT counted. `--drift-after` and `--fail-after` key on the position of a
    // request in the CHAIN, and a caller that polls /health until the store is up would
    // otherwise shift every later request by one — which silently swaps which step the negative
    // mode lands on. (It did: the first run of the proof reported drift serving and the outage
    // refusing, exactly inverted.)
    if path == "/health" {
        return respond(&mut stream, 200, r#"{"ok":true}"#);
    }

    let n = seen.fetch_add(1, Ordering::SeqCst);
    // A store that goes away AFTER verification: the connection is accepted and dropped, so the
    // client sees a transport failure rather than an orderly refusal.
    if cfg.fail_after.is_some_and(|k| n >= k) {
        return;
    }
    let drifted = cfg.drift_after.is_some_and(|k| n >= k);

    // The one write. Handled before the read gate because it authenticates against a different
    // map entirely — a read token reaching this point is simply not in it.
    if path == "/append_activity" {
        // POST only, as the route is declared upstream. A GET reaching the append handler would
        // let a proof exercise a verb Flask would have refused at the router.
        if method != "POST" {
            return respond(&mut stream, 405, r#"{"error":"method_not_allowed"}"#);
        }
        return append_activity(&mut stream, cfg, books, &token, &body_bytes);
    }
    // And the readers are GET, for the same reason.
    if method != "GET" {
        return respond(&mut stream, 405, r#"{"error":"method_not_allowed"}"#);
    }

    // Identity implies scope. The caller presents a credential and cannot assert an org; there
    // is no header or parameter here that could carry one.
    let Some(org) = cfg.identities.get(&token) else {
        // Plain text, matching what the real service's middleware returns.
        return respond_text(&mut stream, 401, "Invalid or missing auth token");
    };
    let answered = if drifted { DRIFT_ORG } else { org.as_str() };
    // Startup refuses an identity whose org has no book, so this cannot miss. Records come from
    // the credential's own org even under drift: what drifts is the org an answer *claims*, and a
    // client that swapped books here would be testing the fixture instead of the scope check.
    let guard = books.lock().expect("books");
    let book = &guard[org.as_str()];

    match path {
        "/list_accounts" => {
            let accounts: Vec<serde_json::Value> = book
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "account_id": r.id,
                        "name": r.name,
                        "updated_at": r.updated_at,
                    })
                })
                .collect();
            let body = serde_json::json!({
                "source": "devstore",
                "org": answered,
                "count": accounts.len(),
                "accounts": accounts,
            });
            respond(&mut stream, 200, &body.to_string())
        }
        "/get_account_context" => {
            let wanted = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("account_id="))
                .map(percent_decode)
                .unwrap_or_default();
            match book.iter().find(|r| r.id == wanted) {
                Some(r) => {
                    let mut body = r.body.clone();
                    if let Some(m) = body.as_object_mut() {
                        m.insert("org".into(), serde_json::Value::String(answered.into()));
                        m.insert("found".into(), serde_json::Value::Bool(true));
                    }
                    respond(&mut stream, 200, &body.to_string())
                }
                None => respond(
                    &mut stream,
                    404,
                    &serde_json::json!({"org": answered, "found": false}).to_string(),
                ),
            }
        }
        _ => respond(&mut stream, 404, r#"{"error":"no such route"}"#),
    }
}

/// Percent-decoding, over bytes throughout.
///
/// The escape is a byte-level encoding, so the two hex digits are read as bytes rather than by
/// re-slicing the `&str`. That re-slice was `&s[i + 1..i + 3]`, which panics whenever a multi-byte
/// character follows a `%` — the offsets are arbitrary, and `str` refuses a cut inside a character.
/// `handle` calls this on a query string from the accept loop with no unwind guard, so the panic
/// was the whole fixture, on input a caller chooses.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // `i + 2 < b.len()` would refuse the escape at the very end of the string; the pair needs
        // `i + 1` and `i + 2` to exist, which is `i + 2 <= b.len() - 1`.
        if let (b'%', Some(hi), Some(lo)) = (b[i], b.get(i + 1), b.get(i + 2))
            && let (Some(hi), Some(lo)) = ((*hi as char).to_digit(16), (*lo as char).to_digit(16))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `%` followed by a multi-byte character must not take the store down.
    ///
    /// `handle` runs on the accept loop with no unwind guard, so a panic here is the whole fixture
    /// dying mid-proof — and `account_id=` is a query string, which is to say caller input. The
    /// decode used to re-slice the `&str` at `i+1..i+3`, byte offsets that land inside a character
    /// whenever one follows the `%`.
    #[test]
    fn a_percent_before_a_multibyte_character_decodes_rather_than_panicking() {
        for raw in ["%€", "ACME-001%€", "%ü%", "100%", "%", "%A", "%ZZ", "a%2Gb"] {
            let _ = percent_decode(raw);
        }
        // Positive control: the escapes this exists to decode still decode, so the cases above
        // are not passing against a function that gave up and returned its input.
        assert_eq!(percent_decode("ACME%2D001"), "ACME-001");
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%E2%82%AC"), "€", "a percent-encoded multi-byte character");
        // Not an escape, so it survives untouched rather than being eaten.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
    }

    /// The production contract, exercised through the same function the route uses.
    ///
    /// Each case is a request `service_guards.parse_append_request` refuses with the named code.
    /// They are here because the fixture's own documentation claims decision parity, and a
    /// fixture that accepted calls production refuses would let a proof pass on them.
    #[test]
    fn devstore_refuses_exactly_what_the_production_parser_refuses() {
        let ok = ("share-abc", "ACME-001", "2026-09-01", "a note", "alice", "org-1");
        assert_eq!(refuse_append_request(ok.0, ok.1, ok.2, ok.3, ok.4, ok.5), None, "the control");

        let long_id = "share-".to_owned() + &"x".repeat(200);
        let long_body = "x".repeat(8001);
        for (case, want) in [
            (("", ok.1, ok.2, ok.3, ok.4, ok.5), "invalid_activity_id"),
            ((long_id.as_str(), ok.1, ok.2, ok.3, ok.4, ok.5), "invalid_activity_id"),
            // A team-authored id: the namespace guard, and the one that keeps this writer away
            // from rows it did not mint.
            (("NW-INT-01", ok.1, ok.2, ok.3, ok.4, ok.5), "activity_id_outside_shared_namespace"),
            ((ok.0, "", ok.2, ok.3, ok.4, ok.5), "invalid_account_id"),
            // A date that is ten characters and not a date. The length check this replaced
            // accepted every one of these.
            ((ok.0, ok.1, "2026-02-31", ok.3, ok.4, ok.5), "invalid_occurred_at"),
            ((ok.0, ok.1, "2026-13-01", ok.3, ok.4, ok.5), "invalid_occurred_at"),
            ((ok.0, ok.1, "2026-00-10", ok.3, ok.4, ok.5), "invalid_occurred_at"),
            ((ok.0, ok.1, "not-a-date", ok.3, ok.4, ok.5), "invalid_occurred_at"),
            ((ok.0, ok.1, "2026/09/01", ok.3, ok.4, ok.5), "invalid_occurred_at"),
            ((ok.0, ok.1, "2026-9-1", ok.3, ok.4, ok.5), "invalid_occurred_at"),
            ((ok.0, ok.1, ok.2, "", ok.4, ok.5), "invalid_body"),
            ((ok.0, ok.1, ok.2, long_body.as_str(), ok.4, ok.5), "invalid_body"),
            // The contributor grammar, asked of `ContributorId` rather than restated.
            ((ok.0, ok.1, ok.2, ok.3, "Alice", ok.4), "invalid_contributor"),
            ((ok.0, ok.1, ok.2, ok.3, "alice smith", ok.5), "invalid_contributor"),
            ((ok.0, ok.1, ok.2, ok.3, "alice_smith", ok.5), "invalid_contributor"),
            ((ok.0, ok.1, ok.2, ok.3, "", ok.5), "invalid_contributor"),
            ((ok.0, ok.1, ok.2, ok.3, ok.4, ""), "invalid_expected_org"),
        ] {
            assert_eq!(
                refuse_append_request(case.0, case.1, case.2, case.3, case.4, case.5),
                Some(want),
                "{case:?}"
            );
        }
    }

    #[test]
    fn the_date_check_is_a_calendar_and_not_a_length() {
        // The divergence this replaced: ten characters used to be enough.
        // `0001-01-01` and `9999-12-31` are the ends of Python's calendar and are accepted
        // there, so they are accepted here: the bound is the same bound, not a stricter one.
        for good in [
            "2026-09-01",
            "2024-02-29",
            "2000-02-29",
            "2026-12-31",
            "2026-01-01",
            "0001-01-01",
            "9999-12-31",
        ] {
            assert!(valid_date(good), "{good}");
        }
        for bad in [
            "2026-02-30",
            "2023-02-29",
            "1900-02-29",
            "2026-04-31",
            "2026-13-01",
            "2026-00-01",
            "2026-01-00",
            "2026-01-32",
            "20260101",
            "2026-01-0a",
            "",
            "2026-01-011",
            // Measured against the production parser, which is `datetime.date.fromisoformat`
            // and runs 1..=9999: `valid_date("0000-01-01") -> False` there, and this fixture
            // answered 201 CREATED to it before the year bound was made explicit.
            "0000-01-01",
            "0000-12-31",
        ] {
            assert!(!valid_date(bad), "{bad}");
        }
    }
}
