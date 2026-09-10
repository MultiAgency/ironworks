//! Content-free, time-bounded evidence that one runtime member was measured at policy.
//!
//! Parsing and semantic validation are pure. Reading a file, checking its custody, asking the
//! runtime, and reading the clock are effects owned by callers outside `domain`.

use crate::{CapabilityProfile, RuntimeIdentity, Slug};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: u32 = 1;
pub const VALIDITY_SECONDS: i64 = 10 * 60;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn parse(raw: impl Into<String>) -> Result<Self, AttestationError> {
        let raw = raw.into();
        if raw.len() != 64
            || !raw.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(AttestationError::Digest);
        }
        Ok(Self(raw))
    }

    pub fn digest(domain: &[u8], bytes: &[u8]) -> Self {
        let mut hasher = sha2::Sha256::new();
        hasher.update(domain);
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
        Self(format!("{:x}", hasher.finalize()))
    }

    pub fn profile(profile: &CapabilityProfile) -> Self {
        let access = |value| match value {
            crate::Access::None => b"none".as_slice(),
            crate::Access::ReadOnly => b"read-only".as_slice(),
        };
        let tools = match profile.runtime_tools {
            crate::ToolAccess::None => b"none".as_slice(),
            crate::ToolAccess::Sandbox => b"sandbox".as_slice(),
        };
        let mut bytes = Vec::new();
        for value in [
            access(profile.records),
            access(profile.writes),
            access(profile.egress),
            access(profile.outreach),
            tools,
        ] {
            bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
            bytes.extend_from_slice(value);
        }
        Self::digest(b"ironworks/capability-profile/v1\0", &bytes)
    }

    /// Fingerprint the complete normalized settings catalogue returned by one read.
    pub fn catalogue(
        states: &BTreeMap<String, String>,
        locked: &BTreeSet<String>,
        auto_approve: bool,
    ) -> Self {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(states.len() as u64).to_be_bytes());
        for (tool, state) in states {
            for value in [tool.as_bytes(), state.as_bytes()] {
                bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
                bytes.extend_from_slice(value);
            }
            bytes.push(u8::from(locked.contains(tool)));
        }
        bytes.push(u8::from(auto_approve));
        Self::digest(b"ironworks/settings-catalogue/v1\0", &bytes)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A UTC instant in the one fixed-width form this system records and orders by.
///
/// `YYYY-MM-DDTHH:MM:SS+00:00`, exactly 25 bytes, and **nothing on either side of it**.
///
/// # Why the offset is part of the type
///
/// Every column holding one of these is ordered lexically by SQLite, and that is only the
/// chronological order while the offset is fixed and the width never varies. `12:34:56+01:00` and
/// `11:34:56+00:00` are the same moment and sort as two; `12:34:56Z` is that moment again and
/// sorts after both, because `Z` is `0x5A` and `+` is `0x2B`. So a parser that checked the digits
/// and waved the tail through would accept precisely the values that break the ordering.
///
/// # It was called `Utc`, and validated twice
///
/// The name came from the first thing that needed it. It is not about attestation: the turn
/// journal's `first_seen`, the channel journal's `received_at` and the lifecycle ledger's `at` are
/// all the same rule, and `ORDER BY first_seen, key` rests on it.
///
/// `ironworks_lifecycle::ObservedAt` was a **second** parser for that rule and a weaker one — it
/// checked length, punctuation, digits and the offset, and never the calendar, so
/// `2026-13-45T99:99:99+00:00` parsed. It is now this type. One rule, one parser, and the stricter
/// of the two.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Utc {
    /// First, so the derived `Ord` is the lexical order the SQL `ORDER BY` uses.
    text: String,
    unix_seconds: i64,
}

/// A value that is not a UTC instant this system can order by.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{raw:?} is not a UTC instant of the form YYYY-MM-DDTHH:MM:SS+00:00")]
pub struct NotUtc {
    pub raw: String,
}

impl Utc {
    pub fn parse(raw: impl Into<String>) -> Result<Self, NotUtc> {
        let text = raw.into();
        let bytes = text.as_bytes();
        if bytes.len() != 25
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || bytes[10] != b'T'
            || bytes[13] != b':'
            || bytes[16] != b':'
            || &bytes[19..] != b"+00:00"
        {
            return Err(NotUtc { raw: text });
        }
        let number = |from: usize, to: usize| -> Result<i64, NotUtc> {
            let slice = &text[from..to];
            if !slice.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(NotUtc { raw: text.clone() });
            }
            slice.parse().map_err(|_| NotUtc { raw: text.clone() })
        };
        let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
        let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        if !(1..=12).contains(&month)
            || day < 1
            || day > month_days[(month - 1) as usize]
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err(NotUtc { raw: text });
        }
        let unix_seconds =
            days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
        Ok(Self { text, unix_seconds })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The instant `secs` after the epoch, in the one form [`Self::parse`] accepts.
    ///
    /// **Pure, and the clock stays with the caller.** This crate owns no effect, and a host clock is
    /// one; what it does own is the calendar, so the inverse of the conversion `parse` already makes
    /// lives beside it rather than in each binary that reads a clock. There were two, and both
    /// mapped a clock before 1970 to `0` — a false instant that still parses and still sorts, which
    /// is the one kind of wrong value an `ORDER BY` cannot notice.
    ///
    /// Refused past `9999-12-31T23:59:59+00:00`, the last instant a four-digit year can spell.
    pub fn from_unix_seconds(secs: u64) -> Result<Self, NotUtc> {
        const LAST: u64 = 253_402_300_799;
        if secs > LAST {
            return Err(NotUtc { raw: format!("{secs} seconds after the epoch") });
        }
        let secs = secs as i64;
        let (days, rem) = (secs / 86_400, secs % 86_400);
        let (year, month, day) = civil_from_days(days);
        let text = format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
            rem / 3_600,
            rem % 3_600 / 60,
            rem % 60
        );
        // Through `parse` rather than around it, so one validator stands behind every `Utc`.
        Self::parse(text)
    }
}

/// The civil date `days` after 1970-01-01 — the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

impl std::fmt::Display for Utc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinementAttestation {
    slug: Slug,
    member: RuntimeIdentity,
    profile: Sha256Digest,
    policy: Sha256Digest,
    catalogue: Sha256Digest,
    observed_at: Utc,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    version: u32,
    slug: String,
    member_id: String,
    profile_sha256: String,
    policy_sha256: String,
    catalogue_sha256: String,
    observed_at: String,
}

impl ConfinementAttestation {
    pub fn new(
        slug: Slug,
        member: RuntimeIdentity,
        profile: Sha256Digest,
        policy: Sha256Digest,
        catalogue: Sha256Digest,
        observed_at: Utc,
    ) -> Self {
        Self { slug, member, profile, policy, catalogue, observed_at }
    }

    pub fn parse(json: &str) -> Result<Self, AttestationError> {
        let wire: Wire = serde_json::from_str(json).map_err(|_| AttestationError::Malformed)?;
        if wire.version != VERSION {
            return Err(AttestationError::Version(wire.version));
        }
        let observed_at = Utc::parse(wire.observed_at).map_err(|_| AttestationError::Timestamp)?;
        Ok(Self {
            slug: Slug::parse(wire.slug).map_err(|_| AttestationError::Slug)?,
            member: RuntimeIdentity::parse(wire.member_id).map_err(|_| AttestationError::Member)?,
            profile: Sha256Digest::parse(wire.profile_sha256)?,
            policy: Sha256Digest::parse(wire.policy_sha256)?,
            catalogue: Sha256Digest::parse(wire.catalogue_sha256)?,
            observed_at,
        })
    }

    pub fn to_json(&self) -> Result<String, AttestationError> {
        serde_json::to_string_pretty(&Wire {
            version: VERSION,
            slug: self.slug.as_str().to_owned(),
            member_id: self.member.as_str().to_owned(),
            profile_sha256: self.profile.as_str().to_owned(),
            policy_sha256: self.policy.as_str().to_owned(),
            catalogue_sha256: self.catalogue.as_str().to_owned(),
            observed_at: self.observed_at.as_str().to_owned(),
        })
        .map(|mut json| {
            json.push('\n');
            json
        })
        .map_err(|_| AttestationError::Malformed)
    }

    pub fn validate_for(
        &self,
        slug: &Slug,
        member: &RuntimeIdentity,
        profile: &Sha256Digest,
        policy: &Sha256Digest,
        now: &Utc,
    ) -> Result<(), AttestationError> {
        if &self.slug != slug {
            return Err(AttestationError::WrongSlug);
        }
        if &self.member != member {
            return Err(AttestationError::WrongMember);
        }
        if &self.profile != profile {
            return Err(AttestationError::WrongProfile);
        }
        if &self.policy != policy {
            return Err(AttestationError::WrongPolicy);
        }
        if now.unix_seconds < self.observed_at.unix_seconds {
            return Err(AttestationError::FutureDated);
        }
        if now.unix_seconds >= self.observed_at.unix_seconds + VALIDITY_SECONDS {
            return Err(AttestationError::Expired);
        }
        Ok(())
    }

    pub fn slug(&self) -> &Slug {
        &self.slug
    }
    pub fn member(&self) -> &RuntimeIdentity {
        &self.member
    }
    pub fn profile_sha256(&self) -> &Sha256Digest {
        &self.profile
    }
    pub fn policy_sha256(&self) -> &Sha256Digest {
        &self.policy
    }
    pub fn catalogue_sha256(&self) -> &Sha256Digest {
        &self.catalogue
    }
    pub fn observed_at(&self) -> &Utc {
        &self.observed_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttestationError {
    #[error("attestation is not valid versioned JSON")]
    Malformed,
    #[error("attestation version {0} is unsupported")]
    Version(u32),
    #[error("attestation contains an invalid slug")]
    Slug,
    #[error("attestation contains an invalid member id")]
    Member,
    #[error("attestation contains an invalid SHA-256 digest")]
    Digest,
    #[error("attestation contains an invalid UTC timestamp")]
    Timestamp,
    #[error("attestation belongs to another tenant")]
    WrongSlug,
    #[error("attestation belongs to another runtime member")]
    WrongMember,
    #[error("attestation was measured for another capability profile")]
    WrongProfile,
    #[error("attestation was measured for another confinement policy")]
    WrongPolicy,
    #[error("attestation observation is future-dated")]
    FutureDated,
    #[error("attestation has expired")]
    Expired,
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * adjusted_month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instant_from_the_clock_is_the_one_parse_accepts() {
        // The epoch, a leap day, an ordinary day and the last instant, each against the spelling
        // `parse` holds it to — so the inverse cannot drift from the conversion it inverts.
        for (secs, text) in [
            (0, "1970-01-01T00:00:00+00:00"),
            (951_782_400, "2000-02-29T00:00:00+00:00"),
            (1_788_998_400, "2026-09-10T00:00:00+00:00"),
            (253_402_300_799, "9999-12-31T23:59:59+00:00"),
        ] {
            assert_eq!(Utc::from_unix_seconds(secs).unwrap(), Utc::parse(text).unwrap(), "{secs}");
        }
        // Past the last instant a four-digit year spells: refused, never wrapped or widened.
        assert!(Utc::from_unix_seconds(253_402_300_800).is_err());
    }

    #[test]
    fn the_calendar_round_trips_through_both_directions() {
        for days in (-1_000..1_000).chain((0..2_932_897).step_by(997)) {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days, "{year}-{month}-{day}");
        }
    }

    fn attestation() -> ConfinementAttestation {
        ConfinementAttestation::new(
            Slug::parse("acme").unwrap(),
            RuntimeIdentity::parse("member-1").unwrap(),
            Sha256Digest::profile(&CapabilityProfile::READ_ONLY_RECORDS),
            Sha256Digest::digest(b"policy", b"one"),
            Sha256Digest::digest(b"catalogue", b"one"),
            Utc::parse("2026-09-06T12:00:00+00:00").unwrap(),
        )
    }

    #[test]
    fn valid_evidence_round_trips_and_accepts_only_its_window() {
        let attestation = attestation();
        let parsed = ConfinementAttestation::parse(&attestation.to_json().unwrap()).unwrap();
        assert_eq!(parsed, attestation);
        assert!(
            parsed
                .validate_for(
                    parsed.slug(),
                    parsed.member(),
                    parsed.profile_sha256(),
                    parsed.policy_sha256(),
                    &Utc::parse("2026-09-06T12:09:59+00:00").unwrap(),
                )
                .is_ok()
        );
        assert_eq!(
            parsed.validate_for(
                parsed.slug(),
                parsed.member(),
                parsed.profile_sha256(),
                parsed.policy_sha256(),
                &Utc::parse("2026-09-06T12:10:00+00:00").unwrap(),
            ),
            Err(AttestationError::Expired)
        );
    }

    #[test]
    fn wrong_bindings_and_future_dates_are_each_refused() {
        let attestation = attestation();
        let now = Utc::parse("2026-09-06T12:00:01+00:00").unwrap();
        let other_slug = Slug::parse("other").unwrap();
        let other_member = RuntimeIdentity::parse("member-2").unwrap();
        let other_digest = Sha256Digest::digest(b"other", b"other");
        assert_eq!(
            attestation.validate_for(
                &other_slug,
                attestation.member(),
                attestation.profile_sha256(),
                attestation.policy_sha256(),
                &now,
            ),
            Err(AttestationError::WrongSlug)
        );
        assert_eq!(
            attestation.validate_for(
                attestation.slug(),
                &other_member,
                attestation.profile_sha256(),
                attestation.policy_sha256(),
                &now,
            ),
            Err(AttestationError::WrongMember)
        );
        assert_eq!(
            attestation.validate_for(
                attestation.slug(),
                attestation.member(),
                &other_digest,
                attestation.policy_sha256(),
                &now,
            ),
            Err(AttestationError::WrongProfile)
        );
        assert_eq!(
            attestation.validate_for(
                attestation.slug(),
                attestation.member(),
                attestation.profile_sha256(),
                &other_digest,
                &now,
            ),
            Err(AttestationError::WrongPolicy)
        );
        assert_eq!(
            attestation.validate_for(
                attestation.slug(),
                attestation.member(),
                attestation.profile_sha256(),
                attestation.policy_sha256(),
                &Utc::parse("2026-09-06T11:59:59+00:00").unwrap(),
            ),
            Err(AttestationError::FutureDated)
        );
    }

    #[test]
    fn malformed_digest_timestamp_and_unknown_fields_fail_closed() {
        let json = attestation().to_json().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["profile_sha256"] = "not-a-digest".into();
        assert_eq!(
            ConfinementAttestation::parse(&value.to_string()),
            Err(AttestationError::Digest)
        );
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["extra"] = true.into();
        assert_eq!(
            ConfinementAttestation::parse(&value.to_string()),
            Err(AttestationError::Malformed)
        );
    }
}
