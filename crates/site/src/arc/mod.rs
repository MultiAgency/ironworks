//! The composition arc: cases down, runs across.
pub mod data;
pub mod render;

use std::path::Path;

/// Build the arc page for a suite, or say why it refuses to.
///
/// # The mirror check
///
/// This module computes part digests itself, and `ironworks compose` computes them too. They must
/// agree before anything on the page rests on them: if the strip-and-hash rule has drifted between
/// the two, **every attribution below would be fiction** — the page would confidently name which
/// part moved, from a digest the binary never produced.
pub fn build(root: &Path, suite_name: &str, since: &str) -> Result<Built, String> {
    let suite = data::suite(suite_name).ok_or_else(|| format!("no suite {suite_name:?}"))?;
    let now = data::compose(root, suite)?;
    // The suite names its service only when it overrides the default; the default's own id is what
    // `compose` echoed back, so take it from there rather than repeating it.
    let service = now.service.split('@').next().unwrap_or(&now.service).to_owned();

    let worktree = data::Reader::Worktree(root.to_path_buf());
    let mirror = data::ingredients(&worktree, &service).ok_or_else(|| {
        format!("the working tree does not hold {service} and every part it names")
    })?;
    if !mirror.missing.is_empty() {
        return Err(format!("the working tree does not hold {service} and every part it names"));
    }
    if mirror.parts != now.parts {
        return Err(format!(
            "this generator and `ironworks compose` disagree about the working tree's part \
             digests.\n         binary: {:?}\n         crate:  {:?}\n         The strip-and-hash \
             rule has drifted; every attribution below would be fiction.",
            now.parts, mirror.parts
        ));
    }

    let (runs, probes) = data::transcripts(&root.join("docs/experiments"), suite)?;

    let historical = data::Reader::Revision { root: root.to_path_buf(), rev: since.to_owned() };
    let then = data::ingredients(&historical, &service);
    let guidance_before = historical.read(suite.guidance);
    let guidance_after = worktree.read(suite.guidance);

    let head_rev = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            (!s.is_empty()).then_some(s)
        })
        .unwrap_or_else(|| "unknown".to_owned());

    let html = render::sections(&render::Context {
        suite,
        service: &service,
        now: &now,
        runs: &runs,
        probes: &probes,
        then: then.as_ref(),
        since,
        guidance_rel: suite.guidance,
        guidance_moved: guidance_before != guidance_after,
        guidance_added: guidance_before.is_none(),
        head_rev: &head_rev,
    });
    Ok(Built { html, service })
}

/// The arc's sections, and the facts the page around them needs.
pub struct Built {
    pub html: String,
    pub service: String,
}
