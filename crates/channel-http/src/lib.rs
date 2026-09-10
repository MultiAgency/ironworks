//! Talking to a channel: polling for inbound updates, and sending an answer in chunks.
//!
//! # Why this is its own crate
//!
//! `ironworks-channel` owns the *semantics* — the cursor, the barrier, what a disposition means —
//! and is forbidden a transport dependency on purpose: it is the one cross-tenant object in the
//! system, and a test over its own manifest keeps it unable to name records, service definitions
//! or a network client. This is the other half, kept apart for that reason.
//!
//! It briefly lived inside the binary, on the argument that one implementation does not need a
//! boundary. That was wrong, and for a reason worth writing down: **the boundary was never about
//! swapping implementations.** Adapters are the only crates that touch the network, which is what
//! makes "who can talk to what" answerable by reading the dependency graph rather than by
//! grepping. A binary that can open a socket of its own ends that property, and nothing about
//! having a single implementation changes it.
//!
//! It speaks to `ironworks-devchannel` today. A real channel is a second implementation behind the
//! same surface, not a second crate.
//!
//! # The credential
//!
//! A real adapter authenticates and a fixture does not, so both constructors exist and the call
//! site says which far end it is talking to. The reason is on the `/poll` side rather than the
//! `/send` side: **the offset is the acknowledgement**, so an unauthenticated adapter is a way for
//! any process that can reach it to discard a channel's whole pending backlog, before a single
//! update has been journalled and with nothing left to recover from.
//!
//! What that does to the reader here is worth stating, because it is the half a refusal gets
//! wrong. An adapter's refusal *before it contacts the far system* is provable absence: it carries
//! `"accepted": false`, [`Channel::send`] reports [`AttemptOutcome::Rejected`], and a resend
//! cannot duplicate. The same refusal read by [`Channel::poll`] carries no `updates` array, so it
//! is [`ChannelError::Malformed`] and the cursor stays where it is — an empty array would read as
//! *nothing is waiting* and acknowledge through the offset that was just refused.

use ironworks_domain::{ActorRef, AttemptOutcome, ChatKind};
use std::time::Duration;

/// One inbound update, as the channel offers it.
pub struct Inbound {
    pub id: u64,
    pub channel: String,
    pub text: String,
    /// Who sent it, when the channel attested a sender. `None` covers a channel that does not
    /// report one, an update with no sender (a broadcast post), and a value this crate could not
    /// read — all of which are *no attested sender*, which is what a command path must refuse.
    pub actor: Option<ActorRef>,
    /// What kind of room it arrived in. [`ChatKind::Unattested`] when the channel made no claim,
    /// which is never read as private.
    pub chat_kind: ChatKind,
}

#[derive(Debug)]
pub enum ChannelError {
    Unreachable(String),
    Malformed(String),
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(d) => write!(f, "the channel could not be reached: {d}"),
            Self::Malformed(d) => write!(f, "the channel answered something unusable: {d}"),
        }
    }
}

/// The credential a channel adapter requires, held only so it can be presented.
///
/// No `Display`, a redacting `Debug`, and the only way out is an `Authorization` header — so this
/// value reaches a log through a deliberate change rather than a stray `{:?}` on a `Channel`.
///
/// **Deliberately not `ironworks_records::Secret`.** That type has exactly this shape, and reusing
/// it would mean this adapter depends on another adapter. Adapter crates are what make "who can
/// talk to what" answerable by reading the dependency graph, and an edge between two of them ends
/// that property to save a dozen lines. The duplication is the cheaper thing to keep in sync.
struct AdapterToken(String);

impl AdapterToken {
    fn header(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl std::fmt::Debug for AdapterToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdapterToken(redacted)")
    }
}

pub struct Channel {
    agent: ureq::Agent,
    base: String,
    /// Present when the adapter on the other end requires one. `None` is not a laxer mode of the
    /// same thing: it is the shape of an adapter that authenticates nothing, which is what
    /// `devchannel` is and what a real one must never be.
    token: Option<AdapterToken>,
}

// ── the HTTP budget ─────────────────────────────────────────────────────────────────────────
//
// Named rather than chained inline, because this exact ladder is written in four other crates and
// `crates/ironclaw/src/client.rs::Ironclaw::new` deliberately runs a longer one. That intentional
// difference was invisible against five silent copies of the same eight-call chain; against named
// constants it is a diff on a number. The crates cannot share the values without an edge none of
// them should have, so each states its own and says which system it is stating them about.

/// DNS for the channel adapter.
const RESOLVE: Duration = Duration::from_secs(5);
/// TCP connect to the channel adapter.
const CONNECT: Duration = Duration::from_secs(5);
/// Writing the request to the channel adapter.
const SEND: Duration = Duration::from_secs(10);
/// Waiting for the channel adapter to begin answering.
const RECV_RESPONSE: Duration = Duration::from_secs(30);
/// Reading the channel adapter's answer.
const RECV_BODY: Duration = Duration::from_secs(30);

impl Channel {
    /// A channel that presents no credential. The proofs' `devchannel` reads none.
    pub fn new(base: impl Into<String>) -> Self {
        Self::build(base, None)
    }

    /// A channel that presents `token` on every request.
    ///
    /// Beside [`Channel::new`] rather than replacing it, because the two are different facts about
    /// the far end rather than a setting: a channel is either one that authenticates or one that
    /// does not, and choosing at the call site keeps the answer readable there.
    ///
    /// **Deploy the caller before the adapter that requires it.** A relay sending a token an
    /// adapter ignores is a working channel; an adapter requiring a token no relay sends is a
    /// stopped one. `deploy/README.md` carries the ordering.
    pub fn authenticated(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self::build(base, Some(AdapterToken(token.into())))
    }

    fn build(base: impl Into<String>, token: Option<AdapterToken>) -> Self {
        // `http_status_as_error(false)`, for the reason the runtime client gives and this file
        // learned the hard way: **HTTP status is not the success signal.** With the default, a
        // channel answering `409 {"accepted": false}` — an explicit, provable refusal — arrives
        // as a transport error and gets recorded as *uncertain*. That is the wrong answer in the
        // direction that looks safe: it makes the one outcome that licenses a free retry
        // unreachable, and tells an operator a resend may duplicate when nothing was sent.
        //
        // Per-phase timeouts for the same reason as elsewhere: a phase-agnostic timeout cannot
        // say whether anything was attempted, and here that distinction *is* the evidence.
        let config = ureq::Agent::config_builder()
            .timeout_global(None)
            .timeout_resolve(Some(RESOLVE))
            .timeout_connect(Some(CONNECT))
            .timeout_send_request(Some(SEND))
            .timeout_recv_response(Some(RECV_RESPONSE))
            .timeout_recv_body(Some(RECV_BODY))
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            base: base.into().trim_end_matches('/').to_owned(),
            token,
        }
    }

    /// Attach this channel's credential to a request, if it has one to present.
    ///
    /// The one place the header is set. Every request leaves through here, so a call site cannot
    /// forget the dance and send an unauthenticated request that looks like every other one.
    fn authorize<Any>(&self, request: ureq::RequestBuilder<Any>) -> ureq::RequestBuilder<Any> {
        match &self.token {
            Some(token) => request.header("authorization", token.header()),
            None => request,
        }
    }

    /// Ask for updates from `offset`.
    ///
    /// **The offset is the acknowledgement.** Asking for everything from `N` is how the channel is
    /// told we are finished with everything below `N` — there is no separate call, which is
    /// exactly why an acknowledgement always lands on a *later* poll than the delivery it
    /// acknowledges, and why `DELIVERED` and `ACKED` cannot be one fact.
    pub fn poll(&self, offset: u64) -> Result<Vec<Inbound>, ChannelError> {
        let body = self
            .authorize(self.agent.get(format!("{}/poll?offset={offset}", self.base)))
            .call()
            .map_err(|e| ChannelError::Unreachable(e.to_string()))?
            .body_mut()
            .read_to_string()
            .map_err(|e| ChannelError::Malformed(e.to_string()))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| ChannelError::Malformed(e.to_string()))?;
        let updates = parsed
            .get("updates")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ChannelError::Malformed("no updates array".into()))?;
        updates
            .iter()
            .map(|u| {
                Ok(Inbound {
                    id: u
                        .get("id")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| ChannelError::Malformed("an update with no id".into()))?,
                    channel: u
                        .get("channel")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    text: u.get("text").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
                    // An unreadable actor is no actor, not a refusal: the update must still
                    // travel or the cursor deadlocks behind it (see the adapter's module note).
                    // What it cannot do is authorize a command, which is what `None` decides.
                    actor: u
                        .get("actor")
                        .and_then(|v| v.as_str())
                        .and_then(|v| ActorRef::parse(v).ok()),
                    chat_kind: match u.get("chat_kind").and_then(|v| v.as_str()) {
                        Some("private") => ChatKind::Private,
                        Some("non_private") => ChatKind::NonPrivate,
                        // A field this build does not recognise is a claim it cannot read, and a
                        // claim it cannot read is not a claim.
                        _ => ChatKind::Unattested,
                    },
                })
            })
            .collect()
    }

    /// Send an answer, one chunk at a time, and report what the channel confirmed.
    ///
    /// # The only judgement in this file
    ///
    /// Which [`AttemptOutcome`] to report is the whole of delivery evidence, and getting it wrong
    /// in the optimistic direction puts a second copy of an answer in a client's room.
    ///
    /// - Every chunk confirmed → `Complete`.
    /// - The channel said *no* on the **first** chunk → `Rejected`. This is the only outcome that
    ///   licenses a free retry, so it requires the channel to have said so explicitly, and only
    ///   when nothing had been confirmed yet.
    /// - Anything else → `Ambiguous`, carrying the number of chunks the channel **confirmed**.
    ///   Not the number sent: a chunk whose reply was lost may well be in the client's room, and
    ///   counting it as unconfirmed is the direction that keeps the operator's decision honest.
    ///
    /// Note what a refusal on a *later* chunk reports: `Ambiguous`, not `Rejected`. The channel
    /// refused that chunk, but earlier ones are already readable, so a resend duplicates.
    pub fn send(&self, channel: &str, chunks: &[String]) -> AttemptOutcome {
        let mut confirmed: u32 = 0;
        for (i, chunk) in chunks.iter().enumerate() {
            let last = i + 1 == chunks.len();
            let payload =
                serde_json::json!({ "channel": channel, "text": chunk, "last": last }).to_string();
            let request = self.authorize(
                self.agent
                    .post(format!("{}/send", self.base))
                    .header("content-type", "application/json"),
            );
            match request.send(&payload) {
                Ok(mut r) => {
                    // Three-valued on purpose. `Some(false)` is the channel saying no; `None` is
                    // a reply this code could not read, which is NOT the same thing and must not
                    // be treated as one. Collapsing them with `unwrap_or(false)` would turn every
                    // garbled response into a claim of provable absence — and that claim is what
                    // tells an operator a resend cannot duplicate.
                    let accepted: Option<bool> = r
                        .body_mut()
                        .read_to_string()
                        .ok()
                        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
                        .and_then(|v| v.get("accepted").and_then(serde_json::Value::as_bool));
                    match accepted {
                        Some(true) => confirmed += 1,
                        Some(false) if confirmed == 0 => return AttemptOutcome::Rejected,
                        _ => {
                            return AttemptOutcome::Ambiguous { acknowledged_chunks: confirmed };
                        }
                    }
                }
                // The reply never came. The chunk may be in the client's room; nothing here can
                // tell. That is uncertainty, not failure, and reporting it as failure is what
                // would license a duplicating retry.
                Err(_) => return AttemptOutcome::Ambiguous { acknowledged_chunks: confirmed },
            }
        }
        AttemptOutcome::Complete
    }
}

/// Strip the markdown a plain-text channel cannot render (§E-14).
///
/// The answer is sent with **no parse mode**, so every marker arrives literally: `**FIT:**` shows
/// up as asterisks and `# Summary` as a hash. Prompt guidance does not fix this. Measured against
/// the reference, telling the model "no markdown at all" cut bold spans from ~6 to 3 per reply but
/// never to 0; measured again here on 2026-08-30, ten graded answers from the eval driver of the day (`dev/eval.py`, since replaced by
/// `ironworks-eval`) carried
/// **15 bold spans** despite three separate instructions saying not to. Compliance is
/// probabilistic, so the guarantee has to be deterministic and live in code.
///
/// **Strip, never convert.** Setting a parse mode would let one unescaped character make the
/// channel reject the whole message — trading a formatting nicety for silent delivery failure on
/// the only channel clients use. That is the wrong direction on the one path that must work.
///
/// **Conservative by construction, because the false positives are worse than the bug.** Every
/// paired marker needs a non-space character just inside it, so a client's arithmetic
/// (`2 * 3 * 4`), a `snake_case` filename and a bare `**` all survive untouched. Mangling those
/// would be a worse defect than the asterisks this removes.
///
/// **Code is literal, and that is a rule the passes obey rather than a hope about their order.**
/// A fenced block loses only its fences and an inline span loses only its backticks; what is
/// inside either is emitted byte for byte. Running the emphasis passes over the whole text first
/// and the backtick pass last — which is what this did — silently broke that: `` `__init__` ``
/// arrived as `init`, and so did the same identifier inside a fenced block whose contents this
/// doc comment promised were kept verbatim. So the text is split into prose and literal runs
/// once, up front, and the emphasis passes can only see a marker that is in prose.
///
/// **A `__…__` run whose body is a bare identifier is a name, not emphasis.** By CommonMark's
/// rules `__init__` *is* an underscore-emphasis span, and stripping it delivers `init` — a
/// different identifier, silently, in an answer a client may act on. There is no context in the
/// text that separates it from `__WHY:__`, so the tie is broken by shape, in the direction this
/// module already leans: a body of `[A-Za-z_][A-Za-z0-9_]*` with no space and no punctuation is
/// kept, markers and all. That covers the whole dunder family without a list of names to
/// maintain, and it costs nothing observed — the fifteen bold spans measured on 2026-08-30 were
/// all asterisks, which this does not touch, and the underscore emphasis these answers actually
/// produce is label-shaped (`__WHY:__`, `__FIT:__`) and carries the punctuation that excludes it.
///
/// The accepted cost is the other side of that trade: `__important__` is a real emphasis span
/// and now survives with its underscores showing. Two visible underscores are a blemish; a
/// mangled identifier is wrong output, and this file's standing rule is that the false positives
/// are worse than the bug.
///
/// **What this deliberately does not do: tables.** A markdown table cannot be repaired into
/// something readable by stripping — the pipes just become a run-on line. That is a *service*
/// concern, fixed in the composition that decides what an answer looks like (the personas say
/// "never use a markdown table"), and it is not transport's to solve. Stripping and the prompt
/// rules are therefore not redundant: each covers what the other cannot.
pub fn to_plain(text: &str) -> String {
    let atoms = split(text);
    let atoms = strip_emphasis(atoms, '*');
    let atoms = strip_emphasis(atoms, '_');
    let mut out = String::with_capacity(text.len());
    for atom in atoms {
        match atom {
            Atom::Ch(c) => out.push(c),
            Atom::Literal(s) => out.push_str(&s),
        }
    }
    out
}

/// One unit of the answer: a character of prose, or a run of literal content.
///
/// A `Literal` is opaque to every later pass — it cannot hold a marker, cannot be looked inside,
/// and is emitted exactly as it arrived. It is the whole of the code guarantee, expressed as a
/// type rather than as an ordering between passes.
#[derive(Clone, Debug, PartialEq)]
enum Atom {
    Ch(char),
    Literal(String),
}

impl Atom {
    /// Whether a marker resting against this atom has whitespace on that side.
    ///
    /// A literal run never counts as whitespace: `**`refresh acme`**` is emphasis around code,
    /// and the markers still go.
    fn is_space(&self) -> bool {
        matches!(self, Atom::Ch(c) if c.is_whitespace())
    }
}

/// Split an answer into prose characters and literal runs, dropping the markers that delimit
/// literals and the heading hashes that no channel renders.
///
/// Fences first, because a fenced block outranks everything inside it: a line between fences is
/// one literal run, so no heading is stripped from it and no backtick in it opens a span.
fn split(text: &str) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::new();
    let mut fenced = false;
    let mut first = true;
    for line in text.split('\n') {
        // A fence line is dropped whole; its contents are not.
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !first {
            out.push(Atom::Ch('\n'));
        }
        first = false;
        if fenced {
            out.push(Atom::Literal(line.to_owned()));
        } else {
            split_line(&strip_heading(line), &mut out);
        }
    }
    out
}

/// One prose line: inline code spans become literal runs, everything else becomes characters.
///
/// The span rules are the ones the paired-marker pass uses, so a stray backtick behaves the same
/// as a stray `**` — it is content, and it survives.
fn split_line(line: &str, out: &mut Vec<Atom>) {
    let mut rest = line;
    while let Some(open) = rest.find('`') {
        let body = &rest[open + 1..];
        let close = if body.starts_with(|c: char| !c.is_whitespace()) {
            body.find('`')
                .filter(|&end| end > 0 && body[..end].ends_with(|c: char| !c.is_whitespace()))
        } else {
            None
        };
        match close {
            Some(end) => {
                out.extend(rest[..open].chars().map(Atom::Ch));
                out.push(Atom::Literal(body[..end].to_owned()));
                rest = &body[end + 1..];
            }
            // Not a span. The backtick is content: emit it and carry on past it.
            None => {
                out.extend(rest[..open + 1].chars().map(Atom::Ch));
                rest = body;
            }
        }
    }
    out.extend(rest.chars().map(Atom::Ch));
}

/// `## Summary` -> `Summary`, keeping any indentation. Seven hashes is not a heading.
fn strip_heading(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let (ws, rest) = line.split_at(indent);
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return line.to_owned();
    }
    let after = &rest[hashes..];
    if !after.starts_with(char::is_whitespace) {
        return line.to_owned();
    }
    format!("{ws}{}", after.trim_start())
}

/// Remove `<mm>text<mm>`, keeping `text`. Literal runs are carried through untouched.
///
/// Both ends must sit against a non-space atom, which is the whole of the false-positive defence:
/// a client's arithmetic (`2 * 3 * 4`) and a bare `**` have space just inside and survive.
/// Scanning resumes *after* the closing marker rather than re-reading what was kept, so content
/// that happens to look like a marker cannot be consumed twice. Emphasis may span lines, as it
/// does in the reference.
fn strip_emphasis(atoms: Vec<Atom>, m: char) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::with_capacity(atoms.len());
    let mut i = 0;
    while i < atoms.len() {
        if !marker_at(&atoms, i, m) {
            out.push(atoms[i].clone());
            i += 1;
            continue;
        }
        let body = i + 2;
        let opens = atoms.get(body).is_some_and(|a| !a.is_space());
        match if opens { find_close(&atoms, body, m) } else { None } {
            // A paired run that spells a bare identifier is a name; the markers are part of it.
            Some(end) if m == '_' && is_identifier(&atoms[body..end]) => {
                out.extend_from_slice(&atoms[i..end + 2]);
                i = end + 2;
            }
            Some(end) => {
                out.extend_from_slice(&atoms[body..end]);
                i = end + 2;
            }
            // Not a pair. The marker is content: emit it and carry on past it.
            None => {
                out.push(Atom::Ch(m));
                out.push(Atom::Ch(m));
                i = body;
            }
        }
    }
    out
}

/// Whether these atoms spell a bare identifier — `__init__`'s `init`, and not `__WHY:__`'s `WHY:`.
///
/// Deliberately narrow: ASCII word characters only, no leading digit, and a literal run inside
/// disqualifies it, because a code span is already protected and does not need this as well.
fn is_identifier(body: &[Atom]) -> bool {
    let mut chars = body.iter().map(|a| match a {
        Atom::Ch(c) => Some(*c),
        Atom::Literal(_) => None,
    });
    match chars.next() {
        Some(Some(c)) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// Whether a two-character marker of `m` starts here. A literal run never spells one.
fn marker_at(atoms: &[Atom], i: usize, m: char) -> bool {
    matches!((atoms.get(i), atoms.get(i + 1)), (Some(Atom::Ch(a)), Some(Atom::Ch(b))) if *a == m && *b == m)
}

/// The first closing marker preceded by a non-space atom, or `None`.
fn find_close(atoms: &[Atom], from: usize, m: char) -> Option<usize> {
    let mut i = from;
    while i < atoms.len() {
        if marker_at(atoms, i, m) {
            if i > from && !atoms[i - 1].is_space() {
                return Some(i);
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    None
}

/// Whether this text reaches a client byte for byte.
///
/// The transport's own knowledge, asked before anything durable is written. [`plain_chunks`] is
/// the only way to produce chunks and it deliberately *transforms* the text — §E-14 strips the
/// markdown a plain-text channel renders literally — so bytes carrying a marker arrive changed.
/// For an answer that is correct and invisible. For a proposal a person is about to confirm it is
/// not: they would approve one string and a different one would be stored.
///
/// So a caller holding bytes that must survive exactly asks here and refuses when the answer is
/// `false`, rather than storing something the echo could not show them.
pub fn echoes_exactly(text: &str) -> bool {
    to_plain(text) == text
}

/// Prepare an answer for delivery: strip what the channel cannot render, then split it.
///
/// **The only way to produce chunks**, and that is the point. §E-15 requires stripping to happen
/// before chunking, so that a marker is never split across two messages and the chunk count
/// describes bytes a client can actually read. Ordering two public functions correctly is a rule
/// a call site can forget; one function that does both is a rule it cannot. `chunk` is private
/// for exactly that reason — there is no way to reach it with unstripped text.
pub fn plain_chunks(text: &str, limit: usize) -> Vec<String> {
    chunk(&to_plain(text), limit)
}

/// Split an answer for a channel with a per-message size limit.
///
/// Line boundaries first; a single over-long line is hard-split, because the alternative is
/// refusing to deliver an answer at all. Lossless by contract — it reorganizes bytes and never
/// changes them, which is what lets [`plain_chunks`] own the one transformation that does.
fn chunk(text: &str, limit: usize) -> Vec<String> {
    assert!(limit > 0, "a chunk limit of zero would never terminate");
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        // A line that cannot fit on its own is split at a character boundary, never mid-character.
        if line.len() > limit {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let mut rest = line;
            while rest.len() > limit {
                let mut cut = limit;
                while cut > 0 && !rest.is_char_boundary(cut) {
                    cut -= 1;
                }
                // A single character wider than the limit would leave `cut` at zero and loop
                // forever; take one character instead, which overshoots rather than hangs.
                if cut == 0 {
                    cut = rest.chars().next().map_or(rest.len(), char::len_utf8);
                }
                out.push(rest[..cut].to_owned());
                rest = &rest[cut..];
            }
            current.push_str(rest);
            continue;
        }
        if current.len() + line.len() > limit {
            out.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        out.push(current);
    }
    // An empty answer is still one message: sending nothing at all would look like a delivery
    // that never happened, which is the one thing the evidence must not be ambiguous about.
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A stand-in channel adapter: one canned reply per request, and a record of the request head
    /// it saw.
    ///
    /// The recording is the point. Whether a credential was *sent* is not visible in the reply, so
    /// a test that only reads replies cannot tell an authenticated channel from one that quietly
    /// dropped the header.
    fn stub_channel(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the stub channel");
        let port = listener.local_addr().expect("the stub channel's address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            'accept: for stream in listener.incoming() {
                let Ok(mut sock) = stream else { continue };
                let Ok(clone) = sock.try_clone() else { continue };
                let mut reader = BufReader::new(clone);
                let mut head = String::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if !reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                        continue 'accept;
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    head.push_str(&line);
                }
                let mut discard = vec![0u8; length];
                if length > 0 && reader.read_exact(&mut discard).is_err() {
                    continue;
                }
                record.lock().expect("the record survives its writers").push(head);
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes());
                let _ = sock.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    #[test]
    fn an_authenticated_channel_presents_its_credential_and_a_plain_one_presents_none() {
        let (base, seen) = stub_channel(200, r#"{"updates":[]}"#);
        Channel::authenticated(base.as_str(), "adapter-secret").poll(7).expect("an empty poll");
        // The control: the same stub and the same call, through the constructor that carries no
        // credential. Without it, a stub that recorded nothing would pass the assertion below.
        Channel::new(base.as_str()).poll(7).expect("an empty poll");

        let seen = seen.lock().expect("the record survives its writers");
        assert_eq!(seen.len(), 2, "both polls must have arrived: {seen:?}");
        let authenticated = seen[0].to_ascii_lowercase();
        assert!(
            authenticated.contains("authorization: bearer adapter-secret"),
            "the credential never left: {}",
            seen[0]
        );
        assert!(!seen[1].to_ascii_lowercase().contains("authorization"), "{}", seen[1]);
    }

    #[test]
    fn the_credential_does_not_leak_through_debug() {
        assert_eq!(
            format!("{:?}", AdapterToken("adapter-secret".into())),
            "AdapterToken(redacted)"
        );
        // The control: the value really is in there, so the assertion above is redaction rather
        // than an empty newtype.
        assert_eq!(AdapterToken("adapter-secret".into()).header(), "Bearer adapter-secret");
    }

    /// The refusal an adapter generates **before** contacting the far system, read the way this
    /// crate actually reads it. Both arms, because collapsing them is the defect.
    #[test]
    fn an_adapter_refusal_is_rejected_and_a_reply_this_code_cannot_read_is_not() {
        let (base, _seen) = stub_channel(401, r#"{"accepted":false}"#);
        assert_eq!(
            Channel::new(base.as_str()).send("-100", &["hello".to_owned()]),
            AttemptOutcome::Rejected,
            "provable absence is the one outcome that licenses a free retry"
        );

        // The control, and the other arm. A refusal after contact carries no `accepted` field at
        // all, and must not arrive as the same answer.
        let (base, _seen) = stub_channel(502, r#"{"delivery":"unknown"}"#);
        assert_eq!(
            Channel::new(base.as_str()).send("-100", &["hello".to_owned()]),
            AttemptOutcome::Ambiguous { acknowledged_chunks: 0 }
        );
    }

    /// The same refusal on the poll side, where a *tidier* body would be the damage: an empty
    /// `updates` array reads as "nothing is waiting" and acknowledges through the offset.
    #[test]
    fn a_refused_poll_is_malformed_rather_than_an_empty_one() {
        let (base, _seen) = stub_channel(401, r#"{"accepted":false}"#);
        // Destructured rather than `expect_err`, which would need `Inbound: Debug` — and a Debug
        // on the struct carrying a client's own message text is not a thing to add for a test.
        let Err(err) = Channel::new(base.as_str()).poll(41) else {
            panic!("a refusal must not read as a poll");
        };
        assert!(matches!(err, ChannelError::Malformed(_)), "{err:?}");

        // The control: the same code path and a body that does carry the array reads as an empty
        // poll, so the assertion above measures the missing key rather than the status code.
        let (base, _seen) = stub_channel(200, r#"{"updates":[]}"#);
        assert!(Channel::new(base.as_str()).poll(41).expect("an empty poll").is_empty());
    }

    #[test]
    fn the_proposal_echo_survives_this_transport_word_for_word() {
        // The two halves of one promise, asserted together. `domain` writes the sentence that says
        // "exactly these bytes"; this crate owns the rendering that could quietly rewrite it. A
        // marker slipping into that prose would strip on the way out and the person would confirm
        // something they were never shown.
        let ironworks_domain::Intent::Share(request) =
            ironworks_domain::share::parse("/share ACME-001 Customer requested a renewal review.")
        else {
            panic!("the fixture must parse as a share");
        };
        let who = ironworks_domain::ContributorId::parse("alice").expect("contributor");
        let echo = ironworks_domain::render_echo(&request, &who);
        assert!(echoes_exactly(&echo), "the echo is rewritten in transit: {echo:?}");
        // And it arrives as one message rather than a sequence a person has to reassemble
        // before deciding whether it is what they meant.
        assert_eq!(plain_chunks(&echo, 3800).len(), 1);
    }

    #[test]
    fn text_the_transport_would_change_is_not_echoable() {
        // The check a proposal depends on. `plain_chunks` is the only chunker and it strips
        // markdown by design, so a note carrying a marker would be confirmed as one string and
        // stored as another.
        assert!(echoes_exactly("Customer requested a September renewal review."));
        assert!(echoes_exactly("2 * 3 * 4 = 24"), "content that only looks like a marker is fine");
        assert!(!echoes_exactly("**bold**"));
        assert!(!echoes_exactly("## heading"));
        assert!(!echoes_exactly("call `refresh` first"));
        // The control: without it the assertions above would pass on a function returning true.
        assert_ne!(to_plain("**bold**"), "**bold**");
    }

    /// The cases are the reference's own, kept verbatim so a divergence is visible rather than
    /// argued about. Its test says the false-positive half "matters as much as the stripping",
    /// and it is right: mangling a client's arithmetic or a filename is a worse defect than the
    /// asterisks this removes.
    #[test]
    fn markdown_the_channel_cannot_render_is_stripped() {
        for (src, want) in [
            ("**FIT:** strong", "FIT: strong"),
            ("## Summary\nbody", "Summary\nbody"),
            ("__WHY:__ evidence", "WHY: evidence"),
            ("use `refresh acme` now", "use refresh acme now"),
            ("```\ncode kept\n```", "code kept"),
            ("**multi\nline** bold", "multi\nline bold"),
        ] {
            assert_eq!(to_plain(src), want, "{src:?}");
        }
    }

    #[test]
    fn content_that_merely_looks_like_a_marker_is_left_alone() {
        for untouched in [
            "2 * 3 * 4 = 24",        // a client's arithmetic
            "a_b_c filename",        // snake_case
            "run send_daily_digest", // snake_case in prose
            "bare ** marker",        // no closing pair
            "a lone ` backtick",     // ditto
            "no markdown here",      // nothing to do
            "####### seven",         // not a heading
        ] {
            assert_eq!(to_plain(untouched), untouched, "{untouched:?}");
        }
    }

    /// The regression. Emphasis used to run over the whole answer before backticks were removed,
    /// so a marked-up identifier was stripped as emphasis and only *then* unmarked: `` `__init__` ``
    /// reached the channel as `init`, and so did the same identifier inside a fenced block that
    /// this module's own doc comment promised was kept verbatim. Marking something as code is the
    /// only way an answer can say "this is literal", and the transport has to honour it.
    #[test]
    fn code_is_literal_and_only_its_delimiters_are_removed() {
        for (src, want) in [
            // inline: the backticks go, nothing inside them does
            ("call `__init__` first", "call __init__ first"),
            ("use `refresh acme` now", "use refresh acme now"),
            ("set `snake_case_name` here", "set snake_case_name here"),
            ("see `**not bold**` there", "see **not bold** there"),
            // fenced: the fences go, the block does not
            ("```\ndef __init__(self):\n```", "def __init__(self):"),
            ("```python\nx = a ** b\n```", "x = a ** b"),
            ("```\n## not a heading\n```", "## not a heading"),
            // a fenced block sits inside an answer without taking the prose with it
            ("**FIT:** strong\n```\n__all__ = []\n```\ndone", "FIT: strong\n__all__ = []\ndone"),
        ] {
            assert_eq!(to_plain(src), want, "{src:?}");
        }
    }

    /// A literal identifier survives with no markup at all around it. This is the direct control:
    /// `__init__` is an emphasis span by CommonMark's rules, and delivering `init` would hand a
    /// client a different name than the model wrote.
    #[test]
    fn a_bare_dunder_identifier_is_a_name_and_is_kept_whole() {
        for src in [
            "__init__",
            "override __init__ on the subclass",
            "__main__",
            "__all__ and __name__",
            "__private_helper__",
        ] {
            assert_eq!(to_plain(src), src, "{src:?}");
        }
        // The two routes to keeping it agree: the code span loses only its delimiters, and the
        // bare identifier loses nothing.
        assert_eq!(to_plain("`__init__` and __init__"), "__init__ and __init__");
    }

    /// The negative control for the same rule, and the reason it is a shape test rather than a
    /// list of names: underscore emphasis in these answers is label-shaped, and the punctuation
    /// or whitespace that makes it a label is exactly what makes it not an identifier.
    #[test]
    fn underscore_emphasis_that_is_not_an_identifier_is_still_stripped() {
        for (src, want) in [
            ("__WHY:__ evidence", "WHY: evidence"),
            ("__FIT:__ unclear", "FIT: unclear"),
            ("__two words__ emphasised", "two words emphasised"),
            ("__multi\nline__ underscore", "multi\nline underscore"),
            ("__1st__ place", "1st place"),
            ("__ends with a dot.__ here", "ends with a dot. here"),
        ] {
            assert_eq!(to_plain(src), want, "{src:?}");
        }
    }

    /// The accepted cost of that trade, asserted so it is a decision on the record rather than a
    /// surprise: a genuine one-word underscore emphasis now reaches the channel with its markers.
    /// Asterisk emphasis — every one of the fifteen bold spans actually measured — is untouched.
    #[test]
    fn one_word_underscore_emphasis_survives_and_that_is_the_accepted_cost() {
        assert_eq!(to_plain("the __important__ part"), "the __important__ part");
        assert_eq!(to_plain("the **important** part"), "the important part");
    }

    /// §E-15. Stripping after chunking would let a marker straddle two messages, and would make
    /// the chunk count describe bytes the client never receives.
    #[test]
    fn stripping_happens_before_chunking_so_no_marker_can_straddle_a_boundary() {
        // Through the public entry point, because that is the one a caller can reach.
        let answer = format!("**x**\n{}", "y".repeat(4000));
        let chunks = plain_chunks(&answer, 3800);
        assert!(chunks.len() > 1, "the fixture must actually span chunks");
        assert!(!chunks.concat().contains("**"), "a marker survived into the channel");
    }

    /// A control. Without it the two tests above would still pass if `to_plain` returned its
    /// input unchanged and the fixtures happened to contain nothing to strip.
    #[test]
    fn the_stripper_actually_changes_something() {
        assert_ne!(to_plain("**bold**"), "**bold**");
    }

    /// Real output, not an invented fixture. These lines are verbatim from the graded answers of
    /// the 2026-08-30 eval run (`dev/eval.py`, 10/10 — that driver is now `ironworks-eval`), which
    /// carried 15 bold spans across ten
    /// replies despite three separate prompt-level instructions not to use markdown. They are the
    /// measurement that justifies this code existing, so they are what it is tested against.
    ///
    /// The last one carries an em dash: a multibyte character adjacent to the stripped region is
    /// exactly where a byte-indexed implementation would panic.
    #[test]
    fn the_bold_actually_observed_in_graded_answers_is_removed() {
        for (src, want) in [
            ("**Account: Perch Analytics (PA-003)**", "Account: Perch Analytics (PA-003)"),
            ("**KNOWN (FACT from account records):**", "KNOWN (FACT from account records):"),
            ("**FIT:** unclear — discovery target", "FIT: unclear — discovery target"),
        ] {
            assert_eq!(to_plain(src), want, "{src:?}");
        }
    }

    #[test]
    fn chunks_break_on_lines_and_never_exceed_the_limit() {
        let text = "alpha\nbeta\ngamma\n";
        let chunks = chunk(text, 12);
        assert!(chunks.iter().all(|c| c.len() <= 12), "{chunks:?}");
        assert_eq!(chunks.concat(), text, "chunking changed the answer");
        assert_eq!(chunks, vec!["alpha\nbeta\n", "gamma\n"]);
    }

    #[test]
    fn a_single_over_long_line_is_hard_split_rather_than_refused() {
        let text = "x".repeat(25);
        let chunks = chunk(&text, 10);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn a_multibyte_character_is_never_split_in_half() {
        // Four-byte characters against a limit that does not divide by four: a naive slice would
        // panic, and a lossy one would deliver a different answer than the model produced.
        let text = "🙂".repeat(10);
        let chunks = chunk(&text, 10);
        assert!(chunks.iter().all(|c| c.len() <= 10 + 4), "{chunks:?}");
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn an_empty_answer_is_still_one_message() {
        assert_eq!(chunk("", 10), vec![String::new()]);
    }

    #[test]
    fn reassembly_is_exact_for_every_limit() {
        let text = "one\ntwo\n\nthree and a rather longer line here\nfour\n";
        for limit in 1..60 {
            assert_eq!(chunk(text, limit).concat(), text, "limit {limit} changed the answer");
        }
    }
}
