//! Reading a graded answer against the records it names.
pub mod anchors;
pub mod book;
pub mod render;
pub mod verify;

use std::path::Path;

/// The reading's parts, for the page that places them.
pub struct Reading {
    pub transcript: String,
    pub book_rel: String,
    pub records: usize,
    /// How the book binding was established, as the sentence the page shows.
    pub binding: String,
    pub cases: String,
    pub finding: (String, String),
}

/// Read a transcript against the book it was bound to, or say why it refuses to.
pub fn sections(
    root: &Path,
    transcript: Option<&str>,
    one_case: Option<&str>,
) -> Result<Reading, String> {
    let experiments = root.join("docs/experiments");
    let book_rel = "fixtures/account-coordination/book";
    let book_dir = root.join(book_rel);

    let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(&experiments)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            n.ends_with(".json") && n.contains("account-coordination")
        })
        .collect();
    found.sort();
    if found.is_empty() {
        return Err("no transcripts for account-coordination".to_owned());
    }
    let path = match transcript {
        Some(name) => experiments.join(name),
        None => found.last().unwrap().clone(),
    };
    if !path.is_file() {
        return Err(format!("no transcript at {}", path.display()));
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();

    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let all = match &doc {
        serde_json::Value::Array(rows) => rows.clone(),
        serde_json::Value::Object(map) => {
            map.get("cases").and_then(|c| c.as_array()).cloned().unwrap_or_default()
        }
        _ => return Err(format!("{name} is neither a list nor an object")),
    };
    let rows: Vec<serde_json::Value> = match one_case {
        Some(want) => all
            .iter()
            .filter(|r| r.get("case").and_then(|c| c.as_str()) == Some(want))
            .cloned()
            .collect(),
        None => all,
    };
    if rows.is_empty() {
        return Err(format!("no case {:?} in {name}", one_case.unwrap_or("")));
    }

    let ledger = ironworks_evidence::regrades::load(&experiments);
    if !ledger.problems.is_empty() {
        return Err(format!(
            "the regrade ledger does not match the transcripts it names:\n         {}",
            ledger.problems.join("\n         ")
        ));
    }

    let loaded = book::load(&book_dir)?;
    let hint = book::year_of(&loaded);
    let binding = verify::verify(&doc, &rows, &loaded, &book_dir, root, &name)?;
    println!(
        "reading {name} — {} case(s) against {} records in {book_rel}\n         bound to that \
         book by {}\n",
        rows.len(),
        loaded.len(),
        binding.terminal
    );

    let distinct: std::collections::BTreeSet<&str> =
        rows.iter().filter_map(|r| r.get("case").and_then(|c| c.as_str())).collect();
    let repeated = distinct.len() < rows.len();

    let mut cases = String::new();
    let mut counts = Vec::new();
    for row in &rows {
        let case = row.get("case").and_then(|c| c.as_str()).unwrap_or_default();
        let regrade = render::regrade_for(&ledger.entries, &name, case);
        let label = render::label_of(row, repeated);
        let out = render::render_case(
            row,
            &loaded,
            hint.as_deref(),
            regrade,
            &label,
            render::Instrumentation::Shown,
        );
        cases.push_str(&out.html);
        counts.push(out.counts);
    }
    let totals = render::total(&counts);
    let finding = render::bare_finding(totals.bare, rows.len(), one_case.is_some());

    Ok(Reading {
        transcript: name,
        book_rel: book_rel.to_owned(),
        records: loaded.len(),
        binding: binding.html,
        cases,
        finding,
    })
}
