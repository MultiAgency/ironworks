//! `ironworks-telegram` — the channel adapter for Telegram.
//!
//! IronWorks talks to a channel through one contract, and nothing else:
//!
//! ```text
//! GET  /poll?offset=N   -> { "updates": [ { "id": u64, "channel": String, "text": String,
//!                                           "actor": String?, "chat_kind": String? } ] }
//! POST /send            <- { "channel": String, "text": String, "last": bool }
//!                       -> { "accepted": true | false }   or a body carrying no `accepted`
//! ```
//!
//! Both carry `Authorization: Bearer <CHANNEL_ADAPTER_TOKEN>`; only `/health` does not.
//!
//! `actor` and `chat_kind` are **evidence, and absent when there is none.** `actor` is Telegram's
//! `from.id` verbatim; `chat_kind` is `private` or `non_private`, and is omitted entirely for an
//! update carrying no chat or a chat type this adapter does not recognise. The seam reads an
//! absent field as *no claim*, never as a permissive default, which is what lets a command path
//! that requires a private chat fail closed against an adapter older than the requirement.
//!
//! This process serves that contract on one side and the Telegram Bot API on the other. It is the
//! same contract `ironworks-devchannel` implements for the proofs, which is the point: the seam
//! cannot tell them apart, so nothing on the serving path changes to reach a real channel.
//!
//! ```text
//! TELEGRAM_BOT_TOKEN=... CHANNEL_ADAPTER_TOKEN=... ironworks-telegram --port 3994
//! ```
//!
//! # Why everything but `/health` needs a token
//!
//! **`/poll` is a destructive write.** The offset *is* the acknowledgement, so
//! `GET /poll?offset=999999999` tells Telegram to discard every pending update — before the seam
//! has journalled one of them. There is no row afterwards to recover from, nothing reports an
//! error, and both processes stay healthy while every message sent in that window is gone.
//! Loopback is not a boundary here: this host also runs a second fleet and one runtime process
//! per client, each under its own account, and any of them can open `127.0.0.1:3994`.
//!
//! So `CHANNEL_ADAPTER_TOKEN` is required at startup rather than defaulted or optional. An
//! adapter that *can* start without authentication is one that eventually does, on the host where
//! it matters, and the resulting deployment looks correct from every angle. `/health` stays open
//! because it is the one route with no backlog behind it: it reads nothing and acknowledges
//! nothing.
//!
//! # Why a refusal is shaped the way it is
//!
//! This is the only reply here that is **evidence about Telegram without having contacted it**,
//! and the two readers of it want opposite things from the same body — see [`REFUSED`].
//!
//! # Why the offset passes straight through
//!
//! `ironworks-channel`'s semantics were modelled on Telegram's, so the translation is an identity
//! rather than an adaptation. Telegram's `getUpdates(offset)` means *confirm everything below
//! `offset` and return from it*; the seam's `poll(offset)` means exactly the same thing, and in
//! both the acknowledgement is the request rather than a separate call. Redelivery until an
//! offset past an update is likewise both systems' behaviour. Nothing here needs to remember a
//! cursor, and it must not: the seam derives the safe cursor from its own durable rows across
//! every tenant, and an adapter keeping a second one would be a second answer to a question that
//! must have one.
//!
//! # What it does with an update it cannot read
//!
//! It returns it anyway, with empty text.
//!
//! Dropping it is the obvious implementation and it deadlocks the bot. The seam advances the
//! offset only past updates it has journalled; an update this process silently swallowed is one
//! the seam never learns of, so it polls from the same offset, Telegram returns the same
//! unreadable update, and every conversation behind it stops — while both processes look healthy.
//! Handing it over lets the seam disposition it `Unroutable` and move the cursor past it, which is
//! what that disposition is for.
//!
//! # `accepted` is evidence, not a status code
//!
//! It decides whether an operator may resend without duplicating an answer already in someone's
//! room, so this file's only real judgement is [`Acceptance`]. `false` is a positive claim that
//! **nothing was delivered** and is reported only when Telegram says so; anything this process
//! cannot read is reported as unknown, by answering with no `accepted` field at all — which the
//! client reads as ambiguous rather than as a refusal.
//!
//! # Standing one up
//!
//! Two things about this are not obvious and cost an afternoon each to find out.
//!
//! **Your chat id comes from the adapter, not from Telegram's UI.** Start this process, send the
//! bot any message, then ask it what it saw — the `channel` field is the id `relay --channel-id`
//! wants. A group is negative, a direct message positive.
//!
//! ```text
//! curl -s -H "authorization: Bearer $CHANNEL_ADAPTER_TOKEN" \
//!   "http://127.0.0.1:3994/poll?offset=0"
//! ```
//!
//! That request acknowledges everything below offset 0, which is nothing. Any *other* offset in
//! that command discards a real backlog, which is why the token is not a formality.
//!
//! **In a group, a bot sees nothing by default.** BotFather ships privacy mode on, which hides
//! every message not addressed to the bot, so `/poll` returns an empty list and everything looks
//! broken while working exactly as configured. `@BotFather` → `/setprivacy` → Disable, then
//! remove and re-add the bot to the group. A direct message needs none of this.
//!
//! # No parse mode, deliberately
//!
//! The seam strips markdown before chunking (§E-14) because a plain-text channel renders none.
//! Setting a parse mode here would re-introduce exactly the failure that stripping removes: one
//! unescaped character makes Telegram reject an entire message, trading a formatting nicety for
//! silent delivery failure on the only path that matters.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

const API: &str = "https://api.telegram.org";
const ADAPTER_IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;

// ── the adapter's own credential ─────────────────────────────────────────────────────────

/// The token a caller must present to reach anything but `/health`.
///
/// Held only to be compared against: no `Display`, a redacting `Debug`, and no accessor — so
/// getting it into a log takes a deliberate change rather than a stray `{:?}`. That is the shape
/// `ironworks_records::Secret` already has, written again here rather than imported, because this
/// adapter depends on **no** IronWorks crate. Reaching for one would put an adapter-to-adapter
/// edge in the graph that answers "who can talk to what", to save six lines.
struct AdapterToken(String);

impl AdapterToken {
    /// Whitespace is trimmed here, and again on the header, so a value that picked up a trailing
    /// space in an environment file is not a channel that stops for a reason nothing reports.
    fn new(value: &str) -> Self {
        Self(value.trim().to_owned())
    }

    /// Whether this request presented it. An absent header, one this code cannot read, and a
    /// wrong value are one answer, and it is *no*.
    fn presented(&self, authorization: Option<&str>) -> bool {
        let Some((scheme, offered)) = authorization.and_then(|v| v.trim().split_once(' ')) else {
            return false;
        };
        // The scheme is case-insensitive (RFC 7235). The credential after it is not.
        scheme.eq_ignore_ascii_case("bearer") && secret_eq(offered.trim().as_bytes(), &self.0)
    }
}

impl std::fmt::Debug for AdapterToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdapterToken(redacted)")
    }
}

/// Compare every byte of equal-length candidates instead of exposing a prefix oracle.
///
/// Length remains visible and Rust does not promise constant-time machine code; this is a narrow
/// defence for a same-host listener, not a cryptographic constant-time API.
fn secret_eq(offered: &[u8], expected: &str) -> bool {
    let expected = expected.as_bytes();
    let mut differing = offered.len() ^ expected.len();
    for (a, b) in offered.iter().zip(expected.iter()) {
        differing |= usize::from(a ^ b);
    }
    differing == 0
}

/// The refusal this adapter generates **before Telegram is contacted**, and the reason one body
/// serves both routes: each reads exactly what it should from it.
///
/// - `Channel::send` reads `accepted` three-valued, and `false` is a positive claim that *nothing
///   was delivered*. Here that claim is true by construction, because nothing was attempted — so
///   this is the one outcome that licenses a free retry, and saying it is honest rather than
///   optimistic. A refusal *after* contact must carry no `accepted` field at all, which is what
///   [`Acceptance::Unknown`] already does.
/// - `Channel::poll` reads the `updates` array and treats its absence as malformed, which leaves
///   the cursor where it is. An empty list would read as "nothing is waiting" and acknowledge
///   through the offset that was asked for — the exact damage the refusal exists to prevent.
///
/// Anything else that reaches this arm — an unknown path, a route added later — inherits
/// provable-absence semantics, which is the safe direction to inherit.
const REFUSED: &str = "{\"accepted\":false}";

/// What a send attempt established about delivery. The whole of this adapter's judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Acceptance {
    /// Telegram confirmed the message. The chunk is in the room.
    Confirmed,
    /// Telegram refused it, and said so. Nothing was delivered, so a resend cannot duplicate —
    /// the only outcome that licenses a free retry, which is why it takes an explicit refusal.
    Refused,
    /// Anything this process could not read. The chunk may or may not be in the room, and
    /// claiming either way is the one mistake that cannot be undone.
    Unknown,
}

impl Acceptance {
    /// Map one Bot API reply. Pure, because it is the part worth testing.
    ///
    /// Telegram answers `{"ok": true, "result": {…}}` on success and `{"ok": false, …}` on a
    /// refusal it is certain about. A body that does not parse, or parses without `ok`, is not a
    /// refusal — it is a reply this code does not understand, and the two must not collapse.
    fn read(status: u16, body: &str) -> Self {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
            return Self::Unknown;
        };
        match parsed.get("ok").and_then(serde_json::Value::as_bool) {
            Some(true) => Self::Confirmed,
            // A refusal is only a refusal when the status agrees it is one. A 5xx carrying
            // `ok:false` is Telegram failing, not Telegram declining, and it may still have sent.
            Some(false) if (400..500).contains(&status) => Self::Refused,
            _ => Self::Unknown,
        }
    }

    /// The reply the seam reads. `Unknown` deliberately carries no `accepted` field: the client
    /// reads a missing or unreadable value as ambiguous, and that is the honest answer.
    fn body(self) -> String {
        match self {
            Self::Confirmed => "{\"accepted\":true}".into(),
            Self::Refused => "{\"accepted\":false}".into(),
            Self::Unknown => "{\"delivery\":\"unknown\"}".into(),
        }
    }

    fn status(self) -> u16 {
        match self {
            Self::Confirmed => 200,
            Self::Refused => 409,
            Self::Unknown => 502,
        }
    }
}

/// One Telegram update, as the channel contract wants it.
///
/// `text` is empty for anything without a readable message body, and the update is still
/// returned — see the note on deadlock above.
fn as_inbound(update: &serde_json::Value) -> Option<serde_json::Value> {
    let id = update.get("update_id").and_then(serde_json::Value::as_u64)?;
    // Any of these carries a chat; an update with none still travels, with an empty channel.
    let message = ["message", "edited_message", "channel_post", "edited_channel_post"]
        .iter()
        .find_map(|k| update.get(*k));
    let channel = message
        .and_then(|m| m.get("chat"))
        .and_then(|c| c.get("id"))
        .map(|id| id.to_string())
        .unwrap_or_default();
    let text =
        message.and_then(|m| m.get("text")).and_then(|t| t.as_str()).unwrap_or_default().to_owned();
    // `from.id`, verbatim and unprefixed. Namespacing it would be inventing a scheme this
    // adapter has no second channel to disambiguate against, and the registry that consumes it
    // is already scoped to one deployment's channels.
    //
    // Deliberately NOT `from.username` or `from.first_name`: both are chosen by the sender and
    // changed at will, and this value selects a contributor. `channel_post` carries no `from` at
    // all, which is exactly the case that must arrive as no actor.
    let actor = message
        .and_then(|m| m.get("from"))
        .and_then(|f| f.get("id"))
        .filter(|id| id.is_number())
        .map(std::string::ToString::to_string);
    // `chat.type` is Telegram's own word for the room. Only `private` is one-to-one; `group`,
    // `supergroup` and `channel` are not, and a type this build has never seen is reported as
    // nothing rather than guessed at.
    let chat_kind = message
        .and_then(|m| m.get("chat"))
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        .and_then(|t| match t {
            "private" => Some("private"),
            "group" | "supergroup" | "channel" => Some("non_private"),
            _ => None,
        });
    let mut out = serde_json::json!({ "id": id, "channel": channel, "text": text });
    let map = out.as_object_mut().expect("a json object literal is an object");
    if let Some(actor) = actor {
        map.insert("actor".into(), serde_json::Value::String(actor));
    }
    if let Some(kind) = chat_kind {
        map.insert("chat_kind".into(), serde_json::Value::String(kind.to_owned()));
    }
    Some(out)
}

// ── the Bot API ──────────────────────────────────────────────────────────────────────────

struct Bot {
    agent: ureq::Agent,
    base: String,
}

// ── the HTTP budget ─────────────────────────────────────────────────────────────────────────
//
// Named rather than chained inline, because this exact ladder is written in four other crates and
// `crates/ironclaw/src/client.rs::Ironclaw::new` deliberately runs a longer one. That intentional
// difference was invisible against five silent copies of the same eight-call chain; against named
// constants it is a diff on a number. The crates cannot share the values without an edge none of
// them should have, so each states its own and says which system it is stating them about.

/// DNS for the Telegram Bot API.
const RESOLVE: Duration = Duration::from_secs(5);
/// TCP connect to the Telegram Bot API.
const CONNECT: Duration = Duration::from_secs(5);
/// Writing the request to the Telegram Bot API.
const SEND: Duration = Duration::from_secs(10);
/// Waiting for the Telegram Bot API to begin answering.
const RECV_RESPONSE: Duration = Duration::from_secs(30);
/// Reading the Telegram Bot API's answer.
const RECV_BODY: Duration = Duration::from_secs(30);

impl Bot {
    /// The token is read from the environment and never from a flag: a flag lands in shell
    /// history and in every `ps` on the box. It is held only to build a URL, and no code path
    /// here prints `base`.
    fn new(token: &str, api: &str) -> Self {
        // Per-phase timeouts, matching `channel-http`'s reasoning: a phase-agnostic timeout
        // cannot say whether a request was attempted, and for a send that distinction is the
        // evidence. `http_status_as_error(false)` because a 4xx is an answer to be read, not a
        // transport failure — reading it as one would turn every refusal into `Unknown` and make
        // the only retry-safe outcome unreachable.
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
            base: format!("{}/bot{}", api.trim_end_matches('/'), token),
        }
    }

    /// `getUpdates` from an offset. Returns the updates, or the reason it could not.
    ///
    /// `timeout=0` — short polling. Long polling would hold the request open for a minute, which
    /// is the right shape for a daemon and the wrong one here: nothing in IronWorks runs a loop,
    /// `relay` handles one cycle and exits, and a poll that blocks for a minute makes every cycle
    /// cost a minute whether or not anything arrived.
    fn updates(&self, offset: u64) -> Result<Vec<serde_json::Value>, String> {
        let mut reply = self
            .agent
            .get(format!("{}/getUpdates?offset={offset}&timeout=0", self.base))
            .call()
            .map_err(|e| format!("getUpdates unreachable: {e}"))?;
        let body =
            reply.body_mut().read_to_string().map_err(|e| format!("getUpdates body: {e}"))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("getUpdates malformed: {e}"))?;
        if parsed.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            // Deliberately not echoed: a Bot API error body can carry the token back in some
            // failure modes, and this process must not be the thing that logs it.
            return Err("getUpdates refused".into());
        }
        let result = parsed
            .get("result")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "getUpdates carried no result array".to_owned())?;
        Ok(result.iter().filter_map(as_inbound).collect())
    }

    /// One chunk to one chat. No parse mode — see the module note.
    fn send(&self, chat: &str, text: &str) -> Acceptance {
        let payload = serde_json::json!({ "chat_id": chat, "text": text }).to_string();
        match self
            .agent
            .post(format!("{}/sendMessage", self.base))
            .header("content-type", "application/json")
            .send(&payload)
        {
            Ok(mut reply) => {
                let status = reply.status().as_u16();
                match reply.body_mut().read_to_string() {
                    Ok(body) => Acceptance::read(status, &body),
                    // The status arrived and the body did not. Telegram may well have sent it.
                    Err(_) => Acceptance::Unknown,
                }
            }
            // No reply at all. Nothing here can tell whether the message left.
            Err(_) => Acceptance::Unknown,
        }
    }
}

// ── the channel contract, served ─────────────────────────────────────────────────────────

fn main() {
    let mut port: u16 = 3994;
    let mut api = API.to_owned();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--port" => {
                port = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--port needs a number"));
            }
            // For pointing the adapter at a stand-in Bot API. Never at a second real endpoint.
            "--api" => api = it.next().cloned().unwrap_or_else(|| die("--api needs a url")),
            other => die(&format!("unknown argument {other}")),
        }
    }
    let token = std::env::var("TELEGRAM_BOT_TOKEN")
        .unwrap_or_else(|_| die("TELEGRAM_BOT_TOKEN is not set (mint one with @BotFather)"));
    if token.trim().is_empty() {
        die("TELEGRAM_BOT_TOKEN is empty");
    }
    // Not optional, and deliberately not defaulted. `/poll` acknowledges a backlog at Telegram,
    // so a build that can serve it unauthenticated is a build that will — and the deployment that
    // does looks healthy from every angle. Refusing to start is the only signal that arrives
    // before the damage rather than after it.
    let adapter = std::env::var("CHANNEL_ADAPTER_TOKEN").unwrap_or_else(|_| {
        die("CHANNEL_ADAPTER_TOKEN is not set (the relay must be given the same value)")
    });
    if adapter.trim().is_empty() {
        die("CHANNEL_ADAPTER_TOKEN is empty");
    }
    let adapter = AdapterToken::new(&adapter);

    let bot = Bot::new(&token, &api);
    // `die`, not `expect`. Every other startup failure in this file goes through it with a
    // sentence an operator can act on, and a port already in use is the most routine of them.
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(why) => die(&format!("cannot bind 127.0.0.1:{port}: {why}")),
    };
    eprintln!("ironworks-telegram: serving the channel contract on 127.0.0.1:{port}");
    eprintln!("ironworks-telegram: relay with --channel http://127.0.0.1:{port}");
    eprintln!(
        "ironworks-telegram: /poll and /send require CHANNEL_ADAPTER_TOKEN; /health does not"
    );
    for stream in listener.incoming() {
        match stream {
            Ok(s) => handle(s, &bot, &adapter),
            Err(_) => continue,
        }
    }
}

fn die(message: &str) -> ! {
    eprintln!("ironworks-telegram: {message}");
    std::process::exit(64);
}

fn handle(mut stream: TcpStream, bot: &Bot, token: &AdapterToken) {
    let _ = stream.set_read_timeout(Some(ADAPTER_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(ADAPTER_IO_TIMEOUT));
    let Ok(clone) = stream.try_clone() else { return };
    let mut reader = BufReader::new(clone);
    let request_line = match read_bounded_line(&mut reader, MAX_REQUEST_LINE_BYTES) {
        Ok(Some(line)) => line,
        Ok(None) => return,
        Err(()) => return respond(&mut stream, 414, "{\"error\":\"request too large\"}"),
    };
    let mut length = 0usize;
    let mut authorization: Option<String> = None;
    let mut header_bytes = 0usize;
    loop {
        match read_bounded_line(&mut reader, MAX_HEADER_LINE_BYTES) {
            Ok(None) | Err(()) => {
                return respond(&mut stream, 431, "{\"error\":\"headers too large\"}");
            }
            Ok(Some(header)) if header == "\r\n" || header == "\n" => break,
            Ok(Some(header)) => {
                header_bytes = header_bytes.saturating_add(header.len());
                if header_bytes > MAX_HEADER_BYTES {
                    return respond(&mut stream, 431, "{\"error\":\"headers too large\"}");
                }
                let Some((name, value)) = header.split_once(':') else { continue };
                match name.trim().to_ascii_lowercase().as_str() {
                    "content-length" => {
                        let Ok(parsed) = value.trim().parse() else {
                            return respond(
                                &mut stream,
                                400,
                                "{\"error\":\"invalid content length\"}",
                            );
                        };
                        length = parsed;
                    }
                    // Only the *name* is folded. A header name is case-insensitive and a bearer
                    // token is not, so lowercasing the whole line — which is how the
                    // content-length above reads — would reject every token carrying a capital.
                    "authorization" => authorization = Some(value.trim().to_owned()),
                    _ => {}
                }
            }
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    match (method, path) {
        // The one route with no backlog behind it, and so the one served without a credential.
        ("GET", "/health") => respond(&mut stream, 200, "{\"ok\":true}"),
        // Everything below this arm reaches Telegram, and `/poll` reaching it acknowledges a
        // backlog irreversibly. The guard sits above the routes rather than inside each of them
        // so that a route added later is refused by default rather than by remembering.
        _ if !token.presented(authorization.as_deref()) => respond(&mut stream, 401, REFUSED),
        ("GET", "/poll") => {
            let offset: u64 = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("offset="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            match bot.updates(offset) {
                Ok(updates) => respond(
                    &mut stream,
                    200,
                    &serde_json::json!({ "updates": updates }).to_string(),
                ),
                // A poll that failed must not answer with an empty update list: the seam would
                // read that as "nothing is waiting" and acknowledge through the offset it asked
                // from. An error keeps the cursor where it is.
                Err(why) => {
                    eprintln!("ironworks-telegram: {why}");
                    respond(&mut stream, 502, "{\"error\":\"upstream\"}")
                }
            }
        }
        ("POST", "/send") => {
            if length > MAX_BODY_BYTES {
                return respond(&mut stream, 413, "{\"error\":\"body too large\"}");
            }
            let mut body = vec![0u8; length];
            if length > 0 && reader.read_exact(&mut body).is_err() {
                return respond(&mut stream, 400, "{\"error\":\"incomplete body\"}");
            }
            let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body) else {
                return respond(&mut stream, 400, "{\"error\":\"unparseable\"}");
            };
            let chat = parsed.get("channel").and_then(|v| v.as_str()).unwrap_or_default();
            let text = parsed.get("text").and_then(|v| v.as_str()).unwrap_or_default();
            if chat.is_empty() {
                return respond(&mut stream, 400, "{\"error\":\"no channel\"}");
            }
            let outcome = bot.send(chat, text);
            respond(&mut stream, outcome.status(), &outcome.body());
        }
        _ => respond(&mut stream, 404, "{\"error\":\"no such path\"}"),
    }
}

fn read_bounded_line(reader: &mut impl BufRead, maximum: usize) -> Result<Option<String>, ()> {
    let mut line = String::new();
    let read = reader.take((maximum + 1) as u64).read_line(&mut line).map_err(|_| ())?;
    if read == 0 {
        Ok(None)
    } else if read > maximum {
        Err(())
    } else {
        Ok(Some(line))
    }
}

fn respond(stream: &mut TcpStream, code: u16, body: &str) {
    let response = format!(
        "HTTP/1.1 {code}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// What the seam does with a reply body: three-valued, and the absence of `accepted` is the
    /// third value rather than a parse failure. Mirrors `channel-http`'s reader, so these tests
    /// assert the contract as the client actually reads it.
    fn client_reads(body: &str) -> Option<bool> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("accepted").and_then(serde_json::Value::as_bool))
    }

    /// A stand-in Bot API that **counts what actually reached it**.
    ///
    /// The count is the measurement, and a status-code assertion is not a substitute for it: a
    /// build that answered 401 and called `getUpdates` anyway would have discarded the backlog
    /// exactly as the unguarded one did, and every assertion about the reply would still pass.
    ///
    /// It answers `connection: close` and reads request-by-request, so a pooled connection cannot
    /// hide a request from the counter and an idle one cannot starve the next accept.
    fn stub_api() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the stub api");
        let port = listener.local_addr().expect("the stub api's address").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut sock) = stream else { continue };
                let Ok(clone) = sock.try_clone() else { continue };
                let mut reader = BufReader::new(clone);
                loop {
                    let mut request_line = String::new();
                    if !reader.read_line(&mut request_line).is_ok_and(|n| n > 0) {
                        break;
                    }
                    let mut header = String::new();
                    while reader.read_line(&mut header).is_ok_and(|n| n > 0) {
                        if header == "\r\n" || header == "\n" {
                            break;
                        }
                        header.clear();
                    }
                    counter.fetch_add(1, Ordering::SeqCst);
                    let body = r#"{"ok":true,"result":[]}"#;
                    let reply = format!(
                        "HTTP/1.1 200\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(reply.as_bytes());
                    let _ = sock.flush();
                }
            }
        });
        (format!("http://127.0.0.1:{port}"), hits)
    }

    /// One request through the real request handler, over a real socket. Returns the status and
    /// the body, because both are part of the contract and only one of them is the damage.
    fn through_adapter(bot: &Bot, token: &AdapterToken, raw: &str) -> (u16, String) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the adapter");
        let addr = listener.local_addr().expect("the adapter's address");
        let mut client = TcpStream::connect(addr).expect("connect to the adapter");
        client.write_all(raw.as_bytes()).expect("write the request");
        client.flush().expect("flush the request");
        // Half-close, so a request whose `content-length` overruns its body fails rather than
        // waiting forever for bytes the test is never going to send.
        client.shutdown(std::net::Shutdown::Write).expect("half-close");
        let (server, _) = listener.accept().expect("accept");
        handle(server, bot, token);
        let mut reply = String::new();
        client.read_to_string(&mut reply).expect("read the reply");
        let (head, body) = reply.split_once("\r\n\r\n").expect("a reply with a body");
        let status =
            head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).expect("a status line");
        (status, body.to_owned())
    }

    /// The load-bearing one. `/poll` is a destructive write — the offset is the acknowledgement —
    /// so what an unauthenticated request must produce is **no upstream request at all**, not a
    /// tidy 401. Counted against a real listener, with the authenticated call as the control.
    #[test]
    fn an_unauthenticated_poll_reaches_telegram_zero_times_and_an_authenticated_one_once() {
        let (api, upstream) = stub_api();
        let bot = Bot::new("stub-bot-token", &api);
        let token = AdapterToken::new("adapter-secret");
        // The offset that discards everything Telegram is holding.
        let destructive = "/poll?offset=999999999";

        let (status, body) = through_adapter(
            &bot,
            &token,
            &format!("GET {destructive} HTTP/1.1\r\nhost: adapter\r\n\r\n"),
        );
        assert_eq!(status, 401);
        assert_eq!(
            upstream.load(Ordering::SeqCst),
            0,
            "a refused poll still acknowledged the backlog at Telegram"
        );
        // And the refusal must not read as an empty poll, or the seam advances its cursor through
        // the very offset it asked from.
        assert!(!body.contains("updates"), "a refusal that reads as `nothing is waiting`: {body}");

        // The control. Without it, a handler that refused everything — or a stub nothing could
        // reach — would satisfy every assertion above.
        let (status, body) = through_adapter(
            &bot,
            &token,
            &format!(
                "GET {destructive} HTTP/1.1\r\nhost: adapter\r\nauthorization: Bearer adapter-secret\r\n\r\n"
            ),
        );
        assert_eq!(status, 200);
        assert!(body.contains("updates"), "{body}");
        assert_eq!(upstream.load(Ordering::SeqCst), 1, "the authenticated poll never arrived");
    }

    #[test]
    fn health_answers_without_a_credential_and_nothing_else_does() {
        let (api, upstream) = stub_api();
        let bot = Bot::new("stub-bot-token", &api);
        let token = AdapterToken::new("adapter-secret");

        let (status, body) = through_adapter(&bot, &token, "GET /health HTTP/1.1\r\n\r\n");
        assert_eq!(status, 200);
        assert!(body.contains("\"ok\":true"), "{body}");

        // The control: the same build, the same absent header, the two routes with a backlog
        // behind them. Without it this test passes on an adapter that authenticates nothing.
        for raw in [
            "GET /poll?offset=1 HTTP/1.1\r\n\r\n",
            "POST /send HTTP/1.1\r\ncontent-length: 31\r\n\r\n{\"channel\":\"-1\",\"text\":\"hello\"}",
        ] {
            let (status, _) = through_adapter(&bot, &token, raw);
            assert_eq!(status, 401, "{raw}");
        }
        assert_eq!(upstream.load(Ordering::SeqCst), 0, "an unauthenticated route reached Telegram");
    }

    #[test]
    fn unauthenticated_or_oversized_requests_never_allocate_or_reach_telegram() {
        let (api, upstream) = stub_api();
        let bot = Bot::new("stub-bot-token", &api);
        let token = AdapterToken::new("adapter-secret");

        let (status, _) = through_adapter(
            &bot,
            &token,
            "POST /send HTTP/1.1\r\ncontent-length: 999999999\r\n\r\n",
        );
        assert_eq!(status, 401, "authentication must precede body allocation");

        let (status, _) = through_adapter(
            &bot,
            &token,
            "POST /send HTTP/1.1\r\nauthorization: Bearer adapter-secret\r\ncontent-length: 999999999\r\n\r\n",
        );
        assert_eq!(status, 413);
        assert_eq!(upstream.load(Ordering::SeqCst), 0);
    }

    /// Both arms of the distinction the refusal turns on. `false` is a claim, and here it is a
    /// true one; a reply this process could not read is not a claim at all.
    #[test]
    fn a_refusal_before_contact_is_provable_absence_and_a_lost_reply_is_not() {
        // Nothing was attempted, so `Channel::send` reading `Some(false)` and reporting
        // `Rejected` — the one outcome that licenses a free retry — is honest.
        assert_eq!(client_reads(REFUSED), Some(false));
        // The control, and the other arm: after contact this process cannot claim absence, so it
        // answers with no `accepted` field at all and `Ambiguous` stays reachable.
        assert_eq!(client_reads(&Acceptance::Unknown.body()), None);

        // The same body read as a poll: `Channel::poll` maps a missing `updates` array to
        // malformed and leaves the cursor where it is.
        let refusal: serde_json::Value =
            serde_json::from_str(REFUSED).expect("the refusal is readable json");
        assert!(refusal.get("updates").is_none(), "a refusal that reads as an empty poll");
        // The control for that assertion: a real poll body does carry the key, so `is_none` is
        // measuring the refusal rather than a typo.
        let real: serde_json::Value = serde_json::json!({ "updates": [] });
        assert!(real.get("updates").is_some());
    }

    #[test]
    fn only_the_exact_credential_is_accepted_and_it_does_not_leak_through_debug() {
        let token = AdapterToken::new("  adapter-secret  ");
        assert!(token.presented(Some("Bearer adapter-secret")));
        assert!(token.presented(Some("bearer adapter-secret")), "the scheme is case-insensitive");
        assert!(token.presented(Some("Bearer   adapter-secret  ")), "and the value is trimmed");
        for wrong in [
            "",
            "Bearer",
            "Bearer ",
            "Bearer adapter-secre",   // a prefix is not the credential
            "Bearer adapter-secrets", // an extension of it
            "Bearer Adapter-Secret",  // the credential itself is case-sensitive
            "Basic adapter-secret",
            "adapter-secret",
        ] {
            assert!(!token.presented(Some(wrong)), "{wrong:?}");
        }
        assert!(!token.presented(None));
        assert_eq!(format!("{token:?}"), "AdapterToken(redacted)");
    }

    #[test]
    fn telegram_confirming_a_message_is_the_only_thing_that_confirms_delivery() {
        let ok = r#"{"ok":true,"result":{"message_id":11,"text":"hi"}}"#;
        assert_eq!(Acceptance::read(200, ok), Acceptance::Confirmed);
        assert_eq!(client_reads(&Acceptance::Confirmed.body()), Some(true));
    }

    #[test]
    fn a_refusal_requires_telegram_to_have_declined_it() {
        // `accepted:false` is a positive claim that NOTHING was delivered, and it is the only
        // outcome that lets an operator resend without risking a second copy. It therefore takes
        // an explicit refusal at a status that agrees it is one.
        let refused = r#"{"ok":false,"error_code":400,"description":"chat not found"}"#;
        assert_eq!(Acceptance::read(400, refused), Acceptance::Refused);
        assert_eq!(client_reads(&Acceptance::Refused.body()), Some(false));
    }

    #[test]
    fn telegram_failing_is_not_telegram_declining() {
        // A 5xx carrying `ok:false` is the API falling over, not refusing the message, and it may
        // still have been sent. Reading it as a refusal would license a duplicating resend.
        let failed = r#"{"ok":false,"error_code":502,"description":"bad gateway"}"#;
        assert_eq!(Acceptance::read(502, failed), Acceptance::Unknown);
    }

    #[test]
    fn a_reply_this_code_cannot_read_is_never_a_refusal() {
        for body in ["<html>gateway timeout</html>", "", "{}", r#"{"ok":"yes"}"#] {
            assert_eq!(
                Acceptance::read(200, body),
                Acceptance::Unknown,
                "unreadable body {body:?} must not claim anything about delivery"
            );
        }
    }

    #[test]
    fn an_unknown_outcome_reports_no_accepted_field_at_all() {
        // The load-bearing one. The seam reads a missing or unreadable `accepted` as ambiguous
        // and a `false` as proof of absence; an adapter that answered `false` here would tell an
        // operator a resend is safe when a chunk may already be in the room.
        assert_eq!(client_reads(&Acceptance::Unknown.body()), None);
        assert_ne!(Acceptance::Unknown.status(), 200);
    }

    #[test]
    fn a_text_message_becomes_an_update_the_channel_contract_understands() {
        let update = serde_json::json!({
            "update_id": 4110,
            "message": { "chat": { "id": -100123 }, "text": "what needs attention?" }
        });
        let got = as_inbound(&update).expect("a message update converts");
        assert_eq!(got["id"], 4110);
        assert_eq!(got["channel"], "-100123");
        assert_eq!(got["text"], "what needs attention?");
    }

    #[test]
    fn a_private_message_carries_its_sender_and_says_the_room_is_private() {
        let update = serde_json::json!({
            "update_id": 4112,
            "message": {
                "chat": { "id": 184223311, "type": "private" },
                "from": { "id": 184223311, "is_bot": false, "first_name": "Alice",
                          "username": "alice" },
                "text": "/share ACME-001 Customer requested a September renewal review."
            }
        });
        let got = as_inbound(&update).expect("a private message converts");
        assert_eq!(got["actor"], "184223311");
        assert_eq!(got["chat_kind"], "private");
        // The display name and the username are the sender's to change, and neither selects
        // anything. Only `from.id` travels.
        let rendered = got.to_string();
        assert!(!rendered.contains("Alice"), "{rendered}");
        assert!(!rendered.contains("alice"), "{rendered}");
    }

    #[test]
    fn a_group_message_says_so_rather_than_staying_silent() {
        // Silence and "this is a group" must be different answers: the seam refuses a share from
        // either, but only one of them is the adapter doing its job.
        for kind in ["group", "supergroup", "channel"] {
            let update = serde_json::json!({
                "update_id": 4113,
                "message": { "chat": { "id": -100123, "type": kind },
                             "from": { "id": 184223311 }, "text": "/share A-1 note" }
            });
            let got = as_inbound(&update).expect("a group message converts");
            assert_eq!(got["chat_kind"], "non_private", "{kind}");
        }
    }

    #[test]
    fn a_room_or_a_sender_telegram_did_not_describe_is_reported_as_absent() {
        // The fail-closed half. An omitted field is the seam's "no claim"; emitting a default
        // here would hand it a claim nothing made.
        let no_type = serde_json::json!({
            "update_id": 4114,
            "message": { "chat": { "id": 5 }, "from": { "id": 7 }, "text": "hi" }
        });
        let got = as_inbound(&no_type).expect("converts");
        assert!(got.get("chat_kind").is_none(), "{got}");
        assert_eq!(got["actor"], "7");

        let unknown_type = serde_json::json!({
            "update_id": 4115,
            "message": { "chat": { "id": 5, "type": "sender" }, "text": "hi" }
        });
        let got = as_inbound(&unknown_type).expect("converts");
        assert!(got.get("chat_kind").is_none(), "a type this build has not met is not a claim");
        assert!(got.get("actor").is_none(), "no `from` is no actor");

        // A channel post has a chat and no sender at all.
        let post = serde_json::json!({
            "update_id": 4116,
            "channel_post": { "chat": { "id": -100, "type": "channel" }, "text": "broadcast" }
        });
        let got = as_inbound(&post).expect("converts");
        assert!(got.get("actor").is_none());
        assert_eq!(got["chat_kind"], "non_private");
    }

    #[test]
    fn an_update_with_nothing_readable_is_still_handed_over() {
        // Dropping it deadlocks the bot: the seam advances the offset only past updates it has
        // journalled, so one this process swallowed is redelivered forever and every conversation
        // behind it stops while both processes look healthy.
        let update = serde_json::json!({ "update_id": 4111, "poll_answer": { "poll_id": "x" } });
        let got = as_inbound(&update).expect("an unreadable update still travels");
        assert_eq!(got["id"], 4111);
        assert_eq!(got["text"], "");
        assert_eq!(got["channel"], "");
    }

    #[test]
    fn an_update_without_an_id_is_not_an_update() {
        // The id is what the seam keys a logical turn from; one without it cannot be journalled,
        // acknowledged or recovered, so there is nothing honest to hand over.
        assert!(as_inbound(&serde_json::json!({ "message": { "text": "hi" } })).is_none());
    }

    #[test]
    fn the_token_is_in_the_url_and_the_url_is_not_printed() {
        // A weak test of a real rule: the token reaches exactly one place. If `base` ever grows a
        // Debug print or an error that echoes it, this is the reminder that it must not.
        let bot = Bot::new("123:SECRET", "https://api.telegram.org");
        assert!(bot.base.ends_with("/bot123:SECRET"));
        assert!(!format!("{:?}", Acceptance::Unknown).contains("SECRET"));
    }
}
