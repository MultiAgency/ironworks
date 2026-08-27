//! The service-wide evidence rules, applied before any case-specific grader.
//!
//! # Why these run first
//!
//! They are the obligations every answer carries whatever it was asked: don't tag one claim with
//! two evidence tiers, don't assert an absence the record cannot support, don't emit a table the
//! delivery surface destroys, don't move ownership outward uncited, don't miscount your own
//! findings. A case grader that ran first could return PASS on an answer that broke one of them.
//!
//! # Two of them return [`Verdict::Review`], and that is deliberate
//!
//! Uncited counterparty ownership and a finding-count mismatch are both *probably* wrong and not
//! mechanically decidable — the citation may be a sentence away, the count may be counting
//! something else. Reporting them as FAIL would assert a defect the grader cannot establish.

use crate::text::{clauses, near, sentences, unquoted, word_position, words};
use crate::{Graded, Verdict};

/// The record artifacts the absence rule is about.
///
/// Named once and shared by both branches of [`absence_as_negative`], because they ask the same
/// question and two copies would drift the moment one gained an entry. Singular and plural are
/// both listed rather than stemmed: stemming would also match `agreed` and `approvals` of things
/// that are not records.
const ARTIFACT_NOUNS: &[&str] = &[
    "agreement",
    "agreements",
    "extension",
    "extensions",
    "variation",
    "variations",
    "change note",
    "change notes",
    "approval",
    "approvals",
    "contract",
    "contracts",
    "signature",
    "signatures",
    "obligation",
    "obligations",
    "amendment",
    "amendments",
];

/// Phrases that scope an absence to the record, so asserting it claims nothing about the world.
const SCOPED: &[&str] = &[
    "no record show",
    "no records show",
    "nothing in the record",
    "nothing in the records",
    "nothing record",
    "nothing records",
    "not in record",
    "not in the record",
    "not on record",
    "not on the record",
    "not recorded",
    "no recorded",
    "on record",
    "on the record",
    "no evidence",
    "in the book",
    "in the record",
    "in the supplied book",
    "in the supplied record",
    "in the loaded book",
    "in the loaded record",
    "nothing in the book",
    "record does not",
    "records do not",
    "evidence does not",
    // Measured: "No signed renewal agreement on file" was graded an overclaim while the identical
    // claim written "...is recorded" was not (0026). It is the same scoping in the commoner
    // business idiom.
    "on file",
];

/// Words whose presence means a clause is talking about the record rather than the world.
/// Listed with their inflections rather than stemmed. Stemming would reach `evident` and
/// `documentary`; leaving inflections out made the rule turn on whether someone remembered a
/// form — "no onboarding is *evidenced*" was read as a claim about the world because only
/// `evidence` was here (0026).
const NAMES_RECORD: &[&str] = &[
    "record",
    "records",
    "recorded",
    "document",
    "documents",
    "documented",
    "documentation",
    "evidence",
    "evidenced",
    "book",
];

/// Words that scope an absence when they *follow* it.
///
/// A trailing participle qualifies everything before it: "there is no signed agreement, change
/// note, or approval document **recorded** for NC-003" is a statement about the record, and so is
/// "...and no contacts **recorded**." Position is the whole point — the same words appearing
/// *before* an assertion belong to a different clause, which is why the sentence branch cannot
/// simply ask whether a record word is present. That looser test would let "the record shows X and
/// there is no written variation" grade clean, which is the failure `scoped_by_own_noun` was
/// written tight to avoid.
const TRAILING_SCOPE: &[&str] = &["recorded", "documented", "evidenced"];

/// Verbs that ask for verification rather than assert an absence.
///
/// Shared with `cases::offboarding_unknown`, which needs the same distinction for the same reason.
/// Two copies would drift the moment one gained an entry.
pub(crate) const VERIFYING: &[&str] = &[
    "confirm",
    "confirms",
    "confirmed",
    "verify",
    "verifies",
    "verified",
    "check",
    "checks",
    "checked",
    "ensure",
    "ensures",
    "ensured",
];

/// The four evidence tiers the persona teaches.
const TIERS: &[&str] = &["FACT", "STATED", "INFERENCE", "UNKNOWN"];

/// Return a sentence that tags one claim with two different evidence tiers.
///
/// The em-dash is the claim/citation delimiter the persona teaches, so two tiers separated by one
/// are two tagged claims rather than one claim tagged twice. Without honouring that boundary a
/// noun conjunction — "the export and the owner name" — reads as a claim conjunction, and a
/// disciplined answer fails.
///
/// # A tier is a tag, and tags are capitals
///
/// This matched a case-folded copy, so the ordinary adjective in *"a **stated** prerequisite"* was
/// read as the STATED tier and `UNKNOWN … and … stated` looked like two tiers on one claim. One of
/// the twenty answers in `docs/experiments/0022` used the bare word and it is the one that failed —
/// in the service-wide prefilter every case runs through, on a sound answer whose own gates all
/// passed. The persona teaches the tiers as capitalised labels, so requiring capitals is the rule
/// reading what it was always about; the lowercase words are ordinary English and no rule here has
/// any business with them.
///
/// Bounding it does not make the rule vacuous — `two_tiers_joined_on_one_claim_fail` is a real
/// compound in capitals and still fails.
pub fn compound_tier_claim(answer: &str) -> Option<String> {
    for sentence in sentences(answer) {
        let mut hits: Vec<(usize, &str)> = Vec::new();
        for tier in TIERS {
            for at in crate::text::word_positions_cased(&sentence, tier) {
                hits.push((at, tier));
            }
        }
        hits.sort_by_key(|(at, _)| *at);
        for pair in hits.windows(2) {
            let (first_at, first) = pair[0];
            let (second_at, second) = pair[1];
            if first.eq_ignore_ascii_case(second) {
                continue;
            }
            let between = &sentence[first_at + first.len()..second_at];
            // A delimiter between them means two separately tagged claims, not one compound.
            if between.contains('—') || between.contains(';') || between.contains('\n') {
                continue;
            }
            let Some(conj) = ["and", "&", "/"].iter().find_map(|c| conjunction_at(between, c))
            else {
                continue;
            };
            // `[^—;\n]{0,80}` on each side of the conjunction in the original.
            if conj <= 80 && between.len().saturating_sub(conj) <= 80 {
                return Some(sentence);
            }
        }
    }
    None
}

/// Byte offset of `conj` in `text` when it stands as its own token.
fn conjunction_at(text: &str, conj: &str) -> Option<usize> {
    if conj.chars().all(|c| c.is_alphanumeric()) {
        word_position(text, conj)
    } else {
        text.find(conj)
    }
}

/// Return an asserted non-existence that is not scoped to the supplied record.
///
/// # The three assertion forms, and why only one is bounded
///
/// `no X exists` and `X does not exist` name the thing said not to exist, so an unbounded subject
/// is safe — they are existence claims whatever X is.
///
/// `there is no <anything>` is not. It also covers abstractions: *no ambiguity*, *no doubt*, *no
/// conflict*. Unbounded, it graded **"the variation is documented and signed by both parties, so
/// there is no ambiguity"** as an overclaim about the record, in a clause that had just cited the
/// evidence (`docs/experiments/0020`). So that form is bounded to [`ARTIFACT_NOUNS`], exactly as
/// the bare-noun branch below already was.
///
/// Bounding it does not make the branch vacuous, and the test
/// `there_is_no_still_fires_on_a_record_artifact` is what says so.
pub fn absence_as_negative(answer: &str) -> Option<String> {
    for sentence in sentences(answer) {
        if is_scoped(&sentence) {
            continue;
        }
        let Some(at) = asserts_non_existence(&sentence) else {
            continue;
        };
        if scoped_after(&sentence, at) {
            continue;
        }
        if denied_inference(&sentence, at) {
            continue;
        }
        return Some(sentence);
    }
    // The artifact form is judged per clause, not per sentence: "renewal records, and the pilot
    // ends with no signed extension" names the record in one clause and asserts in the other, and
    // a sentence-wide test lets the second hide behind the first.
    for clause in clauses(answer) {
        // Both branches must agree on what "scoped" means. Naming the record scopes a clause, and
        // so does any phrase that scopes a sentence — `on file` is the one SCOPED entry with no
        // record word in it, and consulting only NAMES_RECORD here let it through (0026).
        if crate::text::any_word(&clause, NAMES_RECORD) || is_scoped(&clause) {
            continue;
        }
        let Some(found) = bare_artifact_absence(&clause) else {
            continue;
        };
        // "a human should confirm no other obligations remain" asks for verification rather than
        // asserting, so the verb must *govern* the phrase — its position is checked, not its mere
        // presence in the clause.
        let asks = VERIFYING.iter().filter_map(|v| word_position(&clause, v)).min();
        if asks.is_some_and(|at| at < found) {
            continue;
        }
        return Some(clause);
    }
    None
}

fn is_scoped(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    // Collapse whitespace so "no  records   show" scopes the same as "no records show".
    let flat = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    SCOPED.iter().any(|p| flat.contains(p)) || scoped_by_own_noun(&flat)
}

/// `no … record(s) exist(s)` — the thing said not to exist is the record itself.
///
/// Missing this read one graded answer as an overclaim when it had kept the discipline exactly
/// (`0007`). Deliberately tight: `record` must be what does not exist, not merely a word elsewhere
/// in the sentence, or "the record shows X and there is no written variation" would grade clean.
fn scoped_by_own_noun(flat: &str) -> bool {
    let w = words(flat);
    for (i, word) in w.iter().enumerate() {
        if word != "no" {
            continue;
        }
        // `no\s+[^.!?;]{0,60}\brecords?\s+exists?\b` — bounded lookahead for "record(s) exist(s)".
        for j in i + 1..w.len().min(i + 12) {
            if (w[j] == "record" || w[j] == "records")
                && w.get(j + 1).is_some_and(|n| n == "exist" || n == "exists")
            {
                return true;
            }
        }
    }
    false
}

/// Word index at which this sentence asserts a non-existence, if it does.
///
/// The position is what lets the caller ask whether a trailing participle scopes the claim; a
/// bare bool could not tell "no agreement recorded" from "the record shows X and there is no
/// agreement".
fn asserts_non_existence(sentence: &str) -> Option<usize> {
    let w = words(sentence);
    for (i, word) in w.iter().enumerate() {
        // Form 1: `there is|are no <≤3 words> <artifact>` — bounded, see the doc comment.
        if word == "there"
            && w.get(i + 1).is_some_and(|n| n == "is" || n == "are")
            && w.get(i + 2).is_some_and(|n| n == "no")
            && artifact_within(&w, i + 3, 3)
        {
            return Some(i);
        }
        // Form 2: `no <≤3 words> exists`. The subject stays unbounded in *kind* — this is an
        // existence claim whatever X is — but not in *span*. At twenty words the `no` and the
        // `exists` need not belong to the same clause: "**No** live work should proceed until
        // onboarding evidence **exists**" is a recommendation, and was graded an assertion (0026).
        if word == "no"
            && i + 1 < w.len()
            && w[i + 1..].iter().take(4).any(|n| n == "exist" || n == "exists")
        {
            return Some(i);
        }
        // Form 3: `… does not exist`.
        if word == "does"
            && w.get(i + 1).is_some_and(|n| n == "not")
            && w.get(i + 2).is_some_and(|n| n == "exist")
        {
            return Some(i);
        }
    }
    None
}

/// Whether the assertion at word `at` sits inside an inference the sentence is *denying*.
///
/// # The frame `0028` found
///
/// A held-out run of ten unseen answers produced one failure, and its frame was new:
///
/// > *"a member deletion is not proof that all tokens were revoked; a credential being disabled is
/// > **not proof that no alternate access path exists**."*
///
/// The answer is quoting the service's own evidence discipline back — a disabled credential does
/// **not** license the conclusion that no other path exists. It denies the inference, and the rule
/// read the embedded `no alternate access path exists` as an assertion of it. That is the opposite
/// of the claim being made.
///
/// # Why the shape is this tight
///
/// A negator and an establishing word alone are not enough. *"The record does not establish X, and
/// there is no signed agreement"* has both, in that order, before a genuine overclaim — and
/// `a_record_word_before_the_assertion_does_not_scope_it` exists because a loose test on what
/// precedes an assertion is how this rule went wrong before.
///
/// So the frame must be complete and the assertion must sit inside it: a negator, an establishing
/// word within two, `that`, and the assertion beginning within three words of it. The `that` is
/// what makes the absence the *content* of the denied inference rather than a separate clause
/// sharing the sentence.
fn denied_inference(sentence: &str, at: usize) -> bool {
    let w = words(sentence);
    for i in 0..at {
        if !NEGATORS.contains(&w[i].as_str()) {
            continue;
        }
        let Some(est) =
            (i + 1..(i + 3).min(at)).find(|j| ESTABLISHING.iter().any(|e| w[*j].starts_with(e)))
        else {
            continue;
        };
        let Some(that) = (est + 1..(est + 3).min(at)).find(|j| w[*j] == "that") else {
            continue;
        };
        if at > that && at - that <= 3 {
            return true;
        }
    }
    false
}

/// Whether a scoping participle follows the assertion that begins at word `at`.
fn scoped_after(sentence: &str, at: usize) -> bool {
    words(sentence).iter().skip(at + 1).any(|w| TRAILING_SCOPE.contains(&w.as_str()))
}

/// `no <≤3 words> <artifact noun>`, the bare-noun form.
///
/// Matched only where the absent thing is a record artifact. On real answers a plain "no X" fires
/// on recommendation prose — "No action needed", "No further action evident" — far more often
/// than on claims about the record, and a broad pattern flagged ten sentences to catch three real
/// ones. An artifact missing from the list is under-detection, which is the direction that does
/// not manufacture a pass.
fn bare_artifact_absence(clause: &str) -> Option<usize> {
    let w = words(clause);
    for (i, word) in w.iter().enumerate() {
        if word == "no" && artifact_within(&w, i + 1, 3) {
            return word_position(clause, "no");
        }
    }
    None
}

/// True when an artifact noun begins within `gap` words of `from`.
fn artifact_within(w: &[String], from: usize, gap: usize) -> bool {
    for skip in 0..=gap {
        let at = from + skip;
        if at >= w.len() {
            return false;
        }
        for noun in ARTIFACT_NOUNS {
            let parts: Vec<&str> = noun.split(' ').collect();
            if w[at..].len() >= parts.len()
                && parts.iter().enumerate().all(|(k, p)| w[at + k] == *p)
            {
                return true;
            }
        }
    }
    false
}

/// Markdown tables need deterministic handling before they reach the phone channel.
///
/// Two signals, either sufficient: consecutive pipe-delimited rows, or a separator row. §E-14
/// strips markdown on delivery, so a table arrives as run-together prose.
pub fn has_markdown_table(answer: &str) -> bool {
    let lines: Vec<&str> = answer.lines().map(str::trim).collect();
    let pipe_row: Vec<bool> =
        lines.iter().map(|l| l.starts_with('|') && l.ends_with('|') && l.len() > 1).collect();
    if pipe_row.windows(2).any(|w| w[0] && w[1]) {
        return true;
    }
    lines.iter().any(|l| is_separator_row(l))
}

/// `|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)+\|?` — at least two dash-runs joined by pipes.
fn is_separator_row(line: &str) -> bool {
    let trimmed = line.trim().trim_start_matches('|').trim_end_matches('|');
    if !trimmed.contains('|') {
        return false;
    }
    trimmed.split('|').all(|cell| {
        let c = cell.trim().trim_start_matches(':').trim_end_matches(':');
        c.len() >= 3 && c.chars().all(|ch| ch == '-')
    })
}

/// Return ownership moved outward without a nearby cited request or handoff.
///
/// Ownership defaults inward: the service may only place work with a counterparty when the record
/// shows it was actually asked of them. "Waiting on Cedar Harbor" with no cited request is the
/// service inventing a dependency.
pub fn uncited_counterparty_ownership(answer: &str) -> Option<String> {
    for sentence in sentences(answer) {
        let Some((claim, at)) = ownership_claim(&sentence) else {
            continue;
        };
        // The operator owning its own work is the default, not a claim needing a citation.
        if claim.to_lowercase().contains("multiagency") {
            continue;
        }
        // An indirect question is not an assertion. `Whether MultiAgency is waiting on Cedar
        // Harbor's response ... - UNKNOWN` names the thing it cannot settle, which is the
        // discipline working rather than an uncited ownership move (0026). Position is the test,
        // as it is in `absence_as_negative`'s VERIFYING guard: the opener must govern the claim,
        // so an answer that asserts ownership and later says something is unclear is untouched.
        if governed_by_uncertainty(&sentence, at) {
            continue;
        }
        if has_request(&sentence) && has_citation(&sentence) {
            continue;
        }
        return Some(sentence);
    }
    None
}

/// The matched ownership phrase, if the sentence moves work to a named party.
///
/// Three grammars, all from the original pattern:
///
/// - `is | are | remains | sits  with <party>`
/// - a **label form** — the start of a sentence or bullet, or anything after a dash: `Continuation
///   documentation — With Cedar Harbor Studio`. The dash may appear mid-sentence, which is the
///   case a start-of-line-only check misses.
/// - `waiting on <party>`
///
/// Returns the party together with the token index of the marker that moved work to it.
///
/// The index is what lets the caller ask whether something *governs* the claim — a bare party name
/// cannot tell an assertion from an indirect question about one.
/// Openers that turn what follows into a question rather than a claim.
const UNCERTAIN_OPENER: &[&str] = &["whether", "unclear", "unsure", "undetermined"];

/// Whether an uncertainty opener stands before the ownership marker at token `at`.
fn governed_by_uncertainty(sentence: &str, at: usize) -> bool {
    sentence.split_whitespace().take(at).any(|t| {
        let bare = t.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        UNCERTAIN_OPENER.contains(&bare.as_str())
    })
}

fn ownership_claim(sentence: &str) -> Option<(String, usize)> {
    // One token list for everything. `words()` drops a standalone em-dash, so an index taken from
    // it does not line up with the raw text — and the dash is precisely the signal the label form
    // is looking for.
    let raw: Vec<&str> = sentence.split_whitespace().collect();
    let bare: Vec<String> =
        raw.iter().map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).collect();
    for (i, token) in bare.iter().enumerate() {
        let moved = match token.as_str() {
            "with" => {
                bare.get(i.wrapping_sub(1)).is_some_and(|p| {
                    matches!(p.as_str(), "is" | "are" | "remain" | "remains" | "sit" | "sits")
                }) || label_form(&raw, i)
            }
            "on" => bare.get(i.wrapping_sub(1)).is_some_and(|p| p == "waiting"),
            _ => false,
        };
        if !moved {
            continue;
        }
        if let Some(party) = capitalised_party(&raw, &bare, i + 1) {
            return Some((party, i));
        }
    }
    None
}

/// True when the `with` at token `index` opens a label: nothing before it, a bullet, or a dash.
///
/// The dash may sit anywhere in the sentence — "Continuation documentation — With Cedar Harbor
/// Studio" is the same ownership claim as a bulleted one, and a start-of-line-only check misses
/// it. `**With**` is the same claim as `With`, so emphasis markers are stripped first.
fn label_form(raw: &[&str], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).and_then(|p| raw.get(p)) else {
        return true; // `with` opens the sentence.
    };
    let trimmed = previous.trim_end_matches(['*', '_']);
    trimmed.chars().last().is_some_and(|c| matches!(c, '—' | '–' | '-' | '•'))
}

/// A counterparty name beginning at token `from`: "the counterparty", or 1–4 capitalised tokens.
///
/// Capitalisation is the whole signal, so this reads `raw` rather than the lowercased list.
fn capitalised_party(raw: &[&str], bare: &[String], from: usize) -> Option<String> {
    if bare.get(from).is_some_and(|w| w == "the")
        && bare.get(from + 1).is_some_and(|w| w == "counterparty")
    {
        return Some("the counterparty".to_owned());
    }
    let mut taken = Vec::new();
    for token in raw.iter().skip(from).take(4) {
        let word =
            token.trim_matches(|c: char| !c.is_alphanumeric() && c != '&' && c != '.' && c != '-');
        if word.chars().next().is_some_and(char::is_uppercase) {
            taken.push(word);
        } else {
            break;
        }
    }
    (!taken.is_empty()).then(|| taken.join(" "))
}

fn has_request(sentence: &str) -> bool {
    crate::text::any_word(
        sentence,
        &["request", "requested", "ask", "asked", "sent", "handoff", "handed"],
    )
}

/// An activity id (`JW-A2`) or an ISO date (`2026-08-05`).
fn has_citation(sentence: &str) -> bool {
    for token in sentence.split_whitespace() {
        let bare = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
        if is_activity_id(bare) || is_iso_date(bare) {
            return true;
        }
    }
    false
}

fn is_activity_id(token: &str) -> bool {
    let mut parts = token.split('-');
    let Some(head) = parts.next() else { return false };
    if head.len() < 2 || !head.chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    !rest.is_empty()
        && rest.iter().all(|p| {
            !p.is_empty() && p.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
}

fn is_iso_date(token: &str) -> bool {
    let parts: Vec<&str> = token.split('-').collect();
    parts.len() == 3
        && parts[0].len() == 4
        && parts[0].starts_with("20")
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
}

/// Compare an explicit lead count with numbered top-level findings when both exist.
///
/// Returns `(stated, counted)` on a mismatch. Both must be present: an answer that never claims a
/// count cannot contradict itself, and fewer than two numbered items is not a list.
pub fn inconsistent_finding_count(answer: &str) -> Option<(usize, usize)> {
    let lead = answer.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let stated = stated_count(lead)?;
    let counted = answer.lines().filter(|l| numbered_item(l)).count();
    (counted >= 2 && stated != counted).then_some((stated, counted))
}

/// The count a lead claims for its own findings, when it claims exactly one.
///
/// # Two defects, and only the first was a bug in the ordinary sense
///
/// The scan used to `?` inside its loop, so the first word that was not a number returned `None`
/// from the whole function — the loop, the index and the lookahead below it never ran a second
/// time, and the rule could only read a lead that *began* with the number. Both its controls led
/// with the number, so an accepted control and a rejected control agreed with each other on the
/// one input shape the defect spared.
///
/// Fixing that alone left the rule still unable to fire, for a second reason it hid: **the nouns
/// were wrong for this service.** Measured over every recorded answer, no lead contains the word
/// `issue` or `finding` — 156 of 227 state a count and 60 answers carry two or more numbered
/// items, so there was plenty to police; the vocabulary simply did not describe what this service
/// counts. It counts *accounts*. That is `0025`'s finding — a lead vocabulary that was a list
/// rather than a rule — recurring in a different rule in the same file.
///
/// # Why more than one number means no count at all
///
/// Adding `account(s)` on its own would have been worse than the defect. This service leads with a
/// portfolio summary, and its numbers are denominators and breakdowns rather than list lengths:
///
/// ```text
/// Six account records examined. Four need attention now.          -> 6 is the denominator
/// Three accounts need decisions now; two need verification; ...   -> 3 is one part of three
/// Four of six accounts need attention now — two time-sensitive    -> 4 of 6, then a breakdown
/// ```
///
/// Taking the first number followed by a noun reads 6, 3 and 4 against list lengths of 4, 5 and 4.
/// Measured: it reports a mismatch on 17 of the 21 answers it reads, and the leads above are why.
///
/// So a lead that states **more than one number** states a breakdown, and no single number in it
/// is the length of the list. The rule declines rather than guesses. It is deliberately the
/// conservative direction: this reads one distinct lead across the whole recorded corpus and finds
/// it consistent, which is a small reach and an honest one — the alternative reach was seventeen
/// false failures.
///
/// `record(s)` is **not** in the noun set, and that is not an oversight. No recorded lead says
/// `<number> records`, so it would be neutral today — but `two records disagree` is the natural
/// lead for the `contradiction` case, and it names evidence rather than findings. A rule that read
/// it as a count would fail an answer for describing the conflict it was asked about.
fn stated_count(lead: &str) -> Option<usize> {
    const NUMBER_WORDS: [(&str, usize); 10] = [
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("six", 6),
        ("seven", 7),
        ("eight", 8),
        ("nine", 9),
        ("ten", 10),
    ];
    /// What this service counts. See the doc comment for why `record(s)` is absent.
    const COUNTED: &[&str] = &["issue", "issues", "finding", "findings", "account", "accounts"];

    let w = words(lead);
    let value_of = |word: &str| -> Option<usize> {
        NUMBER_WORDS
            .iter()
            .find(|(name, _)| word == *name)
            .map(|(_, v)| *v)
            .or_else(|| word.parse::<usize>().ok())
    };
    // The guard, before anything is read: a breakdown names no single count.
    if w.iter().filter(|word| value_of(word).is_some()).count() != 1 {
        return None;
    }
    for (i, word) in w.iter().enumerate() {
        let Some(value) = value_of(word) else {
            continue;
        };
        // `(?:material\s+)?(?:coordination\s+)?(?:issues?|findings?|accounts?)`
        let mut j = i + 1;
        for optional in ["material", "coordination"] {
            if w.get(j).is_some_and(|n| n == optional) {
                j += 1;
            }
        }
        if w.get(j).is_some_and(|n| COUNTED.contains(&n.as_str())) {
            return Some(value);
        }
    }
    None
}

/// `^\s*(\d+)[.)]\s+` — a numbered top-level item.
fn numbered_item(line: &str) -> bool {
    let t = line.trim_start();
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return false;
    }
    let rest = &t[digits.len()..];
    (rest.starts_with('.') || rest.starts_with(')')) && rest[1..].starts_with(char::is_whitespace)
}

/// Apply the service-wide rules, then the case grader.
///
/// # A FAIL returns; a REVIEW does not
///
/// The two FAIL rules return immediately, and that is right: a service-wide violation is
/// **decided**, and a case grader that never examined it cannot overturn it.
///
/// The two REVIEW rules used to return the same way, and that was the defect. A REVIEW says *this
/// grader cannot establish it mechanically* — so returning one discards a case grader that may have
/// a decided answer, in either direction. `0023` measured it masking a FAIL: run 7 was recorded as
/// "the grader could not tell" when the case grader would have failed it for leading with a winner,
/// and *"the recorded verdict does not say so."* It masks passes too — `contradiction` run 9 and
/// `onboarding` run 2 reach REVIEW with every one of their own gates clean.
///
/// So a review is now held rather than returned, and the case grader runs underneath it:
///
/// - case grader **FAIL** → FAIL. This repository already has the rule and the reason:
///   *"FAIL outranks BLOCKED: a measured contradiction is worse news than an unmeasured one"*
///   (`crates/lifecycle/src/inspect.rs`, and `DESIGN.md`). A measured defect outranks an
///   undecidable one, and the sharper fact is the one to report.
/// - case grader **PASS** → the review stands. The concern is real and undecided, and a grader that
///   did not look at it has established nothing about it. Clearing a review on a passing case
///   grader would turn *cannot tell* into *fine*, which is the collapse [`Verdict::Review`]'s own
///   documentation forbids in the other direction.
///
/// The change can only make the suite harsher — it converts REVIEW to FAIL and never the reverse —
/// which is the direction that cannot manufacture a pass.
pub fn evidence_disciplined(answer: &str, grader: impl Fn(&str) -> Graded) -> Graded {
    if compound_tier_claim(answer).is_some() {
        return Graded::fail(
            "one claim carries two evidence tiers; split supported and unknown claims",
        );
    }
    if absence_as_negative(answer).is_some() {
        return Graded::fail(
            "asserted non-existence exceeds the record; scope the absence to recorded evidence",
        );
    }
    if has_markdown_table(answer) {
        return Graded::fail("markdown tables do not survive the delivery surface");
    }

    let held = if uncited_counterparty_ownership(answer).is_some() {
        Some(Graded::review("counterparty ownership needs a cited request or handoff"))
    } else {
        inconsistent_finding_count(answer).map(|(stated, counted)| {
            Graded::review(format!(
                "lead states {stated} findings but {counted} numbered findings appear"
            ))
        })
    };

    let graded = grader(answer);
    match held {
        // A measured defect outranks an undecidable one.
        Some(_) if graded.verdict == Verdict::Fail => graded,
        Some(review) => review,
        None => graded,
    }
}

/// Quote-stripped clauses that are not refusing, negating or marking a claim unknown.
///
/// Case graders use this to ask "did the answer *assert* this?", so that quoting a forbidden
/// claim in order to refuse it is never read as making it.
pub fn affirmative(answer: &str) -> String {
    // Phrases, not bare words. "does not" and "is not" must negate; a bare `is` or `does` would
    // negate almost every clause and leave `affirmative` returning nothing, which reads as a clean
    // answer to every grader that uses it — a failure that manufactures passes silently.
    const NEGATED: &[&str] = &[
        "unknown",
        "whether",
        "unsupported",
        "unverified",
        "invented",
        "never",
        "cannot",
        "can't",
        "does not",
        "doesn't",
        "is not",
        "isn't",
        "not established",
        "not confirmed",
        "no evidence",
        "refuse",
        "refusing",
        "will not",
        "won't",
    ];
    clauses(answer)
        .into_iter()
        .filter(|c| {
            let flat = c.to_lowercase();
            !NEGATED.iter().any(|p| {
                if p.contains(' ') {
                    flat.contains(p)
                } else {
                    crate::text::contains_word(&flat, p)
                }
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Words that deny. Whole words: `no` must not be found inside `nothing`.
const NEGATORS: &[&str] = &["cannot", "can not", "not", "never", "no"];

/// Stems for saying the record settled something — `verif` reaches "verified" and "verification".
const ESTABLISHING: &[&str] = &["establish", "confirm", "prove", "proof", "verif", "evidence"];

/// True when the line denies, in order, that the record settles something.
///
/// # Why a predicate and not a phrase list
///
/// This began as four literal spellings — `not established`, `cannot establish`, `cannot confirm`,
/// `does not establish` — and a correct answer has many more ways to say it. Measured on ten runs
/// of one question, four leads were rejected for word choice alone: three said *"residual access
/// **cannot be confirmed** closed from the record"* and one said *"the record establishes two
/// closure actions but **does not prove** all residual access was revoked"*
/// (`docs/experiments/0023-…`). A fifth, in `0021`, said *"cannot be verified from the record"*.
/// All five state exactly what the rule asks for.
///
/// The list was also internally inconsistent: `cases::offboarding_unknown`'s own `UNCERTAIN`
/// vocabulary accepts `does not prove` as evidence that residual authority is unresolved, in the
/// same file that rejected it one gate earlier. Reconciling that needs no reference to any score.
///
/// # Why order, and why boundaries
///
/// Unordered proximity would accept *"the record **confirms** renewal, so this is **not** a
/// coordination issue"* — a lead that establishes something, containing both halves. So the
/// negator must come first and the verb must follow within a short window. The negator is matched
/// on word boundaries so `no` is not found inside `nothing`; the verb is matched as a stem that
/// must *begin* a word, because "confirmed" and "verification" are the same claim as "confirm"
/// while the `prove` inside `approved` is not a claim about proof at all.
fn denies_establishment(lead: &str) -> bool {
    let low = lead.to_lowercase();
    for negator in NEGATORS {
        for at in crate::text::word_positions(&low, negator) {
            // `negator` is ASCII and matched on a boundary, so this is a char boundary.
            let rest = &low[at + negator.len()..];
            if ESTABLISHING
                .iter()
                .any(|verb| crate::text::stem_positions(rest, verb).iter().any(|gap| *gap <= 20))
            {
                return true;
            }
        }
    }
    false
}

/// The first non-empty line must state the verdict, not bury uncertainty later.
pub fn lead_is_unresolved(answer: &str) -> bool {
    let lead = answer.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    crate::text::contains_word(lead, "UNKNOWN")
        || crate::text::contains_word(lead, "unresolved")
        || denies_establishment(lead)
}

/// Re-exported so case graders can express proximity without importing `text` directly.
pub fn mentions_near(answer: &str, name: &str, issues: &[&str]) -> bool {
    near(&unquoted(answer), name, issues, 180)
}

#[cfg(test)]
mod tests;
