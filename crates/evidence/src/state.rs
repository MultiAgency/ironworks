//! `E_t` — what was observed about a candidate, appended and never rewritten.
//!
//! # What this holds, and what it must never hold
//!
//! **Claim ids, subject ids, digests and findings.** No answer text, no record text, no message
//! text, no reasons in prose. That is this crate's declared restraint — *digests, ids and counts
//! only* — and it is the same rule the turn journal keeps, for the same reason: everything here
//! outlives a crash on purpose, and a tenant's words are a tenant's.
//!
//! The rule is carried by **types, not by review**. [`ClaimId`] and [`SubjectId`] refuse anything
//! that could hold prose — whitespace, a control character, or more than [`ID_MAX`] bytes — so
//! the store cannot be handed a sentence by a caller who meant well. `docs/INVARIANTS.md` §E-11
//! records the journal's version of this, and notes that its own no-text rule is asserted
//! separately rather than enforced; this one is enforced at the boundary.
//!
//! # Append-only, and it is the API that says so
//!
//! There is no `update` and no `delete`. A claim observed twice is two rows. A later observation
//! that disagrees with an earlier one does not overwrite it — both stand, against the candidate
//! each was made about, which is the only way a reader can see that a finding *moved*.
//!
//! # Every observation names its candidate
//!
//! [`Observation::candidate`] is not optional. arXiv 2609.01481 §3.4.3 requires that "an
//! assessment cannot combine behavior from different artifact versions", and
//! `docs/experiments/0061-the-freeze-holds-because-the-other-role-cannot-reach-the-bytes.md`
//! measured the seam supplying that. A row that did not name its candidate would let a reader
//! attribute an observation to a candidate nobody made it against, which is the same defect in the
//! reading direction.

use ironworks_domain::{Finding, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;

/// The longest an id may be. Long enough for a dotted claim path or a record id; what keeps a
/// sentence out is the refusal of whitespace, and this bounds everything else.
pub const ID_MAX: usize = 128;

/// The schema this implementation writes.
///
/// Refused rather than migrated, exactly as `ironworks-proposals` refuses: a store whose shape we
/// cannot account for may hold evidence somebody is about to plan against, and guessing at it is
/// worse than stopping.
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS observations (
    -- Append order, and the only key. Deliberately not (loop, claim): the same claim observed
    -- twice in one loop is two rows, and a primary key over those columns would have made the
    -- second one silently replace the first.
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    loop_id      TEXT NOT NULL,
    claim        TEXT NOT NULL,
    subject      TEXT NOT NULL,
    -- `Finding::wire`. Stored as its own spelling rather than as a boolean pair, so a reader
    -- cannot reconstruct three states from two columns and get four.
    finding      TEXT NOT NULL,
    candidate    TEXT NOT NULL,
    observed_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS observations_loop ON observations(loop_id);
CREATE INDEX IF NOT EXISTS observations_candidate ON observations(candidate);
";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EvidenceError {
    #[error("{what} is not a usable id: {why}")]
    Id { what: &'static str, why: &'static str },
    #[error("the evidence state is at schema {found}, this build writes {SCHEMA_VERSION}")]
    Version { found: i64 },
    #[error("evidence state at {path}: {why}")]
    Opened { path: String, why: String },
    #[error("the evidence state could not {op}: {why}")]
    Store { op: &'static str, why: String },
    #[error("a finding this build does not understand: {0:?}")]
    UnknownFinding(String),
}

/// Every way the shared open protocol can refuse, named rather than stringified — the same
/// mapping `ironworks-proposals` writes, because a caller that has to parse a message to find out
/// whether its store is the wrong version is a caller that will not.
fn opened(path: &Path, why: ironworks_store::OpenFailure) -> EvidenceError {
    use ironworks_store::OpenFailure as F;
    let path = path.display().to_string();
    match why {
        F::Open(e) => EvidenceError::Opened { path, why: e.to_string() },
        F::NotADatabase => {
            EvidenceError::Opened { path, why: "its bytes are not a database".to_owned() }
        }
        F::Exposed(e) => EvidenceError::Opened {
            path,
            why: format!("it could not be restricted to its owner: {e}"),
        },
        F::UnreadableVersion { raw } => {
            EvidenceError::Opened { path, why: format!("its schema version reads {raw:?}") }
        }
        F::Version { found } => EvidenceError::Version { found },
        F::Sqlite(e) => EvidenceError::Opened { path, why: e.to_string() },
    }
}

/// An id that cannot hold prose.
macro_rules! bounded_id {
    ($name:ident, $what:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Refuses empty, over-long, and anything carrying whitespace or a control character.
            /// A caller that means to store a sentence finds out here rather than three iterations
            /// later when somebody reads the store expecting identifiers.
            pub fn parse(raw: impl Into<String>) -> Result<Self, EvidenceError> {
                let raw = raw.into();
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(EvidenceError::Id { what: $what, why: "it is empty" });
                }
                if trimmed.len() > ID_MAX {
                    return Err(EvidenceError::Id { what: $what, why: "it is longer than ID_MAX" });
                }
                if trimmed.chars().any(char::is_control) {
                    return Err(EvidenceError::Id {
                        what: $what,
                        why: "it carries a control character, so it is text rather than an id",
                    });
                }
                // Whitespace is what a sentence is made of and what no identifier needs. Refusing
                // line breaks alone let any one-line sentence up to `ID_MAX` bytes through.
                if trimmed.chars().any(char::is_whitespace) {
                    return Err(EvidenceError::Id {
                        what: $what,
                        why: "it carries whitespace, so it reads as words rather than an id",
                    });
                }
                Ok(Self(trimmed.to_owned()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

const SCHEMA: ironworks_store::Schema =
    ironworks_store::Schema { sql: SCHEMA_SQL, meta_table: "meta", version: SCHEMA_VERSION };

bounded_id!(ClaimId, "a claim id");
bounded_id!(SubjectId, "a subject id");
bounded_id!(CandidateDigest, "a candidate digest");
bounded_id!(LoopId, "a loop id");

/// One claim, one subject, one finding, against one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// What was asserted. Stable across loops, so a finding can be seen to move.
    pub claim: ClaimId,
    /// What it was asserted about — a record id, a check name, a file. Opaque here on purpose:
    /// this store does not need to know which of those a caller means.
    pub subject: SubjectId,
    pub finding: Finding,
    /// The candidate the observation was made against. Not optional.
    pub candidate: CandidateDigest,
    /// When. A [`Utc`] rather than a string, because the column held two spellings of one clock —
    /// the recorder wrote Unix seconds and every test wrote RFC 3339 — and nothing refused either.
    pub observed_at: Utc,
}

/// The four sets a planner reads, and the reason they are four.
///
/// Merging any two of these loses an action. `repair` and `unmeasured` are the pair that matters
/// most — `docs/experiments/0050-blocked-was-being-read-as-fail.md` measured that collapse costing
/// seven of eleven causes a wrong verdict, and §F-14 is the rule it broke.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Standing {
    /// Upheld: must still hold after the next increment.
    pub preserve: Vec<ClaimId>,
    /// Refuted: measured, and broken. The only set that is repair work.
    pub repair: Vec<ClaimId>,
    /// Indeterminate: observed, and the instrument could not decide. A person reads it; planning
    /// a repair against it is acting on something nobody has established.
    pub unresolved: Vec<ClaimId>,
    /// Unobserved: nobody looked. The action is to measure, never to fix.
    pub unmeasured: Vec<ClaimId>,
}

/// The evidence state. Append-only by construction: no method here updates or deletes a row.
pub struct EvidenceState {
    conn: Mutex<Connection>,
}

impl EvidenceState {
    /// Open, through the workspace's one SQLite open protocol.
    ///
    /// **Not `Connection::open`.** `ironworks-store` owns owner-only permissions, WAL,
    /// `synchronous = FULL`, schema creation and version settlement, and it owns them *once* so
    /// that no store has to remember four of them. The first draft of this file opened the
    /// connection itself and settled its own version — a second mechanism for a solved problem,
    /// and a worse one: it created a world-readable file holding a tenant's record ids.
    ///
    /// **Refused rather than migrated**, like `ironworks-proposals`: at version 1 there is nothing
    /// to migrate from, and a store whose shape this build cannot account for may hold evidence
    /// somebody is about to plan against.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EvidenceError> {
        let path = path.as_ref();
        let conn = ironworks_store::open_durable(path, SCHEMA, |_, _| Ok(false))
            .map_err(|why| opened(path, why))?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// The connection, whatever a previous panic left behind.
    ///
    /// Taken the way `ironworks_proposals::ProposalStore::locked` takes it, and for the reason
    /// recorded there: poisoning means some other thread panicked while holding this, and what is
    /// behind it is a SQLite connection with its own transactional integrity. Aborting every
    /// reader of an append-only evidence store because of an unrelated panic is the worse failure.
    fn locked(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Append one observation. **The only write.**
    pub fn append(&self, loop_id: &LoopId, o: &Observation) -> Result<(), EvidenceError> {
        let conn = self.locked();
        conn.execute(
            "INSERT INTO observations(loop_id,claim,subject,finding,candidate,observed_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                loop_id.as_str(),
                o.claim.as_str(),
                o.subject.as_str(),
                o.finding.wire(),
                o.candidate.as_str(),
                o.observed_at.as_str(),
            ],
        )
        .map_err(|e| EvidenceError::Store { op: "append", why: e.to_string() })?;
        Ok(())
    }

    /// Everything recorded for one loop, in the order it was appended.
    pub fn observations(&self, loop_id: &LoopId) -> Result<Vec<Observation>, EvidenceError> {
        self.read(
            "SELECT claim,subject,finding,candidate,observed_at FROM observations
             WHERE loop_id=?1 ORDER BY seq",
            loop_id.as_str(),
        )
    }

    /// What a planner is handed: every claim's latest finding against one candidate, **kept in
    /// four sets rather than merged into two**.
    ///
    /// This is the control that arXiv 2609.01481 §A.4 fails. Its bundle is `verified ∪ gap`, so a
    /// claim nobody measured arrives at the next planner in the same bucket as one that was
    /// measured and broke — and the planner spends an iteration repairing something that may
    /// already work, then reads its own repair as progress. Here the four are separate because the
    /// actions differ: preserve it, fix it, get a person to read it, or go and measure it.
    pub fn standing(&self, candidate: &CandidateDigest) -> Result<Standing, EvidenceError> {
        let rows = self.read(
            "SELECT claim,subject,finding,candidate,observed_at FROM observations
             WHERE candidate=?1 ORDER BY seq",
            candidate.as_str(),
        )?;
        // Latest wins: a claim upheld and later refuted is not preserved, and the reverse is not
        // still broken. Append-only storage is what makes "latest" meaningful rather than lossy.
        let mut latest: std::collections::BTreeMap<ClaimId, Finding> = Default::default();
        for o in rows {
            latest.insert(o.claim, o.finding);
        }
        let of = |want: Finding| -> Vec<ClaimId> {
            latest.iter().filter(|(_, f)| **f == want).map(|(c, _)| c.clone()).collect()
        };
        Ok(Standing {
            preserve: of(Finding::Upheld),
            repair: of(Finding::Refuted),
            unresolved: of(Finding::Indeterminate),
            unmeasured: of(Finding::Unobserved),
        })
    }

    /// **The preservation set**: every claim standing as [`Finding::Upheld`] against one candidate.
    ///
    /// One line, because [`Self::standing`] already decides what "latest" means. Computing it a
    /// second time here is the shape this repository calls two mechanisms for one rule — and the
    /// two would drift on exactly the question that matters, which finding wins when a claim was
    /// observed twice.
    pub fn upheld_for(&self, candidate: &CandidateDigest) -> Result<Vec<ClaimId>, EvidenceError> {
        Ok(self.standing(candidate)?.preserve)
    }

    fn read(&self, sql: &str, arg: &str) -> Result<Vec<Observation>, EvidenceError> {
        let conn = self.locked();
        let bad = |e: rusqlite::Error| EvidenceError::Store { op: "read", why: e.to_string() };
        let mut stmt = conn.prepare(sql).map_err(bad)?;
        let rows = stmt
            .query_map(params![arg], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(bad)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(bad)?;
        rows.into_iter()
            .map(|(claim, subject, finding, candidate, observed_at)| {
                Ok(Observation {
                    claim: ClaimId::parse(claim)?,
                    subject: SubjectId::parse(subject)?,
                    finding: Finding::parse(&finding)
                        .ok_or(EvidenceError::UnknownFinding(finding))?,
                    candidate: CandidateDigest::parse(candidate)?,
                    observed_at: Utc::parse(observed_at)
                        .map_err(|e| EvidenceError::Store { op: "read", why: e.to_string() })?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ironworks-evidence-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d.join("e.db")
    }

    fn obs(claim: &str, finding: Finding, candidate: &str) -> Observation {
        Observation {
            claim: ClaimId::parse(claim).unwrap(),
            subject: SubjectId::parse("acct-001").unwrap(),
            finding,
            candidate: CandidateDigest::parse(candidate).unwrap(),
            observed_at: Utc::parse("2026-09-10T00:00:00+00:00").unwrap(),
        }
    }

    #[test]
    fn an_id_cannot_hold_prose() {
        // The rule this store is built on, enforced at the boundary rather than reviewed. Each of
        // these is a shape a well-meaning caller reaches for when they have a sentence to store.
        assert!(ClaimId::parse("").is_err(), "empty");
        assert!(ClaimId::parse("   ").is_err(), "whitespace only");
        assert!(ClaimId::parse("a".repeat(ID_MAX + 1)).is_err(), "longer than ID_MAX");
        assert!(ClaimId::parse("the answer said\nsomething").is_err(), "a line break");
        assert!(ClaimId::parse("tab\there").is_err(), "a control character");
        // The shape the line-break rule let through: one line, well under `ID_MAX`, and prose.
        assert!(ClaimId::parse("the customer said they would renew").is_err(), "a sentence");
        // And the positive control: an ordinary id still parses, so the predicate is not simply
        // refusing everything.
        assert_eq!(
            ClaimId::parse(" onboarding.complete ").unwrap().as_str(),
            "onboarding.complete"
        );
    }

    #[test]
    fn a_stored_time_that_is_not_utc_is_refused_on_read() {
        // The column held Unix seconds from the recorder and RFC 3339 from every test. A row in the
        // first spelling reads as an error, never as a second clock beside the first.
        let p = tmp();
        let _ = std::fs::remove_file(&p);
        let s = EvidenceState::open(&p).unwrap();
        s.locked()
            .execute(
                "INSERT INTO observations(loop_id,claim,subject,finding,candidate,observed_at)
                 VALUES('loop-seconds','c.one','acct-001',?1,'cand-a','1789000000')",
                params![Finding::Upheld.wire()],
            )
            .unwrap();
        let seconds = LoopId::parse("loop-seconds").unwrap();
        assert!(
            matches!(s.observations(&seconds), Err(EvidenceError::Store { op: "read", .. })),
            "{:?}",
            s.observations(&seconds)
        );
        // The positive control: a row written through the type reads back.
        let typed = LoopId::parse("loop-typed").unwrap();
        s.append(&typed, &obs("c.one", Finding::Upheld, "cand-a")).unwrap();
        assert_eq!(s.observations(&typed).unwrap().len(), 1);
    }

    #[test]
    fn a_second_observation_appends_and_does_not_overwrite() {
        let p = tmp();
        let _ = std::fs::remove_file(&p);
        let s = EvidenceState::open(&p).unwrap();
        let l = LoopId::parse("loop-1").unwrap();
        s.append(&l, &obs("c.one", Finding::Upheld, "cand-a")).unwrap();
        s.append(&l, &obs("c.one", Finding::Refuted, "cand-a")).unwrap();
        let rows = s.observations(&l).unwrap();
        // Both stand. A store that updated in place would report one row and lose the fact that
        // the finding moved, which is the thing a later planner most needs to see.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].finding, Finding::Upheld);
        assert_eq!(rows[1].finding, Finding::Refuted);
    }

    #[test]
    fn the_preservation_set_is_the_latest_finding_per_claim() {
        let p = tmp();
        let _ = std::fs::remove_file(&p);
        let s = EvidenceState::open(&p).unwrap();
        let l = LoopId::parse("loop-1").unwrap();
        s.append(&l, &obs("kept", Finding::Upheld, "cand-a")).unwrap();
        s.append(&l, &obs("lost", Finding::Upheld, "cand-a")).unwrap();
        s.append(&l, &obs("lost", Finding::Refuted, "cand-a")).unwrap();
        s.append(&l, &obs("unmeasured", Finding::Unobserved, "cand-a")).unwrap();
        s.append(&l, &obs("undecided", Finding::Indeterminate, "cand-a")).unwrap();

        let preserve = s.upheld_for(&CandidateDigest::parse("cand-a").unwrap()).unwrap();
        let names: Vec<&str> = preserve.iter().map(|c| c.as_str()).collect();
        // Only what is *currently* upheld. A claim upheld and later refuted is not preserved, and
        // the two non-verdict findings are not preservation constraints either — a claim nobody
        // measured is not a claim anybody may rely on.
        assert_eq!(names, ["kept"]);
    }

    #[test]
    fn evidence_does_not_leak_across_candidates() {
        // §3.4.3's requirement, in the reading direction: an observation made against one
        // candidate must not become a preservation constraint for another. Without the positive
        // control on the second line, a store that ignored the candidate entirely would pass.
        let p = tmp();
        let _ = std::fs::remove_file(&p);
        let s = EvidenceState::open(&p).unwrap();
        let l = LoopId::parse("loop-1").unwrap();
        s.append(&l, &obs("only.on.a", Finding::Upheld, "cand-a")).unwrap();
        assert_eq!(s.upheld_for(&CandidateDigest::parse("cand-b").unwrap()).unwrap(), []);
        assert_eq!(s.upheld_for(&CandidateDigest::parse("cand-a").unwrap()).unwrap().len(), 1);
    }

    // ── Phase 4: the loop controls, written before any role exists ──────────────────────────
    //
    // arXiv 2609.01481 has no controls anywhere: its QA role can invent a match or invent a
    // difference and the evidence bundle looks identical either way. These three are the checks
    // that make this loop able to fail, and each one is written so that removing the distinction
    // it protects makes it fail rather than pass.

    #[test]
    fn control_an_unobserved_candidate_upholds_nothing() {
        // §K-8: an empty store passes every cross-store test. A QA invocation that observed
        // nothing must not leave a planner believing anything holds — the absence of evidence is
        // not evidence, and this is the shape in which that error is cheapest to make.
        let p = tmp();
        let _ = std::fs::remove_file(&p);
        let s = EvidenceState::open(&p).unwrap();
        let unseen = CandidateDigest::parse("never-observed").unwrap();

        let standing = s.standing(&unseen).unwrap();
        assert_eq!(standing, Standing::default(), "nothing observed, so nothing in any set");
        assert!(standing.preserve.is_empty(), "an unobserved candidate upholds nothing");
        // And the positive control, so this is not passing because the store is simply broken:
        // a candidate that WAS observed reports what was seen.
        let l = LoopId::parse("loop-1").unwrap();
        s.append(&l, &obs("seen", Finding::Upheld, "observed")).unwrap();
        let seen = s.standing(&CandidateDigest::parse("observed").unwrap()).unwrap();
        assert_eq!(seen.preserve.len(), 1);
    }

    #[test]
    fn control_a_broken_claim_is_repair_work_and_says_which() {
        // A refuted claim must reach the planner as repair, named — not as a count, and not
        // merged with anything else. "Something failed" is not a plan.
        let p = tmp();
        let _ = std::fs::remove_file(&p);
        let s = EvidenceState::open(&p).unwrap();
        let l = LoopId::parse("loop-1").unwrap();
        let c = CandidateDigest::parse("cand-a").unwrap();
        s.append(&l, &obs("works", Finding::Upheld, "cand-a")).unwrap();
        s.append(&l, &obs("broken", Finding::Refuted, "cand-a")).unwrap();

        let standing = s.standing(&c).unwrap();
        assert_eq!(standing.repair.iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["broken"]);
        // The working one is not repair work, and the broken one is not a preservation constraint.
        assert_eq!(standing.preserve.iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["works"]);
    }

    #[test]
    fn control_a_blocked_claim_is_never_planned_as_repair() {
        // **The control this whole design exists for.** A planner handed `verified ∪ gap` spends
        // an iteration repairing a claim nobody measured, and then reads its own repair as
        // progress. Refuted and Unobserved must never share a set, and Indeterminate — observed
        // but undecidable — is a third thing again: a person reads it, nobody fixes it.
        let p = tmp();
        let _ = std::fs::remove_file(&p);
        let s = EvidenceState::open(&p).unwrap();
        let l = LoopId::parse("loop-1").unwrap();
        let c = CandidateDigest::parse("cand-a").unwrap();
        s.append(&l, &obs("really.broken", Finding::Refuted, "cand-a")).unwrap();
        s.append(&l, &obs("never.ran", Finding::Unobserved, "cand-a")).unwrap();
        s.append(&l, &obs("cannot.tell", Finding::Indeterminate, "cand-a")).unwrap();

        let standing = s.standing(&c).unwrap();
        let names = |v: &[ClaimId]| v.iter().map(|c| c.as_str().to_owned()).collect::<Vec<_>>();
        assert_eq!(names(&standing.repair), ["really.broken"]);
        assert_eq!(names(&standing.unmeasured), ["never.ran"]);
        assert_eq!(names(&standing.unresolved), ["cannot.tell"]);
        // Stated as the property rather than as three equalities, because it is the property that
        // matters: nothing a planner would fix was merely unmeasured.
        for blocked in standing.unmeasured.iter().chain(standing.unresolved.iter()) {
            assert!(
                !standing.repair.contains(blocked),
                "{} reached the repair set",
                blocked.as_str()
            );
        }
        // A merged bucket would have three entries; the point is that it does not exist.
        assert_eq!(
            standing.repair.len() + standing.unmeasured.len() + standing.unresolved.len(),
            3
        );
    }
}
