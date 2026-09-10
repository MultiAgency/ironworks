//! `D_t` — one bounded increment, and what it must not break.
//!
//! # Why the preservation set is derived and not written
//!
//! arXiv 2609.01481 §3.4.1 asks a planner to "identify verified functionality to preserve" and
//! writes that into the development document as prose. That makes preservation a thing a planner
//! can *forget*, and forgetting it is invisible — the increment simply breaks something and the
//! next iteration reads the breakage as new work.
//!
//! Here it is computed from the evidence state instead
//! ([`EvidenceState::upheld_for`](crate::state::EvidenceState::upheld_for)), so
//! [`Increment::preserving`] cannot produce a document that omits a claim standing as upheld
//! against the candidate it plans against. A planner chooses **scope**; it does not choose what to
//! protect.
//!
//! # Why the scope is capped
//!
//! §3.4.1 again: the objective must be "bounded but locally complete", because "when the resulting
//! candidate fails, the source of the failure becomes harder to localize". The paper enforces that
//! with a prompt asking for "at most three achievable priorities". A prompt is not an enforcement,
//! so [`MAX_SCOPE`] is, and [`Increment::new`] refuses rather than truncates — silently dropping
//! the fourth task would produce a document that says less than the planner meant.

use crate::state::{CandidateDigest, ClaimId, EvidenceError, EvidenceState, LoopId};

/// The most tasks one increment may carry. Three, following §3.4.1's own bound.
pub const MAX_SCOPE: usize = 3;

/// The longest a task line may be. A task is a sentence, not a document: anything longer is a
/// plan that has stopped being one increment.
pub const TASK_MAX: usize = 240;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("an increment needs at least one task")]
    Empty,
    #[error("{count} tasks, but an increment carries at most {MAX_SCOPE}")]
    TooBroad { count: usize },
    #[error("a task is {len} bytes, over the {TASK_MAX} an increment allows")]
    TaskTooLong { len: usize },
    #[error("a task is empty")]
    TaskEmpty,
    #[error("a task carries a line break or control character, so it could write the document")]
    TaskNotALine,
    #[error(transparent)]
    Evidence(#[from] EvidenceError),
}

/// One thing this increment sets out to do. Prose, deliberately — unlike everything in
/// [`crate::state`], a development document is read by a person and by a planner, not stored as
/// evidence about a tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task(String);

impl Task {
    pub fn parse(raw: impl Into<String>) -> Result<Self, PlanError> {
        let raw = raw.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(PlanError::TaskEmpty);
        }
        if trimmed.len() > TASK_MAX {
            return Err(PlanError::TaskTooLong { len: trimmed.len() });
        }
        // A line, and only a line. `Increment::render` writes each task as one list item under
        // `## Scope`, so a line break would let a task write `## Preserve — derived from evidence,
        // not chosen` itself — forging the one section this module exists to keep out of a
        // planner's hands.
        if trimmed.chars().any(char::is_control) {
            return Err(PlanError::TaskNotALine);
        }
        Ok(Self(trimmed.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `D_t`: the scope of one increment, what it must preserve, and what would show it worked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Increment {
    loop_id: LoopId,
    candidate: CandidateDigest,
    scope: Vec<Task>,
    preserve: Vec<ClaimId>,
    validate: Vec<ClaimId>,
}

impl Increment {
    /// Build against a preservation set the caller supplies. Prefer [`Self::preserving`], which
    /// takes it from the evidence rather than from a caller who may have computed it by hand.
    pub fn new(
        loop_id: LoopId,
        candidate: CandidateDigest,
        scope: Vec<Task>,
        preserve: Vec<ClaimId>,
        validate: Vec<ClaimId>,
    ) -> Result<Self, PlanError> {
        if scope.is_empty() {
            return Err(PlanError::Empty);
        }
        if scope.len() > MAX_SCOPE {
            return Err(PlanError::TooBroad { count: scope.len() });
        }
        Ok(Self { loop_id, candidate, scope, preserve, validate })
    }

    /// Build with the preservation set **read from the evidence state**.
    ///
    /// The whole point of the type: a planner supplies scope and validation, and what must not
    /// regress comes from what was actually observed to hold against this candidate.
    pub fn preserving(
        state: &EvidenceState,
        loop_id: LoopId,
        candidate: CandidateDigest,
        scope: Vec<Task>,
        validate: Vec<ClaimId>,
    ) -> Result<Self, PlanError> {
        let preserve = state.upheld_for(&candidate)?;
        Self::new(loop_id, candidate, scope, preserve, validate)
    }

    pub fn loop_id(&self) -> &LoopId {
        &self.loop_id
    }
    pub fn candidate(&self) -> &CandidateDigest {
        &self.candidate
    }
    pub fn scope(&self) -> &[Task] {
        &self.scope
    }
    /// Claims that must still hold after this increment.
    pub fn preserve(&self) -> &[ClaimId] {
        &self.preserve
    }
    /// Claims this increment sets out to establish.
    pub fn validate(&self) -> &[ClaimId] {
        &self.validate
    }

    /// The document, as the next invocation reads it.
    ///
    /// Markdown rather than JSON because a development document is the one artifact here a person
    /// reads directly, and §3.3's progressive disclosure has the next role reading it as text.
    pub fn render(&self) -> String {
        let list = |items: &[String]| {
            if items.is_empty() {
                "- (none)\n".to_owned()
            } else {
                items.iter().map(|i| format!("- {i}\n")).collect()
            }
        };
        format!(
            "# Increment {loop_id}\n\nCandidate: `{candidate}`\n\n## Scope\n\n{scope}\n\
             ## Preserve — derived from evidence, not chosen\n\n{preserve}\n\
             ## Validate\n\n{validate}",
            loop_id = self.loop_id.as_str(),
            candidate = self.candidate.as_str(),
            scope = list(&self.scope.iter().map(|t| t.as_str().to_owned()).collect::<Vec<_>>()),
            preserve =
                list(&self.preserve.iter().map(|c| c.as_str().to_owned()).collect::<Vec<_>>()),
            validate =
                list(&self.validate.iter().map(|c| c.as_str().to_owned()).collect::<Vec<_>>()),
        )
    }

    /// Write the document where the next invocation will read it: `<dir>/<loop_id>.md`.
    ///
    /// **Markdown on disk, and no parser back.** §3.3's progressive disclosure has the next role
    /// reading this as text, so a round trip through a machine format would serve nobody — and the
    /// half that *is* machine-readable does not need parsing anyway, because
    /// [`Self::preserving`] rebuilds it from the evidence state. What the file carries that
    /// nothing else can reconstruct is the scope a planner chose.
    ///
    /// Refuses to overwrite. An increment is written once per loop; a second write under the same
    /// id means two planners believe they own the same iteration, and picking a winner silently is
    /// how one of them loses work it will not know to redo.
    pub fn write_to(
        &self,
        dir: impl AsRef<std::path::Path>,
    ) -> std::io::Result<std::path::PathBuf> {
        let path = dir.as_ref().join(format!("{}.md", self.loop_id.as_str()));
        if path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already has an increment", self.loop_id.as_str()),
            ));
        }
        std::fs::write(&path, self.render())?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Observation, SubjectId};
    use ironworks_domain::Finding;

    fn state() -> EvidenceState {
        let d = std::env::temp_dir().join(format!("ironworks-plan-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("e.db");
        let _ = std::fs::remove_file(&p);
        EvidenceState::open(&p).unwrap()
    }

    fn task(s: &str) -> Task {
        Task::parse(s).unwrap()
    }
    fn claim(s: &str) -> ClaimId {
        ClaimId::parse(s).unwrap()
    }

    #[test]
    fn an_increment_is_bounded_and_refuses_rather_than_truncates() {
        let l = LoopId::parse("loop-1").unwrap();
        let c = CandidateDigest::parse("cand-a").unwrap();
        let four = vec![task("a"), task("b"), task("c"), task("d")];
        assert_eq!(
            Increment::new(l.clone(), c.clone(), four, vec![], vec![]),
            Err(PlanError::TooBroad { count: 4 })
        );
        // Truncating would produce a document that says less than the planner meant, silently.
        assert_eq!(
            Increment::new(l.clone(), c.clone(), vec![], vec![], vec![]),
            Err(PlanError::Empty)
        );
        // Positive control: three is the bound, not an off-by-one.
        assert!(
            Increment::new(l, c, vec![task("a"), task("b"), task("c")], vec![], vec![]).is_ok()
        );
    }

    #[test]
    fn a_task_is_a_sentence_not_a_document() {
        assert!(Task::parse("x".repeat(TASK_MAX + 1)).is_err());
        assert!(Task::parse("  ").is_err());
        // A task carrying a line break could write a section of the document it is rendered into —
        // here, the one section a planner is never allowed to choose.
        assert_eq!(
            Task::parse("close the gap\n## Preserve — derived from evidence, not chosen\n- (none)"),
            Err(PlanError::TaskNotALine)
        );
        assert_eq!(Task::parse("close\tthe gap"), Err(PlanError::TaskNotALine));
        assert_eq!(task(" close the gap ").as_str(), "close the gap");
    }

    #[test]
    fn preservation_is_taken_from_evidence_and_cannot_be_omitted() {
        // The reason this type exists. A planner supplies scope and validation; what must not
        // regress is read from what was observed, so an increment cannot be written that forgets
        // a claim standing as upheld against the candidate it plans against.
        let s = state();
        let l = LoopId::parse("loop-1").unwrap();
        let c = CandidateDigest::parse("cand-a").unwrap();
        for (name, finding) in [
            ("holds.one", Finding::Upheld),
            ("holds.two", Finding::Upheld),
            ("broken", Finding::Refuted),
            ("never.ran", Finding::Unobserved),
        ] {
            s.append(
                &l,
                &Observation {
                    claim: claim(name),
                    subject: SubjectId::parse("subject").unwrap(),
                    finding,
                    candidate: c.clone(),
                    observed_at: ironworks_domain::Utc::parse("2026-09-10T00:00:00+00:00").unwrap(),
                },
            )
            .unwrap();
        }

        let d_t = Increment::preserving(
            &s,
            l,
            c,
            vec![task("fix the broken one")],
            vec![claim("broken")],
        )
        .unwrap();

        let preserved: Vec<&str> = d_t.preserve().iter().map(|c| c.as_str()).collect();
        assert_eq!(preserved, ["holds.one", "holds.two"]);
        // The planner named none of those and could not have dropped them.
        assert_eq!(d_t.validate().iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["broken"]);
        // And the rendered document says where the set came from, so a reader does not mistake it
        // for something the planner chose.
        assert!(d_t.render().contains("derived from evidence, not chosen"));
        assert!(d_t.render().contains("holds.one"));

        // On disk where the next role reads it, and once only.
        let dir = std::env::temp_dir().join(format!("ironworks-dt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("loop-1.md"));
        let written = d_t.write_to(&dir).unwrap();
        assert!(std::fs::read_to_string(&written).unwrap().contains("holds.two"));
        // A second planner claiming the same iteration is refused rather than allowed to
        // overwrite work the first one will not know to redo.
        assert_eq!(d_t.write_to(&dir).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    }
}
