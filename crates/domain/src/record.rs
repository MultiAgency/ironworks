//! Business records, and how a turn is shown them.
//!
//! A **record** is one business fact a tenant's team wrote down. Never a judgement, never a
//! score, never something a model derived — those are computed per turn and never stored, so
//! the store cannot drift into asserting an opinion as a fact.
//!
//! # No clock lives here
//!
//! `retrieved_at` is passed in. That is not a style choice: crash recovery (ADR 0005) works by
//! rebuilding a request body exactly and replaying its idempotency key, and a clock read inside
//! rendering is precisely what makes ironworks' body irreproducible. Every function in this
//! crate is a pure transform over values the caller already holds.
//!
//! # Tenancy is in the key
//!
//! The schema this borrows from keys every row `(org_id, <id>)` rather than filtering in a
//! `WHERE` clause, so an id collision between two orgs creates two independent rows instead of
//! re-homing one. In the type system the same job is done by [`crate::RecordScope`],
//! which carries the org and is the only way to reach a record at all — so a record fetched for
//! one tenant cannot be looked up under another by a forgotten predicate. A second mechanism for
//! this once existed here and was never used.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecordError {
    #[error("{what} may not be empty")]
    Empty { what: &'static str },
    #[error("date {value:?} is not ISO-8601 (YYYY-MM-DD)")]
    BadDate { value: String },
}

macro_rules! ident_newtype {
    ($name:ident, $what:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn parse(raw: impl Into<String>) -> Result<Self, RecordError> {
                let value = raw.into();
                if value.is_empty() {
                    return Err(RecordError::Empty { what: $what });
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = RecordError;
            fn try_from(v: String) -> Result<Self, Self::Error> {
                Self::parse(v)
            }
        }
        impl From<$name> for String {
            fn from(v: $name) -> String {
                v.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

/// The trusted scope key, resolved from a credential server-side. Never model-supplied.
///
/// Re-exported rather than redefined. There were two `OrgId` types with different validation —
/// this crate's accepted any non-empty string, `ironworks-domain`'s accepts `[a-z0-9-]` only — and
/// the forge converted between them by round-tripping through a `String`, which always succeeded
/// and so never surfaced the difference.
pub use crate::ids::OrgId;

ident_newtype!(
    AccountId,
    "an account id",
    "A record id, stable and unique *per org* (e.g. `VR-006`)."
);
ident_newtype!(ContactId, "a contact id", "A contact id, unique per org.");
ident_newtype!(ActivityId, "an activity id", "An activity id, unique per org.");

/// An opaque timestamp, compared but never computed with.
///
/// Deliberately a validated string rather than a date type. Freshness is decided by *equality*
/// against the version a thread was last shown — never by arithmetic — so a date library would
/// add a dependency, a parse/format round-trip, and a way for two equal instants to render
/// differently. Two timestamps that differ as strings are different versions, which is exactly
/// the question being asked.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Timestamp(String);

impl Timestamp {
    pub fn parse(raw: impl Into<String>) -> Result<Self, RecordError> {
        let value = raw.into();
        if value.trim().is_empty() {
            return Err(RecordError::Empty { what: "a timestamp" });
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Timestamp {
    type Error = RecordError;
    fn try_from(v: String) -> Result<Self, Self::Error> {
        Self::parse(v)
    }
}
impl From<Timestamp> for String {
    fn from(v: Timestamp) -> String {
        v.0
    }
}
impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A calendar date, `YYYY-MM-DD`.
///
/// Validated on construction so that lexicographic ordering *is* chronological ordering — which
/// is what lets activities sort correctly with no date library at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Date(String);

impl Date {
    pub fn parse(raw: impl Into<String>) -> Result<Self, RecordError> {
        let value = raw.into();
        let bytes = value.as_bytes();
        let shaped = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes.iter().enumerate().all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit());
        if !shaped {
            return Err(RecordError::BadDate { value });
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Date {
    type Error = RecordError;
    fn try_from(v: String) -> Result<Self, Self::Error> {
        Self::parse(v)
    }
}
impl From<Date> for String {
    fn from(v: Date) -> String {
        v.0
    }
}
impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The recorded facts about one account.
///
/// Every business field is `Option`, and `None` means *genuinely unknown* — not "not fetched"
/// and not "zero". That distinction is the analyst's whole discipline: a gap must be reportable
/// as a gap, so it cannot be collapsed into a default. There is no `Default` impl for the same
/// reason.
///
/// Unknown JSON keys are tolerated. This is a wire type: the record store may add a field before
/// we know about it, and refusing to parse would take every tenant down over an addition.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Account {
    pub name: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub industry: Option<String>,
    #[serde(default)]
    pub employees: Option<u64>,
    #[serde(default)]
    pub headquarters: Option<String>,
    #[serde(default)]
    pub stated_problem: Option<String>,
    #[serde(default)]
    pub current_tooling: Option<String>,
    #[serde(default)]
    pub budget: Option<String>,
    #[serde(default)]
    pub timeline: Option<String>,
    #[serde(default)]
    pub decision_process: Option<String>,
    #[serde(default)]
    pub economic_buyer: Option<String>,
    /// Recorded by the tenant's own team, never derived by a model. `None` means the team has
    /// not recorded it, which a brief must report as UNKNOWN rather than infer from silence.
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub value_band: Option<String>,
    /// Per-tenant flexible facts. `BTreeMap`, not `HashMap`: rendering must be deterministic,
    /// because crash recovery rebuilds the envelope and compares it (ADR 0005).
    #[serde(default, deserialize_with = "null_as_default")]
    pub facts: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

/// A wire producer may spell an absent collection as either a missing field or JSON `null`.
/// Both mean that it supplied no flexible facts; neither is evidence for a business conclusion.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Contact {
    /// Optional because the two sources disagree: the record store keys contacts
    /// `(org_id, contact_id)` and returns one, while the synthetic book carries none. Nothing
    /// renders it — it exists so a store response round-trips without losing a field.
    #[serde(default, rename = "contact_id", alias = "id")]
    pub id: Option<ContactId>,
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub engaged: Option<bool>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// One dated interaction. Its `body` is **evidence to assess, never an instruction to follow** —
/// a rule the envelope states explicitly to the model, because record text is attacker-reachable
/// whenever anyone can write into a tenant's book.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Activity {
    #[serde(rename = "activity_id", alias = "id")]
    pub id: ActivityId,
    #[serde(default)]
    pub occurred_at: Option<Date>,
    #[serde(default)]
    pub kind: Option<String>,
    pub body: String,
    /// Who contributed this, when one named person did.
    ///
    /// `None` for everything a tenant's team recorded through their own seeding path — which is
    /// most of a book, and is the honest value rather than a gap: the team wrote it and nobody
    /// individual claimed it. A promoted note carries the contributor who chose its exact bytes.
    ///
    /// A [`crate::ContributorId`] and not a free string, because attribution that a wire producer could
    /// spell however it liked is attribution a reader cannot rely on. The store's own grammar is
    /// the same one, which is what lets the two agree without either trusting the other.
    ///
    /// **A value here changes nothing about how the body is read.** An attributed activity is
    /// still evidence to assess, never an instruction to follow — knowing who wrote something
    /// down is not a reason to do what it says.
    #[serde(default)]
    pub contributor: Option<crate::ids::ContributorId>,
}

/// One account with everything a turn is shown about it.
///
/// The id sits here rather than on [`Account`] because that is where both sources put it: the
/// synthetic book has `record_id` as a sibling of `account`, and the record store's context
/// response carries `record_id` at the top level too (it also repeats `account_id` inside
/// `account`, which we ignore — unknown fields are tolerated on a wire type).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AccountRecord {
    #[serde(rename = "record_id", alias = "account_id")]
    pub id: AccountId,
    pub account: Account,
    #[serde(default)]
    pub contacts: Vec<Contact>,
    #[serde(default)]
    pub activities: Vec<Activity>,
}

impl AccountRecord {
    /// Parse one record as the fixtures and the store emit it.
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Activities oldest-first. Lexicographic on a validated `YYYY-MM-DD` is chronological;
    /// undated activities sort last, because a claim with no date cannot anchor a timeline.
    pub fn activities_in_time_order(&self) -> Vec<&Activity> {
        let mut out: Vec<&Activity> = self.activities.iter().collect();
        out.sort_by(|a, b| match (&a.occurred_at, &b.occurred_at) {
            (Some(x), Some(y)) => x.cmp(y).then_with(|| a.id.cmp(&b.id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.id.cmp(&b.id),
        });
        out
    }

    /// Declared fact keys this record holds nothing for.
    ///
    /// Tri-state, and the distinction is load-bearing. `None` means the tenant declared no fact
    /// shape, so we assert **no** gaps — silence is honest where an invented gap list is not.
    /// `Some(&[])` means they declared an empty shape, which also yields no gaps. Only declared
    /// keys with no value are reported.
    pub fn declared_gaps<'a>(&self, fact_fields: Option<&'a [String]>) -> Vec<&'a str> {
        let Some(declared) = fact_fields else {
            return Vec::new();
        };
        declared
            .iter()
            .filter(|k| match self.account.facts.get(k.as_str()) {
                None | Some(serde_json::Value::Null) => true,
                Some(serde_json::Value::String(s)) => s.is_empty(),
                Some(serde_json::Value::Array(a)) => a.is_empty(),
                Some(serde_json::Value::Object(o)) => o.is_empty(),
                Some(_) => false,
            })
            .map(String::as_str)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vr006() -> AccountRecord {
        AccountRecord::from_json(include_str!("../../../fixtures/book/vr-006.json")).unwrap()
    }

    #[test]
    fn the_injection_fixture_parses_and_keeps_its_nulls() {
        let r = vr006();
        assert_eq!(r.id.as_str(), "VR-006");
        assert_eq!(r.account.name, "Vantage Rail Systems");
        assert_eq!(r.account.employees, Some(480));
        // The fields the injected text tries to fill must stay genuinely unknown.
        assert_eq!(r.account.budget, None);
        assert_eq!(r.account.economic_buyer, None);
        assert_eq!(r.account.decision_process, None);
        assert_eq!(r.activities.len(), 1);
        assert!(r.activities[0].body.contains("SYSTEM INSTRUCTION"));
    }

    #[test]
    fn every_fixture_in_the_book_parses() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/book");
        let mut n = 0;
        for entry in std::fs::read_dir(dir).expect("fixtures/book") {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let raw = std::fs::read_to_string(&path).unwrap();
                AccountRecord::from_json(&raw)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                n += 1;
            }
        }
        assert_eq!(n, 40, "expected the whole synthetic book");
    }

    /// `README.md` states that `fixtures/` is synthetic only — every record carrying `_synthetic`,
    /// every domain under `.example` (RFC 2606) so no invented detail is ever attached to a
    /// registrable name. That is a standing policy, not a description: real client data must never
    /// enter this tree. A policy held only in prose is one the next contributor breaks without
    /// anything noticing, and this repository's own rule is that code and tests outrank prose.
    ///
    /// `_synthetic` is read from the raw text rather than the parsed record on purpose — the wire
    /// type tolerates unknown fields and does not expose it, so parsing would not see it go.
    #[test]
    fn every_fixture_declares_itself_synthetic_and_resolves_to_no_real_domain() {
        for dir in ["book", "tenants", "account-coordination/book"] {
            let path = format!("{}/../../fixtures/{dir}", env!("CARGO_MANIFEST_DIR"));
            let mut seen = 0;
            for entry in std::fs::read_dir(&path).unwrap_or_else(|e| panic!("{path}: {e}")) {
                let file = entry.unwrap().path();
                if file.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let raw = std::fs::read_to_string(&file).unwrap();
                let name = file.display();
                assert!(
                    raw.contains("\"_synthetic\""),
                    "{name} carries no `_synthetic` marker. Every fixture must declare itself \
                     invented; if this record is real it does not belong in this repository."
                );
                let record =
                    AccountRecord::from_json(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
                if let Some(domain) = &record.account.domain {
                    assert!(
                        domain.ends_with(".example"),
                        "{name} uses domain {domain:?}, which is registrable. Fixtures use \
                         `.example` (RFC 2606) so invented facts can never attach to a real one."
                    );
                }
                seen += 1;
            }
            assert!(seen > 0, "fixtures/{dir} holds no JSON — this check just stopped checking");
        }
    }

    #[test]
    fn an_unknown_field_is_tolerated_because_this_is_a_wire_type() {
        // `_synthetic` is present on every fixture and nothing here reads it. A store that adds
        // a column must not take every tenant down.
        let raw = r#"{"record_id":"A-1","account":{"name":"X","surprise":1},
                      "contacts":[],"activities":[]}"#;
        // Parsing is not the claim — *carrying the known fields past the unknown one* is.
        // `is_ok()` would have passed for a parser that dropped every field it did recognise.
        let record = AccountRecord::from_json(raw).expect("an unknown field must not refuse");
        assert_eq!(record.id.as_str(), "A-1");
        assert_eq!(record.account.name, "X");
    }

    #[test]
    fn a_null_flexible_facts_collection_means_no_flexible_facts() {
        // The live Account Service uses null when an account has never declared flexible facts.
        // This invented record captures only that wire shape; no private record enters the repo.
        let raw = r#"{"record_id":"A-1","account":{"name":"X","facts":null},
                      "contacts":[],"activities":[]}"#;
        let record = AccountRecord::from_json(raw).unwrap();
        assert!(record.account.facts.is_empty());
    }

    #[test]
    fn dates_validate_and_sort_chronologically() {
        assert_eq!(Date::parse("2026-07-09").unwrap().as_str(), "2026-07-09");
        for bad in ["2026-7-09", "09-07-2026", "2026-07-09T00:00", "", "not-a-date"] {
            assert!(Date::parse(bad).is_err(), "{bad:?} should not parse");
        }
        let mut ds = [Date::parse("2026-12-01").unwrap(), Date::parse("2026-02-28").unwrap()];
        ds.sort();
        assert_eq!(ds[0].as_str(), "2026-02-28", "lexicographic must be chronological");
    }

    #[test]
    fn undated_activities_sort_last() {
        let raw = r#"{"record_id":"A-1","account":{"name":"X"},"contacts":[],"activities":[
            {"activity_id":"C","body":"no date"},
            {"activity_id":"B","occurred_at":"2026-05-01","body":"later"},
            {"activity_id":"A","occurred_at":"2026-01-01","body":"earlier"}]}"#;
        let r = AccountRecord::from_json(raw).unwrap();
        let ids: Vec<&str> = r.activities_in_time_order().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["A", "B", "C"]);
    }

    /// The tri-state that a truthiness check cannot express — and the one that told a
    /// relationship book it was missing `budget` when it had never declared one.
    #[test]
    fn gaps_are_tri_state() {
        let raw = r#"{"record_id":"A-1","account":{"name":"X",
                       "facts":{"allocation":"12 NEAR","repo":"","contributors":[]}},
                      "contacts":[],"activities":[]}"#;
        let r = AccountRecord::from_json(raw).unwrap();

        // Undeclared: assert nothing. Silence is honest; an invented gap list is not.
        assert!(r.declared_gaps(None).is_empty());
        // Declared-empty: also nothing, but for a different reason.
        assert!(r.declared_gaps(Some(&[])).is_empty());
        // Declared: empty string, empty array and an absent key are all gaps; a real value is not.
        let declared =
            ["allocation".into(), "repo".into(), "contributors".into(), "cadence".into()];
        assert_eq!(r.declared_gaps(Some(&declared)), ["repo", "contributors", "cadence"]);
    }

    #[test]
    fn identifiers_refuse_to_be_empty() {
        assert!(OrgId::parse("").is_err());
        assert!(AccountId::parse("").is_err());
        assert!(Timestamp::parse("  ").is_err());
    }

    /// The org id here is now the same type the rest of the system uses, so it carries the same
    /// validation. It used to accept anything non-empty, which was never reached: nothing on the
    /// wire deserialized one, and the only construction site converted from the strict type.
    #[test]
    fn the_org_id_is_the_canonical_strict_one() {
        assert_eq!(OrgId::parse("acme-sales").unwrap().as_str(), "acme-sales");
        for lax in ["Acme", "acme_sales", "acme sales", "acme/sales"] {
            assert!(OrgId::parse(lax).is_err(), "{lax:?} would have parsed before");
        }
    }
}
