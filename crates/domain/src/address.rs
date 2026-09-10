//! Is this message for us?
//!
//! [`route`](crate::route) asks *whose conversation is this?*. This asks the other question, and
//! the reference corpus is emphatic that they are separate: conflating them is defect E-4, where
//! "not a registered conversation" and "not addressed to us" returned the same answer and an
//! in-flight turn became unrecoverable. Routing decides whether a tenant owns the room.
//! Addressing decides whether a sentence in that room was meant for us.
//!
//! # Why this is configuration rather than inference
//!
//! In a direct chat every message is for the bot. In a group almost none are — a bot that
//! answered every sentence anyone typed would bill a model turn per remark and drown the room.
//! The seam cannot tell the two apart: the channel contract carries `{id, channel, text}` and
//! nothing about the chat's kind, deliberately, because that is Telegram vocabulary and the
//! channel layer is denied it. So the distinction is declared per tenant in the registry, where
//! an operator already says which room belongs to whom.
//!
//! # What this does not implement, and will not pretend to
//!
//! The reference also treats a **reply to one of the bot's own messages** as addressed. That
//! cannot be decided from text: it needs the update's `reply_to_message`, which the channel
//! contract does not carry. Adding a boolean hint to the contract would move half the decision
//! into the adapter and leave two places deciding one thing. Until the contract carries it
//! properly, a reply without a mention is **not addressed**, which is the safe direction: it
//! costs a missed answer rather than an unasked-for one.

/// How a tenant's room decides that a message is for us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addressing {
    /// Every message in this room is for us. A direct chat, where there is nobody else to talk to.
    Always,
    /// Only messages naming this handle. A group, where most sentences are between other people.
    Mention(String),
}

impl Addressing {
    /// The request, with the mention removed — or `None` when the message was not for us.
    ///
    /// The mention is **stripped** rather than left in place: it is addressing, not content, and
    /// a model asked "@somebot what changed?" should be answered the question rather than shown
    /// the summons. Matching is case-insensitive, because Telegram usernames are.
    ///
    /// Only the first mention is removed. A message that names the bot twice keeps the second,
    /// because at that point it is being talked *about* as well as *to*, and dropping it would
    /// edit the question.
    pub fn addressed<'a>(&self, text: &'a str) -> Option<std::borrow::Cow<'a, str>> {
        match self {
            Self::Always => {
                let trimmed = text.trim();
                // An empty message is not a question. It is terminal rather than an error: a
                // sticker or a photo caption arrives as empty text and must cost no turn.
                (!trimmed.is_empty()).then_some(std::borrow::Cow::Borrowed(trimmed))
            }
            Self::Mention(handle) => {
                let at = format!("@{}", handle.trim_start_matches('@'));
                let found = find_mention(text, &at)?;
                // Only the gap the removal made is closed. Collapsing whitespace generally would
                // edit the question — a pasted table or an indented quote is content.
                let before = text[..found.start].trim_end();
                let after = text[found.end..].trim_start();
                let request = match (before.is_empty(), after.is_empty()) {
                    (true, _) => after.to_owned(),
                    (_, true) => before.to_owned(),
                    _ => format!("{before} {after}"),
                };
                (!request.is_empty()).then_some(std::borrow::Cow::Owned(request))
            }
        }
    }
}

/// Case-insensitive search for `@handle` that is not part of a longer name, as a range of `text`.
///
/// The boundary check is what stops `@bot` matching inside `@botanist`: Telegram would deliver
/// that message to a different bot entirely, and answering it would be answering someone else's
/// conversation.
///
/// # Why this searches the original rather than a lowercased copy
///
/// It used to `find` in `text.to_lowercase()` and hand the offset back to a caller slicing `text`.
/// `to_lowercase` is not length-preserving — `ẞ` is three bytes and lowercases to two, `İ` is two
/// and lowercases to three — so any such character before the mention moved the cut. One byte
/// early lands inside a character and panics; one byte late leaves part of the summons in the
/// question the model is shown. Measured, on group text, which is text a stranger chooses.
///
/// Returning the **range** closes the second half of the same defect: the caller used to resume at
/// `found + at.len()`, the un-lowercased handle's length, against an offset found with the
/// lowercased one. The matched region is the only thing that knows how long it was.
fn find_mention(text: &str, at: &str) -> Option<std::ops::Range<usize>> {
    text.char_indices().find_map(|(start, _)| {
        let end = match_at(text, start, at)?;
        // Not a mention of us if a name continues past it.
        let next_is_name =
            text[end..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
        (!next_is_name).then_some(start..end)
    })
}

/// Where `at` ends in `text` if it matches at `start`, comparing case-insensitively.
///
/// Character by character rather than by slice, because a case-insensitive match is not a claim
/// that the two spans are the same length.
fn match_at(text: &str, start: usize, at: &str) -> Option<usize> {
    let mut rest = text[start..].chars();
    for want in at.chars() {
        if !rest.next()?.to_lowercase().eq(want.to_lowercase()) {
            return None;
        }
    }
    Some(text.len() - rest.as_str().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group() -> Addressing {
        Addressing::Mention("Multiferroic_bot".into())
    }

    /// The table gap 1 asks for: every case, and what each costs.
    #[test]
    fn the_addressing_table() {
        let cases: &[(&str, Option<&str>, &str)] = &[
            // in a direct chat, everything is for us
            ("what changed?", Some("what changed?"), "direct chat: plain question"),
            ("  padded  ", Some("padded"), "direct chat: trimmed"),
            ("", None, "direct chat: a sticker arrives as empty text and costs no turn"),
            ("   ", None, "direct chat: whitespace is not a question"),
        ];
        for (text, want, why) in cases {
            assert_eq!(Addressing::Always.addressed(text).as_deref(), *want, "Always: {why}");
        }

        let group_cases: &[(&str, Option<&str>, &str)] = &[
            (
                "@Multiferroic_bot what changed?",
                Some("what changed?"),
                "the summons is stripped; the model is asked the question, not shown the mention",
            ),
            (
                "@multiferroic_BOT what changed?",
                Some("what changed?"),
                "Telegram usernames are case-insensitive",
            ),
            (
                "hey @Multiferroic_bot what changed?",
                Some("hey what changed?"),
                "a mention mid-sentence still addresses us",
            ),
            ("what changed?", None, "a group remark to nobody costs no turn"),
            (
                "@Multiferroic_botanist what changed?",
                None,
                "a longer handle is a different bot; answering it answers someone else's room",
            ),
            ("@Multiferroic_bot", None, "a bare summons asks nothing"),
            ("@Multiferroic_bot    ", None, "nor does a summons and whitespace"),
            (
                "@Multiferroic_bot ask @Multiferroic_bot",
                Some("ask @Multiferroic_bot"),
                "only the first is addressing; the second is content and editing it would edit \
                 the question",
            ),
        ];
        for (text, want, why) in group_cases {
            assert_eq!(group().addressed(text).as_deref(), *want, "Mention: {why}");
        }
    }

    #[test]
    fn a_handle_written_with_or_without_the_at_behaves_the_same() {
        // An operator will write it both ways in a registry, and neither should silently never
        // match.
        for handle in ["Multiferroic_bot", "@Multiferroic_bot"] {
            let a = Addressing::Mention(handle.into());
            assert_eq!(a.addressed("@Multiferroic_bot hi").as_deref(), Some("hi"), "{handle}");
        }
    }

    #[test]
    fn addressing_never_borrows_a_mention_into_the_request() {
        // The property behind the table: whatever comes back, the summons is not in it.
        let text = "@Multiferroic_bot which accounts need attention?";
        let request = group().addressed(text).expect("addressed");
        assert!(!request.to_lowercase().contains("@multiferroic_bot"), "{request}");
    }

    /// Text before the mention that does not lowercase to its own length must not move the cut.
    ///
    /// `str::to_lowercase` is not length-preserving, and the search runs over a lowercased copy
    /// while the slices are taken from the original. `ẞ` (3 bytes) lowercases to `ß` (2), so the
    /// offset lands one byte early — inside a character, which panics; `İ` (2 bytes) lowercases
    /// to `i̇` (3) and lands one byte late, leaving the `@` in the question. Both are group text,
    /// which is to say text a stranger chooses.
    #[test]
    fn a_mention_is_found_at_the_same_place_whatever_precedes_it() {
        let cases: &[(&str, Option<&str>, &str)] = &[
            ("ẞ @Multiferroic_bot hi", Some("ẞ hi"), "lowercasing shortens what precedes"),
            ("İ @Multiferroic_bot hi", Some("İ hi"), "lowercasing lengthens what precedes"),
            ("ẞ@Multiferroic_bot", Some("ẞ"), "no separator, and the cut is mid-character"),
            ("日本語 @Multiferroic_bot hi", Some("日本語 hi"), "multi-byte that is already lower"),
        ];
        for (text, want, why) in cases {
            assert_eq!(group().addressed(text).as_deref(), *want, "{why}");
        }

        // Positive control: the same handle in the same position with ASCII before it. Without
        // this the assertions above would also pass for a `find_mention` that never matched.
        assert_eq!(group().addressed("s @Multiferroic_bot hi").as_deref(), Some("s hi"));
    }

    #[test]
    fn a_reply_without_a_mention_is_not_addressed_and_that_is_deliberate() {
        // The reference treats a reply to the bot as addressed. The channel contract does not
        // carry `reply_to_message`, so this cannot see it — and guessing would answer messages
        // nobody sent us. Documented as a known narrowing rather than a silent one.
        assert_eq!(group().addressed("yes, that one").as_deref(), None);
    }
}
