//! The `/share` grammar: what a person typed, decided before anything is billed.
//!
//! # Why this is a grammar and not a skill
//!
//! Promotion is the one operation in this system where the bytes that end up in a shared record
//! must be the bytes a person chose. A model that "helped" — summarised, tidied, resolved the
//! account from context — would make the shared record its recollection rather than her
//! statement. So the whole path from message to proposal is deterministic, and it lives here,
//! where there is no IO to reach for and no model to call.
//!
//! # What "exactly" means
//!
//! [`parse`] removes the command syntax and **one** separator on each side of the account, and
//! nothing else. It does not trim the body, normalise whitespace, collapse runs, or lowercase
//! anything: a note that begins with two spaces keeps them, because the person typed them.
//!
//! # The half that is not here
//!
//! Whether the proposed bytes survive the channel's own rendering is the *transport's* knowledge,
//! not the grammar's — `domain` names no transport. The caller asks the channel
//! ([`ironworks_channel_http::echoes_exactly`]) and refuses before a proposal exists, so a
//! proposal never holds bytes the echo could not show. See [`Refusal::TextUnusable`].

use crate::ids::{ContributorId, OrgId};
use crate::record::AccountId;

/// What an inbound message turns out to be.
///
/// Three outcomes, and the third is the one that must stay cheap: an ordinary message is not a
/// command and keeps every behaviour it had before this module existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// A well-formed promotion request. Nothing has happened yet.
    Share(ShareRequest),
    /// A well-formed confirmation of whatever proposal the sender was last shown.
    ///
    /// Carries nothing, because there is nothing to carry: the proposal is found from the
    /// sender's own identity and their conversation, never from anything they typed.
    Confirm,
    /// A command this build refuses, and the sentence the sender is shown.
    ///
    /// **Visible, and never a model turn.** A slash command that fell through to the model would
    /// bill a turn to answer a question about a feature, and would let a typo become a
    /// conversation.
    Refused(Refusal),
    /// Not a command. Existing behaviour, unchanged.
    Conversation,
}

/// A promotion a person has asked for and not yet confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRequest {
    account: AccountId,
    body: String,
}

impl ShareRequest {
    pub fn account(&self) -> &AccountId {
        &self.account
    }
    /// The proposed bytes, exactly as typed.
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// Why a command was refused, written as the sentence its sender reads.
///
/// Every variant is a dead end that costs nothing: no turn, no record fetch, no store write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A slash command this build does not implement.
    UnknownCommand { name: String },
    /// `/confirm` was given arguments.
    ///
    /// It takes none on purpose: what is being confirmed is the proposal the person was just
    /// shown, and a `/confirm <something>` would invite them to name a different one — which is
    /// exactly the choice this design refuses to offer, because the bytes they approved are the
    /// bytes they were shown.
    ConfirmTakesNoArguments,
    /// `/share` with nothing after it.
    ShareNeedsAccount,
    /// `/share <account>` with no note after the account.
    ShareNeedsText,
    /// The account token cannot be an account id.
    AccountUnusable { reason: &'static str },
    /// The note cannot be stored or echoed as typed.
    TextUnusable { reason: &'static str },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCommand { name } => write!(
                f,
                "I do not know the command /{name}. This bot understands /share <account> <note>."
            ),
            Self::ConfirmTakesNoArguments => f.write_str(
                "/confirm takes nothing after it — it shares the note I last showed you, \
                 exactly as it was shown.",
            ),
            Self::ShareNeedsAccount => {
                f.write_str("/share needs an account and a note: /share <account> <note>.")
            }
            Self::ShareNeedsText => f.write_str(
                "/share needs the note to share after the account: /share <account> <note>.",
            ),
            Self::AccountUnusable { reason } => write!(f, "That account {reason}."),
            Self::TextUnusable { reason } => write!(f, "That note {reason}."),
        }
    }
}

/// The note's cap, in bytes.
///
/// Room for a real observation and well inside Telegram's 4096-character message limit, so the
/// echo — which carries the note plus a sentence of instruction — is one message rather than a
/// chunked sequence a person has to reassemble before deciding whether it is what they meant.
pub const MAX_SHARE_BODY_BYTES: usize = 2000;

/// The account token's cap, in bytes. An account id is a short stable handle; anything longer is
/// a note that lost its separator.
const MAX_ACCOUNT_TOKEN_BYTES: usize = 64;

/// Whether this message is addressed to the command surface at all.
///
/// # Why this is separate from [`parse`]
///
/// It answers the only question that may be asked **before** the sender has been authorized.
/// `parse` produces sentences that describe the command surface — its grammar, which commands
/// exist, that `/confirm` is coming — and none of that may be shown to someone this tenant has
/// not bound, or in a room whose privacy the channel did not attest. So the relay asks this
/// first, admits the sender, and only then parses.
///
/// It agrees with `parse` by construction: both take the command word the same way, and a test
/// pins that neither can drift into treating something as a command the other would not.
pub fn is_command(text: &str) -> bool {
    !matches!(command_word(text), None | Some(""))
}

/// The command word — after the `/`, up to the first whitespace, with any `@bot` suffix removed.
/// `None` when the text is not a command at all.
fn command_word(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let (word, _) = split_once_separator(rest);
    Some(word.split('@').next().unwrap_or(word))
}

/// Classify one inbound message.
///
/// Pure and total: every input is a `Share`, a `Refused`, or a `Conversation`, and only the first
/// can lead to a durable write.
///
/// The relay calls this **after** admitting the sender, because its refusals describe the command
/// surface. See [`is_command`].
pub fn parse(text: &str) -> Intent {
    let Some(rest) = text.strip_prefix('/') else {
        return Intent::Conversation;
    };
    let (word, after) = split_once_separator(rest);
    // `/share@somebot` is the same command. Telegram only requires the suffix where several bots
    // can hear one message, which a private chat is not — but people type it from habit, and
    // reading it as a different command would refuse a request that is plainly ours.
    let name = word.split('@').next().unwrap_or(word);
    match name {
        "share" => parse_share(after),
        "confirm" => match after {
            None => Intent::Confirm,
            // A trailing separator with nothing after it is still a bare `/confirm `.
            Some(rest) if rest.trim().is_empty() => Intent::Confirm,
            Some(_) => Intent::Refused(Refusal::ConfirmTakesNoArguments),
        },
        // A bare `/` is not a command anyone typed on purpose, and neither is `/ hello`.
        "" => Intent::Conversation,
        other => Intent::Refused(Refusal::UnknownCommand { name: other.to_owned() }),
    }
}

fn parse_share(after: Option<&str>) -> Intent {
    let Some(after) = after else {
        return Intent::Refused(Refusal::ShareNeedsAccount);
    };
    let (token, body) = split_once_separator(after);
    if token.is_empty() {
        return Intent::Refused(Refusal::ShareNeedsAccount);
    }
    if let Some(reason) = account_unusable(token) {
        return Intent::Refused(Refusal::AccountUnusable { reason });
    }
    let Ok(account) = AccountId::parse(token) else {
        // `AccountId::parse` refuses only the empty string, which `token.is_empty()` already
        // caught. Kept as a branch rather than an unwrap so a future tightening upstream cannot
        // turn a refusal into a panic.
        return Intent::Refused(Refusal::AccountUnusable { reason: "is not a usable account id" });
    };
    let Some(body) = body else {
        return Intent::Refused(Refusal::ShareNeedsText);
    };
    // An absent note and a blank one are the same fact to the person who typed it, and
    // "needs the note" is the sentence that helps. Emptiness is grammar, not unusability.
    if body.trim().is_empty() {
        return Intent::Refused(Refusal::ShareNeedsText);
    }
    if let Some(reason) = body_unusable(body) {
        return Intent::Refused(Refusal::TextUnusable { reason });
    }
    Intent::Share(ShareRequest { account, body: body.to_owned() })
}

fn account_unusable(token: &str) -> Option<&'static str> {
    if token.len() > MAX_ACCOUNT_TOKEN_BYTES {
        return Some("is too long to be an account id");
    }
    // The token ran to the first whitespace, so it cannot contain any; a control character is
    // still reachable and would reach a registry-shaped lookup and a store key.
    if token.chars().any(char::is_control) {
        return Some("contains characters an account id cannot hold");
    }
    None
}

fn body_unusable(body: &str) -> Option<&'static str> {
    if body.len() > MAX_SHARE_BODY_BYTES {
        return Some("is longer than 2000 bytes");
    }
    // Every control character, newlines included. A shared record is one dated observation, and
    // the bytes have to survive a durable column, an echo, and a person reading the echo to
    // decide whether it is what they meant — none of which shows a `\r` or a NUL differently
    // from its absence.
    if body.chars().any(char::is_control) {
        return Some("contains characters that cannot be shown back to you exactly");
    }
    None
}

/// The activity id one proposal always produces.
///
/// # Deterministic, so a retry is a replay
///
/// The store is idempotent on `(org, activity_id)`, which is only useful if the same proposal
/// derives the same id every time — across a retry, a restart, and a second `/confirm`. So it is
/// a function of the proposal's identity and nothing else: no clock, no counter, no randomness.
///
/// # Length-prefixed, so concatenation cannot be ambiguous
///
/// Joining `org` and `account` with a separator makes `("ab", "c")` and `("a", "bc")` distinct
/// only while neither contains the separator — and an account id is a caller-supplied string
/// this crate does not constrain. Prefixing each field with its byte length removes the question:
/// no two different tuples can produce one input.
///
/// # The `share-` namespace is load-bearing
///
/// The store's append route refuses any id outside it, so this function's output is the only
/// shape that can be written — and a bug here can never land on an activity a tenant's team
/// authored (`NW-INT-01`, `HF-A1`), only on another promoted one.
pub fn activity_id(org: &OrgId, account: &AccountId, proposal_update: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in [org.as_str().as_bytes(), account.as_str().as_bytes()] {
        hasher.update(part.len().to_be_bytes());
        hasher.update(part);
    }
    hasher.update(proposal_update.to_be_bytes());
    let digest: String = hasher.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("share-{digest}")
}

/// The message a person is shown before they confirm.
///
/// Its whole job is to let someone check that the bytes about to be promoted are the bytes they
/// meant. So the note is reproduced verbatim on its own lines, and every word around it is plain
/// prose: no markers, no punctuation the channel's own renderer would remove. `channel-http`'s
/// `echoes_exactly` is asserted against this function's output, because an echo that promised
/// "exactly these bytes" while the transport quietly rewrote its own surrounding text would be
/// the defect this whole path exists to prevent, one layer out.
pub fn render_echo(request: &ShareRequest, contributor: &ContributorId) -> String {
    format!(
        "Proposed for account {account}, attributed to {contributor}:\n\n{body}\n\n\
         Nothing has been shared yet. Send /confirm to share exactly these bytes, or send \
         another /share to replace this proposal.",
        account = request.account(),
        contributor = contributor,
        body = request.body(),
    )
}

/// Split at the **first** whitespace character, consuming exactly that one character.
///
/// Not `split_whitespace`, and not `trim_start` on the remainder: the note is the person's bytes,
/// so the only whitespace this is allowed to remove is the one separator the grammar needs to
/// find where the next token ends. `None` means there was no separator at all — a different fact
/// from an empty remainder, which is what distinguishes `/share` from `/share ` .
fn split_once_separator(s: &str) -> (&str, Option<&str>) {
    match s.find(char::is_whitespace) {
        Some(at) => {
            let sep = s[at..].chars().next().map_or(1, char::len_utf8);
            (&s[..at], Some(&s[at + sep..]))
        }
        None => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(text: &str) -> ShareRequest {
        match parse(text) {
            Intent::Share(s) => s,
            other => panic!("{text:?} did not parse as a share: {other:?}"),
        }
    }
    fn refusal(text: &str) -> Refusal {
        match parse(text) {
            Intent::Refused(r) => r,
            other => panic!("{text:?} was not refused: {other:?}"),
        }
    }

    #[test]
    fn the_moment_this_exists_for() {
        let s = share("/share ACME-001 Customer requested a September renewal review.");
        assert_eq!(s.account().as_str(), "ACME-001");
        assert_eq!(s.body(), "Customer requested a September renewal review.");
    }

    #[test]
    fn the_note_is_the_bytes_that_were_typed() {
        // The load-bearing one. Everything downstream — the echo, the confirmation, the record —
        // is only worth anything if what is stored is what she wrote. One separator goes on each
        // side of the account and nothing else is touched.
        for (text, want) in [
            ("/share A-1   leading spaces kept", "  leading spaces kept"),
            ("/share A-1 trailing kept   ", "trailing kept   "),
            ("/share A-1 two  internal   spaces", "two  internal   spaces"),
            ("/share A-1 MiXeD CaSe Is Kept", "MiXeD CaSe Is Kept"),
            ("/share A-1 punctuation: kept, exactly!", "punctuation: kept, exactly!"),
            ("/share A-1 a note with /share inside it", "a note with /share inside it"),
            ("/share A-1 unicode — kept", "unicode — kept"),
        ] {
            assert_eq!(share(text).body(), want, "{text:?}");
        }
    }

    #[test]
    fn the_two_stages_agree_on_what_a_command_is() {
        // `is_command` is asked before the sender is authorized and `parse` after, so a message
        // the first admitted and the second called ordinary would reach a model through the
        // command door — and one the first rejected could never be answered at all.
        for text in [
            "/share ACME-001 note",
            "/share",
            "/confirm",
            "/unknown",
            "/share@bot A-1 note",
            "what needs attention?",
            "the ratio is 3/4",
            "",
            "   ",
            "/",
            "/ hello",
        ] {
            assert_eq!(
                is_command(text),
                !matches!(parse(text), Intent::Conversation),
                "the two stages disagree about {text:?}"
            );
        }
    }

    #[test]
    fn an_ordinary_message_is_not_a_command_and_keeps_its_behaviour() {
        for ordinary in [
            "what needs attention?",
            "the ratio is 3/4",
            "",
            "   ",
            "share ACME-001 no slash",
            "/ leading slash then space",
            "/",
        ] {
            assert_eq!(parse(ordinary), Intent::Conversation, "{ordinary:?}");
        }
    }

    #[test]
    fn an_unknown_command_refuses_visibly_instead_of_reaching_a_model() {
        // A typo must cost nothing and say so. Falling through to the model would bill a turn to
        // answer a question about a command, and would make a mistyped `/shre` a conversation.
        assert_eq!(refusal("/shre A-1 note"), Refusal::UnknownCommand { name: "shre".into() });
        assert_eq!(refusal("/agency"), Refusal::UnknownCommand { name: "agency".into() });
        assert!(refusal("/start").to_string().contains("/share <account> <note>"));
    }

    #[test]
    fn confirm_takes_nothing_after_it() {
        // What is being confirmed is the note the person was just shown. Letting them name
        // something else would offer exactly the choice this design refuses to offer.
        for bare in ["/confirm", "/confirm ", "/confirm   ", "/confirm@bot"] {
            assert_eq!(parse(bare), Intent::Confirm, "{bare:?}");
        }
        assert_eq!(refusal("/confirm yes"), Refusal::ConfirmTakesNoArguments);
        assert_eq!(refusal("/confirm share-abc"), Refusal::ConfirmTakesNoArguments);
    }

    #[test]
    fn one_proposal_always_derives_one_activity_id() {
        // The store is idempotent on this id, which is only worth anything if a retry, a restart
        // and a second /confirm all produce the same one.
        let org = OrgId::parse("multiagency-org").unwrap();
        let account = AccountId::parse("ACME-001").unwrap();
        let first = activity_id(&org, &account, 41);
        assert_eq!(first, activity_id(&org, &account, 41));
        assert!(first.starts_with("share-"), "{first}");

        // Different proposals, different ids — on every axis of the identity.
        assert_ne!(first, activity_id(&org, &account, 42));
        assert_ne!(first, activity_id(&org, &AccountId::parse("ACME-002").unwrap(), 41));
        assert_ne!(first, activity_id(&OrgId::parse("other-org").unwrap(), &account, 41));
    }

    #[test]
    fn concatenation_cannot_be_made_ambiguous_by_a_caller_supplied_account() {
        // An account id is the sender's text and this crate does not constrain its charset, so a
        // separator-joined input would let two different tuples collide by choosing one. Length
        // prefixes remove the question rather than forbidding a character.
        let org_a = OrgId::parse("ab").unwrap();
        let org_b = OrgId::parse("a").unwrap();
        let acc_a = AccountId::parse("c").unwrap();
        let acc_b = AccountId::parse("bc").unwrap();
        assert_ne!(activity_id(&org_a, &acc_a, 1), activity_id(&org_b, &acc_b, 1));
        // And a separator inside the account cannot imitate a boundary either.
        let dashed = AccountId::parse("x-1").unwrap();
        let split = AccountId::parse("x").unwrap();
        assert_ne!(activity_id(&org_a, &dashed, 1), activity_id(&org_a, &split, 1));
    }

    #[test]
    fn a_share_missing_either_half_says_which_half() {
        assert_eq!(refusal("/share"), Refusal::ShareNeedsAccount);
        assert_eq!(refusal("/share "), Refusal::ShareNeedsAccount);
        assert_eq!(refusal("/share ACME-001"), Refusal::ShareNeedsText);
        assert_eq!(refusal("/share ACME-001 "), Refusal::ShareNeedsText);
        assert_eq!(refusal("/share ACME-001    "), Refusal::ShareNeedsText);
    }

    #[test]
    fn a_note_that_cannot_be_stored_or_shown_back_exactly_is_refused() {
        // Not decoration: these bytes would reach a durable column and an echo a person reads to
        // decide whether the proposal is what they meant. A note whose echo cannot show them the
        // difference is one they cannot meaningfully confirm.
        assert!(matches!(refusal("/share A-1 line one\nline two"), Refusal::TextUnusable { .. }));
        assert!(matches!(refusal("/share A-1 tab\there"), Refusal::TextUnusable { .. }));
        assert!(matches!(refusal("/share A-1 nul\0here"), Refusal::TextUnusable { .. }));
        let long = format!("/share A-1 {}", "x".repeat(MAX_SHARE_BODY_BYTES + 1));
        assert!(matches!(refusal(&long), Refusal::TextUnusable { .. }));
        // The bound itself is allowed: a cap that refused its own limit would be a different cap.
        let at_limit = format!("/share A-1 {}", "x".repeat(MAX_SHARE_BODY_BYTES));
        assert_eq!(share(&at_limit).body().len(), MAX_SHARE_BODY_BYTES);
    }

    #[test]
    fn an_account_token_that_cannot_be_an_account_is_refused() {
        assert!(matches!(
            refusal(&format!("/share {} note", "A".repeat(65))),
            Refusal::AccountUnusable { .. }
        ));
        assert!(matches!(refusal("/share A\u{7}1 note"), Refusal::AccountUnusable { .. }));
    }

    #[test]
    fn a_command_addressed_to_the_bot_by_name_is_the_same_command() {
        // People type the suffix from habit. `/share` is private-chat only, where exactly one bot
        // can hear the message, so reading the addressed form as a different command would refuse
        // a request that is plainly ours.
        let s = share("/share@multiagency_bot ACME-001 the note");
        assert_eq!(s.account().as_str(), "ACME-001");
        assert_eq!(s.body(), "the note");
    }

    #[test]
    fn the_echo_reproduces_the_note_verbatim_and_says_nothing_has_happened() {
        let s = share("/share ACME-001 Customer requested a September renewal review.");
        let who = ContributorId::parse("alice").unwrap();
        let echo = render_echo(&s, &who);
        assert!(echo.contains("Customer requested a September renewal review."), "{echo}");
        assert!(echo.contains("ACME-001") && echo.contains("alice"), "{echo}");
        // The two things a person needs to know before deciding.
        assert!(echo.contains("Nothing has been shared yet"), "{echo}");
        assert!(echo.contains("/confirm"), "{echo}");
    }

    #[test]
    fn a_multibyte_separator_is_consumed_whole() {
        // A non-breaking space is whitespace. Consuming one byte of it would leave the rest of
        // the character in the note and produce bytes nobody typed.
        let s = share("/share\u{a0}A-1\u{a0}note");
        assert_eq!(s.account().as_str(), "A-1");
        assert_eq!(s.body(), "note");
    }
}
