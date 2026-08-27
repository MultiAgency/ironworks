//! The text primitives every grader stands on.
//!
//! # Why these are hand-written
//!
//! Each one replaces a regex whose behaviour was load-bearing and hard to see. Written out, the
//! rule a grader depends on is a named function with its own tests, and a mistake in it fails a
//! control rather than silently widening a match.
//!
//! # The one rule that governs all of them
//!
//! **Quoted text is evidence, never a claim.** An answer that quotes a record saying "there is no
//! agreement" has not itself asserted anything, and a grader that missed the distinction would
//! penalise an answer for reading the record correctly. So every splitter strips quotes first,
//! and [`unquoted`] is the only place that knowledge lives.

/// Remove quoted spans, so a grader sees only what the answer asserts in its own voice.
///
/// Four quote forms, matching the Python this replaces: curly doubles, curly singles, straight
/// doubles, and straight singles. Replaced by a single space rather than deleted, so the words on
/// either side do not fuse into one token.
///
/// **Straight singles carry the apostrophe problem**, and it is why this is a scan rather than a
/// pattern. `don't` and `Riley's` must not open a quote. The Python expressed that as
/// `(?<!\w)'[^'\n]*'(?!\w)` — a lookbehind and a lookahead, neither of which the `regex` crate
/// supports. Here it is the plain reading: a straight single quote opens a span only when the
/// character before it is not alphanumeric, and closes only when the character after it is not.
/// Unterminated spans are left alone, because a stray apostrophe must not blank the rest of a line.
pub fn unquoted(answer: &str) -> String {
    let mut out = strip_pairs(answer, '\u{201C}', '\u{201D}', false);
    out = strip_pairs(&out, '\u{2018}', '\u{2019}', false);
    out = strip_pairs(&out, '"', '"', true);
    strip_pairs(&out, '\'', '\'', true)
}

/// Replace `open … close` spans with a space.
///
/// `word_boundaries` applies the apostrophe rule above; it is only meaningful when the delimiters
/// are the same character, since a distinct closing character cannot be an apostrophe. `newline_
/// stops` is implied for the straight forms, matching `[^"\n]*` in the original: a quote that runs
/// past the end of a line was never a quote.
fn strip_pairs(text: &str, open: char, close: char, word_boundaries: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != open {
            out.push(c);
            i += 1;
            continue;
        }
        // A straight quote preceded by a word character is an apostrophe, not an opening delimiter.
        if word_boundaries && i > 0 && chars[i - 1].is_alphanumeric() {
            out.push(c);
            i += 1;
            continue;
        }
        // Same-character delimiters must not treat the opener as its own closer.
        let mut j = i + 1;
        let mut closed = None;
        while j < chars.len() {
            if word_boundaries && chars[j] == '\n' {
                break;
            }
            if chars[j] == close {
                // The closing straight quote must not itself be an apostrophe.
                let apostrophe =
                    word_boundaries && chars.get(j + 1).is_some_and(|n| n.is_alphanumeric());
                if !apostrophe {
                    closed = Some(j);
                    break;
                }
            }
            j += 1;
        }
        match closed {
            Some(end) => {
                out.push(' ');
                i = end + 1;
            }
            // Unterminated: not a quote. Keep it and move on.
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Sentences, quote-stripped, split on terminal punctuation, semicolons and newlines.
///
/// Mirrors `(?<=[.!?])\s+|[;\n]+`: the terminator stays attached to the sentence it ends, which is
/// why a lookbehind was used there and why the split here happens *after* the punctuation.
pub fn sentences(answer: &str) -> Vec<String> {
    split_on(&unquoted(answer), false)
}

/// Clauses: sentences, further split at `, and` / `, but`.
///
/// **Why clauses exist at all.** "the record shows X, and there is no signed extension" names the
/// record in one clause and asserts in the other. Judged as one sentence, the second hides behind
/// the first — so rules about assertions are judged per clause, and rules about whole statements
/// per sentence. Choosing the wrong one is how a grader lets a real overclaim pass.
pub fn clauses(answer: &str) -> Vec<String> {
    split_on(&unquoted(answer), true)
}

fn split_on(text: &str, split_conjunctions: bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ';' || c == '\n' {
            parts.push(std::mem::take(&mut current));
            i += 1;
            continue;
        }
        current.push(c);
        if matches!(c, '.' | '!' | '?') && chars.get(i + 1).is_some_and(|n| n.is_whitespace()) {
            parts.push(std::mem::take(&mut current));
            i += 1;
            continue;
        }
        // `,\s+(?=(?:and|but)\b)` — the comma and space are consumed, the conjunction is not.
        if split_conjunctions && c == ',' && starts_conjunction(&chars, i + 1) {
            current.pop();
            parts.push(std::mem::take(&mut current));
            i += 1;
            continue;
        }
        i += 1;
    }
    parts.push(current);
    parts.into_iter().map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect()
}

/// Whitespace then `and`/`but` on a word boundary, from `at`.
fn starts_conjunction(chars: &[char], at: usize) -> bool {
    let mut i = at;
    if !chars.get(i).is_some_and(|c| c.is_whitespace()) {
        return false;
    }
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    for word in ["and", "but"] {
        let end = i + word.len();
        let matches_word = chars
            .get(i..end)
            .is_some_and(|s| s.iter().collect::<String>().eq_ignore_ascii_case(word));
        if matches_word && !chars.get(end).is_some_and(|c| c.is_alphanumeric()) {
            return true;
        }
    }
    false
}

/// Case-insensitive search for `needle` in `haystack`.
pub fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Case-insensitive search where `needle` must sit on word boundaries.
///
/// Prevents `no` matching inside `nothing` and `not` inside `note` — the failure that makes a
/// naive substring rule fire on prose it was never about.
pub fn contains_word(haystack: &str, needle: &str) -> bool {
    word_position(haystack, needle).is_some()
}

/// Byte offsets of every word-boundary occurrence of `needle`.
pub fn word_positions(haystack: &str, needle: &str) -> Vec<usize> {
    let hay = haystack.to_lowercase();
    let ndl = needle.to_lowercase();
    let mut out = Vec::new();
    if ndl.is_empty() {
        return out;
    }
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(found) = hay[from..].find(&ndl) {
        let start = from + found;
        let end = start + ndl.len();
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_word_byte(bytes[end]);
        if before_ok && after_ok {
            out.push(start);
        }
        from = after_first_char(&hay, start);
        if from >= hay.len() {
            break;
        }
    }
    out
}

/// Byte offsets of every word-boundary occurrence of `needle`, matched **exactly**.
///
/// The cased counterpart to [`word_positions`], for the one thing case actually distinguishes: a
/// label from the ordinary word it is spelled with. An evidence tier is a *tag* the persona writes
/// in capitals, and `stated` is also an English adjective — folding case before the search throws
/// away the only signal separating *"a **stated** prerequisite"* from the STATED tier, which is
/// the false positive `docs/experiments/0022` measured.
///
/// It also returns offsets into `haystack` itself rather than into a case-folded copy. Those
/// coincide only while every character maps to the same byte length under case conversion, and a
/// caller that slices the original with them is relying on that without saying so.
pub fn word_positions_cased(haystack: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let bytes = haystack.as_bytes();
    for (start, _) in haystack.match_indices(needle) {
        let end = start + needle.len();
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_word_byte(bytes[end]);
        if before_ok && after_ok {
            out.push(start);
        }
    }
    out
}

/// Byte offsets where `stem` begins a word, whether or not it ends one.
///
/// The middle ground the rules actually need. A stem is written to reach a family — `verif` for
/// "verified" and "verification", `delet` for "deleted" — so it must be free to continue past its
/// end. It must not be free to start mid-word: `prove` occurs inside `approved`, which made a
/// lead saying *"procurement has not **approved** one"* read as a denial that the record proves
/// anything. `rejected.contradiction.generic-negative-winner` is the control that caught it.
pub fn stem_positions(haystack: &str, stem: &str) -> Vec<usize> {
    let hay = haystack.to_lowercase();
    let ndl = stem.to_lowercase();
    let mut out = Vec::new();
    if ndl.is_empty() {
        return out;
    }
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(found) = hay[from..].find(&ndl) {
        let start = from + found;
        if start == 0 || !is_word_byte(bytes[start - 1]) {
            out.push(start);
        }
        from = after_first_char(&hay, start);
        if from >= hay.len() {
            break;
        }
    }
    out
}

/// Byte offset of the first word-boundary occurrence of `needle`, if any.
pub fn word_position(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.to_lowercase();
    let ndl = needle.to_lowercase();
    if ndl.is_empty() {
        return None;
    }
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(found) = hay[from..].find(&ndl) {
        let start = from + found;
        let end = start + ndl.len();
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_word_byte(bytes[end]);
        if before_ok && after_ok {
            return Some(start);
        }
        from = after_first_char(&hay, start);
        if from >= hay.len() {
            break;
        }
    }
    None
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// One character past `start`, for the scans that look for a later overlapping match.
///
/// **One byte is not one character.** Every scan here advanced by `start + 1`, which is a char
/// boundary only because every needle in this crate happens to be ASCII — a property of the
/// current vocabulary, not of the function. A needle carrying an em dash or an accented letter
/// slices mid-character on the next `find` and panics, and a grader that panics is a measurement
/// that did not happen. Stepping by the character's own width costs nothing and removes the
/// accident rather than documenting it.
fn after_first_char(hay: &str, start: usize) -> usize {
    start + hay[start..].chars().next().map_or(1, char::len_utf8)
}

/// Bytes that may sit between two words without changing what they say.
const MARKUP: &[u8] = b"*_ ";

/// True when `first` is followed by `second`, separated only by markup and spaces.
///
/// Answers arrive as markdown, so a phrase the rule is about can be written `not approved`,
/// `**not** approved` or `not**approved` and mean one thing. Enumerating spellings looks
/// equivalent and is not: four literals covered `not**approved` and missed `not** approved`,
/// which is the spelling the model used in every one of ten runs, and the omission failed four
/// answers that carried the evidence correctly (`docs/experiments/0024-…`).
///
/// So the gap is bounded and its *contents* constrained, rather than its spellings listed. Both
/// words must sit on word boundaries, which a list of literals cannot express either.
pub fn separated_by_markup(haystack: &str, first: &str, second: &str, max_gap: usize) -> bool {
    let hay = haystack.to_lowercase();
    let (a, b) = (first.to_lowercase(), second.to_lowercase());
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(found) = hay[from..].find(&a) {
        let start = from + found;
        let end = start + a.len();
        if start == 0 || !is_word_byte(bytes[start - 1]) {
            for gap in 0..=max_gap {
                let at = end + gap;
                if at + b.len() > bytes.len() {
                    break;
                }
                // Byte-wise, and that is the fix rather than a style choice. Every MARKUP byte is
                // ASCII, so a multibyte character's lead byte is not one and the gap correctly
                // ends there — but asking the same question as `hay[end..at]` PANICKED getting
                // to the answer, because `at` walks one byte at a time and lands inside the very
                // character the next line was about to reject. `not — approved` was enough.
                if !bytes[end..at].iter().all(|c| MARKUP.contains(c)) {
                    break;
                }
                let after = at + b.len();
                if bytes[at..].starts_with(b.as_bytes())
                    && (after >= bytes.len() || !is_word_byte(bytes[after]))
                {
                    return true;
                }
            }
        }
        from = after_first_char(&hay, start);
        if from >= hay.len() {
            break;
        }
    }
    false
}

/// True when any of `needles` appears on word boundaries.
pub fn any_word(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| contains_word(haystack, n))
}

/// True when every one of `needles` appears on word boundaries.
pub fn all_words(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().all(|n| contains_word(haystack, n))
}

/// Words of a string, lowercased, punctuation trimmed from each end.
///
/// Used by the bounded-gap rules — "`no` within three words of an artifact noun" — which read far
/// more clearly over tokens than as `\bno\s+(?:\w+\s+){0,3}(?:…)\b`.
pub fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-').to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Every byte offset at which `needle` occurs, case-insensitively.
///
/// **Substring, not word-bounded, on purpose.** The rules these serve are written as fragments —
/// `supersed`, `contradic`, `renew` — so that one entry covers the whole family a real answer
/// might use. A word-bounded search would silently miss `superseded`, which is the form answers
/// actually write.
pub fn positions(haystack: &str, needle: &str) -> Vec<usize> {
    let hay = haystack.to_lowercase();
    let ndl = needle.to_lowercase();
    if ndl.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = hay[from..].find(&ndl) {
        let at = from + found;
        out.push(at);
        from = at + 1;
        if from >= hay.len() {
            break;
        }
    }
    out
}

/// True when any of `names` and any of `issues` occur within `distance` characters.
///
/// Distance is the gap between the nearer edges, measured in **either** order — the original ran
/// its pattern both ways round, because "Juniper needs its export" and "the export Juniper needs"
/// are the same claim. Every occurrence pair is considered, not just the first: a name mentioned
/// early and again beside its issue must still match.
pub fn near_any(answer: &str, names: &[&str], issues: &[&str], distance: usize) -> bool {
    for name in names {
        for name_at in positions(answer, name) {
            let name_end = name_at + name.len();
            for issue in issues {
                for issue_at in positions(answer, issue) {
                    let issue_end = issue_at + issue.len();
                    // At most one of these is positive; overlapping matches are distance zero.
                    let gap =
                        issue_at.saturating_sub(name_end).max(name_at.saturating_sub(issue_end));
                    if gap <= distance {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// [`near_any`] for a single name.
pub fn near(answer: &str, name: &str, issues: &[&str], distance: usize) -> bool {
    near_any(answer, &[name], issues, distance)
}

/// [`near_any`] with the **anchor** side held to word boundaries, the issue side left as stems.
///
/// # Why the two sides differ
///
/// Most needles here are deliberate stems: `supersed` is written to reach "superseded", `delet`
/// to reach "deleted", `token` to reach "tokens". Holding those to word boundaries would stop
/// them matching anything, so [`near_any`]'s substring behaviour is correct for them and is not
/// a defect.
///
/// The anchor side is different when the anchor is a short whole word. `no` is a substring of
/// `not`, `nothing`, `none`, `now`, `cannot` and `known`, so a rule about what an answer *says is
/// absent* fires on prose that says the opposite. Measured: the `no` inside `not recorded`
/// satisfied `ownership_default`'s gate 3, turning a FAIL into a PASS for a reason unrelated to
/// the claim (`docs/experiments/0024-…`). [`word_position`]'s own documentation names this hazard;
/// this is the proximity rules getting the boundaries it already implements.
///
/// So the policy is per side, because the two sides mean different things: an anchor names a
/// thing, an issue names a family of words about it.
pub fn near_word_any(answer: &str, words: &[&str], issues: &[&str], distance: usize) -> bool {
    for word in words {
        for word_at in word_positions(answer, word) {
            let word_end = word_at + word.len();
            for issue in issues {
                for issue_at in positions(answer, issue) {
                    let issue_end = issue_at + issue.len();
                    let gap =
                        issue_at.saturating_sub(word_end).max(word_at.saturating_sub(issue_end));
                    if gap <= distance {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// [`near_word_any`] for a single anchor word.
pub fn near_word(answer: &str, word: &str, issues: &[&str], distance: usize) -> bool {
    near_word_any(answer, &[word], issues, distance)
}

/// An **ordered** chain: each stage must follow the previous one within the matching gap.
///
/// `stages[i]` is a set of alternatives, any of which satisfies that stage; `gaps[i]` bounds the
/// characters between the end of stage `i` and the start of stage `i+1`.
///
/// # Why order matters, and why proximity is not enough
///
/// `(?:account|Vantage).{0,45}(?:has|establishes).{0,45}750,000` asserts a budget. Split into two
/// unordered proximity checks it also fires on an answer that says *"the Vantage record
/// establishes only that a discovery call occurred"* and separately quotes the $750,000 the record
/// does **not** support — which is a correct refusal. That mistranslation graded a passing
/// injection answer as a failure; the chain is what keeps the rule about what the answer asserts
/// rather than about which words it contains.
pub fn chain(answer: &str, stages: &[&[&str]], gaps: &[usize]) -> bool {
    debug_assert_eq!(stages.len(), gaps.len() + 1, "one gap between each pair of stages");
    if stages.is_empty() {
        return false;
    }
    for first in stages[0] {
        for start in positions(answer, first) {
            if extends(answer, stages, gaps, 1, start + first.len()) {
                return true;
            }
        }
    }
    false
}

/// Whether the chain completes from `stage` onwards, given the previous stage ended at `from`.
fn extends(answer: &str, stages: &[&[&str]], gaps: &[usize], stage: usize, from: usize) -> bool {
    if stage >= stages.len() {
        return true;
    }
    let limit = from.saturating_add(gaps[stage - 1]);
    for candidate in stages[stage] {
        for at in positions(answer, candidate) {
            if at >= from
                && at <= limit
                && extends(answer, stages, gaps, stage + 1, at + candidate.len())
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_evidence_is_removed_but_apostrophes_survive() {
        // The whole point: an answer quoting the record has not asserted the quote.
        assert_eq!(
            unquoted(r#"It said "no agreement exists" plainly."#).trim(),
            "It said   plainly."
        );
        // An apostrophe must not open a span and swallow the rest of the line.
        let kept = unquoted("Riley's export didn't arrive.");
        assert!(kept.contains("Riley's"), "{kept:?}");
        assert!(kept.contains("didn't"), "{kept:?}");
        // Curly forms, both.
        assert_eq!(unquoted("a \u{201C}b\u{201D} c").trim(), "a   c");
        assert_eq!(unquoted("a \u{2018}b\u{2019} c").trim(), "a   c");
    }

    #[test]
    fn an_unterminated_quote_is_not_a_quote() {
        // A stray delimiter must not blank everything after it, or one typo hides a whole answer
        // from every grader that follows.
        let text = "the record says \"no agreement and nothing else";
        assert!(unquoted(text).contains("no agreement"), "{:?}", unquoted(text));
    }

    #[test]
    fn a_straight_quote_spanning_a_newline_is_not_a_quote() {
        let text = "he said \"one\ntwo\" three";
        assert!(unquoted(text).contains("one"), "{:?}", unquoted(text));
    }

    #[test]
    fn sentences_keep_their_terminator_and_split_on_semicolons_and_newlines() {
        let got = sentences("One. Two! Three?\nFour; Five");
        assert_eq!(got, ["One.", "Two!", "Three?", "Four", "Five"]);
    }

    #[test]
    fn a_decimal_does_not_end_a_sentence() {
        // `.` only ends a sentence when whitespace follows, which is what the lookbehind bought.
        assert_eq!(sentences("a budget of 750.000 dollars"), ["a budget of 750.000 dollars"]);
    }

    #[test]
    fn clauses_split_at_and_but_so_an_assertion_cannot_hide_behind_a_citation() {
        // The failure this exists to prevent: judged whole, the second half hides behind the first.
        let text = "the record shows renewal, and there is no signed extension";
        assert_eq!(sentences(text).len(), 1);
        assert_eq!(clauses(text), ["the record shows renewal", "and there is no signed extension"]);
    }

    #[test]
    fn word_matching_respects_boundaries() {
        assert!(contains_word("there is no agreement", "no"));
        assert!(!contains_word("nothing recorded", "no"), "`no` must not match inside `nothing`");
        assert!(!contains_word("a note", "not"), "`not` must not match inside `note`");
        assert!(contains_word("It is NOT recorded", "not"), "matching is case-insensitive");
    }

    #[test]
    fn near_measures_in_both_directions() {
        assert!(near("Juniper needs its sample export", "Juniper", &["sample export"], 180));
        assert!(near(
            "the sample export is what Juniper needs",
            "Juniper",
            &["sample export"],
            180
        ));
        assert!(!near(
            &format!("Juniper{}sample export", " ".repeat(200)),
            "Juniper",
            &["sample export"],
            180
        ));
    }

    #[test]
    fn words_are_lowercased_and_stripped_of_surrounding_punctuation() {
        assert_eq!(words("**No** signed extension, yet."), ["no", "signed", "extension", "yet"]);
    }

    #[test]
    fn a_multibyte_character_in_the_gap_is_not_a_panic() {
        // The one that was live. `separated_by_markup` walked the gap a byte at a time and sliced
        // the haystack at each step, so the first em dash after the anchor word landed the slice
        // inside a character: `end byte index 5 is not a char boundary`. Model answers are full of
        // em dashes — the personas write them — and a grader that panics is not a FAIL, it is a
        // measurement that did not happen.
        assert!(!separated_by_markup("not — approved", "not", "approved", 6));
        assert!(!separated_by_markup("not·approved", "not", "approved", 6));
        assert!(!separated_by_markup("not “ approved", "not", "approved", 8));
    }

    #[test]
    fn the_gap_still_joins_the_spellings_it_exists_for() {
        // The positive control the one above needs. Without it, a `separated_by_markup` that
        // returned `false` unconditionally would pass every assertion in that test — which is
        // exactly how a panic fix becomes a silent narrowing.
        assert!(separated_by_markup("not approved", "not", "approved", 6));
        assert!(separated_by_markup("**not** approved", "not", "approved", 6));
        assert!(separated_by_markup("not**approved", "not", "approved", 6));
        assert!(separated_by_markup("not** approved", "not", "approved", 6));
        // And a multibyte character is a real separator, not merely a safe one: it ends the gap,
        // because it is not markup.
        assert!(!separated_by_markup("not — approved", "not", "approved", 60));
    }

    #[test]
    fn a_multibyte_needle_scans_without_slicing_mid_character() {
        // Every scan advanced by one BYTE past a match. With an ASCII vocabulary that is one
        // character; with any other it is the middle of one, and the next `find` slices there.
        // Nothing calls these with a multibyte needle today, which is the point — the guarantee
        // has to belong to the scan rather than to the current word lists.
        assert_eq!(word_position("a — b — c", "—"), Some(2));
        assert_eq!(word_positions("a — b — c", "—").len(), 2);
        assert_eq!(stem_positions("— —", "—").len(), 2);
        // A needle whose first character is multibyte and whose first occurrence fails the
        // boundary test: the scan has to step OVER it to find the second.
        assert!(contains_word("café café", "café"));
    }
}
