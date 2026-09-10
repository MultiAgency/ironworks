//! A disposable channel, for proving delivery across real process restarts.
//!
//! **This is a semantics fixture, not a Telegram clone.** It models only the four things
//! IronWorks depends on, and deliberately nothing else:
//!
//! ```text
//! poll(offset)              inbound updates from an offset; the offset is the acknowledgement
//! send(chunk)               outbound, one chunk at a time, each separately acknowledged
//! redelivery                an update comes back until the channel is told an offset past it
//! deterministic faults      the three ways a send can go wrong, provoked rather than waited for
//! ```
//!
//! ```text
//! ironworks-devchannel --port 3995 --update 41=g1:"a question" [--update ...]
//!     [--reject-before-accept] [--accept-then-drop] [--fail-after-chunk N]
//!     [--redeliver-unacked]
//! ```
//!
//! # Why per-chunk
//!
//! A reply is sent in pieces, and *how many pieces the channel confirmed* is the only record of
//! whether a retry duplicates content a client can already read. A fixture that accepted a whole
//! message atomically could not produce partial delivery at all, so the one state that matters
//! most would be the one state never exercised.
//!
//! # The observation log
//!
//! Everything externally visible is appended to a log served at `GET /observations`. A proof
//! asserts against **that**, never against this process's internals — so it is testing behaviour
//! a real channel could also exhibit, rather than a shape only this fixture has. Chunk contents
//! appear as a digest, which is what lets a proof assert a duplicate is *byte-identical* without
//! this fixture ever handing back a client's answer.
//!
//! ```text
//! poll offset=41
//! redelivered update=41
//! accepted chunk=0 channel=-1002000001 bytes=214 sha256=6f1a...
//! dropped-after-accept chunk=1
//! poll offset=42
//! acknowledged through=42
//! ```

use ironworks_devhttp::{respond, respond_text};
use sha2::{Digest, Sha256};
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;

/// One inbound update. `text` is a synthetic question; no real message ever reaches a fixture.
struct Update {
    id: u64,
    channel: String,
    text: String,
    /// Who the channel attests sent it, when it attests anyone. Absent by default, because that
    /// is what a channel that does not report a sender looks like — and the seam must fail closed
    /// against exactly that.
    actor: Option<String>,
    /// `private` or `non_private`. Absent by default, for the same reason.
    chat_kind: Option<String>,
}

/// How a send is made to fail. Exactly one applies to any run — they are different failures, and
/// combining them would produce a state no real channel reaches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    /// The channel refuses before accepting any part. Provable absence: a retry cannot duplicate.
    RejectBeforeAccept,
    /// The channel accepts the chunk and the caller never learns. The residual window, at chunk
    /// granularity — the observation log records the acceptance the caller could not see.
    AcceptThenDrop,
    /// The first `n` chunks are accepted and confirmed; the next is accepted and dropped. This is
    /// how partial delivery is produced: `n` chunks are provably in the client's room.
    FailAfterChunk(u32),
}

struct Config {
    port: u16,
    updates: Vec<Update>,
    fault: Fault,
    /// Serve from the *acknowledged* offset, ignoring a higher one in the request.
    ///
    /// Models a channel that only trusts what it has durably been told. Without it a client that
    /// computed a cursor and died before acting on it would never be sent the work again, and B7
    /// would be unprovable — the redelivery it turns on is the whole boundary.
    redeliver_unacked: bool,
}

#[derive(Default)]
struct State {
    /// The offset the channel has been told. Everything below it is finished, for good.
    acknowledged: Option<u64>,
    /// Which updates have already been served at least once, so a repeat can be reported as a
    /// redelivery rather than a first sight.
    served: Vec<u64>,
    /// Chunks accepted for the message currently being sent. Reset when a send completes.
    chunk_index: u32,
    observations: Vec<String>,
}

fn usage() -> ! {
    eprintln!(
        "usage: ironworks-devchannel --port N --update ID=CHANNEL:TEXT [--update ...]\n\
         \x20      [--actor ID=ACTOR] [--chat-kind ID=private|non_private]\n\
         \x20      [--reject-before-accept] [--accept-then-drop] [--fail-after-chunk N]\n\
         \x20      [--redeliver-unacked]"
    );
    std::process::exit(64)
}

fn main() {
    let mut cfg =
        Config { port: 0, updates: Vec::new(), fault: Fault::None, redeliver_unacked: false };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    let mut faults = 0;
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--port" => cfg.port = val().parse().unwrap_or_else(|_| usage()),
            "--update" => {
                // ID=CHANNEL:TEXT
                let raw = val();
                let (id, rest) = raw.split_once('=').unwrap_or_else(|| usage());
                let (channel, text) = rest.split_once(':').unwrap_or_else(|| usage());
                cfg.updates.push(Update {
                    id: id.parse().unwrap_or_else(|_| usage()),
                    channel: channel.to_string(),
                    text: text.to_string(),
                    actor: None,
                    chat_kind: None,
                });
            }
            // Attached to an already-declared update by id rather than woven into `--update`'s
            // syntax: each flag names ONE fact, which is the same reason the seam models the
            // sender and the room as two values instead of one convenient flag meaning both.
            "--actor" => {
                let raw = val();
                let (id, actor) = raw.split_once('=').unwrap_or_else(|| usage());
                let id: u64 = id.parse().unwrap_or_else(|_| usage());
                let u = cfg.updates.iter_mut().find(|u| u.id == id).unwrap_or_else(|| usage());
                u.actor = Some(actor.to_string());
            }
            "--chat-kind" => {
                let raw = val();
                let (id, kind) = raw.split_once('=').unwrap_or_else(|| usage());
                if !matches!(kind, "private" | "non_private") {
                    usage();
                }
                let id: u64 = id.parse().unwrap_or_else(|_| usage());
                let u = cfg.updates.iter_mut().find(|u| u.id == id).unwrap_or_else(|| usage());
                u.chat_kind = Some(kind.to_string());
            }
            "--reject-before-accept" => {
                cfg.fault = Fault::RejectBeforeAccept;
                faults += 1;
            }
            "--accept-then-drop" => {
                cfg.fault = Fault::AcceptThenDrop;
                faults += 1;
            }
            "--fail-after-chunk" => {
                cfg.fault = Fault::FailAfterChunk(val().parse().unwrap_or_else(|_| usage()));
                faults += 1;
            }
            "--redeliver-unacked" => cfg.redeliver_unacked = true,
            _ => usage(),
        }
    }
    if cfg.port == 0 || cfg.updates.is_empty() {
        usage();
    }
    if faults > 1 {
        // Two faults at once would produce a state no real channel reaches, and a proof that
        // passed against it would be evidence about this fixture rather than about delivery.
        eprintln!("devchannel: only one fault may be active at a time");
        std::process::exit(64);
    }
    cfg.updates.sort_by_key(|u| u.id);

    eprintln!(
        "devchannel: {} update(s) on 127.0.0.1:{}, fault={}, redeliver_unacked={}",
        cfg.updates.len(),
        cfg.port,
        match cfg.fault {
            Fault::None => "none".to_string(),
            Fault::RejectBeforeAccept => "reject-before-accept".to_string(),
            Fault::AcceptThenDrop => "accept-then-drop".to_string(),
            Fault::FailAfterChunk(n) => format!("fail-after-chunk {n}"),
        },
        cfg.redeliver_unacked
    );

    let listener = TcpListener::bind(("127.0.0.1", cfg.port)).expect("bind");
    let state = Mutex::new(State::default());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        handle(stream, &cfg, &state);
    }
}

fn handle(mut stream: TcpStream, cfg: &Config, state: &Mutex<State>) {
    ironworks_devhttp::set_timeouts(&stream);
    let Ok(clone) = stream.try_clone() else { return };
    let mut reader = BufReader::new(clone);
    let request = match ironworks_devhttp::read(&mut reader) {
        Ok(request) => request,
        Err(why) => return ironworks_devhttp::respond_refusal(&mut stream, why),
    };

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => respond(&mut stream, 200, "{\"ok\":true}"),
        ("GET", "/observations") => {
            let s = state.lock().expect("state");
            let body = s.observations.join("\n");
            respond_text(&mut stream, 200, &body);
        }
        ("GET", "/poll") => poll(&mut stream, cfg, state, &request.query),
        ("POST", "/send") => send(&mut stream, cfg, state, &request.body),
        _ => respond(&mut stream, 404, "{\"error\":\"no such path\"}"),
    }
}

/// Inbound. **The offset in the request is the acknowledgement**: asking for updates from `N` is
/// how a client says it is finished with everything below `N`, and it is the only way this fixture
/// ever learns that. That is `DELIVERED` versus `ACKED` in one line — the acknowledgement always
/// arrives on a later poll than the delivery it acknowledges.
fn poll(stream: &mut TcpStream, cfg: &Config, state: &Mutex<State>, query: &str) {
    let requested: u64 = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("offset="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let mut s = state.lock().expect("state");
    s.observations.push(format!("poll offset={requested}"));
    // What the channel knew BEFORE this poll. Captured first, because the poll is simultaneously
    // an acknowledgement and a request, and the redelivery window is decided by the former state.
    let previously = s.acknowledged.unwrap_or(0);
    // Monotone, like every acknowledgement: a channel cannot be un-told something.
    if s.acknowledged.is_none_or(|a| requested > a) {
        s.acknowledged = Some(requested);
        s.observations.push(format!("acknowledged through={requested}"));
    }

    // At-least-once. An update that had not been acknowledged when this poll arrived comes back,
    // even though the poll itself acknowledges it — which is exactly the window a process that
    // delivered an answer and died before its next poll lands in. Without this, B7 could not be
    // provoked at all: the redelivery IS the boundary.
    let from = if cfg.redeliver_unacked { previously } else { requested };

    let mut out = Vec::new();
    for u in cfg.updates.iter().filter(|u| u.id >= from) {
        if s.served.contains(&u.id) {
            s.observations.push(format!("redelivered update={}", u.id));
        } else {
            s.served.push(u.id);
            s.observations.push(format!("delivered update={}", u.id));
        }
        let mut update = serde_json::json!({
            "id": u.id, "channel": u.channel, "text": u.text,
        });
        let map = update.as_object_mut().expect("a json object literal is an object");
        // Emitted only when declared. An absent field is the channel making no claim, which is
        // the case the seam has to refuse — so the fixture has to be able to produce it.
        if let Some(actor) = &u.actor {
            map.insert("actor".into(), serde_json::Value::String(actor.clone()));
        }
        if let Some(kind) = &u.chat_kind {
            map.insert("chat_kind".into(), serde_json::Value::String(kind.clone()));
        }
        out.push(update);
    }
    drop(s);
    respond(stream, 200, &serde_json::json!({ "updates": out }).to_string());
}

/// Outbound, one chunk at a time.
///
/// `{"channel": "...", "text": "..."}`. The reply says whether this chunk was accepted; a dropped
/// connection says nothing, which is the point — the caller must record *uncertainty*, not a
/// failure, because the observation log will show the chunk was accepted anyway.
fn send(stream: &mut TcpStream, cfg: &Config, state: &Mutex<State>, body: &[u8]) {
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return respond(stream, 400, "{\"error\":\"unparseable\"}"),
    };
    let text = parsed.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let done = parsed.get("last").and_then(serde_json::Value::as_bool).unwrap_or(false);
    // The room the caller addressed. A real channel knows it for every message it takes, and
    // without it the log says an answer was delivered while staying silent about to whom — which
    // is precisely the question a registry cutover asks.
    let room = parsed.get("channel").and_then(|v| v.as_str()).unwrap_or("");

    let mut s = state.lock().expect("state");
    let index = s.chunk_index;

    let accept = |s: &mut State, index: u32| {
        let digest = Sha256::digest(text.as_bytes());
        s.observations.push(format!(
            "accepted chunk={index} channel={room} bytes={} sha256={}",
            text.len(),
            hex12(&digest)
        ));
    };

    match cfg.fault {
        Fault::RejectBeforeAccept => {
            // Nothing is accepted, and the caller is told so explicitly. This is the only outcome
            // that licenses a free retry, so it is the only one that may be reported positively.
            s.observations
                .push(format!("rejected chunk={index} channel={room} (nothing accepted)"));
            drop(s);
            respond(stream, 409, "{\"accepted\":false,\"reason\":\"rejected before accept\"}");
        }
        Fault::AcceptThenDrop => {
            accept(&mut s, index);
            s.chunk_index += 1;
            s.observations.push(format!("dropped-after-accept chunk={index}"));
            drop(s);
            // No response at all. The socket closes and the caller learns nothing — which is
            // exactly what it must record: uncertainty, not failure.
        }
        Fault::FailAfterChunk(n) if index >= n => {
            accept(&mut s, index);
            s.chunk_index += 1;
            s.observations.push(format!("dropped-after-accept chunk={index}"));
            drop(s);
        }
        _ => {
            accept(&mut s, index);
            s.chunk_index += 1;
            if done {
                s.chunk_index = 0;
                s.observations.push("message complete".to_string());
            }
            drop(s);
            respond(stream, 200, "{\"accepted\":true}");
        }
    }
}

fn hex12(bytes: &[u8]) -> String {
    bytes.iter().take(6).map(|b| format!("{b:02x}")).collect()
}
