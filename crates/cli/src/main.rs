//! `ironworks` — the operator binary. One journalled turn, recoverable across process death.
//!
//!     ironworks turn --slug eval --guidance fixtures/eval.guidance.md \
//!                    --journal /tmp/eval.journal --ask "Which accounts need attention?"
//!
//! Environment: `IRONWORKS_BASE` / `IRONWORKS_TOKEN` (the runtime), `IRONWORKS_RECORDS` /
//! `IRONWORKS_RECORDS_TOKEN` (the record store).
//!
//! **There is no offline mode.** The record store is the only authority on which organization a
//! credential authorises, so without one there is nothing to verify against and nothing serves.
//!
//! # The path
//!
//! ```text
//! ServiceDefinition + Guidance + ContextBinding + ModelSelection + CapabilityProfile
//!   -> ServiceInstance<Unverified>
//!   -> verify(record store)            a control: no instance at all if it fails
//!   -> ServiceInstance<Verified>
//!   -> build_turn(...)                 melts its OWN composition
//!   -> journal.begin(key, digest)      durable BEFORE a byte leaves
//!   -> ModelTurn -> IronClaw
//!   -> journal.commit_completed(...)   response id and pointer, atomically
//! ```
//!
//! # Recovery
//!
//! On start, an unresolved turn for this instance is resolved **from durable journal evidence
//! alone** before any new turn is considered. The rule the recovery obeys:
//!
//! > Never submit a new logical turn. An idempotent replay of the same logical turn is permitted
//! > when byte identity is proven.
//!
//! Which is why the journal stores `retrieved_at`: a rebuild reuses it instead of reading the
//! clock, so the reconstructed body is byte-identical and the replay resolves to the original
//! response rather than executing a second time.

use ironworks_channel::{ChannelJournal, Disposition};
use ironworks_domain::{
    Ask, CapabilityProfile, ContextBinding, CredentialPurpose, CredentialRef, Degradation,
    Guidance, IdempotencyKey, ModelSelection, ModelTurn, Openness, RecordScope, RecordsState,
    ResponseId, RuntimeIdentity, ServiceInstance, Slug, ThreadScope,
};
use ironworks_ironclaw::{
    Accepted, Cause, Deadline, Ironclaw, Outcome, Runtime, TurnFailure, request_bytes,
};
use ironworks_journal::{Journal, Recovery, RequestDigest, TurnOutcome, recover};
use ironworks_proposals::{Confirmation, NewProposal, ProposalStore};
use ironworks_records::{
    Append, AppendClient, AppendFailure, Appended, Book, BookFailure, EnvCredentials,
};
use std::io::{self, Write};
use std::time::Duration;

const USAGE: i32 = 64;
const FAILED: i32 = 2;
/// A measurement that could not be performed. **Never a pass, and never a FAIL either** — scoring
/// an unavailable dependency as a defect in IronWorks trains an operator to skim the ones that are.
/// The convention `dev/prove*.sh` already reads: 0 / 2 / 3.
const BLOCKED: i32 = 3;
/// The exit code a deliberate crash uses, so a proof can tell it from a real failure.
const CRASHED: i32 = 75;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(Exit { code, message }) => {
            eprintln!("ironworks: {message}");
            std::process::ExitCode::from(code as u8)
        }
    }
}

struct Exit {
    code: i32,
    message: String,
}
fn fail(code: i32, message: impl Into<String>) -> Exit {
    Exit { code, message: message.into() }
}

// ── the runtime boundary, counted ────────────────────────────────────────────────────────

/// Counts every call at the adapter boundary and appends it to a log that outlives the process.
///
/// A proof must not infer "no duplicate execution" from equal output — two executions of the same
/// prompt can produce the same words. Counting here, across restarts, is evidence about *calls*;
/// what it can and cannot establish about *model executions* is stated in `dev/prove.sh`.
struct Counted<'a> {
    inner: &'a dyn Runtime,
    log: Option<std::path::PathBuf>,
}

impl Counted<'_> {
    fn note(&self, op: &str, detail: &str) {
        let Some(path) = &self.log else { return };
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{op}\t{detail}");
        }
    }
}

impl Runtime for Counted<'_> {
    fn create(
        &self,
        turn: &ModelTurn,
        key: &IdempotencyKey,
        deadline: &Deadline,
    ) -> Result<Accepted, TurnFailure> {
        self.note("create", key.as_str());
        self.inner.create(turn, key, deadline)
    }
    fn fetch(&self, id: &ResponseId, deadline: &Deadline) -> Result<Outcome, TurnFailure> {
        self.note("fetch", id.as_str());
        self.inner.fetch(id, deadline)
    }
}

// ── argv ─────────────────────────────────────────────────────────────────────────────────

/// What this invocation is for.
///
/// The two differ in exactly one way that matters: `relay` can deliver, and `turn` cannot. Every
/// refusal about delivery state in `turn` follows from that — it has no channel, so a delivery
/// state it finds belongs to a conversation it knows nothing about.
enum Mode {
    Turn {
        ask: String,
    },
    /// Every tenant this process serves comes from the registry, including when there is one of
    /// them. A single-tenant flag beside it would be a second, looser path into the same serving
    /// code — and the channel is the one place this codebase refuses to have two.
    Relay {
        channel: String,
        registry: String,
        chunk_bytes: usize,
    },
    /// An operator's decision, carried out. Never runs a model.
    Redeliver {
        channel: String,
        update: u64,
        chunk_bytes: usize,
    },
    /// Read-only lifecycle inspection. Creates nothing, changes nothing, calls no model.
    LifecycleInspect {
        store: String,
        observe: Option<String>,
        account: bool,
        member: bool,
        confinement: bool,
    },
    /// Apply confinement to a member, then re-measure it. Changes authority; never runs a model.
    ///
    /// There is no policy flag. The policy is whatever the recorded instance's service declares
    /// in `capabilities.runtime_tools`, so an operator cannot confine a member to something its
    /// own service definition contradicts.
    LifecycleConfine {
        store: String,
        dry_run: bool,
    },
    /// Observe an existing record scope and runtime member, then record both in one local
    /// transaction. It creates no foreign authority and accepts no asserted org or member id.
    LifecycleAdopt {
        store: String,
        service: String,
        channel: String,
        records_credential: String,
        member_credential: String,
    },
    /// Fleet-wide member reconciliation. A different question from `inspect`, so a different
    /// command: it discovers authority no instance accounts for, rather than proving custody of
    /// what one instance recorded.
    LifecycleMembers {
        store: String,
    },
    /// Read-only turn-journal inspection. Opens no runtime, record store, guidance or channel.
    Status,
    /// A dry melt: what this binary would compose, fingerprinted. Never runs a model.
    Compose,
}

struct Args {
    slug: String,
    guidance: String,
    service: String,
    journal: String,
    mode: Mode,
    crash_at: Option<String>,
    call_log: Option<String>,
}

/// How many completed turns a thread may hold before the next one starts over.
///
/// A cost bound. `0001-alloy-drift.md` recorded input growing ~3,100 tokens per turn because
/// IronClaw re-persists `instructions`, a 30-turn thread costing $4.91 with its last turn about
/// five times its first, and eight was chosen to keep a conversation's worth of context while the
/// compounding was still small.
///
/// **Eight is provisional, and on current evidence too generous.**
/// `0011-continuation-costs-more-than-it-saves.md` measured what 0001 did not: a chained turn
/// caches nothing (`cached: 0`, across two runs) while a fresh turn's near-constant alloy and
/// envelope cache about half, so chaining costs ~2.4x the uncached input of not chaining and is
/// already losing by the second turn. 0001's curve stands; it was an incomplete basis for *this*
/// number.
///
/// It is not lowered here, because 0011 measures **cost and not answer quality** — a chained turn
/// can answer a follow-up a fresh one cannot, and nothing has measured that. Its own conclusion is
/// that the question is "worth it *where*", which makes this a per-tenant setting rather than a
/// constant: [`ironworks_domain::TenantRegistry`] is where an operator already declares what differs
/// between tenants, and where this belongs once someone decides what each is worth.
fn thread_limit() -> ironworks_domain::ThreadLimit {
    ironworks_domain::ThreadLimit::new(8).expect("eight is not zero")
}

/// Telegram's message limit, which is the one this is modelled on. A chunk boundary is where a
/// partial delivery becomes visible to a client, so the number is a product decision, not a knob.
const DEFAULT_CHUNK_BYTES: usize = 3800;

const USAGE_TEXT: &str = "usage: ironworks coordinate --slug <slug> --guidance <path> --journal <path> --ask <question>\n\
     \x20        selects account coordination; --service is not accepted\n\n\
     \x20      ironworks turn  --slug <slug> --guidance <path> --journal <path> --ask <question>\n\
     \x20      ironworks relay --registry <path> --journal <path> --channel <url>\n\
     \x20        every tenant it serves comes from the registry; one tenant is a registry of one\n\n\
     \x20      ironworks redeliver --slug <slug> --guidance <path> --journal <path> \
     --channel <url> --update <id>\n\n\
     \x20      ironworks status --journal <path>\n\
     \x20        read-only; lists every turn that is not both completed and delivered\n\n\
     \x20      ironworks compose --slug <slug> --guidance <path> --service <name>\n\
     \x20        a dry melt; prints the alloy digest and the digest of every ingredient\n\n\
     turn/redeliver/compose: --service <name>   there is no default service\n\
     turn/relay/redeliver: [--crash-at <point>] [--call-log <path>]\n\
     relay/redeliver: [--chunk-bytes N]\n\n\
     environment: IRONWORKS_BASE, IRONWORKS_TOKEN, IRONWORKS_RECORDS, IRONWORKS_RECORDS_TOKEN\n\n\
     --crash-at is a proof affordance:\n\
     \x20 turn:  before-dispatch | before-response | before-commit | after-output\n\
     \x20 relay: before-dispatch | before-response | before-commit | after-send | after-delivered\n\n\
     \x20      ironworks lifecycle inspect <slug> --store <path> [--observe <path>]\n\
     \x20        read-only; reports intent, recorded progress, observed authority and serving\n\
     \x20        --account-service asks the live Account Service (needs IRONWORKS_RECORDS)\n\
     \x20        --ironclaw asks the IronClaw admin surface (needs IRONCLAW_API)\n\n\
     \x20      ironworks lifecycle members --store <path>\n\
     \x20        fleet-wide: members on the instance that no lifecycle record accounts for";

fn parse_args() -> Result<Args, Exit> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = argv.first().map(String::as_str).unwrap_or("");
    // `lifecycle` is parsed before the others because it shares none of their requirements: it
    // needs no runtime, no record store, and no guidance — it reads two local files and prints.
    if command == "lifecycle" {
        return parse_lifecycle(&argv[1..]);
    }
    if command == "status" {
        return parse_status(&argv[1..]);
    }
    if command == "compose" {
        return parse_compose(&argv[1..]);
    }
    if command == "relay" {
        return parse_relay(&argv[1..]);
    }
    if !matches!(command, "coordinate" | "turn" | "redeliver") {
        return Err(fail(USAGE, USAGE_TEXT));
    }
    let (mut slug, mut guidance, mut ask, mut journal) = (None, None, None, None);
    let (mut crash_at, mut call_log) = (None, None);
    let mut channel = None;
    let mut chunk_bytes = DEFAULT_CHUNK_BYTES;
    let mut update = None;
    // `coordinate` is the one command that names its own service, which is what makes `--service`
    // an error there rather than a redundancy. Every other command must be told: there is no
    // default to fall back to, so an omitted `--service` is a refusal and not a silent choice.
    let mut service =
        (command == "coordinate").then(|| ironworks_services::service::ACCOUNTS.to_string());
    let mut it = argv[1..].iter();
    while let Some(flag) = it.next() {
        let mut value =
            || it.next().cloned().ok_or_else(|| fail(USAGE, format!("{flag} needs a value")));
        match flag.as_str() {
            "--slug" => slug = Some(value()?),
            "--guidance" => guidance = Some(value()?),
            "--ask" => ask = Some(value()?),
            "--journal" => journal = Some(value()?),
            "--service" if command == "coordinate" => {
                return Err(fail(
                    USAGE,
                    "coordinate already selects the account-coordination service; --service is not allowed",
                ));
            }
            "--service" => service = Some(value()?),
            "--crash-at" => crash_at = Some(value()?),
            "--call-log" => call_log = Some(value()?),
            "--channel" => channel = Some(value()?),
            "--update" => {
                update =
                    Some(value()?.parse().map_err(|_| fail(USAGE, "--update needs an update id"))?);
            }
            "--chunk-bytes" => {
                chunk_bytes = value()?
                    .parse()
                    .map_err(|_| fail(USAGE, "--chunk-bytes needs a positive number"))?;
                if chunk_bytes == 0 {
                    return Err(fail(USAGE, "--chunk-bytes must be greater than zero"));
                }
            }
            other => return Err(fail(USAGE, format!("unknown flag {other}"))),
        }
    }
    let mode = match command {
        "coordinate" | "turn" => {
            Mode::Turn { ask: ask.ok_or_else(|| fail(USAGE, "--ask is required"))? }
        }
        _ => Mode::Redeliver {
            channel: channel.ok_or_else(|| fail(USAGE, "--channel is required for redeliver"))?,
            update: update.ok_or_else(|| fail(USAGE, "--update is required for redeliver"))?,
            chunk_bytes,
        },
    };
    Ok(Args {
        slug: slug.ok_or_else(|| fail(USAGE, "--slug is required"))?,
        guidance: guidance.ok_or_else(|| fail(USAGE, "--guidance is required"))?,
        journal: journal.ok_or_else(|| fail(USAGE, "--journal is required"))?,
        service: service.ok_or_else(|| {
            fail(USAGE, "--service is required; there is no default service to fall back to")
        })?,
        mode,
        crash_at,
        call_log,
    })
}

/// `ironworks status --journal <path>`
///
/// Deliberately bypasses common serving flags. A status report needs no tenant guidance,
/// credentials, runtime or record store and must not accidentally acquire any of them.
fn parse_status(argv: &[String]) -> Result<Args, Exit> {
    let mut journal = None;
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--journal" => {
                journal =
                    Some(it.next().cloned().ok_or_else(|| fail(USAGE, "--journal needs a value"))?);
            }
            other => return Err(fail(USAGE, format!("unknown flag {other}"))),
        }
    }
    Ok(Args {
        slug: String::new(),
        guidance: String::new(),
        service: String::new(),
        journal: journal.ok_or_else(|| fail(USAGE, "--journal is required"))?,
        mode: Mode::Status,
        crash_at: None,
        call_log: None,
    })
}

/// `ironworks relay --registry <path> --channel <url> [--chunk-bytes N]`
///
/// Deliberately its own parser, and deliberately without `--slug`, `--guidance` or `--service`.
/// Those are per-tenant now and come from the registry, so accepting them here would let an
/// operator believe they had overridden something for a conversation that belongs to somebody
/// else. One tenant is a registry with one entry.
fn parse_relay(argv: &[String]) -> Result<Args, Exit> {
    let (mut channel, mut registry, mut journal) = (None, None, None);
    let (mut crash_at, mut call_log) = (None, None);
    let mut chunk_bytes = DEFAULT_CHUNK_BYTES;
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        let mut value =
            || it.next().cloned().ok_or_else(|| fail(USAGE, format!("{flag} needs a value")));
        match flag.as_str() {
            "--channel" => channel = Some(value()?),
            "--registry" => registry = Some(value()?),
            // One journal for the process, not one per tenant: the safe cursor is the lowest
            // open update ACROSS every tenant, and two files could not compute it.
            "--journal" => journal = Some(value()?),
            "--crash-at" => crash_at = Some(value()?),
            "--call-log" => call_log = Some(value()?),
            "--chunk-bytes" => {
                chunk_bytes = value()?
                    .parse()
                    .map_err(|_| fail(USAGE, "--chunk-bytes needs a positive number"))?;
                if chunk_bytes == 0 {
                    return Err(fail(USAGE, "--chunk-bytes must be greater than zero"));
                }
            }
            other => return Err(fail(USAGE, format!("unknown flag {other}"))),
        }
    }
    Ok(Args {
        slug: String::new(),
        guidance: String::new(),
        service: String::new(),
        journal: journal.ok_or_else(|| fail(USAGE, "--journal is required for relay"))?,
        mode: Mode::Relay {
            channel: channel.ok_or_else(|| fail(USAGE, "--channel is required for relay"))?,
            registry: registry.ok_or_else(|| fail(USAGE, "--registry is required for relay"))?,
            chunk_bytes,
        },
        crash_at,
        call_log,
    })
}

/// `ironworks compose --slug <slug> --guidance <path> --service <name>`
///
/// Deliberately bypasses the serving flags for the same reason [`parse_status`] does, and one
/// more: a dry melt reads two local inputs and prints hashes of them. It opens no journal,
/// contacts no runtime and no record store, and needs no model key — so it must not be able to
/// acquire any of them by sharing a parser with the commands that do.
fn parse_compose(argv: &[String]) -> Result<Args, Exit> {
    let (mut slug, mut guidance, mut service) = (None, None, None);
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        let mut value =
            || it.next().cloned().ok_or_else(|| fail(USAGE, format!("{flag} needs a value")));
        match flag.as_str() {
            "--slug" => slug = Some(value()?),
            "--guidance" => guidance = Some(value()?),
            "--service" => service = Some(value()?),
            other => return Err(fail(USAGE, format!("unknown flag {other}"))),
        }
    }
    Ok(Args {
        slug: slug.ok_or_else(|| fail(USAGE, "--slug is required"))?,
        guidance: guidance.ok_or_else(|| fail(USAGE, "--guidance is required"))?,
        service: service.ok_or_else(|| {
            fail(USAGE, "--service is required; there is no default service to fall back to")
        })?,
        journal: String::new(),
        mode: Mode::Compose,
        crash_at: None,
        call_log: None,
    })
}

/// `ironworks lifecycle inspect <slug> --store <path> [--observe <path>]`
///
/// Deliberately separate from [`parse_args`]'s common flags. Inspection has no service instance,
/// no credentials and no runtime — sharing `--guidance` and `--slug` handling would imply it
/// needed them.
fn parse_lifecycle(argv: &[String]) -> Result<Args, Exit> {
    let usage = || {
        fail(
            USAGE,
            "usage: ironworks lifecycle inspect <slug> --store <path> [--observe <path>]\n\
             \x20                                          [--confinement]\n\n\
             Read-only. Reports what was intended, what IronWorks recorded reaching, what the\n\
             authoritative systems report, and where those disagree.\n\n\
             --observe supplies a reconciliation fixture. WITHOUT IT NOTHING IS ASKED OF ANY\n\
             system, and the verdict is BLOCKED — an inspection that certified an instance\n\
             nothing checked would be worse than no inspection.\n\n\
             --confinement measures the recorded member's catalogue NOW and compares it with the\n\
             \x20      ledger. It records nothing. A live clean reading is not enough for PASS:\n\
             \x20      the latest observation for this member must also be a clean one taken\n\
             \x20      against the policy in force, which only `lifecycle confine` can append.\n\
             \x20      Needs IRONCLAW_API and IRONCLAW_MEMBER_TOKEN.\n\n\
             \x20      ironworks lifecycle confine <slug> --store <path> [--dry-run]\n\
             \x20        CHANGES AUTHORITY. Applies the confinement policy to the recorded\n\
             \x20        member, approval-off first, then the authority-widening tools, then the\n\
             \x20        rest — and re-measures afterwards rather than trusting its own writes.\n\
             \x20        The policy is the one the recorded instance's SERVICE declares in\n\
             \x20        capabilities.runtime_tools; there is no flag that widens it.\n\
             \x20        It BINDS FIRST: the member credential is resolved against\n\
             \x20        /api/webchat/v2/session and compared with the recorded member. A\n\
             \x20        mismatch exits 2 having made no settings read and no settings write.\n\
             \x20        Every attempt is appended to the confinement ledger; --dry-run opens\n\
             \x20        the store read-only and writes nothing at all.\n\
             \x20        Needs IRONCLAW_API, IRONCLAW_OPERATOR_TOKEN and IRONCLAW_MEMBER_TOKEN.\n\n\
             \x20      ironworks lifecycle adopt <slug> --store <path> --service <service>\n\
             \x20        --channel <channel> --records-credential <handle>\n\
             \x20        --member-credential <handle>\n\
             \x20        OBSERVES ONLY. Resolves the records handle to an org and the member\n\
             \x20        handle through /api/webchat/v2/session, then records both atomically.\n\
             \x20        There is no --org or --member-id; neither identity may be asserted.\n\
             \x20        Refuses an existing slug and writes no partial instance when either\n\
             \x20        authority cannot be observed. Needs IRONWORKS_RECORDS and IRONCLAW_API.",
        )
    };
    match argv.first().map(String::as_str) {
        Some("inspect") => {}
        Some("adopt") => {
            let slug = argv.get(1).cloned().ok_or_else(usage)?;
            if slug.starts_with("--") {
                return Err(usage());
            }
            let (mut store, mut service, mut channel) = (None, None, None);
            let (mut records_credential, mut member_credential) = (None, None);
            let mut it = argv[2..].iter();
            while let Some(flag) = it.next() {
                let mut value = || {
                    it.next().cloned().ok_or_else(|| fail(USAGE, format!("{flag} needs a value")))
                };
                match flag.as_str() {
                    "--store" => store = Some(value()?),
                    "--service" => service = Some(value()?),
                    "--channel" => channel = Some(value()?),
                    "--records-credential" => records_credential = Some(value()?),
                    "--member-credential" => member_credential = Some(value()?),
                    "--org" | "--member" | "--member-id" => {
                        return Err(fail(
                            USAGE,
                            format!(
                                "{flag} is not accepted: adopt measures org and member identity \
                                 from their credentials"
                            ),
                        ));
                    }
                    other => return Err(fail(USAGE, format!("unknown flag {other}"))),
                }
            }
            return Ok(Args {
                slug,
                guidance: String::new(),
                service: String::new(),
                journal: String::new(),
                mode: Mode::LifecycleAdopt {
                    store: store.ok_or_else(|| fail(USAGE, "--store is required"))?,
                    service: service.ok_or_else(|| fail(USAGE, "--service is required"))?,
                    channel: channel.ok_or_else(|| fail(USAGE, "--channel is required"))?,
                    records_credential: records_credential
                        .ok_or_else(|| fail(USAGE, "--records-credential is required"))?,
                    member_credential: member_credential
                        .ok_or_else(|| fail(USAGE, "--member-credential is required"))?,
                },
                crash_at: None,
                call_log: None,
            });
        }
        Some("confine") => {
            let slug = argv.get(1).cloned().ok_or_else(usage)?;
            if slug.starts_with("--") {
                return Err(usage());
            }
            let (mut store, mut dry_run) = (None, false);
            let mut it = argv[2..].iter();
            while let Some(flag) = it.next() {
                match flag.as_str() {
                    "--store" => store = it.next().cloned(),
                    "--dry-run" => dry_run = true,
                    "--workspace" => {
                        return Err(fail(
                            USAGE,
                            "--workspace is gone: the confinement policy now comes from the \
                             recorded instance's service definition (capabilities.runtime_tools). \
                             Declare `\"runtime_tools\": \"sandbox\"` on the service instead.",
                        ));
                    }
                    other => return Err(fail(USAGE, format!("unknown flag {other}"))),
                }
            }
            return Ok(Args {
                slug,
                guidance: String::new(),
                service: String::new(),
                journal: String::new(),
                mode: Mode::LifecycleConfine {
                    store: store.ok_or_else(|| fail(USAGE, "--store is required"))?,
                    dry_run,
                },
                crash_at: None,
                call_log: None,
            });
        }
        Some("members") => {
            let mut store = None;
            let mut it = argv[1..].iter();
            while let Some(flag) = it.next() {
                match flag.as_str() {
                    "--store" => store = it.next().cloned(),
                    other => return Err(fail(USAGE, format!("unknown flag {other}"))),
                }
            }
            return Ok(Args {
                slug: String::new(),
                guidance: String::new(),
                service: String::new(),
                journal: String::new(),
                mode: Mode::LifecycleMembers {
                    store: store.ok_or_else(|| fail(USAGE, "--store is required"))?,
                },
                crash_at: None,
                call_log: None,
            });
        }
        _ => return Err(usage()),
    }
    let slug = argv.get(1).cloned().ok_or_else(usage)?;
    if slug.starts_with("--") {
        return Err(usage());
    }
    let (mut store, mut observe) = (None, None);
    let (mut account, mut member, mut confinement) = (false, false, false);
    let mut it = argv[2..].iter();
    while let Some(flag) = it.next() {
        let mut value =
            || it.next().cloned().ok_or_else(|| fail(USAGE, format!("{flag} needs a value")));
        match flag.as_str() {
            "--store" => store = Some(value()?),
            "--observe" => observe = Some(value()?),
            "--account-service" => account = true,
            "--ironclaw" => member = true,
            "--confinement" => confinement = true,
            other => return Err(fail(USAGE, format!("unknown flag {other}"))),
        }
    }
    Ok(Args {
        slug,
        guidance: String::new(),
        service: String::new(),
        journal: String::new(),
        mode: Mode::LifecycleInspect {
            store: store.ok_or_else(|| fail(USAGE, "--store is required"))?,
            observe,
            account,
            member,
            confinement,
        },
        crash_at: None,
        call_log: None,
    })
}

/// A deliberate crash at a named point. Deterministic — no timing, no signals.
fn crash_if(args: &Args, point: &str) {
    if args.crash_at.as_deref() == Some(point) {
        eprintln!("ironworks: CRASHING at {point} (deliberate)");
        std::process::exit(CRASHED);
    }
}

// ── the turn ─────────────────────────────────────────────────────────────────────────────

fn run() -> Result<(), Exit> {
    let args = parse_args()?;
    // Inspection reads two local files. It does not verify a scope, open a journal, or construct
    // a runtime client, so it must not go through `boot` — which would make a read-only report
    // fail whenever the record store happened to be down.
    if let Mode::LifecycleInspect { store, observe, account, member, confinement } = &args.mode {
        return run_lifecycle_inspect(
            &args.slug,
            store,
            observe.as_deref(),
            *account,
            *member,
            *confinement,
        );
    }
    if let Mode::LifecycleMembers { store } = &args.mode {
        return run_lifecycle_members(store);
    }
    if let Mode::LifecycleConfine { store, dry_run } = &args.mode {
        return run_lifecycle_confine(&args.slug, store, *dry_run);
    }
    if let Mode::LifecycleAdopt { store, service, channel, records_credential, member_credential } =
        &args.mode
    {
        return run_lifecycle_adopt(
            &args.slug,
            store,
            service,
            channel,
            records_credential,
            member_credential,
        );
    }
    if matches!(args.mode, Mode::Status) {
        return run_status(&args.journal);
    }
    if matches!(args.mode, Mode::Compose) {
        return run_compose(&args);
    }
    // Relay serves every tenant in its registry, so it boots per tenant rather than once. It
    // returns before `boot` for the same reason the read-only commands do: there is no single
    // instance for it to be.
    if let Mode::Relay { channel, registry, chunk_bytes } = &args.mode {
        return run_relay(&args, channel, registry, *chunk_bytes);
    }
    let b = boot(&args)?;
    match &args.mode {
        Mode::Turn { ask } => run_turn(&b, &args, ask),
        Mode::Relay { .. } => unreachable!("relay returns before boot"),
        Mode::Redeliver { channel, update, chunk_bytes } => {
            run_redeliver(&b, channel, *update, *chunk_bytes)
        }
        // Answered above, before anything was booted.
        Mode::LifecycleInspect { .. }
        | Mode::LifecycleMembers { .. }
        | Mode::LifecycleConfine { .. }
        | Mode::LifecycleAdopt { .. }
        | Mode::Status
        | Mode::Compose => {
            unreachable!("the read-only commands return before boot")
        }
    }
}

/// Write a whole report to stdout, treating a closed reader as a normal end.
///
/// `println!` panics on a broken pipe, and a report exists to be piped — `compose | head`,
/// `status | grep` — so the default turns "the reader has seen enough" into a crash. It cannot be
/// fixed by resetting `SIGPIPE`, because that needs `unsafe` and the workspace forbids it.
///
/// **The two report commands use this and `turn` deliberately does not.** A successful stdout
/// write is `turn`'s delivery evidence, so a write that failed must surface rather than be
/// swallowed; a report has no such claim to make. One buffer and one write also makes a report
/// atomic, so a reader never sees half of it.
fn report(text: &str) -> Result<(), Exit> {
    write_report(&mut io::stdout().lock(), text)
}

/// The half worth testing: which write failures are a normal end and which are a failure.
fn write_report(out: &mut impl Write, text: &str) -> Result<(), Exit> {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        // The reader closed the pipe. Nothing is wrong and nothing is left to say.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(fail(FAILED, format!("cannot write the report: {e}"))),
    }
}

/// A dry melt: what this binary would compose for one tenant, fingerprinted.
///
/// # Why this is a command
///
/// An answer-quality verdict is bound to an alloy digest — `ironworks-eval` asserts one composition
/// across a run and refuses to score two. So when the digest moves, the next question is always
/// *what moved under it*, and that had no answer: the digest surfaces only as a side effect of
/// running a turn, the alloy text is persisted nowhere, and an alloy fuses scrap **and** guidance,
/// so it can move with no repo diff at all.
///
/// **It prints digests, never text.** The guidance an alloy carries is the tenant's own data and
/// has no business on a terminal, in a shell history or in a pasted report; `Alloy::digest` exists
/// so a composition is comparable without printing it. Which ingredient moved is the fact an
/// operator acts on — *how* it moved is a diff of files they already hold.
///
/// Reads one local file and the embedded corpus. No runtime, no record store, no journal and no
/// model key, which is what lets it answer for a composition offline and at an older revision.
fn run_compose(args: &Args) -> Result<(), Exit> {
    let e = |m: String| fail(FAILED, m);
    let definition =
        ironworks_services::service::load(&args.service).map_err(|x| e(x.to_string()))?;
    let guidance_text = std::fs::read_to_string(&args.guidance)
        .map_err(|x| e(format!("cannot read {}: {x}", args.guidance)))?;
    // Parsed, not merely read: an alloy fingerprint for guidance bound to another tenant or
    // another service describes a composition that would never have served.
    let guidance =
        Guidance::parse(&guidance_text, &args.slug, &args.service).map_err(|x| e(x.to_string()))?;

    let composition = definition.composition();
    let alloy = ironworks_domain::melt(composition, &guidance, &ironworks_services::Heap)
        .map_err(|x| e(x.to_string()))?;
    let parts =
        composition.part_digests(&ironworks_services::Heap).map_err(|x| e(x.to_string()))?;

    let mut out = String::new();
    out.push_str(&format!("service {}\n", definition.service_id()));
    out.push_str(&format!("alloy {} bytes ({})\n", alloy.len(), alloy.digest()));
    out.push_str(&format!("guidance {} bytes ({})\n", guidance.as_str().len(), guidance.digest()));
    for (part, digest) in &parts {
        let role = if part == composition.safety_tail() { "tail" } else { "part" };
        out.push_str(&format!("{role} {part} {digest}\n"));
    }
    out.push_str(&format!(
        "scrap {} piece(s) ({})\n",
        ironworks_services::count(),
        ironworks_services::digest()
    ));
    report(&out)
}

/// List journal alarms without changing the journal or contacting any external system.
fn run_status(path: &str) -> Result<(), Exit> {
    let journal = Journal::open_readonly(path).map_err(|x| fail(FAILED, x.to_string()))?;
    let turns = journal.unresolved().map_err(|x| fail(FAILED, x.to_string()))?;
    let mut out = format!("{} unresolved turn(s) in {}\n", turns.len(), safe_field(path));
    for turn in turns {
        let response = turn.outcome.response().map(ResponseId::as_str).unwrap_or("-");
        out.push_str(&format!(
            "key={} instance={} org={} outcome={} delivery={} retrieved_at={} response={}\n",
            turn.key.as_str(),
            turn.instance.as_str(),
            turn.org.as_str(),
            turn.outcome.tag(),
            turn.delivery,
            safe_field(&turn.retrieved_at),
            response,
        ));
        if let TurnOutcome::OperatorRequired { reason, .. } = &turn.outcome {
            out.push_str(&format!("  reason={}\n", safe_field(reason.as_str())));
        }
    }
    report(&out)
}

fn safe_field(value: &str) -> String {
    value.chars().flat_map(char::escape_default).collect()
}

/// Report on one instance without touching anything.
///
/// The store is opened **read-only**, so a reporting command cannot mutate the state it reports
/// on — and the exit code carries the verdict, because `list | grep` under `pipefail` reports the
/// pipeline's status whatever grep found, and a caller reading that as a boolean concludes the
/// opposite of the truth.
fn run_lifecycle_inspect(
    slug: &str,
    store_path: &str,
    observe: Option<&str>,
    account: bool,
    member: bool,
    confinement: bool,
) -> Result<(), Exit> {
    use ironworks_lifecycle::{LifecycleStore, NoReconciler, Reconcile, inspect};
    let e = |m: String| fail(FAILED, m);
    let blocked = |m: String| Exit { code: BLOCKED, message: m };

    let slug = Slug::parse(slug).map_err(|x| e(x.to_string()))?;
    // Read-only throughout, including under `--confinement`: the live leg observes a runtime and
    // records nothing. A store this command cannot read is a measurement that could not be made.
    let store = LifecycleStore::open_readonly(store_path).map_err(|x| blocked(x.to_string()))?;
    let spec = store
        .spec(&slug)
        .map_err(|x| blocked(x.to_string()))?
        .ok_or_else(|| e(format!("no lifecycle record for {slug}")))?;
    let recorded = store.recorded(&slug).map_err(|x| blocked(x.to_string()))?;
    let serving = store.serving(&slug).map_err(|x| blocked(x.to_string()))?;
    let axis = confinement_axis(&store, &slug, &recorded, confinement)?;

    // Fixtures only in this slice. Without one, nothing is asked of anyone and the verdict says so.
    // The IronClaw member reconciler answers for the member authority alone, from the id we
    // recorded — it cannot attribute anything, so it is composed with the others rather than
    // replacing them.
    if member {
        let base = std::env::var("IRONCLAW_API")
            .map_err(|_| fail(USAGE, "--ironclaw needs IRONCLAW_API (the instance's base URL)"))?;
        let credential =
            CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_OPERATOR_TOKEN");
        let admin = ironworks_reconcile::Admin::new(&base, &credential);
        let recorded_id = recorded.accounted_id(ironworks_lifecycle::Authority::Member).cloned();
        let inventory =
            ironworks_reconcile::IronclawMember::new(&admin, recorded_id).observe(&spec);
        let report = inspect(&spec, &recorded, &inventory, &serving, &axis);
        print!("{}", report.render());
        let code = report.verdict.exit_code();
        return if code == 0 {
            Ok(())
        } else {
            Err(Exit { code, message: format!("verdict {}", report.verdict) })
        };
    }
    let inventory = match (account, observe) {
        // The live Account Service. It speaks for org registration and stays silent about the
        // other four, so this cannot produce a clean overall verdict on its own — which is
        // correct, and is why it is the first reconciler rather than the only one.
        (true, _) => {
            let base = std::env::var("IRONWORKS_RECORDS").map_err(|_| {
                fail(
                    USAGE,
                    "--account-service needs IRONWORKS_RECORDS (the account service base URL)",
                )
            })?;
            let book = Book::new(&base, EnvCredentials);
            let credential =
                CredentialRef::new(CredentialPurpose::RecordScope, "env:IRONWORKS_RECORDS_TOKEN");
            ironworks_reconcile::AccountService::new(&book, credential).observe(&spec)
        }
        (false, Some(path)) => fixture_inventory(path)?,
        (false, None) => NoReconciler.observe(&spec),
    };

    let report = inspect(&spec, &recorded, &inventory, &serving, &axis);
    print!("{}", report.render());
    let code = report.verdict.exit_code();
    if code == 0 {
        Ok(())
    } else {
        Err(Exit { code, message: format!("verdict {}", report.verdict) })
    }
}

/// Build the confinement axis: history always, a live observation only when asked.
///
/// # Why the live leg lives here rather than in `inspect`
///
/// `inspect` is pure — it reads four values and returns a conclusion, so it cannot reach a runtime
/// and cannot mutate anything. Composing the observation at the binary is what keeps that true
/// while still letting the flag mean *measure now*.
fn confinement_axis(
    store: &ironworks_lifecycle::LifecycleStore,
    slug: &Slug,
    recorded: &ironworks_lifecycle::Recorded,
    requested: bool,
) -> Result<ironworks_lifecycle::ConfinementAxis, Exit> {
    use ironworks_lifecycle::{
        Blocked, ConfinementAxis, Ledger, LiveConfinement, ObservedAt, PolicyBinding,
    };
    use ironworks_reconcile::Binding;

    let e = |m: String| fail(FAILED, m);
    let blocked = |m: String| Exit { code: BLOCKED, message: m };
    if !requested {
        return Ok(ConfinementAxis::NotRequested);
    }
    let ledger = if store.schema_version() >= 2 { Ledger::Present } else { Ledger::PredatesLedger };
    let Some(member) = recorded.accounted_id(ironworks_lifecycle::Authority::Member).cloned()
    else {
        // No member to bind against. `inspect` decides whether that is "not yet applicable" or a
        // recorded stage with no identifier; this only supplies what it could read.
        return Ok(ConfinementAxis::Recorded { ledger, latest: None });
    };
    // A v1 store has no ledger table to query at all.
    let latest = match ledger {
        Ledger::PredatesLedger => None,
        Ledger::Present => {
            store.latest_applicable(slug, &member).map_err(|x| blocked(x.to_string()))?
        }
    };

    let base = std::env::var("IRONCLAW_API")
        .map_err(|_| fail(USAGE, "--confinement needs IRONCLAW_API (the instance's base URL)"))?;
    let (definition, policy) = policy_for(store, slug)?;
    let current = PolicyBinding::of(&definition, &policy);

    let operator =
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_OPERATOR_TOKEN");
    let admin = ironworks_reconcile::Admin::new(&base, &operator);
    let member_credential =
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_MEMBER_TOKEN");
    let confinement =
        ironworks_reconcile::MemberConfinement::new(&admin, &EnvCredentials, member_credential);
    let at = ObservedAt::parse(&now_iso()).map_err(|x| e(x.to_string()))?;

    let live = match confinement.bind(&member) {
        Ok(bound) => LiveConfinement::Bound {
            subject: bound.subject().clone(),
            confinement: bound.observe(&policy, &at),
        },
        Err(Binding::Blocked(why)) => LiveConfinement::Blocked(Blocked::from(&why)),
        Err(Binding::Mismatch(mismatch)) => LiveConfinement::Mismatch {
            expected: mismatch.expected().clone(),
            authenticated: mismatch.authenticated().clone(),
        },
    };
    Ok(ConfinementAxis::Measured { ledger, latest, live: Box::new(live), current })
}

/// Apply confinement to one member, then measure whether it took.
///
/// # It authenticates as the member, and that is the guarantee's boundary
///
/// `POST /settings/tools` writes the **caller's own** scope — measured at 1.4.0, `?user_id=` is
/// silently ignored — so confining a member requires that member's credential. The same credential
/// lets its holder undo the work, because the runtime has no non-overridable lock. Custody is the
/// containment; this command cannot change that, only apply it.
///
/// # It re-measures rather than reporting its own writes
///
/// The verdict comes from a fresh observation after the plan is applied, never from the writes
/// succeeding. A command that concluded `Verified` from its own 200s would be certifying its
/// intent. §B-8's rule, one layer over: probed, never inferred.
/// The policy a recorded instance's service declares.
///
/// Its own function so the seam can be tested without a runtime: it is the only place `services` and
/// `lifecycle` meet, and it is what `--workspace` used to bypass.
///
/// **Fails closed when the instance is not recorded.** Without a spec there is no service, without
/// a service there is no declared ceiling, and confining to a guess is how the wrong policy gets
/// applied to the right member — the direction that matters, because the guess that looks harmless
/// is the permissive one.
fn policy_for(
    store: &ironworks_lifecycle::LifecycleStore,
    slug: &Slug,
) -> Result<(ironworks_domain::ServiceDefinition, ironworks_lifecycle::ConfinementPolicy), Exit> {
    let e = |m: String| fail(FAILED, m);
    let spec = store.spec(slug).map_err(|x| e(x.to_string()))?.ok_or_else(|| {
        e(format!(
            "no instance recorded for {}. Confinement applies the policy that instance's service \
             declares, so there is nothing here to derive one from.",
            slug.as_str()
        ))
    })?;
    let definition = ironworks_services::service::load(spec.service.as_str()).map_err(|x| {
        e(format!("{} is recorded against a service that will not load: {x}", slug.as_str()))
    })?;
    let policy =
        ironworks_lifecycle::ConfinementPolicy::for_profile(definition.capability_ceiling());
    Ok((definition, policy))
}

/// Adopt authority that already exists, without creating or changing any of it.
///
/// There is intentionally no org or member-id parameter. The Account Service supplies the former
/// from the records handle, and /session supplies the complete runtime subject from the member
/// handle. Only after both reads succeed does one SQLite transaction record the instance.
fn run_lifecycle_adopt(
    slug: &str,
    store_path: &str,
    service: &str,
    channel: &str,
    records_credential: &str,
    member_credential: &str,
) -> Result<(), Exit> {
    use ironworks_domain::ScopeResolver;
    use ironworks_lifecycle::{AdoptionEvidence, LifecycleError, LifecycleStore};

    let e = |m: String| fail(FAILED, m);
    let blocked = |m: String| Exit { code: BLOCKED, message: m };
    let slug = Slug::parse(slug.to_owned()).map_err(|x| e(x.to_string()))?;
    let service =
        ironworks_domain::ServiceId::parse(service.to_owned()).map_err(|x| e(x.to_string()))?;
    // Loading is the preflight proof that this build actually carries the requested service.
    ironworks_services::service::load(service.as_str()).map_err(|x| e(x.to_string()))?;
    if channel.trim().is_empty() {
        return Err(fail(USAGE, "--channel must not be empty"));
    }
    let channel = ironworks_domain::ChannelRef::new(channel);
    let checked_handle = |raw: &str, flag: &str| -> Result<String, Exit> {
        let Some(name) = raw.strip_prefix("env:") else {
            return Err(fail(USAGE, format!("{flag} must be an env:NAME handle, never a secret")));
        };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(fail(USAGE, format!("{flag} must be an env:NAME handle")));
        }
        Ok(raw.to_owned())
    };
    let records_credential = checked_handle(records_credential, "--records-credential")?;
    let member_credential = checked_handle(member_credential, "--member-credential")?;

    // Open before contacting either authority so an existing slug refuses without resolving any
    // credential. Opening may initialise or migrate schema, but no instance row or observation is
    // written until adopt's single transaction below.
    let store = LifecycleStore::open(store_path).map_err(|x| blocked(x.to_string()))?;
    if store.spec(&slug).map_err(|x| blocked(x.to_string()))?.is_some() {
        return Err(
            e(LifecycleError::AlreadyExists { slug: slug.as_str().to_owned() }.to_string()),
        );
    }

    let records_base = std::env::var("IRONWORKS_RECORDS")
        .map_err(|_| fail(USAGE, "IRONWORKS_RECORDS is not set (the account service base URL)"))?;
    let records_handle = CredentialRef::new(CredentialPurpose::RecordScope, records_credential);
    let book = Book::new(&records_base, EnvCredentials);
    let scope = ScopeResolver::resolve(&book, &records_handle).map_err(|why| {
        blocked(format!("the records credential could not establish an org: {why}"))
    })?;

    let runtime_base = std::env::var("IRONCLAW_API")
        .map_err(|_| fail(USAGE, "IRONCLAW_API is not set (the instance's base URL)"))?;
    // authenticated_subject makes only the member-authenticated GET. Admin retains an operator
    // handle for confinement methods, but this observational command never resolves or uses it.
    let unused_operator =
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_OPERATOR_TOKEN");
    let admin = ironworks_reconcile::Admin::new(&runtime_base, &unused_operator);
    let member_handle = CredentialRef::new(CredentialPurpose::RuntimeMember, member_credential);
    let session =
        ironworks_reconcile::MemberConfinement::new(&admin, &EnvCredentials, member_handle);
    let subject = session.authenticated_subject().map_err(|why| {
        blocked(format!("the member credential could not establish a runtime subject: {why}"))
    })?;

    // The runtime tenant is recorded as measured fact. It is not compared with the local slug:
    // those are separate namespaces, and one host's naming convention is not a lifecycle rule.
    let evidence = AdoptionEvidence::observed(&scope, subject);
    let spec =
        store.adopt(&slug, &service, &channel, &evidence, &now_iso()).map_err(|why| match why {
            LifecycleError::AlreadyExists { .. } => e(why.to_string()),
            _ => blocked(why.to_string()),
        })?;
    println!("ADOPTED   {}", spec.slug);
    println!("  records org    : {}", evidence.org());
    println!("  runtime tenant : {}", evidence.subject().tenant());
    println!("  runtime member : {}", evidence.subject().user());
    println!("  provenance     : observed existing; no foreign authority was created");
    Ok(())
}

fn run_lifecycle_confine(slug: &str, store_path: &str, dry_run: bool) -> Result<(), Exit> {
    use ironworks_lifecycle::{Blocked, ConfinementAttempt, ObservedAt, PolicyBinding};
    use ironworks_reconcile::{Applied, Binding, Reading};

    let e = |m: String| fail(FAILED, m);
    // A measurement that could not be made is BLOCKED, not FAILED. A store that will not open, or
    // a runtime that will not answer, is upstream of IronWorks — scoring it 2 would report a defect
    // here for something that is not one, and an operator who investigates two of those learns to
    // skim the ones that matter.
    let blocked = |m: String| Exit { code: BLOCKED, message: m };

    let base = std::env::var("IRONCLAW_API")
        .map_err(|_| fail(USAGE, "IRONCLAW_API is not set (the instance's base URL)"))?;
    let slug_id = Slug::parse(slug.to_owned()).map_err(|x| e(x.to_string()))?;

    // `--dry-run` opens READ-ONLY, so it cannot migrate a v1 store, create a WAL sidecar, or write
    // a metadata row. "Nothing written" is then a property of the handle rather than of every
    // branch below remembering to check a flag.
    let store = if dry_run {
        ironworks_lifecycle::LifecycleStore::open_readonly(store_path)
    } else {
        ironworks_lifecycle::LifecycleStore::open(store_path)
    }
    .map_err(|x| blocked(x.to_string()))?;

    let recorded = store.recorded(&slug_id).map_err(|x| blocked(x.to_string()))?;
    let member = recorded
        .accounted_id(ironworks_lifecycle::Authority::Member)
        .cloned()
        .ok_or_else(|| {
            e(format!(
                "no member recorded for {slug}. Confinement applies to a member IronWorks \
                 recorded creating; there is nothing here to confine."
            ))
        })?;

    // Derived before any HTTP: the policy is a property of committed files, so a runtime that is
    // down cannot change what this instance is entitled to.
    let (definition, policy) = policy_for(&store, &slug_id)?;
    let binding = PolicyBinding::of(&definition, &policy);
    eprintln!(
        "confine: {slug} runs {}@{} — runtime_tools {:?}, policy {}",
        definition.id().as_str(),
        definition.version(),
        definition.capability_ceiling().runtime_tools,
        policy.fingerprint()
    );

    let operator =
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_OPERATOR_TOKEN");
    let admin = ironworks_reconcile::Admin::new(&base, &operator);
    let member_credential =
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_MEMBER_TOKEN");
    let confinement =
        ironworks_reconcile::MemberConfinement::new(&admin, &EnvCredentials, member_credential);

    // Appending is a closure so every exit path records what happened rather than only the
    // successful one. Under `--dry-run` the store is read-only and this is deliberately inert.
    let append = |attempt: &ConfinementAttempt| {
        if dry_run {
            return;
        }
        if let Err(why) = store.record_attempt(&slug_id, attempt) {
            eprintln!("confine: WARNING the attempt could not be journalled: {why}");
        }
    };

    // ── bind before anything touches the settings surface ───────────────────────────
    let at = ObservedAt::parse(&now_iso()).map_err(|x| e(x.to_string()))?;
    let bound = match confinement.bind(&member) {
        Ok(bound) => bound,
        Err(Binding::Blocked(why)) => {
            append(&ConfinementAttempt::binding_blocked(
                member.clone(),
                at.clone(),
                Blocked::from(&why),
            ));
            println!("BLOCKED   {slug} member {member}");
            println!("  the member credential could not be identified: {why}");
            println!("  No setting was read and none was written. The attempt is journalled.");
            return Err(blocked("the member credential could not be identified".into()));
        }
        Err(Binding::Mismatch(mismatch)) => {
            append(&ConfinementAttempt::subject_mismatch(mismatch.clone(), at.clone()));
            // Both identities, always. A refusal naming only one leaves an operator guessing which
            // half is wrong, and the likeliest cause is a member token that belongs to the
            // env-bearer operator identity rather than to a minted member.
            println!("FAIL      {slug}");
            println!("  lifecycle records member : {}", mismatch.expected());
            println!("  credential authenticates : {}", mismatch.authenticated());
            println!(
                "  REFUSING. No settings read and no settings write was made — this catalogue \
                 would not have been the recorded member's."
            );
            return Err(Exit { code: FAILED, message: "the credential is not this member".into() });
        }
    };
    eprintln!("confine: bound — authenticated as {}", bound.subject());

    // ── measure first ───────────────────────────────────────────────────────────────
    // One read feeds the plan, the initial record and the operator line. A plan built on an unread
    // catalogue would deny an arbitrary set and report success.
    let (observed, locked, auto_approve) = match bound.read(&policy, &binding, &at) {
        Reading::Blocked { attempt } => {
            append(&attempt);
            println!("BLOCKED   {slug} member {member}");
            println!("  REFUSING TO APPLY. The member's authority could not be measured.");
            return Err(blocked("the member's authority could not be measured".into()));
        }
        Reading::Measured { states, locked, auto_approve, attempt } => {
            let initial_clean =
                attempt.measurement().is_some_and(ironworks_lifecycle::Measurement::is_clean);
            append(&attempt);
            (states, locked, (auto_approve, initial_clean))
        }
    };
    let (auto_approve, initial_clean) = auto_approve;

    let plan = ironworks_lifecycle::confinement::plan(&policy, &observed, &locked, auto_approve);
    eprintln!(
        "confine: {slug} member {member} — {} catalogued, {} step(s) to policy",
        observed.len(),
        plan.len()
    );
    for step in plan.steps() {
        match step {
            ironworks_lifecycle::Step::ApprovalOff => eprintln!("  1. automatic approval off"),
            ironworks_lifecycle::Step::Disable { tool } => eprintln!("     disable {tool}"),
        }
    }
    if dry_run {
        eprintln!("confine: --dry-run, nothing written");
        return Ok(());
    }

    if plan.is_empty() {
        eprintln!("confine: already at policy; nothing to write");
        // The initial measurement IS the record; a second identical observation would append a row
        // saying nothing new. It still has to be scored, because an empty plan does not imply a
        // clean member: `AllowedToolAbsent` is drift that `plan` produces no step for.
        return if initial_clean {
            print_verified(slug, &member, &observed, auto_approve, bound.subject());
            Ok(())
        } else {
            println!("DRIFTED   {slug} member {member}");
            println!("  the plan is empty and the member is still off policy — the drift is not");
            println!("  something a settings write can repair. See the recorded observation.");
            Err(Exit { code: FAILED, message: "confinement did not reach policy".into() })
        };
    }

    // ── apply ───────────────────────────────────────────────────────────────────────
    let stopped = match bound.apply(&plan) {
        Applied::Written { steps } => {
            eprintln!("confine: {steps} step(s) written");
            None
        }
        // Deliberately fatal, and deliberately still measured: the plan is ordered so the work
        // cannot be undone by the member while it is in progress, and continuing past a failure
        // abandons that ordering. What the member is left in is worth recording either way.
        Applied::Stopped { applied, at: step, why } => Some(format!(
            "STOPPED after {applied} step(s){}: {why}\n  The member is PARTIALLY confined. \
             Re-run to resume — the plan contains only what is not already at policy.",
            step.map(|s| format!(" at {s:?}")).unwrap_or_default()
        )),
    };

    // ── measure again; this is the verdict ──────────────────────────────────────────
    let after_at = ObservedAt::parse(&now_iso()).map_err(|x| e(x.to_string()))?;
    let after = bound.read(&policy, &binding, &after_at);
    append(after.attempt());

    if let Some(message) = stopped {
        println!("FAIL      {slug} member {member}");
        println!("  {message}");
        if matches!(after, Reading::Blocked { .. }) {
            // Reported as a secondary note. The apply failure is what an operator must act on, and
            // letting a blocked re-read displace it would hide the sharper fact behind a softer one.
            println!(
                "  NOTE: the post-failure re-measurement was also blocked, so the state above is \
                 what was written rather than what was then observed."
            );
        }
        return Err(Exit { code: FAILED, message: "confinement stopped part-way".into() });
    }

    match after {
        Reading::Blocked { .. } => {
            println!("BLOCKED   {slug} member {member}");
            println!("  the plan was written and the result could not be re-measured.");
            println!("  Nothing here certifies the member: a write is not an observation.");
            Err(blocked("confinement could not be measured after applying".into()))
        }
        // `locked` is not read here: the re-measurement's verdict already accounts for it, and
        // this arm reports that verdict rather than re-deriving one.
        Reading::Measured { states, locked: _, auto_approve, attempt } => {
            let measurement = attempt.measurement().expect("a measured reading has a measurement");
            if measurement.is_clean() {
                print_verified(slug, &member, &states, auto_approve, bound.subject());
                Ok(())
            } else {
                println!("DRIFTED   {slug} member {member}");
                for d in measurement.drift() {
                    println!("  {d}");
                }
                println!(
                    "  {} reachable of {} catalogued",
                    measurement.reachable().len(),
                    measurement.catalogued()
                );
                Err(Exit { code: FAILED, message: "confinement did not reach policy".into() })
            }
        }
    }
}

/// The VERIFIED block, including the scope an operator has to read beside it.
fn print_verified(
    slug: &str,
    member: &ironworks_lifecycle::OpaqueId,
    observed: &std::collections::BTreeMap<String, String>,
    auto_approve: bool,
    subject: &ironworks_lifecycle::RuntimeSubject,
) {
    let reachable = observed
        .values()
        .filter(|raw| ironworks_lifecycle::ToolState::parse(raw).is_reachable())
        .count();
    println!("VERIFIED  {slug} member {member}");
    println!("  subject: authenticated as {subject} — this is the recorded member");
    println!(
        "  {} catalogued, {reachable} reachable, approval {}",
        observed.len(),
        if auto_approve { "ON" } else { "off" }
    );
    println!("  custody-backed: the credential that applied this can undo it");
    // The scope belongs where an operator reads it, not only in a module comment. Measured
    // 2026-08-31: after a VERIFIED apply the model was still offered five tools absent from the
    // catalogue under any spelling. Offered is not reachable — `Disabled` is refused at dispatch,
    // not removed from the list — so this states the boundary rather than a breach. See
    // docs/experiments/0012.
    println!(
        "  SCOPE: a statement about the {} catalogued tools, verified by re-measurement.\n\
         \x20        NOT a claim that the model has no other executable authority: tools\n\
         \x20        the catalogue does not name cannot be governed through it.",
        observed.len()
    );
}
/// Fleet-wide: every member on the instance that no lifecycle record accounts for.
///
/// Deliberately **not** part of `inspect`. Per-instance reconciliation proves custody of
/// identities IronWorks recorded; this discovers external authority no instance accounts for, and
/// it reads every instance's records rather than one spec's. Two questions, two commands.
fn run_lifecycle_members(store_path: &str) -> Result<(), Exit> {
    use ironworks_lifecycle::{Authority, LifecycleStore};
    use ironworks_reconcile::{Admin, FleetInventory, unaccounted_members};
    let e = |m: String| fail(FAILED, m);

    let base = std::env::var("IRONCLAW_API")
        .map_err(|_| fail(USAGE, "IRONCLAW_API is not set (the instance's base URL)"))?;
    let store = LifecycleStore::open_readonly(store_path).map_err(|x| e(x.to_string()))?;
    let accounted = store.all_recorded_ids(Authority::Member).map_err(|x| e(x.to_string()))?;

    let credential =
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_OPERATOR_TOKEN");
    let admin = Admin::new(&base, &credential);

    println!("fleet member reconciliation — {base}");
    println!("accounted  {} member id(s) recorded across all instances", accounted.len());
    match unaccounted_members(&admin, &accounted) {
        FleetInventory::Complete { observed, unaccounted, dangling } => {
            println!("observed   {observed} member(s) on the instance");
            if !dangling.is_empty() {
                println!(
                    "\nDANGLING — {} recorded member(s) the instance does not hold:",
                    dangling.len()
                );
                for id in &dangling {
                    println!("  {id}");
                }
                println!(
                    "\n  Our records and the directory disagree. NOTE: a member being absent is \
                     NOT\n  evidence that its session was revoked — deletion is not revocation, \
                     and nothing\n  here measured the token."
                );
            }
            if unaccounted.is_empty() && dangling.is_empty() {
                println!("\nverdict    PASS — the directory and our records agree, both ways");
                Ok(())
            } else if unaccounted.is_empty() {
                println!("\nverdict    FAIL");
                Err(Exit { code: 2, message: "records point at members that do not exist".into() })
            } else {
                println!(
                    "\nUNACCOUNTED — {} member(s) no lifecycle record claims:",
                    unaccounted.len()
                );
                for id in &unaccounted {
                    println!("  {id}");
                }
                println!(
                    "\nverdict    BLOCKED\n\n  Reported, not adopted and not removed. Nothing here \
                     attributes one of these to a tenant:\n  the admin API carries no instance \
                     field, and a display name is a label anyone can change."
                );
                Err(Exit { code: 3, message: "unaccounted member authority".into() })
            }
        }
        // Enumeration finished and there is nothing to compare against. What is out there is
        // still printed — an instance holding members IronWorks accounts for none of is the
        // sharpest thing this command can find — but it cannot be scored as agreement.
        FleetInventory::NothingToReconcile { observed, unaccounted } => {
            println!("observed   {observed} member(s) on the instance");
            if !unaccounted.is_empty() {
                println!(
                    "\nUNACCOUNTED — {} member(s), and no lifecycle record accounts for ANY of them:",
                    unaccounted.len()
                );
                for id in &unaccounted {
                    println!("  {id}");
                }
            }
            println!(
                "\nverdict    BLOCKED — {}\n\n  Nothing was compared. Check that --store points at \
                 the lifecycle store\n  for this instance: a clean result here would be arithmetic \
                 rather than evidence.",
                ironworks_lifecycle::BlockedReason::NothingToReconcile
            );
            Err(Exit { code: 3, message: "nothing to reconcile".into() })
        }
        // A truncated enumeration is not zero unknowns. It is no answer.
        FleetInventory::Blocked(reason) => {
            println!("\nverdict    BLOCKED — the directory could not be enumerated: {reason}");
            println!("  A partial listing cannot produce a set difference, and reporting one as");
            println!("  'no unknown members' is a clean score computed from missing evidence.");
            Err(Exit { code: 3, message: "enumeration incomplete".into() })
        }
    }
}

/// Read a reconciliation fixture: one `authority=state[:id]` per line.
///
/// A file rather than a flag because the shape is a list, and a deliberately dull format because
/// this is scaffolding for adapters that do not exist yet. `?` is the state for *could not
/// measure*, and it is spelled differently from a missing line: a line saying `?` is an answer,
/// an absent line is silence, and they are not the same.
fn fixture_inventory(path: &str) -> Result<ironworks_lifecycle::AuthorityInventory, Exit> {
    use ironworks_lifecycle::{
        Authority, AuthorityFact, AuthorityInventory, BlockedReason, OpaqueId, Presence,
    };
    let e = |m: String| fail(FAILED, m);
    let raw = std::fs::read_to_string(path)
        .map_err(|x| e(format!("cannot read the observation fixture {path}: {x}")))?;
    let mut facts = Vec::new();
    for (n, line) in raw.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (name, rest) = line
            .split_once('=')
            .ok_or_else(|| e(format!("{path}:{}: expected authority=state", n + 1)))?;
        let authority = Authority::ALL
            .iter()
            .copied()
            .find(|a| a.tag() == name.trim())
            .ok_or_else(|| e(format!("{path}:{}: unknown authority {name:?}", n + 1)))?;
        let presence = match rest.trim().split_once(':') {
            Some(("present", id)) => Presence::Present { id: OpaqueId::new(id.trim()) },
            _ => match rest.trim() {
                "absent" => Presence::Absent,
                "?" => Presence::Unverifiable(BlockedReason::NotObserved),
                other => {
                    return Err(e(format!(
                        "{path}:{}: expected present:<id>, absent or ? — got {other:?}",
                        n + 1
                    )));
                }
            },
        };
        facts.push(AuthorityFact { authority, presence });
    }
    Ok(AuthorityInventory::Reconciled(facts))
}

/// Everything both commands need: a verified instance, its record store, and its journal.
///
/// Verification happens here and only here, so there is no path into either command that reaches
/// a turn without it. That is the control, and sharing it is what keeps the two commands from
/// drifting into two different answers about who a caller is.
struct Boot {
    verified: ServiceInstance<ironworks_domain::Verified>,
    book: Book<EnvCredentials>,
    journal: Journal,
    scope: ThreadScope,
    runtime: Ironclaw,
    journal_path: String,
}

fn boot(args: &Args) -> Result<Boot, Exit> {
    let e = |m: String| fail(FAILED, m);

    let base = std::env::var("IRONWORKS_BASE")
        .map_err(|_| fail(USAGE, "IRONWORKS_BASE is not set (the runtime base URL)"))?;
    let token = std::env::var("IRONWORKS_TOKEN")
        .map_err(|_| fail(USAGE, "IRONWORKS_TOKEN is not set (this instance's credential)"))?;
    // No fallback, deliberately. Scope verification is a control: without a record store there is
    // nothing entitled to say which org a credential authorises, and an unverified identity does
    // not serve. A fixture stand-in here would be exactly the fail-open this design removes.
    let store = std::env::var("IRONWORKS_RECORDS").map_err(|_| {
        fail(
            USAGE,
            "IRONWORKS_RECORDS is not set. There is no offline mode: the record store is the only \
             authority on which organization a credential authorises, and serving on an \
             unverified scope is the failure this refuses to have.",
        )
    })?;

    // ── the explicit inputs to a service instance ────────────────────────────────────
    let pin = ironworks_domain::ModelPin::parse(
        &std::fs::read_to_string("MODEL_PIN")
            .map_err(|x| e(format!("cannot read MODEL_PIN: {x}")))?,
    )
    .map_err(|x| e(x.to_string()))?;
    let definition =
        ironworks_services::service::load(&args.service).map_err(|x| e(x.to_string()))?;
    let guidance_text = std::fs::read_to_string(&args.guidance)
        .map_err(|x| e(format!("cannot read {}: {x}", args.guidance)))?;
    let guidance =
        Guidance::parse(&guidance_text, &args.slug, &args.service).map_err(|x| e(x.to_string()))?;

    let instance = ServiceInstance::new(
        Slug::parse(args.slug.clone()).map_err(|x| e(x.to_string()))?,
        definition,
        guidance,
        RuntimeIdentity::parse(format!("member:{}", args.slug)).map_err(|x| e(x.to_string()))?,
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONWORKS_TOKEN"),
        ContextBinding::new(CredentialRef::new(
            CredentialPurpose::RecordScope,
            "env:IRONWORKS_RECORDS_TOKEN",
        ))
        .map_err(|x| e(x.to_string()))?,
        ModelSelection::from_pin(&pin),
        CapabilityProfile::READ_ONLY_RECORDS,
        None,
        &ironworks_services::Heap,
    )
    .map_err(|x| e(x.to_string()))?;

    // ── the control ─────────────────────────────────────────────────────────────────
    let book = Book::new(&store, EnvCredentials);
    let verified = instance.verify(&book).map_err(|x| e(x.to_string()))?;
    eprintln!("ironworks: scope {} verified by {} (evidence: record store)", verified.org(), store);

    let journal = Journal::open(&args.journal).map_err(|x| e(x.to_string()))?;
    let scope = verified.thread_scope();
    let runtime = Ironclaw::new(base, ironworks_ironclaw::Credential::new(token));
    Ok(Boot { verified, book, journal, scope, runtime, journal_path: args.journal.clone() })
}

/// Resolve an `env:` handle to its value.
///
/// Mirrors `EnvCredentials` and stays deliberately as narrow: the runtime client takes a token
/// string rather than a `Secret`, so this is the one place in the binary that turns a handle into
/// a value. Any other scheme is refused rather than silently resolving to nothing, which would
/// look like an outage instead of a configuration error.
fn env_handle(reference: &CredentialRef, what: &str) -> Result<String, Exit> {
    let name = reference.handle().strip_prefix("env:").ok_or_else(|| {
        fail(USAGE, format!("{what} handle {} is not an env: handle", reference.handle()))
    })?;
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| fail(USAGE, format!("{what} handle names {name}, which is unset or empty")))
}

/// Verify one tenant from its registry entry.
///
/// Everything that differs between tenants comes from the entry — service, guidance, and both
/// credential handles. Everything shared comes from the environment: one runtime, one record
/// store, one journal. **Each tenant verifies its own scope against the store**, so isolation
/// remains a property of verified evidence rather than of the routing table that chose which
/// tenant to serve.
fn boot_tenant(entry: &ironworks_domain::TenantEntry, journal_path: &str) -> Result<Boot, Exit> {
    let e = |m: String| fail(FAILED, m);
    let base = std::env::var("IRONWORKS_BASE")
        .map_err(|_| fail(USAGE, "IRONWORKS_BASE is not set (the runtime base URL)"))?;
    let store = std::env::var("IRONWORKS_RECORDS")
        .map_err(|_| fail(USAGE, "IRONWORKS_RECORDS is not set (the record store)"))?;

    let pin = ironworks_domain::ModelPin::parse(
        &std::fs::read_to_string("MODEL_PIN")
            .map_err(|x| e(format!("cannot read MODEL_PIN: {x}")))?,
    )
    .map_err(|x| e(x.to_string()))?;

    let service = entry.service().as_str().to_owned();
    let definition = ironworks_services::service::load(&service).map_err(|x| e(x.to_string()))?;
    let guidance_text = std::fs::read_to_string(entry.guidance().as_str())
        .map_err(|x| e(format!("cannot read {}: {x}", entry.guidance().as_str())))?;
    let guidance = Guidance::parse(&guidance_text, entry.slug().as_str(), &service)
        .map_err(|x| e(x.to_string()))?;

    let instance = ServiceInstance::new(
        entry.slug().clone(),
        definition,
        guidance,
        RuntimeIdentity::parse(format!("member:{}", entry.slug().as_str()))
            .map_err(|x| e(x.to_string()))?,
        entry.runtime().clone(),
        ContextBinding::new(entry.records().clone()).map_err(|x| e(x.to_string()))?,
        ModelSelection::from_pin(&pin),
        CapabilityProfile::READ_ONLY_RECORDS,
        None,
        &ironworks_services::Heap,
    )
    .map_err(|x| e(x.to_string()))?;

    let book = Book::new(&store, EnvCredentials);
    let verified = instance.verify(&book).map_err(|x| e(x.to_string()))?;
    eprintln!(
        "relay: tenant {} scope {} verified by {} (evidence: record store)",
        entry.slug().as_str(),
        verified.org(),
        store
    );

    let journal = Journal::open(journal_path).map_err(|x| e(x.to_string()))?;
    let scope = verified.thread_scope();
    let runtime = Ironclaw::new(
        base,
        ironworks_ironclaw::Credential::new(env_handle(entry.runtime(), "the runtime")?),
    );
    Ok(Boot { verified, book, journal, scope, runtime, journal_path: journal_path.to_owned() })
}

/// One question, answered on the terminal. A successful stdout write is its delivery evidence.
fn run_turn(b: &Boot, args: &Args, ask: &str) -> Result<(), Exit> {
    let e = |m: String| fail(FAILED, m);
    // A one-shot command has no update to key from, so it adopts whatever this instance left
    // unfinished. Recovery comes first, and only from durable evidence.
    let unresolved: Vec<_> = b
        .journal
        .unresolved()
        .map_err(|x| e(x.to_string()))?
        .into_iter()
        .filter(|r| r.instance == *b.scope.slug())
        .collect();
    let key = match unresolved.first() {
        Some(record) => {
            eprintln!(
                "ironworks: recovering {} from journal evidence ({:?})",
                record.key.as_str(),
                record.outcome
            );
            record.key.clone()
        }
        None => IdempotencyKey::parse(mint_key()).map_err(|x| e(x.to_string()))?,
    };
    // No conversation: an answer printed to a terminal is not a room anything can continue in.
    let answer = drive(b, args, &key, ask, None)?;
    if let Err(write_error) = write_terminal_answer(&answer.text) {
        // A failed write can still have placed a prefix in a pipe or terminal. Preserve that
        // ambiguity instead of calling it unsent and licensing an automatic duplicate.
        b.journal
            .record_delivery_attempt(
                &key,
                ironworks_domain::AttemptOutcome::Ambiguous { acknowledged_chunks: 0 },
                &now_iso(),
            )
            .map_err(|x| e(x.to_string()))?;
        return Err(e(format!("could not write the answer to stdout: {write_error}")));
    }

    // Deterministic B5 proof: the bytes reached stdout, but the durable delivery fact did not.
    // Recovery may print an identical duplicate; it must never execute a new model turn.
    crash_if(args, "after-output");

    let delivery = b
        .journal
        .record_delivery_attempt(&key, ironworks_domain::AttemptOutcome::Complete, &now_iso())
        .map_err(|x| e(x.to_string()))?;
    eprintln!("ironworks: terminal delivery={delivery}");
    Ok(())
}

fn write_terminal_answer(answer: &str) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write_answer_to(&mut out, answer)
}

fn write_answer_to(out: &mut impl Write, answer: &str) -> io::Result<()> {
    writeln!(out, "{answer}")?;
    out.flush()
}

/// One poll cycle against a channel: acknowledge, take the batch, answer it, deliver it.
///
/// Deliberately **one cycle, then exit**. A daemon would need a crash point to be a signal at a
/// moment; a one-shot makes every boundary a separate process that a script starts and kills, so
/// "across a real restart" means what it says.
fn run_relay(
    args: &Args,
    channel_url: &str,
    registry_path: &str,
    chunk_bytes: usize,
) -> Result<(), Exit> {
    let e = |m: String| fail(FAILED, m);

    // ── who is served, decided before anything is polled ────────────────────────────
    // The whole registry loads or none of it does. A cycle that began serving three tenants out
    // of four would look exactly like a healthy one to everything downstream.
    let registry_text = std::fs::read_to_string(registry_path)
        .map_err(|x| fail(USAGE, format!("cannot read the registry at {registry_path}: {x}")))?;
    let registry = ironworks_domain::TenantRegistry::parse(&registry_text)
        .map_err(|x| fail(USAGE, format!("registry {registry_path}: {x}")))?;
    let claims = registry.claims();
    eprintln!("relay: {} tenant(s) in {registry_path}", registry.len());

    // Verified instances, built on demand and kept for the cycle. A batch carrying two updates
    // for one tenant must not verify that tenant twice; a batch carrying none for a tenant must
    // not verify it at all, so a tenant whose store is briefly unreachable does not stop the
    // conversations that had no traffic anyway.
    let mut tenants: std::collections::BTreeMap<String, Boot> = std::collections::BTreeMap::new();

    // The process-wide journal, opened here because the cursor spans every tenant.
    let cursor_journal = Journal::open(&args.journal).map_err(|x| e(x.to_string()))?;
    let ch = ironworks_channel_http::Channel::new(channel_url);
    // The same file as the turn journal. Separate tables, one store — two files could disagree in
    // exactly one direction that silently corrupts a live conversation, and there is no reason to
    // buy that risk here.
    let updates_journal =
        ChannelJournal::open(&args.journal).map_err(|x| e(format!("channel journal: {x}")))?;
    // Beside the journal and named after it, so an operator supplies one path and knows where the
    // second file is. It is deliberately NOT a table in that journal: the journal holds
    // identifiers only, and a proposal is the person's words.
    let proposals_path = format!("{}.proposals", args.journal);
    let proposals =
        ProposalStore::open(&proposals_path).map_err(|x| e(format!("proposal store: {x}")))?;

    // ── acknowledge, by asking for what comes after ─────────────────────────────────
    // The offset in the poll IS the acknowledgement; there is no separate call. Which is exactly
    // why an acknowledgement lands on a LATER poll than the delivery it acknowledges.
    let offset = updates_journal.poll_offset(&cursor_journal).map_err(|x| e(x.to_string()))?;
    eprintln!("relay: poll offset={offset}");
    let inbound = ch.poll(offset).map_err(|x| e(x.to_string()))?;
    if let Some(safe) =
        updates_journal.safe_cursor(&cursor_journal).map_err(|x| e(x.to_string()))?
        && let Err(refused) = updates_journal.acknowledge(safe, &cursor_journal)
    {
        // Never fatal: the channel has already been told, and a barrier refusal here means a
        // slower tenant journalled work in the meantime. Reporting it beats pretending.
        eprintln!("relay: acknowledgement not recorded — {refused}");
    }

    // ── journal the whole batch before any of it is worked on ───────────────────────
    // One offset is shared across tenants, so an update that is not yet on disk cannot hold the
    // barrier. Writing them all first is what makes the barrier apply to work that has not
    // started — and what leaves a crash mid-batch with the rest still queued.
    let rows: Vec<(ironworks_channel::UpdateId, ironworks_domain::ChannelRef)> = inbound
        .iter()
        .map(|u| {
            (ironworks_channel::UpdateId::new(u.id), ironworks_domain::ChannelRef::new(&u.channel))
        })
        .collect();
    updates_journal.note_batch(&rows, &now_iso()).map_err(|x| e(x.to_string()))?;
    eprintln!("relay: batch of {} journalled", rows.len());

    for update in &inbound {
        let id = ironworks_channel::UpdateId::new(update.id);
        let record = updates_journal
            .record(id)
            .map_err(|x| e(x.to_string()))?
            .ok_or_else(|| e(format!("update {id} vanished from the batch")))?;

        // ── routing: whose conversation is this? ────────────────────────────────────
        // Resolved against the claims table rather than looked up in a map: a map would have
        // lost a collision before anyone could ask about it.
        let routed =
            ironworks_domain::resolve(&claims, &ironworks_domain::ChannelRef::new(&update.channel));

        // ── a command, decided before anything is billed ────────────────────────────
        // Before addressing and before a turn is minted. Before addressing, because in a group
        // the addressing rule would otherwise settle a `/share` silently and the refusal has to
        // be in words. Before the turn, because a slash command must not be able to reach a
        // model at all — not by falling through, not by being mistyped.
        //
        // Only `is_command` is asked here. The grammar is not parsed until the sender has been
        // admitted, because every refusal `parse` produces describes the command surface, and an
        // unbound sender is owed nothing about it.
        let is_command = matches!(record.disposition, Disposition::Unclaimed)
            && ironworks_domain::is_command(&update.text);
        if is_command && let Some(slug) = routed.tenant() {
            let entry = registry
                .tenant(slug)
                .ok_or_else(|| e(format!("routed to {slug}, which the registry does not hold")))?;
            answer_command(&ch, &updates_journal, &proposals, entry, update, id, chunk_bytes)?;
            continue;
        }

        // Addressing, asked only of a routed update and only once. In a direct chat every
        // message is for us; in a group almost none are, and a bot that answered each remark
        // would bill a turn per sentence. `NotAddressed` is terminal and costs nothing — the
        // reference's own test for this is named "an unaddressed message is terminal and costs
        // no turn".
        let request = routed.tenant().and_then(|slug| {
            registry.tenant(slug).and_then(|t| t.addressing().addressed(&update.text))
        });

        let key = match &record.disposition {
            Disposition::Unclaimed => match &routed {
                ironworks_domain::RouteState::Present(_) if request.is_none() => {
                    updates_journal
                        .disposition(id, &Disposition::NotAddressed, &now_iso())
                        .map_err(|x| e(x.to_string()))?;
                    eprintln!("relay: update {id} not addressed to us — no turn");
                    continue;
                }
                ironworks_domain::RouteState::Present(_) => {
                    let key = update_key(id)?;
                    updates_journal
                        .disposition(id, &Disposition::Turn(key.clone()), &now_iso())
                        .map_err(|x| e(x.to_string()))?;
                    key
                }
                ironworks_domain::RouteState::Absent => {
                    // Nobody here claims it. Settled, and recorded rather than discarded: an
                    // update for a channel no tenant owns is a configuration defect.
                    updates_journal
                        .disposition(id, &Disposition::Unroutable, &now_iso())
                        .map_err(|x| e(x.to_string()))?;
                    eprintln!("relay: update {id} unroutable (channel {})", update.channel);
                    continue;
                }
                ironworks_domain::RouteState::Ambiguous(a) => {
                    // The registry refuses duplicates at parse, so reaching this means the table
                    // and the file disagree. Left UNCLAIMED on purpose: answering would pick one
                    // tenant's credentials by accident, and dispositioning would settle a
                    // conversation nobody has decided about. It holds the cursor, which is the
                    // alarm.
                    eprintln!("relay: update {id} NEEDS AN OPERATOR — {a}");
                    continue;
                }
            },
            Disposition::Turn(key) => key.clone(),
            other => {
                eprintln!("relay: update {id} already {other:?}");
                continue;
            }
        };

        // Which tenant drives it — asked after the disposition, and separately, because an
        // in-flight turn must never be re-dispositioned by a routing table that has since
        // changed. That is defect E-4 in the reference corpus: an update with a possibly-billed
        // answer, whose tenant had left the registry, recorded as "not addressed to us", its
        // cursor advanced past it, unrecoverable. Routing decides whether to answer; it cannot
        // decide what already happened.
        let Some(slug) = routed.tenant() else {
            eprintln!(
                "relay: update {id} is in flight but its channel no longer routes — NEEDS AN \
                 OPERATOR; it is left as it is rather than relabelled"
            );
            continue;
        };
        let entry = registry
            .tenant(slug)
            .ok_or_else(|| e(format!("routed to {slug}, which the registry does not hold")))?;
        if !tenants.contains_key(slug.as_str()) {
            let booted = boot_tenant(entry, &args.journal)?;
            tenants.insert(slug.as_str().to_owned(), booted);
        }
        let b = &tenants[slug.as_str()];

        // ── has this already been finished? ─────────────────────────────────────────
        // The one question the channel side asks about a tenant's work. A settled update gets no
        // rerun and no resend — only an acknowledgement, which the next poll's offset carries.
        if let Some(prior) = cursor_journal.turn(&key).map_err(|x| e(x.to_string()))?
            && prior.openness() == Openness::Settled
        {
            eprintln!(
                "relay: update {id} already settled (delivery={}) — no rerun, no resend",
                prior.delivery
            );
            continue;
        }

        eprintln!("relay: update {id} -> key {}", key.as_str());
        // The summons is addressing, not content: a model asked "@somebot what changed?" is
        // asked the question, never shown the mention. An in-flight turn whose text no longer
        // addresses us still runs — it was already claimed, and routing cannot decide what
        // already happened.
        let ask = request.as_deref().unwrap_or(update.text.trim());
        let conversation = ironworks_domain::ChannelRef::new(&update.channel);
        let answer = drive(b, args, &key, ask, Some(&conversation))?;

        // ── delivery ────────────────────────────────────────────────────────────────
        // Strip before chunking (§E-14, §E-15): the chunk count must describe the bytes a client
        // actually receives, and a marker must never be split across two messages.
        let chunks = ironworks_channel_http::plain_chunks(&answer.text, chunk_bytes);
        let outcome = ch.send(&update.channel, &chunks);
        eprintln!("relay: sent {} chunk(s), channel reported {outcome:?}", chunks.len());

        crash_if(args, "after-send");

        let delivery = b
            .journal
            .record_delivery_attempt(&key, outcome, &now_iso())
            .map_err(|x| e(x.to_string()))?;
        eprintln!("relay: update {id} delivery={delivery}");

        crash_if(args, "after-delivered");

        if delivery.repair().requires_operator() {
            eprintln!(
                "relay: update {id} NEEDS AN OPERATOR — {}",
                if delivery.repair() == ironworks_domain::Repair::ResendMayDuplicate {
                    "a resend may put a second copy of this answer in the client's room"
                } else {
                    "a resend is safe; nothing reached the client"
                }
            );
        }
    }
    Ok(())
}

/// Answer one deterministic command.
///
/// # What it is allowed to do
///
/// Reply, write at most one proposal, and — on `/confirm` — append exactly one activity. It
/// never runs a model and never fetches a record.
///
/// It does reach the Account Service, and the distinction is worth stating precisely: `/share`
/// resolves the organization its credential authorises and writes nothing; `/confirm` appends the
/// one confirmed note under a separate append credential. Neither reads an account record, and
/// nothing here can update or delete one.
///
/// # The order, and why every step of it is load-bearing
///
/// Admission first, grammar second. `parse`'s refusals describe the command surface — which verbs
/// exist, what `/share` takes, that `/confirm` is coming — and none of that may be shown to a
/// sender this tenant has not bound, or in a room the channel did not attest is private. So the
/// room and the `(channel, actor)` pair are settled from the **registry alone**, before a single
/// byte of command-specific help exists.
///
/// Nothing here boots a tenant. A deterministic refusal must not depend on a runtime credential,
/// a model composition, a turn journal, or the Account Service being reachable — an unknown
/// sender who could force all four by typing a slash would be a denial-of-service surface and a
/// reason for refusals to fail when nothing is wrong with them. Only an **authorized** `/share`
/// resolves the one thing it genuinely needs: the server-backed organization
/// ([`share_scope`]), which still comes from the record-store credential and never from registry
/// prose.
fn answer_command(
    ch: &ironworks_channel_http::Channel,
    updates: &ChannelJournal,
    proposals: &ProposalStore,
    entry: &ironworks_domain::TenantEntry,
    update: &ironworks_channel_http::Inbound,
    id: ironworks_channel::UpdateId,
    chunk_bytes: usize,
) -> Result<(), Exit> {
    let e = |m: String| fail(FAILED, m);
    let conversation = ironworks_domain::ChannelRef::new(&update.channel);

    // ── admission, from the registry and nothing else ────────────────────────────────
    let (actor, contributor) =
        match admit_command(update.chat_kind, update.actor.as_ref(), &conversation, entry) {
            Admission::Refuse { reply, why } => {
                eprintln!("relay: update {id} command refused — {why}");
                return finish_command(
                    ch,
                    updates,
                    proposals,
                    update,
                    id,
                    reply,
                    Lane::None,
                    chunk_bytes,
                );
            }
            Admission::Allow { actor, contributor } => (actor, contributor),
        };

    // ── the grammar, now that the sender is owed an answer about it ──────────────────
    let request = match ironworks_domain::share::parse(&update.text) {
        ironworks_domain::Intent::Refused(why) => {
            eprintln!("relay: update {id} refused — no turn, nothing stored");
            return finish_command(
                ch,
                updates,
                proposals,
                update,
                id,
                &why.to_string(),
                Lane::None,
                chunk_bytes,
            );
        }
        ironworks_domain::Intent::Share(request) => request,
        ironworks_domain::Intent::Confirm => {
            return confirm_command(
                ch,
                updates,
                proposals,
                entry,
                update,
                id,
                actor,
                contributor,
                chunk_bytes,
            );
        }
        // `is_command` admitted it, so this is unreachable; treated as an ordinary message
        // rather than asserted, because a disagreement between the two stages must not panic.
        ironworks_domain::Intent::Conversation => return Ok(()),
    };

    // ── the exact confirmation text, checked whole ───────────────────────────────────
    // Not just the note. The rendered echo also carries the account and the contributor, and the
    // account is a caller-supplied string the record store's grammar does not constrain — so an
    // account of `**A**` would survive a body-only check and then be rewritten in transit. The
    // person would confirm one string having been shown another.
    let echo = ironworks_domain::render_echo(&request, contributor);
    if !ironworks_channel_http::echoes_exactly(&echo) {
        eprintln!(
            "relay: update {id} share refused — the confirmation would not survive the channel"
        );
        return finish_command(
            ch,
            updates,
            proposals,
            update,
            id,
            "I cannot show that back to you exactly as you typed it, so I will not store it. \
             Send it again without markdown formatting in the note or the account.",
            Lane::None,
            chunk_bytes,
        );
    }

    // ── the one authoritative fact a proposal needs ──────────────────────────────────
    // A store that cannot be reached stops THIS update and nothing else. The turn path treats a
    // scope failure as fatal to the whole cycle, which is right there — a turn cannot be built
    // without a verified scope — but wrong here: it would let one tenant's outage swallow the
    // deterministic refusals owed to every update behind it, which is exactly the dependency a
    // refusal must not have.
    let org = match share_scope(entry) {
        Ok(org) => org,
        Err(why) => {
            eprintln!(
                "relay: update {id} left open — {why} Nothing was stored and nothing was sent; \
                 the next cycle tries again."
            );
            return Ok(());
        }
    };
    let stored = proposals
        .propose(
            NewProposal {
                update: id.get(),
                channel: &conversation,
                actor,
                contributor,
                slug: entry.slug(),
                org: &org,
                account: request.account(),
                body: request.body(),
            },
            &now_iso(),
        )
        .map_err(|x| e(format!("proposal store: {x}")))?;
    eprintln!(
        "relay: update {id} proposal for {} by {} — nothing shared",
        stored.account, stored.contributor
    );
    finish_command(ch, updates, proposals, update, id, &echo, Lane::Echo(id.get()), chunk_bytes)
}

/// Promote the newest confirmable proposal into shared, attributed evidence.
///
/// # The order, and why it is this order
///
/// The sender is already admitted — the private `(channel, actor)` pair resolved a contributor
/// before the grammar was parsed, so nothing below reveals anything to someone unauthorized.
/// What remains is to establish, in this sequence, that:
///
/// 1. there is a newest proposal and it is confirmable — provably shown to this person, in this
///    conversation;
/// 2. the read organization is still what it was;
/// 3. this tenant has an append credential at all;
/// 4. every axis of the proposal's authority still matches what was just re-verified.
///
/// Only then is anything appended. Steps 2 and 4 are separate on purpose: 2 asks the store what
/// this tenant's records scope is *now*, and 4 asks whether the stored proposal was made under
/// that same authority. A registry edit or a re-pointed credential fails 4 rather than promoting
/// yesterday's words into an organization they were never meant for.
///
/// **No model, no record fetch.** The append carries the person's exact bytes and nothing else.
#[allow(clippy::too_many_arguments)]
fn confirm_command(
    ch: &ironworks_channel_http::Channel,
    updates: &ChannelJournal,
    proposals: &ProposalStore,
    entry: &ironworks_domain::TenantEntry,
    update: &ironworks_channel_http::Inbound,
    id: ironworks_channel::UpdateId,
    actor: &ironworks_domain::ActorRef,
    contributor: &ironworks_domain::ContributorId,
    chunk_bytes: usize,
) -> Result<(), Exit> {
    let e = |m: String| fail(FAILED, m);
    let conversation = ironworks_domain::ChannelRef::new(&update.channel);

    // ── 1. the newest proposal, and only if it is confirmable ────────────────────────
    // `confirmable` finds the newest whatever its state and returns it only when delivered, so a
    // newer pending or uncertain proposal correctly blocks an older one.
    let org = match share_scope(entry) {
        Ok(org) => org,
        Err(why) => {
            eprintln!("relay: update {id} left open — {why} Nothing was appended.");
            return Ok(());
        }
    };
    let Some(append_credential) = entry.append() else {
        // A tenant with no append handle cannot promote. Not a failure of authorization — a
        // configuration an operator has not made — so it says so plainly.
        eprintln!("relay: update {id} confirm refused — this tenant has no append credential");
        return finish_command(
            ch,
            updates,
            proposals,
            update,
            id,
            "Sharing is not switched on for this workspace yet. Your note is saved and nothing \
             has been shared.",
            Lane::None,
            chunk_bytes,
        );
    };
    let authority = ironworks_proposals::ConfirmAuthority {
        channel: &conversation,
        actor,
        slug: entry.slug(),
        org: &org,
        contributor,
    };
    let Some(confirmable) =
        proposals.confirmable(authority).map_err(|x| e(format!("proposal store: {x}")))?
    else {
        eprintln!("relay: update {id} confirm — nothing confirmable for this sender");
        return finish_command(
            ch,
            updates,
            proposals,
            update,
            id,
            "I have nothing to share for you. Send /share <account> <note> first, and I will \
             show you exactly what would be shared.",
            Lane::None,
            chunk_bytes,
        );
    };
    let proposal = confirmable.proposal().clone();

    // ── a confirmation that already happened ─────────────────────────────────────────
    match &proposal.confirm {
        Confirmation::Committed { activity_id, org: committed, confirm_update } => {
            let reply = if committed_answer(*confirm_update, id.get())
                == CommittedAnswer::CompleteTheReply
            {
                // The append landed and the reply did not. Reconstructing the canonical success
                // text and sending it through `Lane::Reply` is what makes the recovery finish
                // the job rather than repeat it: the lane refuses a second send after
                // `Attempting`, `Delivered` or `Uncertain`, and permits one after a proven
                // rejection.
                eprintln!(
                    "relay: update {id} confirm — {activity_id} is committed; completing the \
                     success reply through its durable lane"
                );
                return finish_command(
                    ch,
                    updates,
                    proposals,
                    update,
                    id,
                    &shared_reply(&proposal, activity_id, committed),
                    Lane::Reply(proposal.update),
                    chunk_bytes,
                );
            } else {
                eprintln!("relay: update {id} confirm — already shared as {activity_id}");
                format!(
                    "Already shared. That note is recorded against {} in {committed} as \
                     {activity_id}, attributed to {}. Nothing was written again.",
                    proposal.account, proposal.contributor
                )
            };
            // A later `/confirm` is its own message with no durable lane of its own. Repeating
            // it costs nothing and stores nothing.
            return finish_command(
                ch,
                updates,
                proposals,
                update,
                id,
                &reply,
                Lane::None,
                chunk_bytes,
            );
        }
        Confirmation::Contested { activity_id, differing_field, .. } => {
            eprintln!(
                "relay: update {id} NEEDS AN OPERATOR — {activity_id} exists with a different \
                 {differing_field}; confirmed evidence is never overwritten"
            );
            return finish_command(
                ch,
                updates,
                proposals,
                update,
                id,
                "Something is already recorded under this note's identity and it does not match. \
                 Nothing was changed, and an operator has to look at it.",
                Lane::None,
                chunk_bytes,
            );
        }
        Confirmation::Refused { activity_id, code, .. } => {
            // A decision the store already made. Sending it again would receive the same answer,
            // so the only thing this can do is repeat what it said the first time.
            eprintln!(
                "relay: update {id} NEEDS AN OPERATOR — the append of {activity_id} was already \
                 refused ({code}); it is not retried"
            );
            return finish_command(
                ch,
                updates,
                proposals,
                update,
                id,
                REFUSED_REPLY,
                Lane::None,
                chunk_bytes,
            );
        }
        Confirmation::Appending { activity_id, .. } => {
            // A previous attempt left no durable outcome. The retry below is the identical
            // request, which the store is idempotent on — so this is a replay, not a second
            // write. Nothing else in this function may run twice, so it jumps straight to it.
            eprintln!("relay: update {id} confirm — replaying the identical append {activity_id}");
        }
        Confirmation::Unconfirmed => {}
    }

    // ── what will actually be sent ───────────────────────────────────────────────────
    //
    // Derived once when unconfirmed and PERSISTED; read back from the persisted intent on every
    // replay. That order is the point: a replay is only a replay at an idempotent endpoint if it
    // carries the same bytes, and re-deriving them would make the request a function of this
    // binary's arithmetic rather than of what was recorded.
    //
    // The derivation still runs on a replay — as a drift guard, never as a source. If it
    // disagrees with what was persisted, something changed under an append that may already have
    // landed, and the only safe answer is to stop: appending the new values could put one
    // person's words in twice under two identities, and appending the old ones while believing
    // the new ones would make the record and this process disagree about what happened.
    let derived_id = ironworks_domain::share::activity_id(&org, &proposal.account, proposal.update);
    let derived_date = proposal.proposed_at.get(..10).unwrap_or_default().to_owned();
    if derived_date.len() != 10 {
        return Err(e(format!(
            "proposal {} carries an unusable timestamp {:?}",
            proposal.update, proposal.proposed_at
        )));
    }
    let (activity_id, occurred_at) = match &proposal.confirm {
        Confirmation::Appending { activity_id, occurred_at, .. } => {
            for (what, persisted, derived) in [
                ("activity id", activity_id, &derived_id),
                ("occurrence date", occurred_at, &derived_date),
            ] {
                if persisted != derived {
                    eprintln!(
                        "relay: update {id} NEEDS AN OPERATOR — an append is in flight whose \
                         {what} is {persisted:?} but this build derives {derived:?}. Nothing was \
                         sent: replaying the derived value could write the same words twice, and \
                         the persisted one is what may already be in the store."
                    );
                    return Ok(());
                }
            }
            // Persisted, not derived. They are equal here — the guard above proved it — and
            // taking them from the row is what keeps that true if the guard ever loosens.
            (activity_id.clone(), occurred_at.clone())
        }
        _ => (derived_id, derived_date),
    };

    // ── the durable append intent, before the request leaves ─────────────────────────
    if proposal.confirm.may_begin() {
        proposals
            .begin_append(proposal.update, &activity_id, &occurred_at, id.get())
            .map_err(|x| e(format!("proposal store: {x}")))?;
    }

    let store = std::env::var("IRONWORKS_RECORDS")
        .map_err(|_| fail(USAGE, "IRONWORKS_RECORDS is not set (the record store)"))?;
    let client = AppendClient::new(&store, EnvCredentials);
    let outcome = client.append(
        append_credential,
        &Append {
            activity_id: &activity_id,
            account: &proposal.account,
            occurred_at: &occurred_at,
            body: &proposal.body,
            contributor: &proposal.contributor,
            expected_org: &org,
        },
    );

    match outcome {
        Ok(appended) => {
            let committed = appended.org().clone();
            proposals
                .record_commit(proposal.update, appended.activity_id(), &committed)
                .map_err(|x| e(format!("proposal store: {x}")))?;
            eprintln!(
                "relay: update {id} SHARED {} for {} by {} ({})",
                appended.activity_id(),
                proposal.account,
                proposal.contributor,
                match appended {
                    Appended::Created { .. } => "created",
                    Appended::Replayed { .. } => "replayed",
                }
            );
            let reply = shared_reply(&proposal, appended.activity_id(), &committed);
            finish_command(
                ch,
                updates,
                proposals,
                update,
                id,
                &reply,
                // The success reply gets the durable lane: telling someone twice that their note
                // was shared reads as two shares, and nothing they can see would say otherwise.
                Lane::Reply(proposal.update),
                chunk_bytes,
            )
        }
        Err(AppendFailure::Conflict { activity_id: contested, differing_field }) => {
            proposals
                .record_contest(proposal.update, &contested, &differing_field)
                .map_err(|x| e(format!("proposal store: {x}")))?;
            eprintln!(
                "relay: update {id} NEEDS AN OPERATOR — {contested} exists with a different \
                 {differing_field}; nothing was overwritten"
            );
            finish_command(
                ch,
                updates,
                proposals,
                update,
                id,
                "Something is already recorded under this note's identity and it does not match. \
                 Nothing was changed, and an operator has to look at it.",
                Lane::None,
                chunk_bytes,
            )
        }
        Err(why) if why.may_retry_identical() => {
            // The store established nothing. The proposal stays `appending`, so the next
            // `/confirm` replays the identical request rather than composing a new one — and
            // the update is left open so the channel brings it back.
            eprintln!(
                "relay: update {id} left open — the append could not be completed: {why}. The \
                 identical request will be replayed."
            );
            Ok(())
        }
        Err(why) => {
            // A decision the store made: unauthorized, unknown account, a shape it will not take.
            // Permanent, and now RECORDED as permanent.
            //
            // It used to be left `appending`, on the argument that this stopped a second attempt
            // starting behind it. It did not: `appending` is the state that says replay the
            // identical request, so the next `/confirm` re-sent a request the store had already
            // decided about, received the same refusal, and said so again — for as long as anyone
            // kept typing. `record_refusal` makes the decision durable, which is what actually
            // stops the replay, and keeps `appending` meaning the one thing it must mean.
            let code = append_refusal_code(&why);
            proposals
                .record_refusal(proposal.update, &activity_id, &code)
                .map_err(|x| e(format!("proposal store: {x}")))?;
            eprintln!(
                "relay: update {id} NEEDS AN OPERATOR — the append was refused ({code}): {why}. \
                 Nothing was written and nothing will be retried."
            );
            finish_command(
                ch,
                updates,
                proposals,
                update,
                id,
                REFUSED_REPLY,
                Lane::None,
                chunk_bytes,
            )
        }
    }
}

/// What a `/confirm` that finds an already-committed proposal is owed.
///
/// # The crash this decides
///
/// The append lands, `record_commit` makes it durable, and the process dies before the success
/// reply goes out. The channel redelivers **the same update**, and it must finish the job — send
/// the reply it never sent — rather than answer as though someone had confirmed twice. Getting
/// that wrong is the defect: the "already shared" answer went out through no durable lane at all,
/// so a crash anywhere after the commit produced a message that could repeat forever.
///
/// The persisted `confirm_update` is what separates the two cases, and it is why it is persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommittedAnswer {
    /// This is the original confirmation, recovering. Send the canonical success reply through
    /// its durable lane, which sends once and refuses to repeat.
    CompleteTheReply,
    /// A genuinely later `/confirm`, from a person asking again. Its own message, its own answer,
    /// no durable state, and nothing written.
    AlreadyShared,
}

fn committed_answer(confirm_update: u64, this_update: u64) -> CommittedAnswer {
    if confirm_update == this_update {
        CommittedAnswer::CompleteTheReply
    } else {
        CommittedAnswer::AlreadyShared
    }
}

/// What a person is told when the store refused the append outright.
///
/// One sentence, said the same way whether the refusal happens now or is found already recorded,
/// for the reason [`shared_reply`] is one function: a person meeting the second must not be able
/// to tell it came from a different path. It names no code — `unauthorized` is an operator's word
/// about an operator's mistake, and telling a contributor their workspace credential is wrong
/// helps nobody who can act on it.
const REFUSED_REPLY: &str = "I could not share that note, and nothing was written. An operator \
                             has to look at it.";

/// The store's own word for what it refused, for the operator queue.
///
/// A closed classification rather than the error's prose: these strings are durable, and
/// [`AppendFailure`]'s `Unreachable`, `Malformed` and `NoCredential` variants interpolate adapter
/// text that can carry a URL, a credential handle or a response body — the same hazard
/// `ironworks_lifecycle::attempt::Blocked` exists to keep out of the confinement ledger. The rich
/// reason still reaches the operator's terminal beside it, where it is useful and ephemeral.
fn append_refusal_code(why: &AppendFailure) -> String {
    match why {
        AppendFailure::Unauthorized => "unauthorized".to_owned(),
        AppendFailure::UnknownAccount { .. } => "unknown_account".to_owned(),
        AppendFailure::ScopeDrift { .. } => "scope_drift".to_owned(),
        AppendFailure::WrongPurpose { .. } => "wrong_credential_purpose".to_owned(),
        AppendFailure::NoCredential { .. } => "no_credential".to_owned(),
        AppendFailure::Malformed { .. } => "unreadable_answer".to_owned(),
        // The one variant carrying a word the STORE chose. Bounded and stripped of anything that
        // is not an identifier, because it is about to become a durable column.
        AppendFailure::Refused { code } => {
            let clean: String = code
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == ' ')
                .take(64)
                .collect();
            let clean = clean.trim();
            if clean.is_empty() { "refused".to_owned() } else { clean.to_owned() }
        }
        // Neither reaches here: `Conflict` is handled above and `Unreachable` is the retryable
        // class. Named rather than defaulted so a rearrangement of the match cannot silently file
        // one of them as a permanent refusal.
        AppendFailure::Conflict { .. } => "conflict".to_owned(),
        AppendFailure::Unreachable { .. } => "unreachable".to_owned(),
    }
}

/// The canonical success reply for a committed promotion.
///
/// One function, because it is sent from two places — the confirmation that commits, and the
/// recovery that finds a commit whose reply never went out — and a person who received the
/// second must not be able to tell it came from a different code path.
fn shared_reply(
    proposal: &ironworks_proposals::Proposal,
    activity_id: &str,
    org: &ironworks_domain::OrgId,
) -> String {
    format!(
        "Shared. That note is now recorded against {} in {org} as {activity_id}, attributed to \
         {}. It cannot be edited or removed from here.",
        proposal.account, proposal.contributor
    )
}

/// The organization a proposal is made in, resolved server-side from the tenant's record-store
/// credential.
///
/// # Why this is not `boot_tenant`
///
/// `boot_tenant` builds a whole verified [`ServiceInstance`]: it reads `MODEL_PIN`, loads the
/// service definition and the tenant's guidance, resolves the runtime credential and constructs
/// an IronClaw client. A proposal needs none of that — it runs no turn — and making one depend on
/// all of it would mean a promotion fails when the model pin is unreadable, and would resolve a
/// runtime credential for an operation that never authenticates to the runtime.
///
/// What it does **not** relax is where the organization comes from. This is the same
/// [`ScopeResolver`] call `verify` makes, against the same record-store credential, so the org a
/// proposal records is still the store's answer and never the registry's prose.
///
/// [`ScopeResolver`]: ironworks_domain::ScopeResolver
/// `Err` carries the operator's sentence. It is a reason to abandon **one** update, never the
/// cycle: see the call site.
fn share_scope(entry: &ironworks_domain::TenantEntry) -> Result<ironworks_domain::OrgId, String> {
    let store = std::env::var("IRONWORKS_RECORDS")
        .map_err(|_| "IRONWORKS_RECORDS is not set (the record store).".to_owned())?;
    let book = Book::new(&store, EnvCredentials);
    let evidence =
        ironworks_domain::ScopeResolver::resolve(&book, entry.records()).map_err(|x| {
            format!(
                "REFUSING TO PROPOSE: the organization behind {}'s records could not be \
                 established ({x}). A proposal names the organization it would be promoted into, \
                 and guessing it is the failure this refuses to have.",
                entry.slug()
            )
        })?;
    Ok(evidence.org().clone())
}

/// What an actor this tenant has not bound is told. It names nothing.
///
/// Not silence, which reads as a broken bot, and not a fuller sentence either: an unbound actor
/// must not learn that a promotion feature exists here, which account ids are real, or that they
/// were one registry line away from being able to use it. It is the answer to every command they
/// might type, so it cannot describe any of them.
const UNAUTHORIZED: &str = "I cannot do that for you here.";

/// Whether a command may be answered at all, decided from attested evidence and the registry.
///
/// Pure, and asked **before** the command grammar is parsed, because the order of these questions
/// is the whole of the disclosure rule. A group is refused in words — someone typed a command and
/// is owed an answer about why nothing happened — and everything after that is answered blandly,
/// so an unbound sender learns nothing about the tenant they are not part of, whatever they typed.
#[derive(Debug, PartialEq, Eq)]
enum Admission<'entry, 'actor> {
    /// Answer in words and store nothing. `why` is for the operator's log, `reply` for the room.
    Refuse { reply: &'static str, why: &'static str },
    /// Answer the command, attributing anything it stores to this contributor.
    ///
    /// Both, because the pair is what was established: the actor owns the proposal and the
    /// contributor is what a promoted record will carry, and handing back only one would make the
    /// caller re-derive the other from the evidence this just checked.
    Allow {
        actor: &'actor ironworks_domain::ActorRef,
        contributor: &'entry ironworks_domain::ContributorId,
    },
}

fn admit_command<'entry, 'actor>(
    chat_kind: ironworks_domain::ChatKind,
    actor: Option<&'actor ironworks_domain::ActorRef>,
    conversation: &ironworks_domain::ChannelRef,
    entry: &'entry ironworks_domain::TenantEntry,
) -> Admission<'entry, 'actor> {
    // First, and before anything about who is asking: a room whose privacy the channel did not
    // attest is a room this must not act in, however well-known the sender. Malformed commands
    // included — a `/share` with no account is still a share attempt in a group.
    if !chat_kind.is_attested_private() {
        return Admission::Refuse {
            reply: "Sharing works only in a direct message with me, not in a group.",
            why: "the room is not an attested private chat",
        };
    }
    let Some(actor) = actor else {
        return Admission::Refuse { reply: UNAUTHORIZED, why: "the channel attested no sender" };
    };
    // The pair, never the actor alone. An id lifted from one person's chat and replayed in a
    // colleague's matches no binding, because the key never described it.
    let Some(contributor) = entry.contributor(conversation, actor) else {
        return Admission::Refuse {
            reply: UNAUTHORIZED,
            why: "this conversation and sender are not a contributor this tenant bound",
        };
    };
    Admission::Allow { actor, contributor }
}

/// Which durable delivery lane a reply belongs to.
///
/// Two messages in this flow must never be sent twice by accident — the proposal echo and the
/// confirmation's success reply — and they need separate state because a proposal has one of
/// each. Rather than two near-identical send paths, one function takes the lane, so the ordering
/// rule (durable intent, send, durable outcome, settle) has exactly one implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lane {
    /// A refusal or an informational answer. Nothing durable, nothing to duplicate but words.
    None,
    /// The proposal echo: the bytes a person is about to confirm.
    Echo(u64),
    /// The confirmation's success reply: the message that says evidence was written.
    ///
    /// Sending this twice would tell someone their note was shared twice. It was not — the
    /// append is idempotent — but a person reading two confirmations has no way to know that.
    Reply(u64),
}

impl Lane {
    fn proposal(self) -> Option<u64> {
        match self {
            Self::None => None,
            Self::Echo(update) | Self::Reply(update) => Some(update),
        }
    }
    fn state(self, proposal: &ironworks_proposals::Proposal) -> ironworks_proposals::Echo {
        match self {
            Self::Reply(_) => proposal.reply,
            _ => proposal.echo,
        }
    }
    fn what(self) -> &'static str {
        match self {
            Self::Reply(_) => "confirmation",
            _ => "proposal",
        }
    }
    fn begin(
        self,
        proposals: &ProposalStore,
        update: u64,
    ) -> Result<(), ironworks_proposals::ProposalError> {
        match self {
            Self::Reply(_) => proposals.begin_reply(update),
            _ => proposals.begin_echo(update),
        }
    }
    fn record(
        self,
        proposals: &ProposalStore,
        update: u64,
        outcome: ironworks_domain::AttemptOutcome,
    ) -> Result<ironworks_proposals::Echo, ironworks_proposals::ProposalError> {
        match self {
            Self::Reply(_) => proposals.record_reply(update, outcome),
            _ => proposals.record_echo(update, outcome, &now_iso()),
        }
    }
}

/// Send a command's reply, record what that established, and settle the update.
///
/// One place, so the ordering rule cannot drift between the reply paths.
///
/// # The order, for a reply that carries a proposal
///
/// **Durable intent, then send, then durable outcome, then settle.** The intent is what closes
/// the duplicate window: a crash between an accepted send and a recorded outcome leaves
/// [`Echo::Attempting`], which never sends again, instead of a pending proposal a redelivery
/// would echo a second time. The cost is conservative false ambiguity — a send that never left
/// still needs a person — and that is the direction this must fail in.
///
/// A redelivery therefore consults the stored state before sending anything, and settles the
/// update when the state proves no retry is owed.
///
/// A refusal carries no proposal and no durable state. A crash between sending one and settling
/// the update repeats the refusal on redelivery, which is harmless: it stores nothing, bills
/// nothing, and telling someone twice that a command does not exist is better than never telling
/// them at all.
#[allow(clippy::too_many_arguments)]
fn finish_command(
    ch: &ironworks_channel_http::Channel,
    updates: &ChannelJournal,
    proposals: &ProposalStore,
    update: &ironworks_channel_http::Inbound,
    id: ironworks_channel::UpdateId,
    text: &str,
    lane: Lane,
    chunk_bytes: usize,
) -> Result<(), Exit> {
    let e = |m: String| fail(FAILED, m);

    // ── may this be sent at all? ─────────────────────────────────────────────────────
    // Asked of the store rather than assumed from having just written the proposal, because a
    // redelivery arrives here with a proposal that already exists in some state.
    if let Some(proposal) = lane.proposal() {
        let stored =
            proposals
                .proposal(proposal)
                .map_err(|x| e(format!("proposal store: {x}")))?
                .ok_or_else(|| e(format!("proposal {proposal} vanished after being written")))?;
        let state = lane.state(&stored);
        if !state.may_attempt() {
            if state.needs_operator() {
                eprintln!(
                    "relay: update {id} NEEDS AN OPERATOR — a previous attempt to send this \
                     {} left no durable outcome ({state:?}). It is preserved and NOT re-sent; a \
                     resend may put a second copy in the room.",
                    lane.what()
                );
            } else {
                eprintln!("relay: update {id} {} already sent — not sent again", lane.what());
            }
            // Settled either way: the retry it would hold the cursor for is one that must not
            // happen, so leaving it open would stop every conversation behind it forever.
            updates
                .disposition(id, &Disposition::Command, &now_iso())
                .map_err(|x| e(x.to_string()))?;
            return Ok(());
        }
        // The durable send intent. Everything after this point is safe to crash in.
        lane.begin(proposals, proposal).map_err(|x| e(format!("proposal store: {x}")))?;
    }

    let chunks = ironworks_channel_http::plain_chunks(text, chunk_bytes);
    let outcome = ch.send(&update.channel, &chunks);
    eprintln!("relay: update {id} reply sent, channel reported {outcome:?}");

    if let Some(proposal) = lane.proposal() {
        let echo = lane
            .record(proposals, proposal, outcome)
            .map_err(|x| e(format!("proposal store: {x}")))?;
        if echo.needs_operator() {
            eprintln!(
                "relay: update {id} NEEDS AN OPERATOR — the {} was stored but the channel could \
                 not confirm showing it. It is preserved and NOT re-sent; a resend may put a \
                 second copy in the room.",
                lane.what()
            );
        }
    }

    // A reply the channel PROVED never left is the one outcome that licenses a free retry: the
    // person has seen nothing. Leaving the update unclaimed is how the retry happens — the
    // channel redelivers it, and the proposal is found by its update id rather than remade.
    if outcome == ironworks_domain::AttemptOutcome::Rejected {
        eprintln!("relay: update {id} reply provably not delivered — left open for redelivery");
        return Ok(());
    }
    updates.disposition(id, &Disposition::Command, &now_iso()).map_err(|x| e(x.to_string()))?;
    Ok(())
}

/// Carry out an operator's decision to send a stored answer again.
///
/// # Why this is a command and not a retry
///
/// Delivery evidence that is anything other than *provably nothing was sent* means a resend may
/// put a second copy of an answer in a client's room. That is not a decision a process makes on
/// its own — the reference implementation asserts against exactly this, *"a permanent rejection
/// was retried without operator action"* — so the automatic path stops and this command is the
/// only way past it.
///
/// **It never runs a model.** The answer is fetched by the response id the journal already holds,
/// which is also what makes a resend byte-identical to the original rather than a second opinion.
fn run_redeliver(b: &Boot, channel_url: &str, update: u64, chunk_bytes: usize) -> Result<(), Exit> {
    let e = |m: String| fail(FAILED, m);
    let id = ironworks_channel::UpdateId::new(update);
    let updates_journal =
        ChannelJournal::open(&b.journal_path).map_err(|x| e(format!("channel journal: {x}")))?;
    let record = updates_journal
        .record(id)
        .map_err(|x| e(x.to_string()))?
        .ok_or_else(|| e(format!("update {id} is not journalled")))?;
    let Disposition::Turn(key) = &record.disposition else {
        return Err(e(format!(
            "update {id} never became a turn ({:?}); there is no stored answer to send",
            record.disposition
        )));
    };
    let turn = b
        .journal
        .turn(key)
        .map_err(|x| e(x.to_string()))?
        .ok_or(e(format!("update {id} points at turn {} which is not journalled", key.as_str())))?;

    // Only a parked delivery may be redelivered. Refusing the other two is the point: `Pending`
    // belongs to the automatic path, and `Delivered` would be a duplicate created by this command
    // rather than by a crash.
    if !turn.delivery.repair().requires_operator() {
        return Err(e(format!(
            "update {id} has delivery {} — nothing here needs an operator. Redelivering would \
             create the duplicate this command exists to avoid.",
            turn.delivery
        )));
    }
    let Some(response) = turn.outcome.response().cloned() else {
        return Err(e(format!(
            "update {id} has no response id, so there is no stored answer to send. This turn \
             needs reconciliation against the runtime, not a resend."
        )));
    };

    eprintln!(
        "redeliver: update {id} delivery={} — operator-authorised resend of {response}",
        turn.delivery
    );
    if turn.delivery.repair() == ironworks_domain::Repair::ResendMayDuplicate {
        eprintln!("redeliver: WARNING — a client may already be able to read part of this answer");
    }

    let deadline = Deadline::starting_now(Duration::from_secs(180));
    let outcome = fetch_answer(&b.runtime, &response, &deadline)?;
    let Outcome::Completed { text, .. } = outcome else {
        return Err(e(format!("response {response} is not completed; there is nothing to send")));
    };

    let ch = ironworks_channel_http::Channel::new(channel_url);
    // The same transformation as the original send, and deterministic — so a redelivery is the
    // same bytes rather than a differently-formatted second version of the answer.
    let chunks = ironworks_channel_http::plain_chunks(&text, chunk_bytes);
    let attempt = ch.send(&record.channel.to_string(), &chunks);
    let delivery = b
        .journal
        .record_delivery_attempt(key, attempt, &now_iso())
        .map_err(|x| e(x.to_string()))?;
    eprintln!("redeliver: sent {} chunk(s), channel reported {attempt:?}", chunks.len());
    eprintln!("redeliver: update {id} delivery={delivery}");
    Ok(())
}

/// One update, one logical turn.
///
/// Deriving the key from the update id rather than minting a fresh one is what makes a
/// *redelivered* update find the turn it already started, instead of starting a second one.
///
/// # The assumption this rests on
///
/// **A channel never reuses an update id.** Telegram numbers them monotonically per bot, so the
/// mapping is total — but it is an assumption about a foreign system, not a property of this
/// code, and it fails loudly rather than quietly: a reused id produces a `409` from the runtime,
/// which [`drive`] turns into an operator alarm rather than a retry loop. The alternative,
/// minting a fresh key per attempt, would make every redelivery a second billed turn.
fn update_key(id: ironworks_channel::UpdateId) -> Result<IdempotencyKey, Exit> {
    IdempotencyKey::parse(format!("upd-{id}")).map_err(|x| fail(FAILED, x.to_string()))
}

struct Answer {
    text: String,
}

/// Drive one logical turn to an answer, recovering from durable evidence first.
///
/// Shared by both commands, which is the point: there is exactly one path from a key to an
/// answer, so a channel cannot acquire a second, looser one.
fn drive(
    b: &Boot,
    args: &Args,
    key: &IdempotencyKey,
    ask: &str,
    // Which room this turn continues, when there is one. `turn` answers on a terminal and has
    // none, so it writes no thread: a thread that can never be continued is not a thread.
    conversation: Option<&ironworks_domain::ChannelRef>,
) -> Result<Answer, Exit> {
    let e = |m: String| fail(FAILED, m);
    let deadline = Deadline::starting_now(Duration::from_secs(180));
    // Before anything can return: a recovered turn commits too, and the thread it commits to has
    // to record what composition it was built on.
    let alloy = b.verified.alloy_digest(&ironworks_services::Heap).map_err(|x| e(x.to_string()))?;
    let runtime =
        Counted { inner: &b.runtime, log: args.call_log.as_ref().map(std::path::PathBuf::from) };

    // ── recovery comes first, and only from durable evidence ────────────────────────
    let existing = b.journal.turn(key).map_err(|x| e(x.to_string()))?;
    // ...and only from *this instance's* evidence. A key is `upd-{id}` and names no tenant, while
    // one journal holds every tenant's turns, so a row found by key alone may be another's.
    // Adopting one fetches their response under our credential and commits a thread pointing at
    // it — and `ForeignThread` cannot catch that, because the branch below returns before a turn
    // is built. See `TurnRecord::belongs_to`.
    if let Some(record) = &existing
        && !record.belongs_to(&b.scope)
    {
        return Err(e(format!(
            "REFUSING TO SERVE. Turn {} belongs to {} ({}), not to {} ({}).\n  \
             → A channel was re-pointed at another tenant while this update was in flight. The \
             turn is left exactly as it is: nothing is adopted, re-driven, or relabelled.",
            key.as_str(),
            record.instance,
            record.org,
            b.scope.slug(),
            b.scope.org(),
        )));
    }
    let (retrieved_at, plan) = match &existing {
        Some(record) => match recover(record) {
            Recovery::Fetch { response } | Recovery::Deliver { response } => {
                // A known response is fetched. `create` is never called again.
                eprintln!("ironworks: fetching known response {response} (no create)");
                let outcome = fetch_answer(&runtime, &response, &deadline)?;
                return finish(
                    &b.journal,
                    &b.scope,
                    key,
                    outcome,
                    &response,
                    ThreadCommit { conversation, alloy: &alloy, supplied: None },
                );
            }
            Recovery::Dispatch { retrieved_at, .. }
            | Recovery::ReplayIdentical { retrieved_at, .. } => {
                (retrieved_at, Some(record.clone()))
            }
            // The delivery half is not this function's to decide. Re-driving a turn whose answer
            // has reached a client — or may have — is the duplicate this whole layer exists to
            // prevent, so it refuses rather than guessing.
            Recovery::Settled { delivery } => {
                return Err(e(format!(
                    "REFUSING TO SERVE. Turn {} has already been delivered ({delivery}); there is \
                     nothing to add and it will not be re-driven.",
                    key.as_str()
                )));
            }
            Recovery::ReconcileDelivery { delivery, repair, response } => {
                return Err(e(format!(
                    "REFUSING TO SERVE. Turn {} has an unresolved delivery ({delivery}) and needs \
                     an operator: {}\n  → response id: {}",
                    key.as_str(),
                    if repair == ironworks_domain::Repair::ResendMayDuplicate {
                        "a resend may put a second copy of this answer in the client's room"
                    } else {
                        "a resend is safe; nothing reached the client"
                    },
                    response.map_or_else(|| "none recorded".to_owned(), |r| r.to_string()),
                )));
            }
            Recovery::OperatorRequired { reason, response } => {
                return Err(e(format!(
                    "REFUSING TO SERVE. Turn {} needs an operator: {reason}\n  → response id: {}",
                    key.as_str(),
                    response.map_or_else(|| "none recorded".to_owned(), |r| r.to_string()),
                )));
            }
        },
        None => (now_iso(), None),
    };

    // ── the data source: it degrades, honestly ──────────────────────────────────────
    let (records, records_state) = fetch_records(&b.book, &b.verified.record_scope())?;

    // ── continuation, decided before the turn is built ──────────────────────────────
    // Whether to chain is an input to building the turn, so the digest has to be known first.
    // A recovered turn is NOT re-decided: it already chose, its body is journalled, and a second
    // decision could produce a body the idempotency key no longer names.
    let mut prior_supplied = None;
    let previous = match (&plan, conversation) {
        // A recovered turn REPLAYS what it chose. This used to fall through to `None`, on the
        // reasoning that a recovered turn must not be re-decided — which is right, but deciding
        // nothing is not the same as not deciding. Reconstructing a chained turn as fresh
        // produced a different body, so its digest could never match and its replay was refused
        // forever (`docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`).
        //
        // The pointer is rebuilt through this tenant's own scope rather than stored whole, so a
        // pointer naming another tenant stays unrepresentable rather than merely unwritten.
        (Some(record), _) => {
            let p = record.decision.previous.as_ref().map(|r| b.scope.pointer(r.clone()));
            match &p {
                Some(p) => eprintln!("thread: replaying the chain from {}", p.response().as_str()),
                None => eprintln!("thread: replaying a fresh turn"),
            }
            p
        }
        (None, Some(room)) => {
            let prior = b.journal.prior_thread(&b.scope, room).map_err(|x| e(x.to_string()))?;
            prior_supplied = prior.as_ref().and_then(|p| p.supplied.clone());
            let decision = ironworks_domain::decide(prior.as_ref(), &alloy, thread_limit());
            match &decision {
                ironworks_domain::Continuation::Fresh(why) => eprintln!("thread: fresh — {why}"),
                ironworks_domain::Continuation::Chain(p) => {
                    eprintln!("thread: continuing {}", p.response().as_str())
                }
            }
            decision.previous().cloned()
        }
        // No room to continue: `ironworks turn` answers on a terminal and has no conversation.
        (None, None) => None,
    };

    // ── supply: does this conversation already carry the book? ──────────────────────
    // Only a chained turn can, and only while the book has not moved under it. The fingerprint
    // is over the rendered records, so "moved" means what a reader would mean by it.
    // The tenant's declared fact policy, from the verified instance that owns its guidance.
    // `build` below reads it from that same instance rather than being handed a copy, so the
    // digest and the envelope cannot be rendered under two different declarations.
    let fact_fields = b.verified.fact_fields();
    let catalog = records_state
        .has_records()
        .then(|| ironworks_domain::supply::fingerprint(&records, Some(fact_fields)));
    // Replayed on recovery for the same reason the pointer is: the size of the envelope is part
    // of the body, so re-deciding it is another way to rebuild something the key does not name.
    let supply = match &plan {
        Some(record) => match record.decision.carried {
            Some(records) => ironworks_domain::Supply::CarriedByThread { records },
            None => ironworks_domain::supply::decide(false, None, None),
        },
        None => ironworks_domain::supply::decide(
            previous.is_some(),
            prior_supplied.as_ref(),
            catalog.as_deref(),
        ),
    };
    let (records, records_state) = match &supply {
        ironworks_domain::Supply::Full(why) => {
            if previous.is_some() {
                eprintln!("supply: full — {why}");
            }
            (records, records_state)
        }
        ironworks_domain::Supply::CarriedByThread { records: n } => {
            eprintln!("supply: carried by the thread — {n} record(s) already in this conversation");
            (Vec::new(), RecordsState::CarriedByThread { records: *n as usize })
        }
    };
    // What the thread carries after this turn. `None` when nothing was supplied and nothing is
    // known to supply, which leaves whatever an earlier turn established.
    let mark = match (&supply, &catalog) {
        (ironworks_domain::Supply::Full(_), Some(c)) => Some(ironworks_domain::SuppliedMark {
            catalog: c.clone(),
            records: records.len() as u32,
        }),
        (ironworks_domain::Supply::CarriedByThread { records: n }, Some(c)) => {
            Some(ironworks_domain::SuppliedMark { catalog: c.clone(), records: *n })
        }
        _ => None,
    };

    // Captured before `build` consumes the pointer: this is half of what the journal must carry
    // for a recovering process to rebuild the same body.
    let chained_from = previous.as_ref().map(|p| p.response().clone());
    let turn = build(&b.verified, ask, &records, &records_state, &retrieved_at, previous)?;
    let bytes = request_bytes(&turn).map_err(|x| e(x.to_string()))?;
    let digest = RequestDigest::of(&bytes);

    match &plan {
        // The book could not be read, so a mismatch is not a fact about this turn — it is a fact
        // about the store. Refuse WITHOUT settling: the next cycle re-reads and decides then.
        Some(record)
            if matches!(
                rebuild_verdict(record, &digest, &records_state),
                Rebuild::Unestablished(_)
            ) =>
        {
            return Err(fail(
                FAILED,
                format!(
                    "cannot yet rebuild {}: the record store is unavailable, so this rebuild's \
                     envelope is a degraded one and cannot be compared with what was journalled. \
                     Refusing WITHOUT settling — the next cycle re-reads the book and decides \
                     then.\n  → if the store stays down this update stays open, which is a store \
                     outage rather than a stuck turn.",
                    record.key.as_str()
                ),
            ));
        }
        // A rebuild that does not match cannot be replayed: the runtime refuses a changed body
        // under an existing key, so this is a local decision rather than a remote 409.
        Some(record) if !record.may_replay(&digest) => {
            // **It will never match**, which is why this settles rather than only refusing.
            // The mismatch is not a transient the next cycle might clear: whatever the rebuild
            // differs on, it will differ on again, every cycle, forever. Left open, this update
            // holds the safe cursor — and the safe cursor is the lowest open update across every
            // tenant, so one turn nobody can reconstruct stops every conversation in the process
            // while the loop backs off politely and the process looks healthy. Measured, cycles 3
            // and 4 of `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`.
            //
            // `OperatorRequired` is the state for exactly this: it releases the cursor and keeps
            // the alarm, because nothing compacts an update that cost a turn.
            let reason = ironworks_domain::NonEmptyReason::parse(format!(
                "the journalled body cannot be rebuilt: recorded {}, rebuilt {}. \
                 Replay is impossible, so this turn will not resolve without a person.",
                record.request_digest.as_str(),
                digest.as_str()
            ))
            .map_err(|x| e(x.to_string()))?;
            b.journal
                .record_operator_required(&record.key, &reason, &now_iso())
                .map_err(|x| e(x.to_string()))?;
            return Err(fail(
                FAILED,
                format!(
                    "REFUSING TO SERVE. The journalled turn {} cannot be reconstructed \
                     byte-identically, so replaying its idempotency key would create a NEW \
                     logical turn rather than recovering the original.\n  \
                     recorded digest {} · rebuilt {}\n  \
                     It is now journalled OperatorRequired, so it no longer holds the channel \
                     cursor.\n  → reconcile {} by response id.",
                    record.key.as_str(),
                    record.request_digest.as_str(),
                    digest.as_str(),
                    record.key.as_str()
                ),
            ));
        }
        // Both recovered plans need the rebuild to match, and for different reasons: a replay
        // must match or the runtime refuses it, and a dispatch must match or the key would name
        // a body nobody journalled. Saying "replaying" for a turn that never left was wrong.
        Some(record) => eprintln!(
            "ironworks: rebuild is byte-identical; {}",
            if matches!(record.outcome, TurnOutcome::SubmittedUnknown) {
                "replaying the same key"
            } else {
                "dispatching the original turn"
            }
        ),
        // Durable BEFORE a byte leaves — and now carrying what this turn decided, so the process
        // that recovers it can rebuild the same body instead of deciding again.
        None => b
            .journal
            .begin(
                &b.scope,
                key,
                &retrieved_at,
                &digest,
                &ironworks_journal::Decision {
                    previous: chained_from,
                    carried: match &supply {
                        ironworks_domain::Supply::CarriedByThread { records } => Some(*records),
                        ironworks_domain::Supply::Full(_) => None,
                    },
                },
                &now_iso(),
            )
            .map_err(|x| e(x.to_string()))?,
    }

    eprintln!(
        "alloy {} bytes ({}) · envelope {} bytes · {} record(s) [{}] · request {} · key {}",
        turn.instructions().len(),
        turn.instructions().digest(),
        turn.input().as_str().len(),
        records.len(),
        describe_records(&records_state),
        &digest.as_str()[..16],
        key.as_str(),
    );

    crash_if(args, "before-dispatch");

    // Ambiguity is the default: once a request is about to leave, its outcome is unknown until
    // evidence says otherwise. Recording that BEFORE dispatch is what makes B3 recoverable.
    b.journal.record_unknown(key, &now_iso()).map_err(|x| e(x.to_string()))?;

    let accepted = match runtime.create(&turn, key, &deadline) {
        Ok(a) => a,
        Err(f) => {
            // A conflict on a body we just proved byte-identical means the key was used by a turn
            // this journal never saw — a different logical turn wearing the same name. Every
            // future replay will conflict identically, so leaving it open would have the relay
            // rebuild and re-POST it on every poll, forever, holding the shared cursor behind it.
            //
            // Automatic work stops here and an operator is told why. Note it is NOT recorded as
            // unsent: a turn does exist under that key, it is simply not ours to claim.
            if matches!(f.cause(), Cause::IdempotencyConflict) {
                // Operator reasons are durable and displayed by `status`. Keep every producer
                // technical: never interpolate model output, record text, credentials or channel
                // content here. This reason contains only a seam-minted identifier.
                let why = ironworks_domain::NonEmptyReason::parse(format!(
                    "the runtime already holds a different turn under key {} — this key was \
                     minted for an update whose id the channel had used before. The answer under \
                     that key belongs to another turn and must not be served here.",
                    key.as_str()
                ))
                .map_err(|x| e(x.to_string()))?;
                b.journal
                    .record_operator_required(key, &why, &now_iso())
                    .map_err(|x| e(x.to_string()))?;
                return Err(e(format!("REFUSING TO SERVE. {why}")));
            }
            // Only positive evidence downgrades the record. Anything ambiguous stays unknown.
            if f.provably_unsent() {
                b.journal.record_unsent(key, &now_iso()).map_err(|x| e(x.to_string()))?;
            }
            return Err(describe(f));
        }
    };

    // A proof affordance, labelled as such: the id is announced before it is discarded so a
    // recovery check can prove the replay resolved to THIS response rather than a new one.
    if args.crash_at.as_deref() == Some("before-response") {
        eprintln!("ironworks: DISCARDING response {} (deliberate crash)", accepted.id);
        eprintln!("ironworks: CRASHING at before-response (deliberate)");
        std::process::exit(CRASHED);
    }
    b.journal.record_accepted(key, &accepted.id, &now_iso()).map_err(|x| e(x.to_string()))?;
    eprintln!("response {} ({})", accepted.id, label(&accepted.outcome));

    let outcome = if accepted.outcome.is_terminal() {
        accepted.outcome
    } else {
        ironworks_ironclaw::poll_to_completion(
            &runtime,
            &accepted.id,
            &deadline,
            std::thread::sleep,
        )
        .map_err(describe)?
    };

    if args.crash_at.as_deref() == Some("before-commit") {
        eprintln!("ironworks: response {} is durable (accepted)", accepted.id);
        eprintln!("ironworks: CRASHING at before-commit (deliberate)");
        std::process::exit(CRASHED);
    }
    finish(
        &b.journal,
        &b.scope,
        key,
        outcome,
        &accepted.id,
        ThreadCommit { conversation, alloy: &alloy, supplied: mark.as_ref() },
    )
}

/// Fetch an answer that already exists. Never calls `create`.
fn fetch_answer(
    runtime: &dyn Runtime,
    response: &ResponseId,
    deadline: &Deadline,
) -> Result<Outcome, Exit> {
    runtime.fetch(response, deadline).map_err(describe)
}

/// Commit the completed turn and hand back the answer. Printing and delivering are the callers'
/// business; this is the one place a turn becomes durably complete.
/// What a completed turn tells its thread.
///
/// One value rather than three arguments because they are one fact — *this conversation, built on
/// this composition, now carrying this much of the book* — and passing them separately is how a
/// caller ends up recording one thread's mark against another's room.
struct ThreadCommit<'a> {
    conversation: Option<&'a ironworks_domain::ChannelRef>,
    alloy: &'a str,
    supplied: Option<&'a ironworks_domain::SuppliedMark>,
}

fn finish(
    journal: &Journal,
    scope: &ThreadScope,
    key: &IdempotencyKey,
    outcome: Outcome,
    response: &ResponseId,
    thread: ThreadCommit<'_>,
) -> Result<Answer, Exit> {
    match outcome {
        Outcome::Completed { text, usage } => {
            // The response id and the continuation pointer become durable together, or neither.
            journal
                .commit_completed(
                    key,
                    &scope.pointer(response.clone()),
                    thread.conversation,
                    thread.alloy,
                    thread.supplied,
                    &now_iso(),
                )
                .map_err(|x| fail(FAILED, x.to_string()))?;
            if let Some(u) = usage {
                // Tokens are the runtime's own count and are trustworthy. The cost is not: it is
                // priced from IronClaw's table, which classifies the pinned `qwen/…` as a free
                // local model on create and as an unknown model — GPT-4o's rates — on fetch. So
                // it is labelled rather than printed as a figure, because an operator reading
                // `cost 0` every turn would reasonably conclude the turns were free.
                // `docs/experiments/0040-the-cost-field-misses-the-pin-twice.md`.
                eprintln!(
                    "usage: {} in ({} cached), {} out, runtime cost estimate {} (not the \
                     provider's price; see INVARIANTS §G-7)",
                    u.input_tokens,
                    u.cached_tokens,
                    u.output_tokens,
                    u.total_cost_usd.as_deref().unwrap_or("unreported")
                );
            }
            if let Some(found) = ironworks_ironclaw::malformed_answer(&text) {
                // The commit above already happened, and must have: the turn completed, the
                // response id is durable, and this is never re-run. What is refused is delivery.
                // Same call `Incomplete` makes below — text a reader cannot tell is broken is
                // worse than no text, and here the reader has been told the answer is cited.
                eprintln!("ironworks: the runtime returned {found} instead of an answer;");
                eprintln!("           it is printed below and NOT delivered.");
                eprintln!("{text}");
                return Err(fail(FAILED, "malformed"));
            }
            Ok(Answer { text })
        }
        Outcome::Parked { calls, .. } => {
            // The runtime is waiting on tool outputs we have no way to produce: nothing in this
            // binary declares `tools`, so a parked run means the turn was built somewhere this
            // code does not know about.
            //
            // **No thread pointer is committed.** A parked run holds no answer, and chaining from
            // it would replay a conversation whose last act was an unanswered call. The turn is
            // marked `OperatorRequired` instead — which keeps the alarm (nothing compacts an
            // update that cost a turn) while releasing the shared cursor, so one parked turn
            // cannot deafen every conversation behind it.
            let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
            let reason = ironworks_domain::NonEmptyReason::parse(format!(
                "the runtime parked this turn on {} unanswered tool call(s): {}. \
                 It reports status=completed and holds no answer.",
                calls.len(),
                names.join(", ")
            ))
            .map_err(|x| fail(FAILED, x.to_string()))?;
            journal
                .record_operator_required(key, &reason, &now_iso())
                .map_err(|x| fail(FAILED, x.to_string()))?;
            Err(fail(
                FAILED,
                format!(
                    "the turn parked on {} tool call(s) [{}] and cannot be delivered. \
                     Response {response} is journalled OperatorRequired.",
                    calls.len(),
                    names.join(", ")
                ),
            ))
        }
        Outcome::Incomplete { text } => {
            // Partial output is still output: it is printed rather than delivered, because a
            // half-answer a client cannot tell is half is worse than none.
            eprintln!("ironworks: the turn stopped early; partial output follows");
            eprintln!("{text}");
            Err(fail(FAILED, "incomplete"))
        }
        Outcome::Failed { code, message } => {
            Err(fail(FAILED, format!("the turn failed: {code} — {message}")))
        }
        Outcome::Cancelled => Err(fail(FAILED, "the turn was cancelled")),
        Outcome::Running => Err(fail(FAILED, "the turn is still running")),
    }
}

fn build(
    verified: &ServiceInstance<ironworks_domain::Verified>,
    ask: &str,
    records: &[ironworks_domain::record::AccountRecord],
    state: &RecordsState,
    retrieved_at: &str,
    previous: Option<ironworks_domain::ThreadPointer>,
) -> Result<ModelTurn, Exit> {
    let e = |m: String| fail(FAILED, m);
    let ts =
        ironworks_domain::record::Timestamp::parse(retrieved_at).map_err(|x| e(x.to_string()))?;

    // Materials, not text. This function used to render the envelope itself and hand it over as
    // a finished string — which meant the binary decided what a tenant's model was told about its
    // own records. It cannot any more: `Ask` has no field for rendered context, and no field for
    // an org.
    verified
        .build_turn(
            &ironworks_services::Heap,
            &Ask {
                question: ask,
                records,
                records_state: state,
                retrieved_at: &ts,
                speaker: None,
                fact_fields: Some(verified.fact_fields()),
            },
            previous,
        )
        .map_err(|x| e(x.to_string()))
}

/// What to do when a recovered turn is rebuilt.
///
/// Extracted because the wrong answer is expensive in both directions, and neither direction is
/// reachable from a unit test through [`drive`] — which needs a runtime, a record store and a
/// journal at once.
#[derive(Debug, PartialEq, Eq)]
enum Rebuild {
    /// Byte-identical. Replay the key, or dispatch the turn that never left.
    Matches,
    /// It differs, **and the book was read** to establish that. A rebuild that differs on a
    /// complete read differs again on the next one, so this settles rather than retrying forever.
    Differs,
    /// It differs, but the book could not be read — so **nothing has been established**. This
    /// rebuild's envelope is a degraded one; the original was built from a complete read, and the
    /// two cannot be compared.
    ///
    /// This case is why the settling verdict is not simply "digest mismatch". Settling on it would
    /// let a store that blinked between `resolve` and `catalog` permanently destroy a turn the
    /// next cycle would have recovered. Controls fail closed; a data source degrades — and a
    /// degraded data source must not produce a permanent verdict.
    Unestablished(Degradation),
}

fn rebuild_verdict(
    record: &ironworks_journal::TurnRecord,
    digest: &RequestDigest,
    records: &RecordsState,
) -> Rebuild {
    if record.may_replay(digest) {
        return Rebuild::Matches;
    }
    match records {
        RecordsState::Degraded(why) => Rebuild::Unestablished(why.clone()),
        _ => Rebuild::Differs,
    }
}

/// Say plainly whether a second attempt is safe.
fn describe(f: TurnFailure) -> Exit {
    let advice = if f.provably_unsent() {
        "no turn ran; it is safe to try again"
    } else {
        "a turn MAY have run — do NOT retry; recover it from the journal"
    };
    let note = if matches!(f.cause(), Cause::BudgetExhausted) {
        " (the answer may still be fetchable)"
    } else {
        ""
    };
    fail(FAILED, format!("{f}{note}\n  → {advice}"))
}

fn label(o: &Outcome) -> &'static str {
    match o {
        Outcome::Running => "running",
        Outcome::Completed { .. } => "completed",
        // Not "completed", though that is the status the runtime reports. An operator reading a
        // log needs the fact the status hides.
        Outcome::Parked { .. } => "parked on a tool call",
        Outcome::Failed { .. } => "failed",
        Outcome::Cancelled => "cancelled",
        Outcome::Incomplete { .. } => "incomplete",
    }
}

fn describe_records(s: &RecordsState) -> &'static str {
    match s {
        RecordsState::Complete => "complete",
        RecordsState::Empty => "empty",
        RecordsState::Degraded { .. } => "DEGRADED",
        RecordsState::CarriedByThread { .. } => "carried-by-thread",
    }
}

/// Read the book for this scope. A data source, so every failure degrades — except scope drift,
/// which is a control failing late and refuses.
fn fetch_records(
    book: &Book<EnvCredentials>,
    scope: &RecordScope,
) -> Result<(Vec<ironworks_domain::record::AccountRecord>, RecordsState), Exit> {
    let catalog = match book.catalog(scope) {
        Ok(c) => c,
        Err(drift @ BookFailure::ScopeDrift { .. }) => return Err(scope_drift_exit(&drift)),
        Err(_) => {
            // A cause, not a sentence. The words the model sees are core's to choose.
            return Ok((Vec::new(), RecordsState::Degraded(Degradation::Unavailable)));
        }
    };
    if catalog.is_empty() {
        return Ok((Vec::new(), RecordsState::Empty));
    }
    let mut out = Vec::with_capacity(catalog.len());
    let mut unreadable = 0usize;
    for entry in catalog.entries() {
        match book.record(scope, &entry.id) {
            Ok(Some(r)) => out.push(r),
            Ok(None) => unreadable += 1,
            Err(drift @ BookFailure::ScopeDrift { .. }) => return Err(scope_drift_exit(&drift)),
            Err(_) => unreadable += 1,
        }
    }
    let state = if unreadable > 0 {
        RecordsState::Degraded(Degradation::Partial { unreadable })
    } else {
        RecordsState::Complete
    };
    Ok((out, state))
}

fn scope_drift_exit(f: &BookFailure) -> Exit {
    fail(
        FAILED,
        format!(
            "{f}\n  → REFUSING TO SERVE. This is not a records outage: the store no longer agrees \
             about which organization this credential authorises, so the verified scope can no \
             longer be asserted to the model. Check whether the credential was re-pointed."
        ),
    )
}

/// Opaque, unique-enough handle for one turn. Upstream forbids colons and path separators.
fn mint_key() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("ironworks-{:x}-{:x}", std::process::id(), nanos)
}

/// The one clock read in a fresh turn, at the edge where it belongs (ADR 0005). A *recovered*
/// turn never calls this for its body — it reuses the journalled value, which is what makes the
/// reconstruction byte-identical.
fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}+00:00")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `coordinate` is the only command that supplies its own service. It used to be checked
    /// against the default to prove it selected something rather than inheriting it; there is no
    /// default to differ from now, so what is checked is that the name it supplies resolves.
    #[test]
    fn account_coordination_alias_is_explicit_and_loadable() {
        let service =
            ironworks_services::service::load(ironworks_services::service::ACCOUNTS).unwrap();
        assert_eq!(service.version(), 2);
        assert_eq!(service.evaluation(), Some(ironworks_services::service::ACCOUNTS_EVALUATION));
    }

    /// One tenant, two people, two private conversations — the shape the prototype needs.
    ///
    /// Test-only: nothing here reads or writes an operator's registry, which is why the shape is
    /// exercised through a fixture.
    fn shared_tenant() -> ironworks_domain::TenantRegistry {
        ironworks_domain::TenantRegistry::parse(
            r#"{"tenants":[{"slug":"multiagency","channel":"184223311",
                "service":"account-analysis","guidance":"/etc/ironworks/multiagency.guidance.md",
                "records_credential":"env:RECORDS","runtime_credential":"env:RUNTIME",
                "contributors":[
                  {"channel":"184223311","actor":"184223311","contributor":"alice"},
                  {"channel":"713300284","actor":"713300284","contributor":"bob"}]}]}"#,
        )
        .expect("the fixture registry is valid")
    }

    fn alice() -> ironworks_domain::ActorRef {
        ironworks_domain::ActorRef::parse("184223311").expect("actor")
    }
    fn bob() -> ironworks_domain::ActorRef {
        ironworks_domain::ActorRef::parse("713300284").expect("actor")
    }
    fn stranger() -> ironworks_domain::ActorRef {
        ironworks_domain::ActorRef::parse("999999999").expect("actor")
    }
    fn chat(id: &str) -> ironworks_domain::ChannelRef {
        ironworks_domain::ChannelRef::new(id)
    }

    #[test]
    fn each_persons_own_private_chat_admits_them_as_themselves() {
        // The positive control, and the moment this patch exists for. Without it every refusal
        // below would pass on a function that refused everything.
        let registry = shared_tenant();
        let entry = &registry.entries()[0];
        for (room, who, expected) in [("184223311", alice(), "alice"), ("713300284", bob(), "bob")]
        {
            let room = chat(room);
            let Admission::Allow { contributor, .. } =
                admit_command(ironworks_domain::ChatKind::Private, Some(&who), &room, entry)
            else {
                panic!("{room} must admit its own bound sender");
            };
            assert_eq!(contributor.as_str(), expected);
        }
    }

    #[test]
    fn a_sender_replayed_into_a_colleagues_conversation_is_refused_blandly() {
        // Authorization is the pair. Alice's id arriving in Bob's chat matches no binding, and
        // she is told nothing about it — she is a stranger *there*, whatever she is elsewhere.
        let registry = shared_tenant();
        let entry = &registry.entries()[0];
        let Admission::Refuse { reply, .. } = admit_command(
            ironworks_domain::ChatKind::Private,
            Some(&alice()),
            &chat("713300284"),
            entry,
        ) else {
            panic!("a cross-wired pair must not be admitted");
        };
        assert_eq!(reply, UNAUTHORIZED);
    }

    #[test]
    fn a_room_that_is_not_an_attested_private_chat_is_refused_before_anything_else() {
        // Ordering is the disclosure rule. The group refusal comes first, so it is the answer
        // even for a sender who would otherwise be allowed — and a room the channel merely did
        // not describe is refused on the same branch as one it described as a group.
        let registry = shared_tenant();
        let entry = &registry.entries()[0];
        let who = alice();
        for kind in [ironworks_domain::ChatKind::NonPrivate, ironworks_domain::ChatKind::Unattested]
        {
            let Admission::Refuse { reply, .. } =
                admit_command(kind, Some(&who), &chat("184223311"), entry)
            else {
                panic!("{kind:?} must not be admitted");
            };
            assert!(reply.contains("direct message"), "{kind:?}: {reply}");
        }
    }

    #[test]
    fn an_unbound_sender_is_told_nothing_about_the_command_surface() {
        // "No substantive response" as a property of the sentence: it names no tenant, no
        // account, no verb, and no feature they were close to reaching. It is also the answer to
        // EVERY command they could type, because admission runs before the grammar does.
        let registry = shared_tenant();
        let entry = &registry.entries()[0];
        let unknown = stranger();
        for actor in [Some(&unknown), None] {
            let Admission::Refuse { reply, .. } = admit_command(
                ironworks_domain::ChatKind::Private,
                actor,
                &chat("184223311"),
                entry,
            ) else {
                panic!("an unbound sender must not be admitted");
            };
            assert_eq!(reply, UNAUTHORIZED);
            for leak in ["multiagency", "alice", "share", "confirm", "account"] {
                assert!(!reply.to_ascii_lowercase().contains(leak), "{reply:?} names {leak}");
            }
        }
    }

    #[test]
    fn admission_precedes_the_grammar_so_a_malformed_command_reveals_nothing_either() {
        // The bug this ordering closes: a malformed `/share` used to return the grammar's
        // refusal before the room and the sender were checked, so a group message and an unbound
        // sender both got command-specific help.
        let registry = shared_tenant();
        let entry = &registry.entries()[0];
        for text in ["/share", "/share ACME-001", "/unknown", "/confirm", "/share@bot"] {
            assert!(ironworks_domain::is_command(text), "{text:?} must reach the command door");
            // In a group, whatever the grammar would have said.
            let Admission::Refuse { reply, .. } = admit_command(
                ironworks_domain::ChatKind::NonPrivate,
                Some(&alice()),
                &chat("184223311"),
                entry,
            ) else {
                panic!("{text:?} in a group must be refused");
            };
            assert!(reply.contains("direct message"), "{text:?}: {reply}");
        }
    }

    #[test]
    fn the_exactness_check_covers_the_whole_confirmation_not_just_the_note() {
        // An account id is a caller-supplied string the record store's grammar does not
        // constrain, so a body-only check passes `**A**` and the transport then rewrites the
        // account in the very line that promises exact bytes.
        let ironworks_domain::Intent::Share(request) =
            ironworks_domain::share::parse("/share **A** a perfectly plain note")
        else {
            panic!("the fixture must parse as a share");
        };
        let who = ironworks_domain::ContributorId::parse("alice").unwrap();
        assert!(
            ironworks_channel_http::echoes_exactly(request.body()),
            "the note alone survives, which is exactly why checking only it is not enough"
        );
        assert!(
            !ironworks_channel_http::echoes_exactly(&ironworks_domain::render_echo(&request, &who)),
            "the rendered confirmation does not survive, and that is what must be checked"
        );
    }

    #[test]
    fn a_slash_command_never_becomes_a_turn() {
        // The relay asks this before addressing and before minting a key, so the property worth
        // pinning is the classification itself: anything beginning with a command is not a
        // conversation, and everything else is untouched.
        for command in ["/share ACME-001 note", "/confirm", "/unknown", "/share"] {
            assert!(
                ironworks_domain::is_command(command),
                "{command:?} would have reached a model"
            );
        }
        for ordinary in ["what needs attention?", "the ratio is 3/4", "", "/", "/ hello"] {
            assert!(
                !ironworks_domain::is_command(ordinary),
                "{ordinary:?} lost its existing behaviour"
            );
        }
    }

    #[test]
    fn confirm_is_a_typed_command_that_takes_nothing() {
        // What is confirmed is the note the person was just shown. `/confirm <anything>` would
        // invite them to name a different one, which is the choice this refuses to offer.
        assert_eq!(ironworks_domain::share::parse("/confirm"), ironworks_domain::Intent::Confirm);
        assert!(matches!(
            ironworks_domain::share::parse("/confirm share-abc"),
            ironworks_domain::Intent::Refused(ironworks_domain::Refusal::ConfirmTakesNoArguments)
        ));
    }

    #[test]
    fn a_confirmation_is_admitted_on_the_same_pair_as_a_share() {
        // `/confirm` runs the same admission as every other command, so an unbound sender and a
        // group both get the same answers they would for `/share` — before the grammar is even
        // parsed, and long before anything is appended.
        let registry = shared_tenant();
        let entry = &registry.entries()[0];
        assert!(matches!(
            admit_command(
                ironworks_domain::ChatKind::Private,
                Some(&alice()),
                &chat("184223311"),
                entry
            ),
            Admission::Allow { .. }
        ));
        for (kind, actor, room) in [
            (ironworks_domain::ChatKind::NonPrivate, Some(alice()), "184223311"),
            (ironworks_domain::ChatKind::Private, Some(stranger()), "184223311"),
            // Alice's id in Bob's chat: the pair, never the actor alone.
            (ironworks_domain::ChatKind::Private, Some(alice()), "713300284"),
            (ironworks_domain::ChatKind::Private, None, "184223311"),
        ] {
            assert!(
                matches!(
                    admit_command(kind, actor.as_ref(), &chat(room), entry),
                    Admission::Refuse { .. }
                ),
                "{kind:?} in {room} must not be admitted to confirm"
            );
        }
    }

    #[test]
    fn a_tenant_without_an_append_credential_cannot_confirm() {
        // The absence makes `/confirm` unavailable; it does not make it unauthenticated. The
        // fixture registry declares no append handle, which is the default every existing
        // deployment has.
        let registry = shared_tenant();
        assert_eq!(registry.entries()[0].append(), None);
    }

    #[test]
    fn every_permanent_append_refusal_carries_a_durable_code_and_none_of_the_error_prose() {
        // Two things at once, and both matter. A permanent refusal must be classifiable — it is
        // about to become a durable column an operator reads — and the classification must not
        // carry the error's own text: `Unreachable`, `Malformed` and `NoCredential` interpolate
        // adapter strings that can hold a URL, a credential handle or a response body, which is
        // the hazard `ironworks_lifecycle::attempt::Blocked` exists to keep out of a ledger.
        let secret = "keyring:acme/append-token";
        let cases = [
            (AppendFailure::Unauthorized, "unauthorized"),
            (
                AppendFailure::UnknownAccount {
                    org: ironworks_domain::OrgId::parse("acme-sales").unwrap(),
                    account: "ACME-001".into(),
                },
                "unknown_account",
            ),
            (AppendFailure::NoCredential { handle: secret.into() }, "no_credential"),
            (
                AppendFailure::Malformed {
                    detail: format!("https://user:pass@host said {secret}"),
                },
                "unreadable_answer",
            ),
            (AppendFailure::Refused { code: "invalid_occurred_at".into() }, "invalid_occurred_at"),
        ];
        for (failure, want) in cases {
            let code = append_refusal_code(&failure);
            assert_eq!(code, want, "{failure:?}");
            assert!(!code.contains(secret), "a credential handle reached a durable column: {code}");
            assert!(
                !code.contains("user:pass"),
                "a URL authority reached a durable column: {code}"
            );
        }
    }

    #[test]
    fn a_store_supplied_refusal_code_is_bounded_before_it_is_stored() {
        // The one code the STORE chooses rather than this build. It lands in a durable column and
        // in an operator's terminal, so it is held to identifier shape and a length — the same
        // reason `ActorRef` is bounded, and for the same column.
        let wild = AppendFailure::Refused { code: "x".repeat(500) };
        assert_eq!(append_refusal_code(&wild).len(), 64, "unbounded is not a column value");

        let injected = AppendFailure::Refused { code: "bad\ncode: keyring:acme/tok".into() };
        let code = append_refusal_code(&injected);
        assert!(!code.contains('\n'), "{code}");
        assert!(!code.contains(':'), "{code}");

        // And a code that survives the filter as nothing at all still names something.
        assert_eq!(append_refusal_code(&AppendFailure::Refused { code: "///".into() }), "refused");
        assert_eq!(append_refusal_code(&AppendFailure::Refused { code: String::new() }), "refused");
    }

    #[test]
    fn a_refused_append_is_told_the_same_way_whichever_path_finds_it() {
        // `shared_reply` is one function so a recovered success cannot be told apart from a fresh
        // one. The refusal is one constant for the same reason: a person meeting it on a later
        // `/confirm` must not be able to tell it came from the already-refused branch rather than
        // from the append that was just refused.
        assert!(REFUSED_REPLY.contains("nothing was written"));
        assert!(REFUSED_REPLY.contains("operator"));
        // It names no code. `unauthorized` is an operator's word about an operator's mistake.
        for operator_word in ["unauthorized", "unknown_account", "credential", "token"] {
            assert!(!REFUSED_REPLY.contains(operator_word), "{operator_word} is not hers to read");
        }
    }

    #[test]
    fn the_two_durable_lanes_are_separate() {
        // A proposal has an echo and, later, a confirmation reply. They must not share state:
        // a delivered echo would otherwise mark the success reply as already sent, and the
        // person would never learn their note was shared.
        assert_eq!(Lane::Echo(41).proposal(), Some(41));
        assert_eq!(Lane::Reply(41).proposal(), Some(41));
        assert_eq!(Lane::None.proposal(), None);
        assert_ne!(Lane::Echo(41).what(), Lane::Reply(41).what());
    }

    #[test]
    fn the_original_confirmation_recovering_completes_its_reply_through_the_durable_lane() {
        // The gap this closes. A crash between the commit and the reply redelivers the SAME
        // update; answering "already shared" through no lane would let that message repeat on
        // every cycle, which is exactly what the reply lane exists to prevent.
        assert_eq!(committed_answer(42, 42), CommittedAnswer::CompleteTheReply);
        // A genuinely later `/confirm` is a different message and gets the other answer.
        for later in [43, 44, 1000] {
            assert_eq!(committed_answer(42, later), CommittedAnswer::AlreadyShared);
        }
    }

    #[test]
    fn the_recovering_reply_is_the_same_text_the_first_send_would_have_used() {
        // One function for both, so a person who receives the recovered one cannot tell it came
        // from a different path — and so the two cannot drift into saying different things about
        // the same committed row.
        let proposal = ironworks_proposals::Proposal {
            update: 41,
            channel: chat("184223311"),
            contributor: ironworks_domain::ContributorId::parse("alice").unwrap(),
            slug: ironworks_domain::Slug::parse("multiagency").unwrap(),
            org: ironworks_domain::OrgId::parse("multiagency-org").unwrap(),
            account: ironworks_domain::record::AccountId::parse("ACME-001").unwrap(),
            body: "Customer requested a September renewal review.".into(),
            proposed_at: "2026-09-01T09:15:00+00:00".into(),
            echo: ironworks_proposals::Echo::Delivered,
            confirm: ironworks_proposals::Confirmation::Committed {
                activity_id: "share-abc".into(),
                org: ironworks_domain::OrgId::parse("multiagency-org").unwrap(),
                confirm_update: 42,
            },
            reply: ironworks_proposals::Echo::Pending,
        };
        let org = ironworks_domain::OrgId::parse("multiagency-org").unwrap();
        let reply = shared_reply(&proposal, "share-abc", &org);
        assert!(reply.contains("share-abc") && reply.contains("ACME-001"), "{reply}");
        assert!(reply.contains("attributed to alice"), "{reply}");
        assert!(reply.starts_with("Shared."), "{reply}");
        // And it survives the transport unchanged, like every other message this sends.
        assert!(ironworks_channel_http::echoes_exactly(&reply), "{reply}");
    }

    #[test]
    fn the_reply_lane_permits_exactly_one_send_and_a_retry_after_a_proven_rejection() {
        // The four recovery positions the committed branch can be reached in, as the lane sees
        // them. `Pending` sends; the two ambiguous states never do; a proven rejection returns to
        // `Pending` so the redelivery finishes the job.
        use ironworks_proposals::Echo;
        let lane = Lane::Reply(41);
        let with = |reply: Echo| ironworks_proposals::Proposal {
            update: 41,
            channel: chat("184223311"),
            contributor: ironworks_domain::ContributorId::parse("alice").unwrap(),
            slug: ironworks_domain::Slug::parse("multiagency").unwrap(),
            org: ironworks_domain::OrgId::parse("multiagency-org").unwrap(),
            account: ironworks_domain::record::AccountId::parse("ACME-001").unwrap(),
            body: "n".into(),
            proposed_at: "2026-09-01T00:00:00+00:00".into(),
            // The echo is DELIVERED throughout: the two lanes are separate, and a delivered echo
            // must not make the success reply look already sent.
            echo: Echo::Delivered,
            confirm: ironworks_proposals::Confirmation::Committed {
                activity_id: "share-abc".into(),
                org: ironworks_domain::OrgId::parse("multiagency-org").unwrap(),
                confirm_update: 42,
            },
            reply,
        };
        assert!(lane.state(&with(Echo::Pending)).may_attempt(), "a reply never sent must go out");
        assert!(!lane.state(&with(Echo::Attempting)).may_attempt(), "crash mid-send: never again");
        assert!(!lane.state(&with(Echo::Delivered)).may_attempt(), "already sent");
        assert!(
            !lane.state(&with(Echo::Uncertain { acknowledged_chunks: 0 })).may_attempt(),
            "the channel could not say: an operator's, not a resend"
        );
        // And the echo lane on the same row still reads its own state.
        assert!(!Lane::Echo(41).state(&with(Echo::Pending)).may_attempt());
    }

    #[test]
    fn status_needs_only_a_journal() {
        let Ok(args) = parse_status(&["--journal".into(), "/tmp/status.db".into()]) else {
            panic!("status did not accept its one required flag")
        };
        assert_eq!(args.journal, "/tmp/status.db");
        assert!(matches!(args.mode, Mode::Status));
        assert!(args.slug.is_empty());
        assert!(args.guidance.is_empty());
    }

    /// A writer that fails every write with one kind, standing in for a reader that went away.
    struct Closed(io::ErrorKind);
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "no reader"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_report_whose_reader_closed_the_pipe_ends_normally() {
        // `compose | head` and `status | grep` close the pipe as soon as they have enough. That
        // is the reader being satisfied, not the command failing.
        assert!(write_report(&mut Closed(io::ErrorKind::BrokenPipe), "line\n").is_ok());
    }

    #[test]
    fn a_report_that_could_not_be_written_for_any_other_reason_fails() {
        // The positive control for the case above: swallowing every write error would hide a
        // report that never reached anyone behind an exit code of zero.
        let Err(err) = write_report(&mut Closed(io::ErrorKind::PermissionDenied), "line\n") else {
            panic!("a report that was never written reported success")
        };
        assert_eq!(err.code, FAILED);
        assert!(err.message.contains("cannot write the report"));
    }

    #[test]
    fn a_report_reaches_a_reader_that_is_listening() {
        let mut sink = Vec::new();
        assert!(write_report(&mut sink, "service x\nalloy y\n").is_ok());
        assert_eq!(String::from_utf8(sink).unwrap(), "service x\nalloy y\n");
    }

    #[test]
    fn relay_serves_a_registry_and_never_a_single_tenant() {
        let Ok(args) = parse_relay(&[
            "--registry".into(),
            "/etc/ironworks/registry.json".into(),
            "--journal".into(),
            "/var/lib/ironworks/serve.journal".into(),
            "--channel".into(),
            "http://127.0.0.1:3994".into(),
        ]) else {
            panic!("relay did not accept a registry")
        };
        let Mode::Relay { registry, channel, chunk_bytes } = &args.mode else {
            panic!("not a relay")
        };
        assert_eq!(registry, "/etc/ironworks/registry.json");
        assert_eq!(channel, "http://127.0.0.1:3994");
        assert_eq!(*chunk_bytes, DEFAULT_CHUNK_BYTES);
        assert!(args.slug.is_empty(), "a slug belongs to a tenant, not to the process");
    }

    #[test]
    fn relay_refuses_the_per_tenant_flags_it_no_longer_owns() {
        // Accepting these would let an operator believe they had overridden something for a
        // conversation that belongs to somebody else. They are per-tenant now, and the registry
        // is the only place they come from.
        for flag in ["--slug", "--guidance", "--service", "--channel-id"] {
            let Err(err) = parse_relay(&[
                "--registry".into(),
                "r.json".into(),
                "--journal".into(),
                "j".into(),
                "--channel".into(),
                "c".into(),
                flag.into(),
                "x".into(),
            ]) else {
                panic!("relay accepted {flag}")
            };
            assert_eq!(err.code, USAGE);
            assert!(err.message.contains(&format!("unknown flag {flag}")));
        }
    }

    #[test]
    fn relay_requires_a_registry_and_a_journal() {
        for missing in ["--registry", "--journal"] {
            let mut argv: Vec<String> = vec![
                "--registry".into(),
                "r.json".into(),
                "--journal".into(),
                "j".into(),
                "--channel".into(),
                "c".into(),
            ];
            let at = argv.iter().position(|a| a == missing).unwrap();
            argv.drain(at..at + 2);
            let Err(err) = parse_relay(&argv) else { panic!("relay ran without {missing}") };
            assert_eq!(err.code, USAGE);
            assert!(err.message.contains(missing), "{}", err.message);
        }
    }

    #[test]
    fn compose_takes_the_composition_inputs_and_nothing_else() {
        let Ok(args) = parse_compose(&[
            "--slug".into(),
            "coordination-eval".into(),
            "--guidance".into(),
            "g.md".into(),
            "--service".into(),
            "relationship-intelligence".into(),
        ]) else {
            panic!("compose did not accept its inputs")
        };
        assert!(matches!(args.mode, Mode::Compose));
        assert_eq!(args.service, "relationship-intelligence");
        assert!(args.journal.is_empty(), "a dry melt acquires no journal");
    }

    /// The inverse of what this used to assert. Omitting `--service` once meant the default; it
    /// now means nothing is named, and a melt over a service nobody chose is the thing the missing
    /// default exists to prevent.
    #[test]
    fn compose_without_a_service_is_refused_rather_than_defaulted() {
        let Err(err) =
            parse_compose(&["--slug".into(), "t".into(), "--guidance".into(), "g.md".into()])
        else {
            panic!("compose must not invent a service")
        };
        assert!(
            err.message.contains("--service is required"),
            "the refusal must name the missing flag, got: {}",
            err.message
        );
    }

    #[test]
    fn compose_rejects_the_flags_of_a_command_that_serves() {
        // Not tidiness: a dry melt that could be handed a journal or a channel is one an operator
        // can mistake for a turn, and the whole claim is that it reaches nothing.
        for flag in ["--journal", "--channel", "--ask", "--crash-at"] {
            let Err(err) = parse_compose(&[
                "--slug".into(),
                "t".into(),
                "--guidance".into(),
                "g.md".into(),
                flag.into(),
                "x".into(),
            ]) else {
                panic!("compose accepted {flag}")
            };
            assert_eq!(err.code, USAGE);
            assert!(err.message.contains(&format!("unknown flag {flag}")));
        }
    }

    #[test]
    fn compose_requires_guidance_because_there_is_no_unguided_composition() {
        let Err(err) = parse_compose(&["--slug".into(), "t".into()]) else {
            panic!("compose composed without guidance")
        };
        assert_eq!(err.code, USAGE);
        assert!(err.message.contains("--guidance is required"));
    }

    /// No service this binary ships reaches a built-in tool.
    ///
    /// This is the production assertion, and it is deliberately a negative: `confine` derives its
    /// policy from whatever the recorded instance's service declares, so "what would be applied
    /// today" is exactly this. It crosses `services` and `lifecycle`, which do not depend on each
    /// other — the binary is the only place the two meet, so this is the only place it can be
    /// checked.
    ///
    /// **Its positive control lives in `ironworks-lifecycle`**
    /// (`the_policy_is_a_function_of_the_declared_ceiling`), which proves `for_profile` can return
    /// a non-empty policy. Without that pairing this would pass against a function that returned
    /// the empty set for everything, which is green for the wrong reason.
    #[test]
    fn no_shipped_service_reaches_a_built_in_tool() {
        let mut seen = 0;
        for name in ironworks_services::service::available() {
            let d =
                ironworks_services::service::load(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            let policy =
                ironworks_lifecycle::ConfinementPolicy::for_profile(d.capability_ceiling());
            assert!(
                policy.allowed().is_empty(),
                "{name} would be confined with {:?} reachable — if that is intended, this test is \
                 where the decision gets recorded, and it owes two things first. §A needs a new \
                 accounting of `ModelInput`, because a tools-declaring service puts a model-visible \
                 path outside `envelope`. And serving must require confinement *evidence*, which \
                 nothing enforces today: `0035` measured that declaring `tools` puts the runtime \
                 into its own loop over the built-in catalogue, and unconfined that loop read 63 KB \
                 of this tenant's records out of `ironclaw.memory.*` and never called the declared \
                 tool. Confinement is the precondition that makes the lane work, not hardening on \
                 top of one that already does.",
                policy.allowed()
            );
            assert!(!policy.auto_approve(), "{name} would leave automatic approval on");
            seen += 1;
        }
        assert!(seen >= 1, "the loop must actually have examined the shipped services");
    }

    /// A digest mismatch and an *unestablished* mismatch are different facts, and only one of
    /// them may settle a turn permanently.
    ///
    /// Three-sided, because each verdict is the failure of the other two: reading `Differs` as
    /// `Matches` replays a body the runtime refuses; reading `Unestablished` as `Differs`
    /// permanently destroys a turn the next cycle would have recovered — the regression this
    /// function exists to prevent — and reading `Differs` as `Unestablished` restores the
    /// indefinite retry that held the channel in
    /// `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`.
    #[test]
    fn a_degraded_read_cannot_establish_that_the_body_differs() {
        use ironworks_domain::{Degradation, RecordsState};
        let recorded = RequestDigest::from_hex("aa".repeat(32));
        let other = RequestDigest::from_hex("bb".repeat(32));
        let record = ironworks_journal::TurnRecord {
            key: IdempotencyKey::parse("upd-1".to_owned()).unwrap(),
            instance: Slug::parse("acme".to_owned()).unwrap(),
            org: ironworks_domain::OrgId::parse("acme-sales").unwrap(),
            retrieved_at: "2026-09-02T00:00:00+00:00".to_owned(),
            request_digest: recorded.clone(),
            outcome: TurnOutcome::SubmittedUnknown,
            delivery: ironworks_domain::DeliveryState::Pending,
            attempts: 1,
            first_seen: "2026-09-02T00:00:00+00:00".to_owned(),
            updated_at: "2026-09-02T00:00:00+00:00".to_owned(),
            decision: ironworks_journal::Decision::default(),
        };

        // Same bytes: replayable, whatever the store is doing.
        assert_eq!(rebuild_verdict(&record, &recorded, &RecordsState::Complete), Rebuild::Matches);
        assert_eq!(
            rebuild_verdict(&record, &recorded, &RecordsState::Degraded(Degradation::Unavailable)),
            Rebuild::Matches,
            "a degraded read must not block a rebuild that already matches"
        );

        // Different bytes on a complete read: established, and it will not change.
        assert_eq!(rebuild_verdict(&record, &other, &RecordsState::Complete), Rebuild::Differs);
        assert_eq!(rebuild_verdict(&record, &other, &RecordsState::Empty), Rebuild::Differs);

        // Different bytes because we could not read the book: nothing established.
        assert_eq!(
            rebuild_verdict(&record, &other, &RecordsState::Degraded(Degradation::Unavailable)),
            Rebuild::Unestablished(Degradation::Unavailable)
        );
        assert_eq!(
            rebuild_verdict(
                &record,
                &other,
                &RecordsState::Degraded(Degradation::Partial { unreadable: 3 })
            ),
            Rebuild::Unestablished(Degradation::Partial { unreadable: 3 }),
            "a partial read is degraded too — some of the book is missing from this envelope"
        );
    }

    /// The seam `--workspace` used to bypass: what is recorded against a slug decides its policy,
    /// and an unrecorded slug is refused rather than confined to a default.
    #[test]
    fn the_policy_comes_from_what_is_recorded_and_refuses_when_nothing_is() {
        let dir = std::env::temp_dir().join(format!("ironworks-confine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lifecycle.store");
        let _ = std::fs::remove_file(&path);
        let store = ironworks_lifecycle::LifecycleStore::open(path.to_str().unwrap()).unwrap();
        let known = Slug::parse("acme".to_owned()).unwrap();

        // Nothing recorded: refuse, and say why rather than applying a default.
        let Err(err) = policy_for(&store, &known) else {
            panic!("an unrecorded slug must not yield a policy")
        };
        assert!(err.message.contains("no instance recorded"), "got: {}", err.message);

        // Recorded against a shipped service: the ceiling decides, and both shipped services
        // declare no tools — so the policy is empty and `confine` would disable the catalogue.
        store
            .declare(
                &ironworks_lifecycle::InstanceSpec {
                    slug: known.clone(),
                    org: ironworks_domain::OrgId::parse("acme-sales").unwrap(),
                    service: ironworks_domain::ServiceId::parse(
                        ironworks_services::service::ACCOUNTS,
                    )
                    .unwrap(),
                    channel: ironworks_domain::ChannelRef::new("-100900001"),
                },
                "2026-09-02T00:00:00+00:00",
            )
            .unwrap();
        let Ok((definition, policy)) = policy_for(&store, &known) else {
            panic!("a recorded slug must resolve to its service's policy")
        };
        assert_eq!(definition.id().as_str(), ironworks_services::service::ACCOUNTS);
        assert!(policy.allowed().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_confinement_policy_cannot_be_widened_from_the_command_line() {
        // `--workspace` is refused rather than ignored, and the refusal says where the decision
        // moved to. A flag that silently did nothing would leave an operator believing they had
        // enabled a shell.
        let Err(err) = parse_lifecycle(&[
            "confine".into(),
            "client".into(),
            "--store".into(),
            "/tmp/lifecycle.store".into(),
            "--workspace".into(),
        ]) else {
            panic!("--workspace must be refused, not accepted")
        };
        assert_eq!(err.code, USAGE);
        assert!(err.message.contains("runtime_tools"), "the refusal must name the replacement");

        // The positive control: the command still parses without it, so the check above is not
        // passing because `confine` stopped working.
        let Ok(ordinary) = parse_lifecycle(&[
            "confine".into(),
            "client".into(),
            "--store".into(),
            "/tmp/lifecycle.store".into(),
            "--dry-run".into(),
        ]) else {
            panic!("ordinary confinement should parse")
        };
        assert!(matches!(ordinary.mode, Mode::LifecycleConfine { dry_run: true, .. }));
    }

    #[test]
    fn adoption_accepts_handles_and_has_no_asserted_identity_flags() {
        let Ok(args) = parse_lifecycle(&[
            "adopt".into(),
            "local-name".into(),
            "--store".into(),
            "/tmp/lifecycle.store".into(),
            "--service".into(),
            "relationship-intelligence".into(),
            "--channel".into(),
            "-1001".into(),
            "--records-credential".into(),
            "env:TENANT_RECORDS".into(),
            "--member-credential".into(),
            "env:TENANT_MEMBER".into(),
        ]) else {
            panic!("adopt should accept credential handles")
        };
        assert!(matches!(
            args.mode,
            Mode::LifecycleAdopt {
                records_credential,
                member_credential,
                ..
            } if records_credential == "env:TENANT_RECORDS"
                && member_credential == "env:TENANT_MEMBER"
        ));

        for flag in ["--org", "--member", "--member-id"] {
            let Err(err) = parse_lifecycle(&[
                "adopt".into(),
                "local-name".into(),
                "--store".into(),
                "s".into(),
                "--service".into(),
                "relationship-intelligence".into(),
                "--channel".into(),
                "-1001".into(),
                "--records-credential".into(),
                "env:R".into(),
                "--member-credential".into(),
                "env:M".into(),
                flag.into(),
                "asserted".into(),
            ]) else {
                panic!("adopt accepted asserted identity flag {flag}")
            };
            assert_eq!(err.code, USAGE);
            assert!(err.message.contains("measures org and member identity"));
        }
    }

    #[test]
    fn adoption_refuses_a_raw_credential_without_echoing_it() {
        let secret = "member-secret-that-must-not-appear";
        let err = run_lifecycle_adopt(
            "local-name",
            "/tmp/not-opened-because-the-handle-refuses-first",
            "relationship-intelligence",
            "-1001",
            "env:TENANT_RECORDS",
            secret,
        )
        .expect_err("a raw credential is not a handle");
        assert_eq!(err.code, USAGE);
        assert!(err.message.contains("env:NAME"));
        assert!(!err.message.contains(secret));
    }

    #[test]
    fn status_rejects_serving_flags() {
        let Err(err) = parse_status(&[
            "--journal".into(),
            "/tmp/status.db".into(),
            "--slug".into(),
            "tenant".into(),
        ]) else {
            panic!("status accepted a serving flag")
        };
        assert_eq!(err.code, USAGE);
        assert!(err.message.contains("unknown flag --slug"));
    }

    #[test]
    fn status_fields_cannot_forge_another_output_line() {
        assert_eq!(safe_field("a\nb\tc"), "a\\nb\\tc");
    }

    #[test]
    fn terminal_output_is_exact_and_flushed_as_one_answer() {
        let mut out = Vec::new();
        write_answer_to(&mut out, "first\nsecond").unwrap();
        assert_eq!(out, b"first\nsecond\n");
    }

    #[test]
    fn the_epoch_renders_correctly() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
    }

    #[test]
    fn a_minted_timestamp_and_key_are_both_valid() {
        assert!(ironworks_domain::record::Timestamp::parse(now_iso()).is_ok());
        assert!(IdempotencyKey::parse(mint_key()).is_ok());
        assert_eq!(now_iso().len(), 25);
    }

    /// Every store failure that is not scope drift yields a state carrying no records. What the
    /// model is *told* about that state is asserted in core, where the words now live — this
    /// crate can no longer author them, which is the point.
    #[test]
    fn every_store_failure_degrades_rather_than_refusing() {
        for f in [
            BookFailure::Unreachable { detail: "connection refused".into() },
            BookFailure::Unauthorized,
            BookFailure::Malformed { detail: "bad json".into() },
            BookFailure::NoCredential { handle: "env:X".into() },
        ] {
            assert!(!matches!(f, BookFailure::ScopeDrift { .. }));
            let state = RecordsState::Degraded(Degradation::Unavailable);
            assert!(!state.has_records());
        }
    }

    #[test]
    fn scope_drift_refuses_rather_than_degrading() {
        let drift = BookFailure::ScopeDrift {
            asked: ironworks_domain::OrgId::parse("acme").unwrap(),
            answered: ironworks_domain::OrgId::parse("rival").unwrap(),
        };
        let refusal = scope_drift_exit(&drift).message;
        assert!(refusal.contains("REFUSING TO SERVE"));
        assert!(refusal.contains("not a records outage"));
    }

    #[test]
    fn an_unsent_failure_advises_a_retry_and_an_unknown_one_forbids_it() {
        assert!(
            describe(TurnFailure::NotSent(Cause::Connect)).message.contains("safe to try again")
        );
        assert!(
            describe(TurnFailure::Unknown(Cause::ReceiveTimeout)).message.contains("do NOT retry")
        );
    }

    /// The three crash points are the proof's only non-determinism budget: named, not timed.
    #[test]
    fn the_crash_points_are_named_not_timed() {
        for point in [
            "before-dispatch",
            "before-response",
            "before-commit",
            "after-output",
            "after-send",
            "after-delivered",
        ] {
            let args = Args {
                slug: "s".into(),
                guidance: "g".into(),
                service: "svc".into(),
                journal: "j".into(),
                mode: Mode::Turn { ask: "a".into() },
                crash_at: Some("something-else".into()),
                call_log: None,
            };
            // A point that is not the configured one does nothing at all.
            crash_if(&args, point);
        }
    }
}
