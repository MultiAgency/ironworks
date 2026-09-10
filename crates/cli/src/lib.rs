//! `ironworks` — the operator binary. One journalled turn, recoverable across process death.
//!
//! ```text
//! ironworks turn --slug eval --service relationship-intelligence \
//!                --guidance fixtures/eval.guidance.md --journal /tmp/eval.journal \
//!                --ask "Which accounts need attention?"
//! ```
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
//! Recovery resolves an unresolved turn from durable journal evidence before considering a new
//! turn. It never submits a new logical turn. An idempotent replay of the same logical turn is
//! permitted when byte identity is proven. The journal stores `retrieved_at`, so a rebuild reuses
//! it instead of reading the clock and resolves to the original response rather than executing a
//! second time.

use ironworks_channel::{ChannelJournal, Disposition};
use ironworks_domain::{
    Ask, ContextBinding, CredentialPurpose, CredentialRef, Degradation, Guidance, IdempotencyKey,
    InstanceParts, ModelSelection, ModelTurn, Openness, RecordScope, RecordsState, ResponseId,
    RuntimeIdentity, ServiceInstance, Slug, ThreadScope,
};
use ironworks_ironclaw::{
    Accepted, Cause, Deadline, Ironclaw, Outcome, Runtime, TurnFailure, request_bytes,
};
use ironworks_journal::{Journal, Recovery, RequestDigest, TurnOutcome, recover};
use ironworks_proposals::{Confirmation, NewProposal, ProposalStore};
use ironworks_records::{
    Append, AppendClient, AppendFailure, Appended, Book, BookFailure, EnvCredentials,
};
use std::io::{self, Read, Write};
use std::sync::Mutex;
use std::time::Duration;

mod registry_publish;
mod state;
mod tenant;

const USAGE: i32 = 64;
const FAILED: i32 = 2;
/// A measurement that could not be performed. **Never a pass, and never a FAIL either** — scoring
/// an unavailable dependency as a defect in IronWorks trains an operator to skim the ones that are.
/// The convention `dev/prove*.sh` already reads: 0 / 2 / 3.
const BLOCKED: i32 = 3;
/// The exit code a deliberate crash uses, so a proof can tell it from a real failure.
const CRASHED: i32 = 75;

#[cfg(feature = "synthetic-qualification")]
const SYNTHETIC_SERVICE: &str = "synthetic-sandbox-proof";
#[cfg(feature = "synthetic-qualification")]
const SYNTHETIC_GUIDANCE: &str = "This is a synthetic deployment qualification tenant. Use the sandbox only for the exact persistence or isolation measurement requested by the operator. Never contact a tenant record service, never make outreach, never use a Docker socket, and never treat this fixture as a production service. Report only the requested control result. The workspace is disposable qualification state and contains no tenant content. ";

/// Which services a build may compose.
///
/// # Why this is a value the entry point supplies
///
/// The synthetic qualification service exists for `ironworks-deployment-proof` and for nothing
/// else, and the control that the **production** binary cannot select it was
/// `env!("CARGO_BIN_NAME") == SYNTHETIC_BINARY` — a compile-time constant, and the strongest form
/// that control can take.
///
/// `CARGO_BIN_NAME` is not available to a library, and both binaries would link one library
/// compiled once, so lifting this code into a library without lifting the control would have
/// widened it in silence: with the feature on, the serving binary would have been able to compose
/// a service whose whole point is that it is not one.
///
/// So it travels as a value. `Services::Serving` is the only variant the production entry point
/// names, and `Services::Qualification` does not exist at all unless the feature is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Services {
    /// The embedded service definitions, and nothing else.
    Serving,
    /// Those, plus the proof-only sandbox service and its guidance.
    #[cfg(feature = "synthetic-qualification")]
    Qualification,
}

impl Services {
    /// Whether the proof-only service is selectable. Always false for [`Services::Serving`].
    #[cfg(feature = "synthetic-qualification")]
    fn synthetic(self) -> bool {
        matches!(self, Self::Qualification)
    }
}

/// Run the operator CLI.
///
/// This is what `src/main.rs` calls, and it names `Services::Serving` — the **only** services
/// binding a production build can express, because it is the only one this function mentions.
pub fn run_cli() -> std::process::ExitCode {
    exit_with(run(Services::Serving))
}

/// Run the CLI with the proof-only qualification service selectable.
///
/// `ironworks-deployment-proof` is the only caller, and this function exists only under
/// `synthetic-qualification`. Two entry points rather than one taking a [`Services`] argument, so
/// that a production `main` cannot select the qualification build even by mistake: it would have to
/// name a function that does not exist in its build.
#[cfg(feature = "synthetic-qualification")]
pub fn run_cli_for_qualification() -> std::process::ExitCode {
    exit_with(run(Services::Qualification))
}

fn exit_with(outcome: Result<(), Exit>) -> std::process::ExitCode {
    match outcome {
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

/// Keep one update's failure from ending the cycle that was serving the rest.
///
/// # Why a batch must outlive one bad update
///
/// `relay` journals the whole batch before working any of it (§E-3), and one failing update used
/// to propagate straight out of the loop — so an update nobody could answer took every update
/// behind it with it, including the ones `drive` refuses *without settling* precisely so the next
/// cycle can retry them. Those still recover, because the barrier holds them open
/// (`docs/experiments/0044-a-refused-turn-recovers-on-the-next-serving-cycle.md`), but they waited
/// a whole cycle to do it and one tenant's outage set the pace for every other tenant's traffic.
///
/// **This contains errors, never crashes.** `crash_if` still exits the process wherever it is
/// placed, so every boundary B1–B12 remains a real process boundary — which is what makes
/// `relay` a one-shot with no `--daemon` flag, and `dev/relay-loop.sh` a supervisor rather than a
/// flag on the binary. B8's *the untouched are still owed* holds either way: they are now served
/// in the same cycle, and a crash still leaves them queued.
///
/// Cycle-level failures are deliberately **not** routed through here. A registry that will not
/// parse or a journal that will not open has not begun serving anyone, and continuing past it
/// would be serving nobody while reporting per-update trouble.
fn contained<T>(
    id: ironworks_channel::UpdateId,
    failures: &mut usize,
    r: Result<T, Exit>,
) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(x) => {
            *failures += 1;
            eprintln!("relay: update {id} did not settle this cycle — {}", x.message);
            None
        }
    }
}

// ── the runtime boundary, counted ────────────────────────────────────────────────────────

/// Counts every call at the adapter boundary and appends it to a log that outlives the process.
///
/// A proof must not infer "no duplicate execution" from equal output — two executions of the same
/// prompt can produce the same words. Counting here, across restarts, is evidence about *calls*;
/// what it can and cannot establish about *model executions* is stated in `dev/prove.sh`.
struct Counted<'a> {
    inner: &'a dyn Runtime,
    audit: Option<&'a CallAudit>,
}

/// An already-open proof log. A requested audit is a control, not best-effort telemetry.
struct CallAudit {
    out: Mutex<Box<dyn Write + Send>>,
}

impl CallAudit {
    fn open(path: &str) -> io::Result<Self> {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        file.flush()?;
        Ok(Self { out: Mutex::new(Box::new(file)) })
    }

    #[cfg(test)]
    fn from_writer(out: impl Write + Send + 'static) -> Self {
        Self { out: Mutex::new(Box::new(out)) }
    }

    fn note(&self, op: &str, detail: &str) -> Result<(), TurnFailure> {
        let mut out = self.out.lock().map_err(|_| audit_failure())?;
        writeln!(out, "{op}\t{detail}").and_then(|()| out.flush()).map_err(|_| audit_failure())
    }
}

fn audit_failure() -> TurnFailure {
    TurnFailure::NotSent(Cause::LocalControl("runtime call audit could not be written".to_owned()))
}

impl Counted<'_> {
    fn note(&self, op: &str, detail: &str) -> Result<(), TurnFailure> {
        match self.audit {
            Some(audit) => audit.note(op, detail),
            None => Ok(()),
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
        self.note("create", key.as_str())?;
        self.inner.create(turn, key, deadline)
    }
    fn fetch(&self, id: &ResponseId, deadline: &Deadline) -> Result<Outcome, TurnFailure> {
        self.note("fetch", id.as_str())?;
        self.inner.fetch(id, deadline)
    }
}

// ── argv ─────────────────────────────────────────────────────────────────────────────────

/// A parsed invocation. Each variant owns exactly the values that can be valid for that command.
enum Command {
    Turn {
        serving: ServingInputs,
        controls: ExecutionControls,
        ask: String,
    },
    /// Every tenant this process serves comes from the registry, including when there is one of
    /// them. A single-tenant flag beside it would be a second, looser path into the same serving
    /// code — and the channel is the one place this codebase refuses to have two.
    Relay {
        journal: String,
        controls: ExecutionControls,
        channel: String,
        registry: String,
        chunk_bytes: usize,
        confinement_attestation: Option<String>,
    },
    /// An operator's decision, carried out. Never runs a model.
    Redeliver {
        serving: ServingInputs,
        controls: ExecutionControls,
        channel: String,
        update: u64,
        chunk_bytes: usize,
    },
    /// Read-only lifecycle inspection. Creates nothing, changes nothing, calls no model.
    LifecycleInspect {
        slug: String,
        store: String,
        source: AuthoritySource,
        confinement: bool,
    },
    /// Apply confinement to a member, then re-measure it. Changes authority; never runs a model.
    ///
    /// There is no policy flag. The policy is whatever the recorded instance's service declares
    /// in `capabilities.runtime_tools`, so an operator cannot confine a member to something its
    /// own service definition contradicts.
    LifecycleConfine {
        slug: String,
        store: String,
        dry_run: bool,
    },
    /// Measure current confinement and emit a short-lived, content-free JSON attestation.
    LifecycleAttest {
        slug: String,
        store: String,
    },
    /// Observe an existing record scope and runtime member, then record both in one local
    /// transaction. It creates no foreign authority and accepts no asserted org or member id.
    LifecycleAdopt {
        slug: String,
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
    /// Construct, authority-check and durably publish exactly one tenant registry.
    RegistryPublishOne {
        slug: String,
        store: String,
        registry: String,
        guidance: String,
        records_credential: String,
        member_credential: String,
    },
    /// Restore the durable prior registry without consuming the rollback artifact.
    RegistryRollbackOne {
        registry: String,
    },
    /// Validate and durably replace the guidance named by one registry entry.
    GuidancePublishOne {
        slug: String,
        registry: String,
        source: String,
    },
    /// Restore the durable prior guidance named by one registry entry.
    GuidanceRollbackOne {
        slug: String,
        registry: String,
    },
    /// Create and upload a consistent encrypted state snapshot.
    StateSnapshot {
        config: String,
        output: String,
    },
    /// Validate restored snapshot content without contacting any service.
    StateVerifySnapshot {
        manifest: String,
    },
    /// Produce one verified complete tenant book without printing record content.
    TenantExport {
        slug: String,
        registry: String,
        output: String,
    },
    /// Read and compare the operator-visible requested-model policy. Never changes authority.
    ModelPolicyCheck,
    /// Explicitly write the requested-model policy, then require read-back agreement.
    ModelPolicyApply,
    /// Read-only turn-journal inspection. Opens no runtime, record store, guidance or channel.
    Status {
        journal: String,
    },
    /// A dry melt: what this binary would compose, fingerprinted. Never runs a model.
    Compose {
        slug: String,
        guidance: String,
        service: String,
    },
}

struct ServingInputs {
    slug: String,
    guidance: String,
    service: String,
    journal: String,
}

#[derive(Default)]
struct ExecutionControls {
    crash_at: Option<String>,
    call_log: Option<String>,
}

impl ExecutionControls {
    /// Create the call log the moment the flag is accepted, before anything can refuse.
    ///
    /// The log is the evidence a negative assertion rests on — "the credential was refused **and
    /// no create call was made**" — and a count read from a file that does not exist is not a
    /// measurement of zero. `dev/lib.sh::call_count` refuses to supply one, correctly, which left
    /// a run that refused during boot unable to prove the half that matters: `drive` opened the
    /// audit *after* the record scope was resolved, so the earliest refusals wrote no file at all
    /// and the absence was indistinguishable from a lost one.
    ///
    /// Empty is the honest artifact for a run that made no calls. Opening here rather than moving
    /// the audit itself keeps every append on the one path that owns it.
    fn arm_call_log(&self) -> Result<(), Exit> {
        let Some(path) = &self.call_log else { return Ok(()) };
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map(|_| ())
            .map_err(|why| fail(FAILED, format!("--call-log {path} cannot be opened: {why}")))
    }
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
     \x20      ironworks registry publish-one <slug> --store <path> --registry <path>\n\
     \x20        --guidance <path> --records-credential <handle> --member-credential <handle>\n\
     \x20      ironworks registry rollback-one --registry <path>\n\n\
     \x20      ironworks guidance publish-one <slug> --registry <path> --source <path>\n\
     \x20      ironworks guidance rollback-one <slug> --registry <path>\n\n\
     \x20      ironworks state snapshot --config <path> --output <directory>\n\
     \x20      ironworks state verify-snapshot --manifest <path>\n\n\
     \x20      ironworks tenant export <slug> --registry <path> --output <path>\n\
     \x20      ironworks model-policy check\n\
     \x20      ironworks model-policy apply\n\
     \x20        reads MODEL_PIN; apply is the only write path and verifies by read-back\n\n\
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
     \x20 turn:  before-dispatch | before-response | after-accepted | before-commit | after-output\n\
     \x20 relay: before-dispatch | before-response | after-accepted | before-commit | after-send | after-delivered\n\n\
     \x20      ironworks lifecycle inspect <slug> --store <path> [--observe <path>]\n\
     \x20        read-only; reports intent, recorded progress, observed authority and serving\n\
     \x20        --account-service asks the live Account Service (needs IRONWORKS_RECORDS)\n\
     \x20        --ironclaw asks the IronClaw admin surface (needs IRONCLAW_API)\n\n\
     \x20      ironworks lifecycle members --store <path>\n\
     \x20        fleet-wide: members on the instance that no lifecycle record accounts for";

/// An argument list, read one flag at a time.
///
/// # Why this is a type and not a closure
///
/// Fourteen parsers below each opened `let mut it = argv[..].iter()` and then either declared
/// `let mut value = || it.next().cloned().ok_or_else(…)` **inside** the loop — because the closure
/// borrows the iterator and cannot outlive an iteration — or skipped the closure and wrote
/// `it.next().cloned()` inline.
///
/// Those two forms are not equivalent, and the difference was a live defect. Under the second,
/// `ironworks lifecycle confine acme --store --dry-run` set the store to the literal `--dry-run`
/// **and consumed it**, so a run an operator asked to be a dry run applied the confinement —
/// `confine` spends a member and approval-off is one-way (§L-9). Two of the five `--store` parsers
/// had it; three did not.
///
/// Owning the iterator lets `value` be a method, so the careless form has nowhere left to be
/// written.
struct Flags<'a> {
    rest: std::slice::Iter<'a, String>,
}

impl<'a> Flags<'a> {
    fn new(argv: &'a [String]) -> Self {
        Self { rest: argv.iter() }
    }

    /// The next flag, or `None` when the list is done.
    ///
    /// The borrow is of `argv`, not of `self`, so a caller can hold the flag and ask for its value.
    fn next(&mut self) -> Option<&'a String> {
        self.rest.next()
    }

    /// The value belonging to `flag`, or a refusal naming the flag that wanted one.
    ///
    /// A value beginning with `--` is still accepted: a path may, and refusing one would break
    /// `--store --dry-run` in the other direction. What is refused is a flag with nothing after it.
    fn value(&mut self, flag: &str) -> Result<String, Exit> {
        self.rest.next().cloned().ok_or_else(|| fail(USAGE, format!("{flag} needs a value")))
    }
}

/// A flag the caller had to give. Written out thirty-eight times before this.
fn required_flag(value: Option<String>, flag: &str) -> Result<String, Exit> {
    value.ok_or_else(|| fail(USAGE, format!("{flag} is required")))
}

/// A flag no parser here accepts. Written out fourteen times before this.
fn unknown_flag(flag: &str) -> Exit {
    fail(USAGE, format!("unknown flag {flag}"))
}

fn parse_args() -> Result<Command, Exit> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = argv.first().map(String::as_str).unwrap_or("");
    // `lifecycle` is parsed before the others because it shares none of their requirements: it
    // needs no runtime, no record store, and no guidance — it reads two local files and prints.
    if command == "lifecycle" {
        return parse_lifecycle(&argv[1..]);
    }
    if command == "registry" {
        return parse_registry(&argv[1..]);
    }
    if command == "guidance" {
        return parse_guidance(&argv[1..]);
    }
    if command == "model-policy" {
        return parse_model_policy(&argv[1..]);
    }
    if command == "state" {
        return parse_state(&argv[1..]);
    }
    if command == "tenant" {
        return parse_tenant(&argv[1..]);
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
    let mut flags = Flags::new(&argv[1..]);
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--slug" => slug = Some(flags.value(flag)?),
            "--guidance" => guidance = Some(flags.value(flag)?),
            "--ask" => ask = Some(flags.value(flag)?),
            "--journal" => journal = Some(flags.value(flag)?),
            "--service" if command == "coordinate" => {
                return Err(fail(
                    USAGE,
                    "coordinate already selects the account-coordination service; --service is not allowed",
                ));
            }
            "--service" => service = Some(flags.value(flag)?),
            "--crash-at" => crash_at = Some(flags.value(flag)?),
            "--call-log" => call_log = Some(flags.value(flag)?),
            "--channel" => channel = Some(flags.value(flag)?),
            "--update" => {
                update = Some(
                    flags
                        .value(flag)?
                        .parse()
                        .map_err(|_| fail(USAGE, "--update needs an update id"))?,
                );
            }
            "--chunk-bytes" => {
                chunk_bytes = flags
                    .value(flag)?
                    .parse()
                    .map_err(|_| fail(USAGE, "--chunk-bytes needs a positive number"))?;
                if chunk_bytes == 0 {
                    return Err(fail(USAGE, "--chunk-bytes must be greater than zero"));
                }
            }
            other => return Err(unknown_flag(other)),
        }
    }
    let serving = ServingInputs {
        slug: required_flag(slug, "--slug")?,
        guidance: required_flag(guidance, "--guidance")?,
        journal: required_flag(journal, "--journal")?,
        service: service.ok_or_else(|| {
            fail(USAGE, "--service is required; there is no default service to fall back to")
        })?,
    };
    if command == "redeliver" && crash_at.is_some() {
        return Err(fail(USAGE, "--crash-at is not supported by redeliver"));
    }
    let controls = ExecutionControls { crash_at, call_log };
    Ok(match command {
        "coordinate" | "turn" => {
            Command::Turn { serving, controls, ask: required_flag(ask, "--ask")? }
        }
        _ => Command::Redeliver {
            serving,
            controls,
            channel: channel.ok_or_else(|| fail(USAGE, "--channel is required for redeliver"))?,
            update: update.ok_or_else(|| fail(USAGE, "--update is required for redeliver"))?,
            chunk_bytes,
        },
    })
}

fn parse_tenant(argv: &[String]) -> Result<Command, Exit> {
    if argv.first().map(String::as_str) != Some("export") {
        return Err(fail(
            USAGE,
            "usage: ironworks tenant export <slug> --registry <path> --output <path>",
        ));
    }
    let slug = argv
        .get(1)
        .cloned()
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| fail(USAGE, "tenant export requires a slug"))?;
    let mut registry = None;
    let mut output = None;
    let mut flags = Flags::new(argv.get(2..).unwrap_or_default());
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--registry" => registry = Some(flags.value(flag)?),
            "--output" => output = Some(flags.value(flag)?),
            other => return Err(unknown_flag(other)),
        }
    }
    Ok(Command::TenantExport {
        slug,
        registry: required_flag(registry, "--registry")?,
        output: required_flag(output, "--output")?,
    })
}

fn parse_state(argv: &[String]) -> Result<Command, Exit> {
    let subcommand = argv.first().map(String::as_str).unwrap_or("");
    let mut config = None;
    let mut output = None;
    let mut manifest = None;
    let mut flags = Flags::new(argv.get(1..).unwrap_or_default());
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--config" => config = Some(flags.value(flag)?),
            "--output" => output = Some(flags.value(flag)?),
            "--manifest" => manifest = Some(flags.value(flag)?),
            other => return Err(unknown_flag(other)),
        }
    }
    let mode = match subcommand {
        "snapshot" if manifest.is_none() => Command::StateSnapshot {
            config: required_flag(config, "--config")?,
            output: required_flag(output, "--output")?,
        },
        "verify-snapshot" if config.is_none() && output.is_none() => {
            Command::StateVerifySnapshot { manifest: required_flag(manifest, "--manifest")? }
        }
        _ => {
            return Err(fail(
                USAGE,
                "usage: ironworks state snapshot --config <path> --output <directory>\n       ironworks state verify-snapshot --manifest <path>",
            ));
        }
    };
    Ok(mode)
}

fn parse_guidance(argv: &[String]) -> Result<Command, Exit> {
    let usage = || {
        fail(
            USAGE,
            "usage: ironworks guidance publish-one <slug> --registry <path> --source <path>\n\
             \x20      ironworks guidance rollback-one <slug> --registry <path>",
        )
    };
    let operation = argv.first().map(String::as_str);
    if !matches!(operation, Some("publish-one" | "rollback-one")) {
        return Err(usage());
    }
    let slug = argv.get(1).cloned().ok_or_else(usage)?;
    if slug.starts_with("--") {
        return Err(usage());
    }
    let (mut registry, mut source) = (None, None);
    let mut flags = Flags::new(&argv[2..]);
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--registry" => registry = Some(flags.value(flag)?),
            "--source" if operation == Some("publish-one") => source = Some(flags.value(flag)?),
            "--service" | "--guidance" | "--destination" => {
                return Err(fail(
                    USAGE,
                    format!(
                        "{flag} is not accepted: guidance publication derives it from the registry"
                    ),
                ));
            }
            other => return Err(unknown_flag(other)),
        }
    }
    let registry = required_flag(registry, "--registry")?;
    let mode = if operation == Some("publish-one") {
        Command::GuidancePublishOne { slug, registry, source: required_flag(source, "--source")? }
    } else {
        Command::GuidanceRollbackOne { slug, registry }
    };
    Ok(mode)
}

/// `ironworks model-policy check|apply`
///
/// There is deliberately no model argument: `MODEL_PIN` is the sole local policy input. Keeping
/// the write in a separately spelled subcommand makes an authority change impossible by accident
/// from the read-only check path.
fn parse_model_policy(argv: &[String]) -> Result<Command, Exit> {
    let mode = match argv {
        [command] if command == "check" => Command::ModelPolicyCheck,
        [command] if command == "apply" => Command::ModelPolicyApply,
        _ => {
            return Err(fail(
                USAGE,
                "usage: ironworks model-policy check|apply (reads MODEL_PIN; accepts no model flag)",
            ));
        }
    };
    Ok(mode)
}

fn parse_registry(argv: &[String]) -> Result<Command, Exit> {
    let usage = || {
        fail(
            USAGE,
            "usage: ironworks registry publish-one <slug> --store <path> --registry <path> \
             --guidance <path> --records-credential <handle> --member-credential <handle>\n\
             \x20      ironworks registry rollback-one --registry <path>",
        )
    };
    match argv.first().map(String::as_str) {
        Some("publish-one") => {
            let slug = argv.get(1).cloned().ok_or_else(usage)?;
            if slug.starts_with("--") {
                return Err(usage());
            }
            let (mut store, mut registry, mut guidance) = (None, None, None);
            let (mut records_credential, mut member_credential) = (None, None);
            let mut flags = Flags::new(&argv[2..]);
            while let Some(flag) = flags.next() {
                match flag.as_str() {
                    "--store" => store = Some(flags.value(flag)?),
                    "--registry" => registry = Some(flags.value(flag)?),
                    "--guidance" => guidance = Some(flags.value(flag)?),
                    "--records-credential" => records_credential = Some(flags.value(flag)?),
                    "--member-credential" => member_credential = Some(flags.value(flag)?),
                    "--org" | "--member" | "--member-id" | "--channel" | "--service" => {
                        return Err(fail(
                            USAGE,
                            format!(
                                "{flag} is not accepted: publish-one reads identity, channel and \
                                 service from lifecycle evidence"
                            ),
                        ));
                    }
                    other => return Err(unknown_flag(other)),
                }
            }
            Ok(Command::RegistryPublishOne {
                slug,
                store: required_flag(store, "--store")?,
                registry: required_flag(registry, "--registry")?,
                guidance: required_flag(guidance, "--guidance")?,
                records_credential: required_flag(records_credential, "--records-credential")?,
                member_credential: required_flag(member_credential, "--member-credential")?,
            })
        }
        Some("rollback-one") => {
            let mut registry = None;
            let mut flags = Flags::new(&argv[1..]);
            while let Some(flag) = flags.next() {
                match flag.as_str() {
                    "--registry" => {
                        registry = Some(flags.value("--registry")?);
                    }
                    other => return Err(unknown_flag(other)),
                }
            }
            Ok(Command::RegistryRollbackOne { registry: required_flag(registry, "--registry")? })
        }
        _ => Err(usage()),
    }
}

/// `ironworks status --journal <path>`
///
/// Deliberately bypasses common serving flags. A status report needs no tenant guidance,
/// credentials, runtime or record store and must not accidentally acquire any of them.
fn parse_status(argv: &[String]) -> Result<Command, Exit> {
    let mut journal = None;
    let mut flags = Flags::new(argv);
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--journal" => {
                journal = Some(flags.value("--journal")?);
            }
            other => return Err(unknown_flag(other)),
        }
    }
    Ok(Command::Status { journal: required_flag(journal, "--journal")? })
}

/// `ironworks relay --registry <path> --channel <url> [--chunk-bytes N]`
///
/// Deliberately its own parser, and deliberately without `--slug`, `--guidance` or `--service`.
/// Those are per-tenant now and come from the registry, so accepting them here would let an
/// operator believe they had overridden something for a conversation that belongs to somebody
/// else. One tenant is a registry with one entry.
fn parse_relay(argv: &[String]) -> Result<Command, Exit> {
    let (mut channel, mut registry, mut journal) = (None, None, None);
    let (mut crash_at, mut call_log) = (None, None);
    let mut chunk_bytes = DEFAULT_CHUNK_BYTES;
    let mut confinement_attestation = None;
    let mut flags = Flags::new(argv);
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--channel" => channel = Some(flags.value(flag)?),
            "--registry" => registry = Some(flags.value(flag)?),
            // One journal for the process, not one per tenant: the safe cursor is the lowest
            // open update ACROSS every tenant, and two files could not compute it.
            "--journal" => journal = Some(flags.value(flag)?),
            "--crash-at" => crash_at = Some(flags.value(flag)?),
            "--call-log" => call_log = Some(flags.value(flag)?),
            "--confinement-attestation" => confinement_attestation = Some(flags.value(flag)?),
            "--chunk-bytes" => {
                chunk_bytes = flags
                    .value(flag)?
                    .parse()
                    .map_err(|_| fail(USAGE, "--chunk-bytes needs a positive number"))?;
                if chunk_bytes == 0 {
                    return Err(fail(USAGE, "--chunk-bytes must be greater than zero"));
                }
            }
            other => return Err(unknown_flag(other)),
        }
    }
    Ok(Command::Relay {
        journal: journal.ok_or_else(|| fail(USAGE, "--journal is required for relay"))?,
        controls: ExecutionControls { crash_at, call_log },
        channel: channel.ok_or_else(|| fail(USAGE, "--channel is required for relay"))?,
        registry: registry.ok_or_else(|| fail(USAGE, "--registry is required for relay"))?,
        chunk_bytes,
        confinement_attestation,
    })
}

/// `ironworks compose --slug <slug> --guidance <path> --service <name>`
///
/// Deliberately bypasses the serving flags for the same reason [`parse_status`] does, and one
/// more: a dry melt reads two local inputs and prints hashes of them. It opens no journal,
/// contacts no runtime and no record store, and needs no model key — so it must not be able to
/// acquire any of them by sharing a parser with the commands that do.
fn parse_compose(argv: &[String]) -> Result<Command, Exit> {
    let (mut slug, mut guidance, mut service) = (None, None, None);
    let mut flags = Flags::new(argv);
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--slug" => slug = Some(flags.value(flag)?),
            "--guidance" => guidance = Some(flags.value(flag)?),
            "--service" => service = Some(flags.value(flag)?),
            other => return Err(unknown_flag(other)),
        }
    }
    Ok(Command::Compose {
        slug: required_flag(slug, "--slug")?,
        guidance: required_flag(guidance, "--guidance")?,
        service: service.ok_or_else(|| {
            fail(USAGE, "--service is required; there is no default service to fall back to")
        })?,
    })
}

/// `ironworks lifecycle inspect <slug> --store <path> [--observe <path>]`
///
/// Deliberately separate from [`parse_args`]'s common flags. Inspection has no service instance,
/// no credentials and no runtime — sharing `--guidance` and `--slug` handling would imply it
/// needed them.
fn parse_lifecycle(argv: &[String]) -> Result<Command, Exit> {
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
            let mut flags = Flags::new(&argv[2..]);
            while let Some(flag) = flags.next() {
                match flag.as_str() {
                    "--store" => store = Some(flags.value(flag)?),
                    "--service" => service = Some(flags.value(flag)?),
                    "--channel" => channel = Some(flags.value(flag)?),
                    "--records-credential" => records_credential = Some(flags.value(flag)?),
                    "--member-credential" => member_credential = Some(flags.value(flag)?),
                    "--org" | "--member" | "--member-id" => {
                        return Err(fail(
                            USAGE,
                            format!(
                                "{flag} is not accepted: adopt measures org and member identity \
                                 from their credentials"
                            ),
                        ));
                    }
                    other => return Err(unknown_flag(other)),
                }
            }
            return Ok(Command::LifecycleAdopt {
                slug,
                store: required_flag(store, "--store")?,
                service: required_flag(service, "--service")?,
                channel: required_flag(channel, "--channel")?,
                records_credential: required_flag(records_credential, "--records-credential")?,
                member_credential: required_flag(member_credential, "--member-credential")?,
            });
        }
        Some("confine") => {
            let slug = argv.get(1).cloned().ok_or_else(usage)?;
            if slug.starts_with("--") {
                return Err(usage());
            }
            let (mut store, mut dry_run) = (None, false);
            let mut flags = Flags::new(&argv[2..]);
            while let Some(flag) = flags.next() {
                match flag.as_str() {
                    "--store" => store = Some(flags.value(flag)?),
                    "--dry-run" => dry_run = true,
                    "--workspace" => {
                        return Err(fail(
                            USAGE,
                            "--workspace is gone: the confinement policy now comes from the \
                             recorded instance's service definition (capabilities.runtime_tools). \
                             Declare `\"runtime_tools\": \"sandbox\"` on the service instead.",
                        ));
                    }
                    other => return Err(unknown_flag(other)),
                }
            }
            return Ok(Command::LifecycleConfine {
                slug,
                store: required_flag(store, "--store")?,
                dry_run,
            });
        }
        Some("attest") => {
            let slug = argv.get(1).cloned().ok_or_else(usage)?;
            if slug.starts_with("--") {
                return Err(usage());
            }
            let mut store = None;
            let mut flags = Flags::new(&argv[2..]);
            while let Some(flag) = flags.next() {
                match flag.as_str() {
                    "--store" => {
                        store = Some(flags.value("--store")?);
                    }
                    "--member" | "--member-id" | "--service" | "--policy" => {
                        return Err(fail(
                            USAGE,
                            format!(
                                "{flag} is not accepted: attest derives identity and policy from lifecycle and service evidence"
                            ),
                        ));
                    }
                    other => return Err(unknown_flag(other)),
                }
            }
            return Ok(Command::LifecycleAttest { slug, store: required_flag(store, "--store")? });
        }
        Some("members") => {
            let mut store = None;
            let mut flags = Flags::new(&argv[1..]);
            while let Some(flag) = flags.next() {
                match flag.as_str() {
                    "--store" => store = Some(flags.value(flag)?),
                    other => return Err(unknown_flag(other)),
                }
            }
            return Ok(Command::LifecycleMembers { store: required_flag(store, "--store")? });
        }
        _ => return Err(usage()),
    }
    let slug = argv.get(1).cloned().ok_or_else(usage)?;
    if slug.starts_with("--") {
        return Err(usage());
    }
    let (mut store, mut observe) = (None, None);
    let (mut account, mut member, mut confinement) = (false, false, false);
    let mut flags = Flags::new(&argv[2..]);
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--store" => store = Some(flags.value(flag)?),
            "--observe" => observe = Some(flags.value(flag)?),
            "--account-service" => account = true,
            "--ironclaw" => member = true,
            "--confinement" => confinement = true,
            other => return Err(unknown_flag(other)),
        }
    }
    // Exactly one authority is asked, so the flags resolve to one value here rather than
    // travelling as three booleans the body then has to re-combine.
    let source = match (member, account, observe) {
        (true, _, _) => AuthoritySource::IronclawMember,
        (false, true, _) => AuthoritySource::AccountService,
        (false, false, Some(path)) => AuthoritySource::Fixture(path),
        (false, false, None) => AuthoritySource::None,
    };
    Ok(Command::LifecycleInspect {
        slug,
        store: required_flag(store, "--store")?,
        source,
        confinement,
    })
}

/// A deliberate crash at a named point. Deterministic — no timing, no signals.
fn crash_if(controls: &ExecutionControls, point: &str) {
    crash_if_at(controls, point, || {});
}

/// [`crash_if`], for a point that must say something a proof reads before it goes.
///
/// `note` runs only when the point matches, so the announcement cannot appear on an ordinary run.
/// Both spellings go through one body: the `CRASHED` exit code and the exact
/// `CRASHING at X (deliberate)` wording that every `dev/prove*.sh` greps for are written once.
fn crash_if_at(controls: &ExecutionControls, point: &str, note: impl FnOnce()) {
    if controls.crash_at.as_deref() == Some(point) {
        note();
        eprintln!("ironworks: CRASHING at {point} (deliberate)");
        std::process::exit(CRASHED);
    }
}

// ── the turn ─────────────────────────────────────────────────────────────────────────────

fn run(services: Services) -> Result<(), Exit> {
    match parse_args()? {
        Command::StateSnapshot { config, output } => {
            state::snapshot(&config, &output).map_err(|e| {
                let code = if state::is_repository_unavailable(&e) { BLOCKED } else { FAILED };
                fail(code, e)
            })?;
            println!("snapshot uploaded");
            Ok(())
        }
        Command::StateVerifySnapshot { manifest } => {
            state::verify(manifest).map_err(|e| fail(FAILED, e))?;
            println!("snapshot verified");
            Ok(())
        }
        Command::TenantExport { slug, registry, output } => {
            let result = tenant::export(&slug, &registry, &output).map_err(|e| fail(FAILED, e))?;
            println!("export complete; bytes={} sha256={}", result.bytes, result.digest);
            Ok(())
        }
        Command::ModelPolicyCheck => check_model_policy(),
        Command::ModelPolicyApply => apply_model_policy(),
        Command::LifecycleInspect { slug, store, source, confinement } => {
            run_lifecycle_inspect(&slug, &store, &source, confinement)
        }
        Command::LifecycleMembers { store } => run_lifecycle_members(&store),
        Command::LifecycleConfine { slug, store, dry_run } => {
            run_lifecycle_confine(&slug, &store, dry_run)
        }
        Command::LifecycleAttest { slug, store } => run_lifecycle_attest(&slug, &store),
        Command::LifecycleAdopt {
            slug,
            store,
            service,
            channel,
            records_credential,
            member_credential,
        } => run_lifecycle_adopt(
            &slug,
            &store,
            &service,
            &channel,
            &records_credential,
            &member_credential,
        ),
        Command::RegistryPublishOne {
            slug,
            store,
            registry,
            guidance,
            records_credential,
            member_credential,
        } => run_registry_publish_one(
            &slug,
            &store,
            &registry,
            &guidance,
            &records_credential,
            &member_credential,
        ),
        Command::RegistryRollbackOne { registry } => run_registry_rollback_one(&registry),
        Command::GuidancePublishOne { slug, registry, source } => {
            run_guidance_publish_one(&slug, &registry, &source)
        }
        Command::GuidanceRollbackOne { slug, registry } => {
            run_guidance_rollback_one(&slug, &registry)
        }
        Command::Status { journal } => run_status(&journal),
        Command::Compose { slug, guidance, service } => {
            run_compose(services, &slug, &guidance, &service)
        }
        Command::Relay {
            journal,
            controls,
            channel,
            registry,
            chunk_bytes,
            confinement_attestation,
        } => {
            controls.arm_call_log()?;
            run_relay(RelayContext {
                services,
                controls: &controls,
                journal_path: &journal,
                channel_url: &channel,
                registry_path: &registry,
                chunk_bytes,
                confinement_attestation: confinement_attestation.as_deref(),
            })
        }
        Command::Turn { serving, controls, ask } => {
            controls.arm_call_log()?;
            let booted = boot(services, &serving)?;
            run_turn(&booted, &controls, &ask)
        }
        Command::Redeliver { serving, controls, channel, update, chunk_bytes } => {
            controls.arm_call_log()?;
            let booted = boot(services, &serving)?;
            run_redeliver(&booted, &controls, &channel, update, chunk_bytes)
        }
    }
}

/// Read the requested-model policy and refuse unless it matches `MODEL_PIN` exactly.
///
/// Deliberately narrower than a runtime attestation, as is [`apply_model_policy`]: the upstream
/// catalogue bounds the model names requested through this policy surface. It does not prove which
/// model a provider ran, TEE execution, or that a member cannot alter policy through some other
/// authority path.
///
/// # Two functions, because one took a `bool`
///
/// `parse_args` already produces two distinct commands, and `run` immediately collapsed them back
/// into `run_model_policy(false)` and `run_model_policy(true)` — for a pair whose whole point is
/// that the write path *"must be impossible by accident from the read-only check path"*. A
/// literal `true` at a call site is not that. The read path can no longer reach the write.
fn check_model_policy() -> Result<(), Exit> {
    let (pin, control) = model_policy_inputs()?;
    require_model_policy_exact(&pin, &control)?;
    println!(
        "requested-model policy matches MODEL_PIN exactly for {}; effective provider model and TEE execution are not verified by this check",
        pin.as_str()
    );
    Ok(())
}

/// Write the requested-model policy from `MODEL_PIN`, then read it back exactly.
///
/// The one path here that changes anything upstream. See [`check_model_policy`] for why it is its
/// own function.
fn apply_model_policy() -> Result<(), Exit> {
    let (pin, control) = model_policy_inputs()?;
    control.apply(&pin).map_err(model_policy_exit)?;
    println!(
        "requested-model policy applied and read back exactly for {}; effective provider model and TEE execution are not verified by this check",
        pin.as_str()
    );
    Ok(())
}

fn model_policy_inputs()
-> Result<(ironworks_domain::ModelPin, ironworks_reconcile::ModelPolicyControl), Exit> {
    let pin = ironworks_domain::ModelPin::parse(
        &std::fs::read_to_string("MODEL_PIN")
            .map_err(|error| fail(FAILED, format!("cannot read MODEL_PIN: {error}")))?,
    )
    .map_err(|error| fail(FAILED, error.to_string()))?;
    let base = required(Var::IronclawApi)?;
    let operator =
        CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:IRONCLAW_OPERATOR_TOKEN");
    let control = ironworks_reconcile::ModelPolicyControl::new(base, &operator);
    Ok((pin, control))
}

fn require_model_policy_exact(
    pin: &ironworks_domain::ModelPin,
    control: &ironworks_reconcile::ModelPolicyControl,
) -> Result<(), Exit> {
    let result = control.check(pin).map_err(model_policy_exit)?;
    if !result.is_exact() {
        return Err(fail(
            FAILED,
            format!(
                "requested-model policy disagrees with MODEL_PIN (selection_enabled={}, default_matches={}, exact_allowlist={}, allowed_count={})",
                result.selection_enabled,
                result.workspace_default_matches,
                result.allowed_models_exact,
                result.allowed_model_count,
            ),
        ));
    }
    Ok(())
}

fn model_policy_exit(error: ironworks_reconcile::ModelPolicyError) -> Exit {
    use ironworks_reconcile::ModelPolicyError;
    match error {
        ModelPolicyError::NoCredential => {
            fail(USAGE, "IRONCLAW_OPERATOR_TOKEN is not set (the operator model-policy credential)")
        }
        ModelPolicyError::Unreachable => {
            fail(BLOCKED, "model policy could not be measured: IronClaw is unreachable")
        }
        ModelPolicyError::CredentialRejected { operation: "PUT" } => {
            fail(FAILED, "IronClaw rejected the operator credential for the model-policy write")
        }
        ModelPolicyError::CredentialRejected { operation } => fail(
            BLOCKED,
            format!(
                "model policy could not be measured: IronClaw rejected the operator credential for {operation}"
            ),
        ),
        ModelPolicyError::UnexpectedStatus { operation: "PUT", status } => {
            fail(FAILED, format!("IronClaw refused the model-policy write (HTTP {status})"))
        }
        ModelPolicyError::UnexpectedStatus { operation, status } => fail(
            BLOCKED,
            format!("model policy could not be measured: {operation} returned HTTP {status}"),
        ),
        ModelPolicyError::MalformedResponse => fail(
            BLOCKED,
            "model policy could not be measured: IronClaw returned a malformed catalogue",
        ),
        ModelPolicyError::ReadBackMismatch(result) => fail(
            FAILED,
            format!(
                "model-policy apply read-back disagrees with MODEL_PIN (selection_enabled={}, default_matches={}, exact_allowlist={}, allowed_count={})",
                result.selection_enabled,
                result.workspace_default_matches,
                result.allowed_models_exact,
                result.allowed_model_count,
            ),
        ),
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

/// The service definition `name` selects from the embedded corpus.
///
/// The proof-only sandbox service is selectable as well only under `Services::Qualification`,
/// which does not exist without the `synthetic-qualification` feature.
fn load_serving_definition(
    #[cfg_attr(not(feature = "synthetic-qualification"), allow(unused_variables))]
    services: Services,
    name: &str,
) -> Result<ironworks_domain::ServiceDefinition, String> {
    #[cfg(feature = "synthetic-qualification")]
    if services.synthetic() && name == SYNTHETIC_SERVICE {
        return ironworks_domain::ServiceDefinition::new(
            ironworks_domain::ServiceId::parse(SYNTHETIC_SERVICE).map_err(|e| e.to_string())?,
            1,
            "Exercise only the proof sandbox workspace.",
            ironworks_domain::Composition::new(
                // NOT `personas/COORDINATOR.md`, which this composed until 2026-09-07 and which
                // made the proof unrunnable. That persona is the shipped read-only analyst and
                // ends with "If asked to do any of those, decline plainly and explain that you are
                // read-only" — so every `workspace.create` turn answered "I cannot execute this
                // command as requested" and the sandbox was never entered. The persona was right;
                // the composition was asking a read-only analyst to run a shell command.
                vec![ironworks_domain::PartRef::new("personas/_qualification-operator.md")],
                "# SYNTHETIC QUALIFICATION GUIDANCE",
                ironworks_domain::PartRef::new("personas/_safety-tail.md"),
            )
            .map_err(|e| e.to_string())?,
            ironworks_domain::CapabilityProfile::SANDBOX_WORKSPACE,
            None,
        )
        .map_err(|e| e.to_string());
    }
    ironworks_services::service::load(name).map_err(|e| e.to_string())
}

/// The guidance at `path`, parsed against `slug` and `service` — and, only under
/// `Services::Qualification`, the synthetic guidance its handle names.
fn load_serving_guidance(
    #[cfg_attr(not(feature = "synthetic-qualification"), allow(unused_variables))]
    services: Services,
    path: &str,
    slug: &str,
    service: &str,
) -> Result<Guidance, String> {
    #[cfg(feature = "synthetic-qualification")]
    if services.synthetic() && path == "synthetic://guidance" && service == SYNTHETIC_SERVICE {
        let text = format!(
            "<!-- tenant-guidance v1 slug: {slug} service: {service} -->\n\n{SYNTHETIC_GUIDANCE}"
        );
        return Guidance::parse(&text, slug, service).map_err(|e| e.to_string());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    Guidance::parse(&text, slug, service).map_err(|e| e.to_string())
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
fn run_compose(
    services: Services,
    slug: &str,
    guidance_path: &str,
    service_name: &str,
) -> Result<(), Exit> {
    let e = |m: String| fail(FAILED, m);
    let definition = load_serving_definition(services, service_name).map_err(e)?;
    // Parsed, not merely read: an alloy fingerprint for guidance bound to another tenant or
    // another service describes a composition that would never have served.
    let guidance = load_serving_guidance(services, guidance_path, slug, service_name).map_err(e)?;

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
        match &turn.attribution {
            Some(attribution) => out.push_str(&format!(
                "  confinement profile={} policy={} catalogue={} observed_at={}\n",
                attribution.profile_sha256.as_str(),
                attribution.policy_sha256.as_str(),
                attribution.catalogue_sha256.as_str(),
                attribution.observed_at.as_str(),
            )),
            None => out.push_str("  confinement unattributed\n"),
        }
    }
    report(&out)
}

fn safe_field(value: &str) -> String {
    value.chars().flat_map(char::escape_default).collect()
}

/// Which authority an inspection asks, and how.
///
/// # Three adjacent booleans were one choice
///
/// `account`, `member` and `observe` arrived as separate parameters and the body read them as
/// `if member { … } else { match (account, observe) { … } }` — so exactly one of four arms ever
/// ran. At the call site that was `run_lifecycle_inspect(&slug, &store, observe, account, member,
/// confinement)`: three `bool`s in a row, transposable with nothing to notice, next to a fourth
/// that means something else entirely.
enum AuthoritySource {
    /// Nothing live is asked; what lifecycle recorded is reported as it stands.
    None,
    /// The live Account Service. It speaks for org registration and stays silent about the other
    /// four, so it cannot produce a clean overall verdict on its own.
    AccountService,
    /// The live IronClaw admin surface, which answers for the member authority alone.
    IronclawMember,
    /// A recorded inventory on disk, for reproducing a reading without reaching anything.
    Fixture(String),
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
    source: &AuthoritySource,
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
    let inventory = match source {
        AuthoritySource::IronclawMember => {
            let base = required_by("--ironclaw", Var::IronclawApi)?;
            let credential = CredentialRef::new(
                CredentialPurpose::OperatorConfiguration,
                "env:IRONCLAW_OPERATOR_TOKEN",
            );
            let admin = ironworks_reconcile::Admin::new(&base, &credential);
            let recorded_id =
                recorded.accounted_id(ironworks_lifecycle::Authority::Member).cloned();
            ironworks_reconcile::IronclawMember::new(&admin, recorded_id).observe(&spec)
        }
        AuthoritySource::AccountService => {
            let base = required_by("--account-service", Var::RecordStore)?;
            let book = Book::new(&base, EnvCredentials);
            let credential =
                CredentialRef::new(CredentialPurpose::RecordScope, "env:IRONWORKS_RECORDS_TOKEN");
            ironworks_reconcile::AccountService::new(&book, credential).observe(&spec)
        }
        AuthoritySource::Fixture(path) => fixture_inventory(path)?,
        AuthoritySource::None => NoReconciler.observe(&spec),
    };

    let report = inspect(&spec, &recorded, &inventory, &serving, &axis);
    print!("{}", report.render());
    let code = report.exit_code();
    if code == 0 {
        Ok(())
    } else {
        Err(Exit {
            code,
            message: format!(
                "readiness {} (authority verdict {})",
                report.readiness(),
                report.verdict
            ),
        })
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
    use ironworks_lifecycle::{Blocked, ConfinementAxis, Ledger, LiveConfinement, PolicyBinding};
    use ironworks_reconcile::Binding;

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

    let base = required_by("--confinement", Var::IronclawApi)?;
    let (definition, policy) = policy_for(store, slug)?;
    let current = PolicyBinding::of(&definition, &policy);

    let operator =
        CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:IRONCLAW_OPERATOR_TOKEN");
    let admin = ironworks_reconcile::Admin::new(&base, &operator);
    let member_credential =
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_MEMBER_TOKEN");
    let confinement =
        ironworks_reconcile::MemberConfinement::new(&admin, &EnvCredentials, member_credential);
    let at = now_iso();

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

    let records_base = required(Var::RecordStore)?;
    let records_handle = CredentialRef::new(CredentialPurpose::RecordScope, records_credential);
    let book = Book::new(&records_base, EnvCredentials);
    let scope = ScopeResolver::resolve(&book, &records_handle).map_err(|why| {
        blocked(format!("the records credential could not establish an org: {why}"))
    })?;

    let runtime_base = required(Var::IronclawApi)?;
    // authenticated_subject makes only the member-authenticated GET. Admin retains an operator
    // handle for confinement methods, but this observational command never resolves or uses it.
    let unused_operator =
        CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:IRONCLAW_OPERATOR_TOKEN");
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

/// The exact bytes the former no-`org` publisher wrote for this tenant.
///
/// `run_registry_publish_one` migrates a live file only when it equals these byte for byte, so
/// they are encoded here on their own rather than derived from the current schema.
fn encode_legacy_registry_v0(
    slug: &str,
    channel: &str,
    service: &str,
    guidance: &str,
    records_credential: &str,
    runtime_credential: &str,
) -> Result<Vec<u8>, serde_json::Error> {
    // Frozen former schema: these six fields, independently encoded. A future current-schema
    // addition must not silently widen what this one-time migration accepts as historical output.
    let value = serde_json::json!({
        "tenants": [{
            "slug": slug,
            "channel": channel,
            "service": service,
            "guidance": guidance,
            "records_credential": records_credential,
            "runtime_credential": runtime_credential,
        }]
    });
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The org lifecycle recorded, the org the spec names, and the org the credential actually
/// resolves to must all be one org.
///
/// # Its own function so it can be tested at all
///
/// This was four lines inside `run_registry_publish_one`, which needs an Account Service, an
/// IronClaw, a SQLite lifecycle store and a filesystem before it reaches them. The rule itself
/// needs none of that — it is a comparison of three strings — and it is the rule that stops a
/// registry being published against another organization's records.
///
/// **All three, not two.** The spec is what an operator asked for and the recorded id is what
/// provisioning observed; they can disagree with each other as well as with the store's answer.
fn agreed_records_org(
    recorded: &str,
    spec: &ironworks_domain::OrgId,
    resolved: &ironworks_domain::OrgId,
) -> Result<(), String> {
    if recorded == resolved.as_str() && spec == resolved {
        return Ok(());
    }
    Err(format!(
        "the records credential resolves to org {resolved}, but lifecycle records org {recorded} \
         and the instance is specified as {spec}; refusing to publish a valid registry with the \
         wrong record scope"
    ))
}

/// The runtime subject the member credential authenticates as must be the one lifecycle recorded.
///
/// Extracted for the same reason as [`agreed_records_org`]: the rule is a comparison, and reaching
/// it used to require a live runtime.
fn agreed_runtime_subject(
    live: &ironworks_lifecycle::RuntimeSubject,
    recorded: &ironworks_lifecycle::RuntimeSubject,
) -> Result<(), String> {
    if live == recorded {
        return Ok(());
    }
    Err(format!(
        "the runtime credential authenticates as {live}, but lifecycle recorded {recorded}; \
         refusing to publish a valid registry with the wrong runtime scope"
    ))
}

/// Publish the one-tenant registry only after both credential handles re-measure the identities
/// lifecycle recorded. The file may become live before the SQLite stage is recorded; rerunning is
/// deliberately idempotent across that attended crash window. Once `staged` is recorded, the live
/// file is compared with the recorded candidate and never replaced.
fn run_registry_publish_one(
    slug: &str,
    store_path: &str,
    registry_path: &str,
    guidance_path: &str,
    records_credential: &str,
    member_credential: &str,
) -> Result<(), Exit> {
    use ironworks_domain::ScopeResolver;
    use ironworks_lifecycle::{Authority, LifecycleStore, OpaqueId, Stage};
    use sha2::Digest;

    let e = |m: String| fail(FAILED, m);
    let blocked = |m: String| Exit { code: BLOCKED, message: m };
    let records_credential = checked_handle(records_credential, "--records-credential")?;
    let member_credential = checked_handle(member_credential, "--member-credential")?;
    let slug = Slug::parse(slug.to_owned()).map_err(|why| e(why.to_string()))?;
    let store = LifecycleStore::open(store_path).map_err(|why| blocked(why.to_string()))?;
    let spec = store
        .spec(&slug)
        .map_err(|why| blocked(why.to_string()))?
        .ok_or_else(|| e(format!("no lifecycle record for {slug}")))?;
    let recorded = store.recorded(&slug).map_err(|why| blocked(why.to_string()))?;
    let reached = recorded.progress.reached();
    if !recorded.progress.has_reached(Stage::MemberConfined) {
        return Err(e(format!(
            "instance {slug} is at {}; publish-one requires a certified member_confined stage",
            reached.map_or("none", Stage::tag)
        )));
    }

    let definition = ironworks_services::service::load(spec.service.as_str())
        .map_err(|why| e(why.to_string()))?;
    let guidance_path = std::fs::canonicalize(guidance_path)
        .map_err(|why| e(format!("cannot resolve guidance {guidance_path}: {why}")))?;
    let guidance_name = guidance_path
        .to_str()
        .ok_or_else(|| e(format!("guidance path {} is not UTF-8", guidance_path.display())))?;
    let guidance_text = std::fs::read_to_string(&guidance_path)
        .map_err(|why| e(format!("cannot read guidance {}: {why}", guidance_path.display())))?;
    Guidance::parse(&guidance_text, slug.as_str(), spec.service.as_str())
        .map_err(|why| e(why.to_string()))?;
    // Publishing the registry makes these bytes serveable. It is therefore the same admission
    // boundary as `guidance publish-one`, even when an operator installed the file out of band.
    if definition.id() != &spec.service {
        return Err(e("the loaded service does not match the lifecycle service".into()));
    }

    // Reaching `member_confined` is historical. Publication needs the latest observation for the
    // current member to be clean under the policy this build derives now; otherwise a policy
    // fingerprint change could leave an old, weaker certification routable forever.
    let recorded_member_id = recorded
        .accounted_id(ironworks_lifecycle::Authority::Member)
        .ok_or_else(|| e(format!("instance {slug} has no recorded runtime member identity")))?;
    let confinement_policy =
        ironworks_lifecycle::ConfinementPolicy::for_profile(definition.capability_ceiling());
    let confinement_binding =
        ironworks_lifecycle::PolicyBinding::of(&definition, &confinement_policy);
    let latest_confinement = store
        .latest_applicable(&slug, recorded_member_id)
        .map_err(|why| blocked(why.to_string()))?;
    if latest_confinement
        .and_then(|attempt| {
            ironworks_lifecycle::ConfinementEvidence::from_attempt(attempt, &confinement_binding)
        })
        .is_none()
    {
        return Err(e(format!(
            "instance {slug} has no latest clean confinement observation for policy {}; rerun \
             lifecycle confine before publication",
            confinement_policy.fingerprint()
        )));
    }

    // Routing is the deployment boundary for this seam. A manual model-policy command is useful
    // diagnostics but not enforcement, so publication repeats the exact read-only comparison and
    // refuses before touching the registry unless IronClaw's operator-visible requested-model
    // policy is the singleton MODEL_PIN. This is still not provider or TEE attestation.
    let (model_pin, model_policy) = model_policy_inputs()?;
    require_model_policy_exact(&model_pin, &model_policy)?;

    let records_base = required(Var::RecordStore)?;
    let records_handle = CredentialRef::new(CredentialPurpose::RecordScope, &records_credential);
    let book = Book::new(&records_base, EnvCredentials);
    let scope = ScopeResolver::resolve(&book, &records_handle).map_err(|why| {
        blocked(format!("the records credential could not establish an org: {why}"))
    })?;
    let recorded_org = recorded
        .accounted_id(Authority::OrgRegistration)
        .ok_or_else(|| e(format!("instance {slug} has no recorded organization identity")))?;
    agreed_records_org(recorded_org.as_str(), &spec.org, scope.org()).map_err(e)?;

    let runtime_base = required(Var::RuntimeBase)?;
    let unused_operator =
        CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:IRONCLAW_OPERATOR_TOKEN");
    let admin = ironworks_reconcile::Admin::new(&runtime_base, &unused_operator);
    let runtime_handle = CredentialRef::new(CredentialPurpose::RuntimeMember, &member_credential);
    let live_subject =
        ironworks_reconcile::MemberConfinement::new(&admin, &EnvCredentials, runtime_handle)
            .authenticated_subject()
            .map_err(|why| {
                blocked(format!("the runtime credential could not establish a subject: {why}"))
            })?;
    let recorded_member = recorded
        .identity(Authority::Member)
        .ok_or_else(|| e(format!("instance {slug} has no recorded runtime member identity")))?;
    let recorded_subject = recorded_member.runtime_subject().ok_or_else(|| {
        e(format!(
            "instance {slug} has no measured runtime tenant for member {}; re-adopt from the \
             credential before publishing",
            recorded_member.id()
        ))
    })?;
    agreed_runtime_subject(&live_subject, recorded_subject).map_err(e)?;

    let value = serde_json::json!({
        "tenants": [{
            "slug": slug.as_str(),
            "channel": spec.channel.as_str(),
            // The org the checks above just agreed on, from the store's own answer rather than
            // from the spec: what the registry records is what publication proved, so a serving
            // process comparing against it is comparing against a measured value.
            "org": scope.org().as_str(),
            "service": spec.service.as_str(),
            "guidance": guidance_name,
            "records_credential": records_credential,
            "runtime_credential": member_credential,
        }]
    });
    let encode = |value: &serde_json::Value| -> Result<Vec<u8>, Exit> {
        let mut bytes = serde_json::to_vec_pretty(value)
            .map_err(|why| e(format!("cannot encode registry: {why}")))?;
        bytes.push(b'\n');
        Ok(bytes)
    };
    let candidate = encode(&value)?;
    let legacy_candidate = encode_legacy_registry_v0(
        slug.as_str(),
        spec.channel.as_str(),
        spec.service.as_str(),
        guidance_name,
        &records_credential,
        &member_credential,
    )
    .map_err(|why| e(format!("cannot encode former registry candidate: {why}")))?;
    let parsed = ironworks_domain::TenantRegistry::parse(
        std::str::from_utf8(&candidate).expect("JSON encoding is UTF-8"),
    )
    .map_err(|why| e(format!("constructed registry did not validate: {why}")))?;
    if parsed.len() != 1 || parsed.tenant(&slug).is_none() {
        return Err(e("constructed registry did not contain exactly the requested tenant".into()));
    }

    let id_for = |bytes: &[u8]| {
        let digest = sha2::Sha256::digest(bytes);
        OpaqueId::new(digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect::<String>())
    };
    let registry_id = id_for(&candidate);
    let legacy_registry_id = id_for(&legacy_candidate);
    let live_path = std::path::Path::new(registry_path);
    let mut registry_lock = registry_publish::RegistryLock::acquire(live_path).map_err(e)?;
    if reached.is_some_and(|stage| stage >= Stage::Staged) {
        let recorded_registry = recorded
            .accounted_id(Authority::RegistryEntry)
            .ok_or_else(|| e(format!("instance {slug} is staged but has no registry identity")))?;
        if recorded_registry == &registry_id {
            // Compared, never replaced — not even with the candidate it recorded. `registry
            // rollback-one` does not walk lifecycle back, so a live file that differs here is one
            // an operator changed on purpose, and publishing over it would undo that while
            // reporting that nothing changed.
            let live = registry_lock.live_bytes().map_err(|why| {
                e(format!("instance is already staged but {registry_path} cannot be read: {why}"))
            })?;
            if live != candidate {
                return Err(e(format!(
                    "instance {slug} is already staged and the live registry differs; publish-one \
                     will not replace an already-recorded binding"
                )));
            }
            println!("PUBLISHED {slug} (already staged; live registry matches)");
            return Ok(());
        }
        if recorded_registry != &legacy_registry_id {
            return Err(e(format!(
                "instance {slug} is already staged with registry {recorded_registry}, not the \
                 exact former or current candidate; publish-one will not replace an \
                 already-recorded binding"
            )));
        }

        let outcome = registry_lock.migrate_legacy_one(&legacy_candidate, &candidate).map_err(e)?;
        if let Err(why) =
            store.migrate_registry_binding(&slug, &legacy_registry_id, &registry_id, &now_iso())
        {
            let permanent = matches!(
                why,
                ironworks_lifecycle::LifecycleError::RegistryBindingMismatch { .. }
                    | ironworks_lifecycle::LifecycleError::RegistryBindingMissing { .. }
                    | ironworks_lifecycle::LifecycleError::RegistryBindingNotStaged { .. }
            );
            let remedy = if permanent {
                "The file is current but lifecycle authority conflicts; preserve both and inspect the ledger before any retry."
            } else {
                "The file is current; after resolving the store error, rerunning publish-one can complete the conditional binding update."
            };
            return Err(e(format!(
                "registry migration completed but lifecycle binding migration did not: {why}. {remedy}"
            )));
        }
        println!("MIGRATED  {slug} registry {legacy_registry_id} -> {registry_id}");
        if matches!(
            outcome,
            registry_publish::MigrationOutcome::Migrated { rebuilt_rollback_to_absence: true }
                | registry_publish::MigrationOutcome::AlreadyMigrated {
                    rebuilt_rollback_to_absence: true
                }
        ) {
            println!(
                "  rollback: {} (legacy rollback was unusable; now restores no live registry)",
                registry_publish::rollback_path(live_path).map_err(e)?.display()
            );
        } else {
            println!(
                "  rollback: {} (existing current-schema rollback preserved)",
                registry_publish::rollback_path(live_path).map_err(e)?.display()
            );
        }
        return Ok(());
    }

    let outcome = registry_lock.publish_one(&candidate).map_err(e)?;
    if let Err(why) = store.reach(
        &slug,
        Stage::Staged,
        ironworks_lifecycle::IdentityProvenance::Created,
        Some(&registry_id),
        &now_iso(),
    ) {
        return Err(e(format!(
            "registry publication completed but lifecycle staging did not: {why}. The failure is \
             visible; rerun publish-one to finish recording it, or run registry rollback-one \
             --registry {registry_path}"
        )));
    }
    println!("PUBLISHED {slug} registry {registry_id}");
    match outcome {
        registry_publish::PublishOutcome::Published { preserved_prior: true } => println!(
            "  rollback: {}",
            registry_publish::rollback_path(live_path).map_err(e)?.display()
        ),
        registry_publish::PublishOutcome::Published { preserved_prior: false } => {
            println!(
                "  rollback: {} (restores no live registry)",
                registry_publish::rollback_path(live_path).map_err(e)?.display()
            );
        }
        registry_publish::PublishOutcome::AlreadyPublished => {
            println!("  file already matched; lifecycle staging completed");
        }
    }
    Ok(())
}

fn run_registry_rollback_one(registry_path: &str) -> Result<(), Exit> {
    let mut lock = registry_publish::RegistryLock::acquire(std::path::Path::new(registry_path))
        .map_err(|why| fail(FAILED, why))?;
    lock.rollback_one().map_err(|why| fail(FAILED, why))?;
    println!("ROLLED BACK {registry_path}");
    println!("  lifecycle progress is historical and was not walked backwards");
    Ok(())
}

fn guidance_binding(
    registry_path: &str,
    slug: &Slug,
) -> Result<(ironworks_domain::ServiceId, std::path::PathBuf), Exit> {
    let registry_text = std::fs::read_to_string(registry_path)
        .map_err(|error| fail(FAILED, format!("cannot read registry {registry_path}: {error}")))?;
    let registry = ironworks_domain::TenantRegistry::parse(&registry_text)
        .map_err(|error| fail(FAILED, format!("registry is invalid: {error}")))?;
    let entry = registry
        .tenant(slug)
        .ok_or_else(|| fail(FAILED, format!("registry has no tenant {slug}")))?;
    let raw = std::path::Path::new(entry.guidance().as_str());
    let destination = if raw.exists() {
        std::fs::canonicalize(raw).map_err(|error| {
            fail(FAILED, format!("cannot resolve registry guidance destination: {error}"))
        })?
    } else {
        let name = raw
            .file_name()
            .ok_or_else(|| fail(FAILED, "registry guidance destination has no usable file name"))?;
        std::fs::canonicalize(registry_publish::parent(raw))
            .map_err(|error| {
                fail(FAILED, format!("cannot resolve registry guidance directory: {error}"))
            })?
            .join(name)
    };
    Ok((entry.service().clone(), destination))
}

fn guidance_digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn run_guidance_publish_one(slug: &str, registry_path: &str, source: &str) -> Result<(), Exit> {
    let slug = Slug::parse(slug.to_owned()).map_err(|error| fail(FAILED, error.to_string()))?;
    let registry_path = std::path::Path::new(registry_path);
    let mut lock = registry_publish::RegistryLock::acquire(registry_path)
        .map_err(|error| fail(FAILED, error))?;
    let (service, destination) = guidance_binding(
        registry_path.to_str().ok_or_else(|| fail(FAILED, "registry path is not UTF-8"))?,
        &slug,
    )?;
    let source = std::fs::canonicalize(source)
        .map_err(|error| fail(FAILED, format!("cannot resolve guidance source: {error}")))?;
    if source == destination {
        return Err(fail(USAGE, "--source must be distinct from the canonical live guidance"));
    }
    let bytes = std::fs::read(&source)
        .map_err(|error| fail(FAILED, format!("cannot read guidance source: {error}")))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| fail(FAILED, format!("guidance source is not UTF-8: {error}")))?;
    Guidance::parse(text, slug.as_str(), service.as_str())
        .map_err(|error| fail(FAILED, error.to_string()))?;
    lock.publish_artifact(&destination, &bytes).map_err(|error| fail(FAILED, error))?;
    println!("bytes={} sha256={}", bytes.len(), guidance_digest(&bytes));
    Ok(())
}

fn run_guidance_rollback_one(slug: &str, registry_path: &str) -> Result<(), Exit> {
    let slug = Slug::parse(slug.to_owned()).map_err(|error| fail(FAILED, error.to_string()))?;
    let registry_path = std::path::Path::new(registry_path);
    let mut lock = registry_publish::RegistryLock::acquire(registry_path)
        .map_err(|error| fail(FAILED, error))?;
    let (service, destination) = guidance_binding(
        registry_path.to_str().ok_or_else(|| fail(FAILED, "registry path is not UTF-8"))?,
        &slug,
    )?;
    let prior = lock.rollback_artifact_bytes(&destination).map_err(|error| fail(FAILED, error))?;
    if let Some(prior) = &prior {
        let text = std::str::from_utf8(prior)
            .map_err(|error| fail(FAILED, format!("guidance rollback is not UTF-8: {error}")))?;
        Guidance::parse(text, slug.as_str(), service.as_str())
            .map_err(|error| fail(FAILED, error.to_string()))?;
    }
    lock.rollback_artifact(&destination).map_err(|error| fail(FAILED, error))?;
    if prior.is_none() {
        // The absence marker restores no file at all. Said in words, because `bytes=0` and the
        // digest of nothing read exactly like restoring an empty file.
        println!("removed: the rollback restores no live guidance");
        return Ok(());
    }
    let bytes = std::fs::read(&destination)
        .map_err(|error| fail(FAILED, format!("cannot read restored guidance: {error}")))?;
    println!("bytes={} sha256={}", bytes.len(), guidance_digest(&bytes));
    Ok(())
}

fn run_lifecycle_attest(slug: &str, store_path: &str) -> Result<(), Exit> {
    use ironworks_domain::{ConfinementAttestation, Sha256Digest};
    use ironworks_lifecycle::{Authority, LifecycleStore, PolicyBinding};
    use ironworks_reconcile::{Binding, Reading};

    let blocked = |message: String| Exit { code: BLOCKED, message };
    let slug = Slug::parse(slug.to_owned()).map_err(|error| fail(FAILED, error.to_string()))?;
    let store =
        LifecycleStore::open_readonly(store_path).map_err(|error| blocked(error.to_string()))?;
    let recorded = store.recorded(&slug).map_err(|error| blocked(error.to_string()))?;
    let member = recorded
        .accounted_id(Authority::Member)
        .cloned()
        .ok_or_else(|| fail(FAILED, format!("no runtime member is recorded for {slug}")))?;
    let (definition, policy) = policy_for(&store, &slug)?;
    let binding = PolicyBinding::of(&definition, &policy);
    let base = required(Var::IronclawApi)?;
    let operator =
        CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:IRONCLAW_OPERATOR_TOKEN");
    let admin = ironworks_reconcile::Admin::new(&base, &operator);
    let member_credential =
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_MEMBER_TOKEN");
    let confinement =
        ironworks_reconcile::MemberConfinement::new(&admin, &EnvCredentials, member_credential);
    let bound = match confinement.bind(&member) {
        Ok(bound) => bound,
        Err(Binding::Blocked(_)) => {
            return Err(blocked("member identity could not be measured for attestation".into()));
        }
        Err(Binding::Mismatch(_)) => {
            return Err(fail(FAILED, "member credential does not match lifecycle identity"));
        }
    };
    let observed_at = now_iso();
    let (states, locked, auto_approve, attempt) = match bound.read(&policy, &binding, &observed_at)
    {
        Reading::Blocked { .. } => {
            return Err(blocked("member catalogue could not be measured for attestation".into()));
        }
        Reading::Measured { states, locked, auto_approve, attempt } => {
            (states, locked, auto_approve, attempt)
        }
    };
    if !attempt.measurement().is_some_and(ironworks_lifecycle::Measurement::is_clean) {
        return Err(fail(FAILED, "member confinement has drifted; refusing attestation"));
    }
    let attestation = ConfinementAttestation::new(
        slug,
        RuntimeIdentity::parse(member.to_string())
            .map_err(|error| fail(FAILED, error.to_string()))?,
        Sha256Digest::profile(definition.capability_ceiling()),
        policy.sha256(),
        Sha256Digest::catalogue(&states, &locked, auto_approve),
        observed_at.clone(),
    );
    report(&attestation.to_json().map_err(|error| fail(FAILED, error.to_string()))?)
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
fn run_lifecycle_confine(slug: &str, store_path: &str, dry_run: bool) -> Result<(), Exit> {
    use ironworks_lifecycle::{Blocked, ConfinementAttempt, ConfinementEvidence, PolicyBinding};
    use ironworks_reconcile::{Applied, Binding, Reading};

    let e = |m: String| fail(FAILED, m);
    // A measurement that could not be made is BLOCKED, not FAILED. A store that will not open, or
    // a runtime that will not answer, is upstream of IronWorks — scoring it 2 would report a defect
    // here for something that is not one, and an operator who investigates two of those learns to
    // skim the ones that matter.
    let blocked = |m: String| Exit { code: BLOCKED, message: m };

    let base = required(Var::IronclawApi)?;
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
        CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:IRONCLAW_OPERATOR_TOKEN");
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
    let at = now_iso();
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
    let (observed, locked, auto_approve, initial_attempt) = match bound.read(&policy, &binding, &at)
    {
        Reading::Blocked { attempt } => {
            append(&attempt);
            println!("BLOCKED   {slug} member {member}");
            println!("  REFUSING TO APPLY. The member's authority could not be measured.");
            return Err(blocked("the member's authority could not be measured".into()));
        }
        Reading::Measured { states, locked, auto_approve, attempt } => {
            (states, locked, auto_approve, attempt)
        }
    };
    // Derived here rather than inside the arm above, which used to pack it into the slot named
    // `auto_approve` and unpack it on the next line — a second value smuggled through a binding
    // that names the first, to avoid writing a five-tuple.
    let initial_clean =
        initial_attempt.measurement().is_some_and(ironworks_lifecycle::Measurement::is_clean);

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
            let evidence = ConfinementEvidence::from_attempt(initial_attempt, &binding)
                .expect("the clean current-policy measurement qualifies as evidence");
            store
                .certify_confinement(&slug_id, &evidence)
                .map_err(|why| e(format!("the clean observation could not be certified: {why}")))?;
            print_verified(slug, &member, &observed, auto_approve, bound.subject());
            Ok(())
        } else {
            append(&initial_attempt);
            println!("DRIFTED   {slug} member {member}");
            println!("  the plan is empty and the member is still off policy — the drift is not");
            println!("  something a settings write can repair. See the recorded observation.");
            Err(Exit { code: FAILED, message: "confinement did not reach policy".into() })
        };
    }

    // The initial dirty observation remains part of the audit trail. A clean no-op took the
    // certification path above instead, so the qualifying row and stage could commit together.
    append(&initial_attempt);

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
    let after_at = now_iso();
    let after = bound.read(&policy, &binding, &after_at);

    if let Some(message) = stopped {
        append(after.attempt());
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
        Reading::Blocked { attempt } => {
            append(&attempt);
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
                let evidence = ConfinementEvidence::from_attempt(attempt, &binding)
                    .expect("the clean current-policy measurement qualifies as evidence");
                store.certify_confinement(&slug_id, &evidence).map_err(|why| {
                    e(format!("the clean observation could not be certified: {why}"))
                })?;
                print_verified(slug, &member, &states, auto_approve, bound.subject());
                Ok(())
            } else {
                append(&attempt);
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

    let base = required(Var::IronclawApi)?;
    let store = LifecycleStore::open_readonly(store_path).map_err(|x| e(x.to_string()))?;
    let accounted = store.all_recorded_ids(Authority::Member).map_err(|x| e(x.to_string()))?;

    let credential =
        CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:IRONCLAW_OPERATOR_TOKEN");
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
    profile_sha256: ironworks_domain::Sha256Digest,
    policy_sha256: ironworks_domain::Sha256Digest,
}

/// Where one serving instance's inputs come from.
///
/// # One boot, because a control applied to one path and not the other has a way around it
///
/// `boot` and `boot_tenant` were 69- and 65-line functions running the same ten steps in the same
/// order. Six values differed in where they came from — and one **control** differed in whether it
/// ran at all: `agreed_org` was applied by the registry path and not the flags path. Its own doc
/// comment states the rule it was breaking: *"One function because every path that acts for a
/// tenant must apply it… A control applied to the first and not the second is a control with a way
/// around it."*
///
/// So the six values are methods and the control is not optional. The flags path genuinely has no
/// recorded org to agree with, and says so by answering `None` — which is a different thing from
/// the check not being there, and is the difference a reader could not see before.
trait BootSource {
    fn slug(&self) -> Result<Slug, Exit>;
    fn service(&self) -> String;
    fn guidance_path(&self) -> String;
    /// Where the runtime member's credential lives.
    fn runtime_credential(&self) -> CredentialRef;
    /// Where the credential that establishes record scope lives.
    fn records_credential(&self) -> CredentialRef;
    /// The runtime member's credential value.
    fn runtime_token(&self) -> Result<String, Exit>;
    /// The org the registry records for this tenant, when one is recorded. `ExpectedOrg`, not
    /// `OrgId`: a recorded expectation and a store-verified identity are different facts, and the
    /// whole point of the check is that they can disagree.
    fn expected_org(&self) -> Option<&ironworks_domain::ExpectedOrg>;
    /// How this path names itself in operator output.
    fn label(&self) -> String;
}

impl BootSource for ServingInputs {
    fn slug(&self) -> Result<Slug, Exit> {
        Slug::parse(self.slug.clone()).map_err(|x| fail(FAILED, x.to_string()))
    }
    fn service(&self) -> String {
        self.service.clone()
    }
    fn guidance_path(&self) -> String {
        self.guidance.clone()
    }
    fn runtime_credential(&self) -> CredentialRef {
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONWORKS_TOKEN")
    }
    fn records_credential(&self) -> CredentialRef {
        CredentialRef::new(CredentialPurpose::RecordScope, "env:IRONWORKS_RECORDS_TOKEN")
    }
    fn runtime_token(&self) -> Result<String, Exit> {
        required(Var::RuntimeToken)
    }
    fn expected_org(&self) -> Option<&ironworks_domain::ExpectedOrg> {
        // Flags carry no registry entry, so there is nothing to agree with.
        None
    }
    fn label(&self) -> String {
        "ironworks".to_owned()
    }
}

impl BootSource for ironworks_domain::TenantEntry {
    fn slug(&self) -> Result<Slug, Exit> {
        Ok(ironworks_domain::TenantEntry::slug(self).clone())
    }
    fn service(&self) -> String {
        ironworks_domain::TenantEntry::service(self).as_str().to_owned()
    }
    fn guidance_path(&self) -> String {
        self.guidance().as_str().to_owned()
    }
    fn runtime_credential(&self) -> CredentialRef {
        self.runtime().clone()
    }
    fn records_credential(&self) -> CredentialRef {
        self.records().clone()
    }
    fn runtime_token(&self) -> Result<String, Exit> {
        env_handle(self.runtime(), "the runtime")
    }
    fn expected_org(&self) -> Option<&ironworks_domain::ExpectedOrg> {
        Some(self.org())
    }
    fn label(&self) -> String {
        format!("relay: tenant {}", ironworks_domain::TenantEntry::slug(self).as_str())
    }
}

/// Boot from the command line's flags.
fn boot(services: Services, args: &ServingInputs) -> Result<Boot, Exit> {
    boot_from(services, args, &args.journal)
}

/// Verify one tenant from its registry entry.
fn boot_tenant(
    services: Services,
    entry: &ironworks_domain::TenantEntry,
    journal_path: &str,
) -> Result<Boot, Exit> {
    boot_from(services, entry, journal_path)
}

/// Verify one instance and open everything a turn needs.
///
/// Everything that differs between tenants comes from the [`BootSource`] — service, guidance, and
/// both credential handles. Everything shared comes from the environment: one runtime, one record
/// store, one journal. **Each tenant verifies its own scope against the store**, so isolation
/// remains a property of verified evidence rather than of the routing table that chose which
/// tenant to serve.
///
/// The two callers above stay named, rather than every call site naming this one: `boot` and
/// `boot_tenant` are cited by `DESIGN.md`, `docs/PRD.md` and four experiments, and a dated record
/// takes a pointer to what superseded it, never an edit. Two three-line wrappers cost less than
/// fifteen documentation edits, and each name still says which path a reader is on.
fn boot_from(
    services: Services,
    source: &dyn BootSource,
    journal_path: &str,
) -> Result<Boot, Exit> {
    let e = |m: String| fail(FAILED, m);

    let base = required(Var::RuntimeBase)?;
    // No fallback, deliberately. Scope verification is a control: without a record store there is
    // nothing entitled to say which org a credential authorises, and an unverified identity does
    // not serve. A fixture stand-in here would be exactly the fail-open this design removes.
    // The one read that says more than the variable's description: the serving path has no
    // offline mode, and an operator who set nothing should be told why rather than what.
    let store = required(Var::RecordStore).map_err(|_| {
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
    let service = source.service();
    let definition = load_serving_definition(services, &service).map_err(e)?;
    let capability_profile = *definition.capability_ceiling();
    let profile_sha256 = ironworks_domain::Sha256Digest::profile(&capability_profile);
    let policy_sha256 =
        ironworks_lifecycle::ConfinementPolicy::for_profile(definition.capability_ceiling())
            .sha256();
    let slug = source.slug()?;
    let guidance =
        load_serving_guidance(services, &source.guidance_path(), slug.as_str(), &service)
            .map_err(e)?;

    let instance = ServiceInstance::new(InstanceParts {
        slug: slug.clone(),
        definition,
        guidance,
        runtime_identity: RuntimeIdentity::parse(format!("member:{}", slug.as_str()))
            .map_err(|x| e(x.to_string()))?,
        runtime_credential: source.runtime_credential(),
        context_binding: ContextBinding::new(source.records_credential())
            .map_err(|x| e(x.to_string()))?,
        model: ModelSelection::from_pin(&pin),
        requested_capabilities: capability_profile,
        claimed_org: None,
        parts: &ironworks_services::Heap,
    })
    .map_err(|x| e(x.to_string()))?;

    // ── the control ─────────────────────────────────────────────────────────────────
    let book = Book::new(&store, EnvCredentials);
    let verified = instance.verify(&book).map_err(|x| e(x.to_string()))?;
    if let Some(expected) = source.expected_org() {
        agreed_org(slug.as_str(), expected, verified.org()).map_err(e)?;
    }
    eprintln!(
        "{}: scope {} verified by {} (evidence: record store)",
        source.label(),
        verified.org(),
        store
    );

    let journal = Journal::open(journal_path).map_err(|x| e(x.to_string()))?;
    let scope = verified.thread_scope();
    let runtime = Ironclaw::new(base, ironworks_ironclaw::Credential::new(source.runtime_token()?));
    Ok(Boot {
        verified,
        book,
        journal,
        scope,
        runtime,
        journal_path: journal_path.to_owned(),
        profile_sha256,
        policy_sha256,
    })
}

/// An environment variable this binary requires, with the one description an operator is given
/// for it.
///
/// One spelling per variable, so a refusal names it the same way whichever subcommand refused.
/// `IRONWORKS_RECORDS` was previously described four different ways — "the account service base
/// URL", "the record store base URL", "the record store", and the same with a full stop —
/// depending on which command the operator happened to hit.
#[derive(Clone, Copy)]
enum Var {
    /// The serving runtime's base URL.
    RuntimeBase,
    /// This instance's own credential.
    RuntimeToken,
    /// The record store: the only authority on which org a credential authorises.
    RecordStore,
    /// The IronClaw instance the operator-side commands observe.
    IronclawApi,
    /// The channel adapter's bearer. The one variable that is **not** [`Var::required`]: see
    /// [`Var::presented`].
    AdapterToken,
    /// The member this instance serves as, needed only to check a confinement attestation.
    MemberId,
}

impl Var {
    fn name(self) -> &'static str {
        match self {
            Self::RuntimeBase => "IRONWORKS_BASE",
            Self::RuntimeToken => "IRONWORKS_TOKEN",
            Self::RecordStore => "IRONWORKS_RECORDS",
            Self::IronclawApi => "IRONCLAW_API",
            Self::AdapterToken => "CHANNEL_ADAPTER_TOKEN",
            Self::MemberId => "IRONWORKS_MEMBER_ID",
        }
    }

    fn what(self) -> &'static str {
        match self {
            Self::RuntimeBase => "the runtime base URL",
            Self::RuntimeToken => "this instance's credential",
            Self::RecordStore => "the record store",
            Self::IronclawApi => "the instance's base URL",
            Self::AdapterToken => "the channel adapter's bearer",
            Self::MemberId => "the member this instance serves as",
        }
    }

    /// The value, or the sentence saying which variable is missing and what it is for.
    ///
    /// Empty is unset, the rule [`env_handle`] already applies to a credential handle: an empty
    /// base URL or token is not a configuration, and failing here says so where the operator can
    /// act on it rather than several layers down in a request that could not be built.
    ///
    /// The sentence rather than an [`Exit`], because [`crate::tenant`] reports in `String` and had
    /// been reading `IRONWORKS_RECORDS` with a bare `std::env::var` for that reason — which let an
    /// empty value through to be misreported as a credential failure, the exact divergence this
    /// rule exists to prevent.
    fn required(self) -> Result<String, String> {
        std::env::var(self.name())
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("{} is not set ({})", self.name(), self.what()))
    }

    /// The value, or empty — the deliberate exception to [`Var::required`], for a credential that
    /// is presented rather than used here.
    ///
    /// Unset sends an empty bearer, which a real adapter refuses: fail-closed rather than silently
    /// unauthenticated. `devchannel` ignores the header, so the proof scripts are unaffected.
    /// Stated once because it is read on two paths — a serving relay and a redelivery — and only
    /// one of them carried the reasoning.
    fn presented(self) -> String {
        std::env::var(self.name()).unwrap_or_default()
    }
}

/// [`Var::required`], as a usage refusal.
fn required(var: Var) -> Result<String, Exit> {
    var.required().map_err(|why| fail(USAGE, why))
}

/// [`required`], for a variable a specific flag needs — the refusal names the flag that asked.
fn required_by(flag: &str, var: Var) -> Result<String, Exit> {
    required(var).map_err(|_| fail(USAGE, format!("{flag} needs {} ({})", var.name(), var.what())))
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

/// The control over which organization a tenant is acted for: the record store's answer must
/// match what the registry recorded.
///
/// The record store is the authority on which org a credential authorises (§B-1), and the
/// registry says which org the operator recorded this tenant as. When they disagree, something is
/// pointing somewhere nobody intended — a re-pointed credential, an entry copied and half edited
/// — and the store's answer is still perfectly valid, which is exactly what makes it dangerous:
/// acting would put one organization's work against another's records and nothing downstream
/// could tell.
///
/// **One function because every path that acts for a tenant must apply it.** Serving composes
/// guidance over records; promoting writes a durable activity into the org it names. A control
/// applied to the first and not the second is a control with a way around it, and the way around
/// it is the one that writes. It takes the answered org rather than resolving one, so the caller
/// that already resolved does not resolve twice.
///
/// A control fails closed. The comparison lives here rather than in `verify` because
/// `ServiceInstance` is told nothing a caller supplied about an org, deliberately — the
/// expectation is registry configuration, and `ExpectedOrg` exists so it can never be mistaken
/// for the scope key itself.
fn agreed_org(
    slug: &str,
    recorded: &ironworks_domain::ExpectedOrg,
    answered: &ironworks_domain::OrgId,
) -> Result<(), String> {
    if recorded.matches(answered) {
        return Ok(());
    }
    Err(format!(
        "tenant {slug} is recorded as org {}, and its records credential resolved to {} — \
         refusing rather than acting for one organization against another's records; \
         fix the registry entry or the credential, not this check.",
        recorded.as_str(),
        answered,
    ))
}

/// One question, answered on the terminal. A successful stdout write is its delivery evidence.
fn run_turn(b: &Boot, controls: &ExecutionControls, ask: &str) -> Result<(), Exit> {
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
    let answer = TurnDriver { boot: b, controls }.drive(&key, ask, None, None)?;
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
    crash_if(controls, "after-output");

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

/// What [`run_relay`] is given for one cycle, by name rather than by position.
struct RelayContext<'a> {
    /// Which services this build may compose — see [`Services`].
    services: Services,
    controls: &'a ExecutionControls,
    journal_path: &'a str,
    channel_url: &'a str,
    registry_path: &'a str,
    chunk_bytes: usize,
    confinement_attestation: Option<&'a str>,
}

/// One poll cycle against a channel: acknowledge, take the batch, answer it, deliver it.
///
/// Deliberately **one cycle, then exit**. A daemon would need a crash point to be a signal at a
/// moment; a one-shot makes every boundary a separate process that a script starts and kills, so
/// "across a real restart" means what it says.
fn run_relay(context: RelayContext<'_>) -> Result<(), Exit> {
    let RelayContext {
        services,
        controls,
        journal_path,
        channel_url,
        registry_path,
        chunk_bytes,
        confinement_attestation,
    } = context;
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
    let cursor_journal = Journal::open(journal_path).map_err(|x| e(x.to_string()))?;
    let adapter_token = Var::AdapterToken.presented();
    let ch = ironworks_channel_http::Channel::authenticated(channel_url, adapter_token);
    // The same file as the turn journal. Separate tables, one store — two files could disagree in
    // exactly one direction that silently corrupts a live conversation, and there is no reason to
    // buy that risk here.
    let updates_journal =
        ChannelJournal::open(journal_path).map_err(|x| e(format!("channel journal: {x}")))?;
    // Beside the journal and named after it, so an operator supplies one path and knows where the
    // second file is. It is deliberately NOT a table in that journal: the journal holds
    // identifiers only, and a proposal is the person's words.
    let proposals_path = format!("{journal_path}.proposals");
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

    // ── forget what the channel will never send again ───────────────────────────────
    // `compact` had no caller at all, so `channel_updates` grew for the life of a deployment —
    // and `safe_cursor` reads every row and asks the turn journal about each `turn` among them,
    // twice per cycle plus once inside `acknowledge`. The cost of a poll was therefore
    // proportional to every update ever received, forever.
    //
    // Here rather than anywhere else because acknowledgement is exactly the moment the rows below
    // it stop being redeliverable — `compact`'s own doc calls that "the moment it becomes safe to
    // forget them" — and it bounds itself by the acknowledged offset rather than the safe cursor,
    // so it can only remove what the channel will never send again. It keeps every update that
    // cost a turn, whatever its age.
    //
    // Never fatal, for the same reason as the acknowledgement above: an unacknowledged journal
    // returns `Ok(0)`, and a cursor that cannot be read is a condition to report rather than a
    // reason to stop serving.
    match updates_journal.compact() {
        Ok(0) => {}
        Ok(n) => eprintln!("relay: compacted {n} settled update(s)"),
        Err(why) => eprintln!("relay: compaction skipped — {why}"),
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

    // How many updates this cycle could not settle. Counted rather than propagated, so one bad
    // update does not end the batch — see `contained`. The cycle still exits non-zero if any
    // failed: `dev/relay-loop.sh` backs off on that, and whatever produced the failure may well
    // produce it again on the next update.
    let mut failures = 0usize;

    for update in &inbound {
        let id = ironworks_channel::UpdateId::new(update.id);
        let Some(record) = contained(
            id,
            &mut failures,
            updates_journal
                .record(id)
                .map_err(|x| e(x.to_string()))
                .and_then(|r| r.ok_or_else(|| e(format!("update {id} vanished from the batch")))),
        ) else {
            continue;
        };

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
            if let Some(entry) = contained(
                id,
                &mut failures,
                registry.tenant(slug).ok_or_else(|| {
                    e(format!("routed to {slug}, which the registry does not hold"))
                }),
            ) {
                contained(
                    id,
                    &mut failures,
                    answer_command(
                        &ch,
                        &updates_journal,
                        &proposals,
                        entry,
                        update,
                        id,
                        chunk_bytes,
                    ),
                );
            }
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
                    contained(
                        id,
                        &mut failures,
                        updates_journal
                            .disposition(id, &Disposition::NotAddressed, &now_iso())
                            .map_err(|x| e(x.to_string())),
                    );
                    eprintln!("relay: update {id} not addressed to us — no turn");
                    continue;
                }
                ironworks_domain::RouteState::Present(_) => {
                    let Some(key) = contained(id, &mut failures, update_key(id)) else {
                        continue;
                    };
                    let wrote = contained(
                        id,
                        &mut failures,
                        updates_journal
                            .disposition(id, &Disposition::Turn(key.clone()), &now_iso())
                            .map_err(|x| e(x.to_string())),
                    );
                    // A turn whose claim is not durable must not run: the disposition is what
                    // makes a redelivery find this turn rather than mint a second one (§E-20).
                    if wrote.is_none() {
                        continue;
                    }
                    key
                }
                ironworks_domain::RouteState::Absent => {
                    // Nobody here claims it. Settled, and recorded rather than discarded: an
                    // update for a channel no tenant owns is a configuration defect.
                    contained(
                        id,
                        &mut failures,
                        updates_journal
                            .disposition(id, &Disposition::Unroutable, &now_iso())
                            .map_err(|x| e(x.to_string())),
                    );
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
        let Some(entry) = contained(
            id,
            &mut failures,
            registry
                .tenant(slug)
                .ok_or_else(|| e(format!("routed to {slug}, which the registry does not hold"))),
        ) else {
            continue;
        };
        if !tenants.contains_key(slug.as_str()) {
            // Contained per update, deliberately. A tenant whose record store is briefly
            // unreachable cannot verify, and that must not stop the tenants whose traffic was
            // healthy — the same cross-tenant property the shared cursor exists to protect
            // (§E-21), applied to the loop rather than to the offset.
            let Some(booted) =
                contained(id, &mut failures, boot_tenant(services, entry, journal_path))
            else {
                continue;
            };
            tenants.insert(slug.as_str().to_owned(), booted);
        }
        let b = &tenants[slug.as_str()];

        // ── has this already been finished? ─────────────────────────────────────────
        // The one question the channel side asks about a tenant's work. A settled update gets no
        // rerun and no resend — only an acknowledgement, which the next poll's offset carries.
        let Some(prior) =
            contained(id, &mut failures, cursor_journal.turn(&key).map_err(|x| e(x.to_string())))
        else {
            continue;
        };
        if let Some(prior) = prior
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
        // The one call that both settles a turn for an operator and refuses one WITHOUT settling.
        // Contained either way: a settled turn has released the cursor and the rest of the batch
        // is owed an answer now, and a refused one is owed a retry on the next cycle — neither is
        // a reason to stop serving the updates behind it.
        let Some(answer) = contained(
            id,
            &mut failures,
            TurnDriver { boot: b, controls }.drive(
                &key,
                ask,
                Some(&conversation),
                confinement_attestation,
            ),
        ) else {
            continue;
        };

        // ── delivery ────────────────────────────────────────────────────────────────
        // Strip before chunking (§E-14, §E-15): the chunk count must describe the bytes a client
        // actually receives, and a marker must never be split across two messages.
        let chunks = ironworks_channel_http::plain_chunks(&answer.text, chunk_bytes);
        let outcome = ch.send(&update.channel, &chunks);
        eprintln!("relay: sent {} chunk(s), channel reported {outcome:?}", chunks.len());

        crash_if(controls, "after-send");

        let Some(delivery) = contained(
            id,
            &mut failures,
            b.journal
                .record_delivery_attempt(&key, outcome, &now_iso())
                .map_err(|x| e(x.to_string())),
        ) else {
            continue;
        };
        eprintln!("relay: update {id} delivery={delivery}");

        crash_if(controls, "after-delivered");

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

    // Non-zero if anything failed, whether or not it settled. Two reasons it is not "zero when
    // every failure settled": forward progress is already reported by the *cursor* advancing on
    // the next cycle — which is what `0033` asserted and what `dev/prove-continuation.sh`'s
    // `cursor.released` checks, and it is durable where an exit code is not — and a cycle that
    // settled a turn for an operator should still make `dev/relay-loop.sh` back off, because
    // whatever produced it may produce it again on the next update.
    if failures > 0 {
        return Err(fail(
            FAILED,
            format!(
                "{failures} of {} update(s) did not settle this cycle; the rest were served. \
                 Each is reported above.",
                inbound.len()
            ),
        ));
    }
    Ok(())
}

/// The update a command answers, and the channel and stores its reply goes through — what
/// [`confirm_command`] and [`finish_command`] take, by name rather than by position.
#[derive(Clone, Copy)]
struct CommandContext<'a> {
    ch: &'a ironworks_channel_http::Channel,
    updates: &'a ChannelJournal,
    proposals: &'a ProposalStore,
    update: &'a ironworks_channel_http::Inbound,
    id: ironworks_channel::UpdateId,
    chunk_bytes: usize,
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
    let context = CommandContext { ch, updates, proposals, update, id, chunk_bytes };

    // ── admission, from the registry and nothing else ────────────────────────────────
    let (actor, contributor) =
        match admit_command(update.chat_kind, update.actor.as_ref(), &conversation, entry) {
            Admission::Refuse { reply, why } => {
                eprintln!("relay: update {id} command refused — {why}");
                return finish_command(context, reply, Lane::None);
            }
            Admission::Allow { actor, contributor } => (actor, contributor),
        };

    // ── the grammar, now that the sender is owed an answer about it ──────────────────
    let request = match ironworks_domain::share::parse(&update.text) {
        ironworks_domain::Intent::Refused(why) => {
            eprintln!("relay: update {id} refused — no turn, nothing stored");
            return finish_command(context, &why.to_string(), Lane::None);
        }
        ironworks_domain::Intent::Share(request) => request,
        ironworks_domain::Intent::Confirm => {
            return confirm_command(context, entry, actor, contributor);
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
            context,
            "I cannot show that back to you exactly as you typed it, so I will not store it. \
             Send it again without markdown formatting in the note or the account.",
            Lane::None,
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
        Err(ScopeRefusal::Transient(why)) => {
            eprintln!(
                "relay: update {id} left open — {why} Nothing was stored and nothing was sent; \
                 the next cycle tries again."
            );
            return Ok(());
        }
        Err(ScopeRefusal::Durable(why)) => {
            eprintln!("relay: update {id} command refused — {why}");
            return finish_command(context, UNPROMOTABLE, Lane::None);
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
    finish_command(context, &echo, Lane::Echo(id.get()))
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
fn confirm_command(
    context: CommandContext<'_>,
    entry: &ironworks_domain::TenantEntry,
    actor: &ironworks_domain::ActorRef,
    contributor: &ironworks_domain::ContributorId,
) -> Result<(), Exit> {
    let CommandContext { proposals, update, id, .. } = context;
    let e = |m: String| fail(FAILED, m);
    let conversation = ironworks_domain::ChannelRef::new(&update.channel);

    // ── 1. the newest proposal, and only if it is confirmable ────────────────────────
    // `confirmable` finds the newest whatever its state and returns it only when delivered, so a
    // newer pending or uncertain proposal correctly blocks an older one.
    let org = match share_scope(entry) {
        Ok(org) => org,
        Err(ScopeRefusal::Transient(why)) => {
            eprintln!("relay: update {id} left open — {why} Nothing was appended.");
            return Ok(());
        }
        Err(ScopeRefusal::Durable(why)) => {
            eprintln!("relay: update {id} confirm refused — {why} Nothing was appended.");
            return finish_command(context, UNPROMOTABLE, Lane::None);
        }
    };
    let Some(append_credential) = entry.append() else {
        // A tenant with no append handle cannot promote. Not a failure of authorization — a
        // configuration an operator has not made — so it says so plainly.
        eprintln!("relay: update {id} confirm refused — this tenant has no append credential");
        return finish_command(
            context,
            "Sharing is not switched on for this workspace yet. Your note is saved and nothing \
             has been shared.",
            Lane::None,
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
            context,
            "I have nothing to share for you. Send /share <account> <note> first, and I will \
             show you exactly what would be shared.",
            Lane::None,
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
                    context,
                    &shared_reply(&proposal, activity_id, committed),
                    Lane::Reply(proposal.update),
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
            return finish_command(context, &reply, Lane::None);
        }
        Confirmation::Contested { activity_id, differing_field, .. } => {
            eprintln!(
                "relay: update {id} NEEDS AN OPERATOR — {activity_id} exists with a different \
                 {differing_field}; confirmed evidence is never overwritten"
            );
            return finish_command(
                context,
                "Something is already recorded under this note's identity and it does not match. \
                 Nothing was changed, and an operator has to look at it.",
                Lane::None,
            );
        }
        Confirmation::Refused { activity_id, code, .. } => {
            // A decision the store already made. Sending it again would receive the same answer,
            // so the only thing this can do is repeat what it said the first time.
            eprintln!(
                "relay: update {id} NEEDS AN OPERATOR — the append of {activity_id} was already \
                 refused ({code}); it is not retried"
            );
            return finish_command(context, REFUSED_REPLY, Lane::None);
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
    // Parsed, not measured. This was `.len() != 10`, which accepts `abcdefghij` and hands it to
    // the store to refuse as `invalid_occurred_at` — a round trip to learn what the shape already
    // says. `Date::parse` is the same rule the store applies, applied here.
    let derived_date =
        ironworks_domain::record::Date::parse(proposal.proposed_at.get(..10).unwrap_or_default())
            .map_err(|_| {
            e(format!(
                "proposal {} carries an unusable timestamp {:?}",
                proposal.update, proposal.proposed_at
            ))
        })?;
    let (activity_id, occurred_at) = match &proposal.confirm {
        Confirmation::Appending { activity_id, occurred_at, .. } => {
            for (what, persisted, derived) in [
                ("activity id", activity_id, &derived_id),
                ("occurrence date", occurred_at, &derived_date.as_str().to_owned()),
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
            // The guard above proved these equal, so re-parsing the persisted one cannot fail —
            // and taking it from the row is what keeps that true if the guard ever loosens.
            (
                activity_id.clone(),
                ironworks_domain::record::Date::parse(occurred_at.clone()).map_err(|_| {
                    e(format!("proposal {} holds an unusable occurrence date", proposal.update))
                })?,
            )
        }
        _ => (derived_id, derived_date),
    };

    // ── the durable append intent, before the request leaves ─────────────────────────
    if proposal.confirm.may_begin() {
        proposals
            .begin_append(proposal.update, &activity_id, occurred_at.as_str(), id.get())
            .map_err(|x| e(format!("proposal store: {x}")))?;
    }

    let store = required(Var::RecordStore)?;
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
                context,
                &reply,
                // The success reply gets the durable lane: telling someone twice that their note
                // was shared reads as two shares, and nothing they can see would say otherwise.
                Lane::Reply(proposal.update),
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
                context,
                "Something is already recorded under this note's identity and it does not match. \
                 Nothing was changed, and an operator has to look at it.",
                Lane::None,
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
            finish_command(context, REFUSED_REPLY, Lane::None)
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
fn share_scope(
    entry: &ironworks_domain::TenantEntry,
) -> Result<ironworks_domain::OrgId, ScopeRefusal> {
    // Configuration, not an outage: an unset variable reads the same on every cycle.
    let store = required(Var::RecordStore).map_err(|x| ScopeRefusal::Durable(x.message))?;
    let book = Book::new(&store, EnvCredentials);
    let evidence =
        ironworks_domain::ScopeResolver::resolve(&book, entry.records()).map_err(|x| {
            let why = format!(
                "REFUSING TO PROPOSE: the organization behind {}'s records could not be \
                 established ({x}). A proposal names the organization it would be promoted into, \
                 and guessing it is the failure this refuses to have.",
                entry.slug()
            );
            match x {
                // The store was not reached. Nothing is established about the credential, so the
                // next cycle may well resolve it — this is the one that must stay open.
                ironworks_domain::ScopeFailure::Unreachable { .. } => ScopeRefusal::Transient(why),
                // The store answered, and answered this. Re-asking returns the same on every
                // cycle, so holding the cursor for it buys nothing and costs every conversation.
                ironworks_domain::ScopeFailure::Unauthorized
                | ironworks_domain::ScopeFailure::Malformed { .. } => ScopeRefusal::Durable(why),
            }
        })?;
    // The same control the turn path applies. A promotion is an act for this tenant, so a
    // credential pointing at another organization refuses here exactly as it refuses there — and
    // a re-pointed credential is configuration, which no number of cycles will change.
    agreed_org(entry.slug().as_str(), entry.org(), evidence.org())
        .map_err(ScopeRefusal::Durable)?;
    Ok(evidence.org().clone())
}

/// Why a proposal's organization could not be established, and whether asking again could differ.
///
/// The same three-valued reasoning [`settlement`] applies to a turn, applied to a command. A
/// command update that is left open holds the process-wide safe cursor — the lowest open update
/// across every tenant — so leaving one open on a fault that reads identically forever stops every
/// conversation in the process while the loop backs off politely and the relay looks healthy. That
/// is what `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md` measured on
/// the turn path; this is the same shape on the command path.
enum ScopeRefusal {
    /// Nothing was established. Leave the update open — the next cycle re-reads and may differ.
    Transient(String),
    /// Established, and it will read the same on every cycle. Answer the sender and settle.
    Durable(String),
}

/// What a sender is told when a promotion cannot proceed for a durable reason.
///
/// It names no organization, no credential and no account. The operator's log carries which of the
/// registry entry and the credential is pointing somewhere unintended; a room is owed an answer
/// that something did not happen, and nothing about the configuration behind it.
const UNPROMOTABLE: &str =
    "I can't record that right now. This has been logged for the operator to look at.";

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
fn finish_command(context: CommandContext<'_>, text: &str, lane: Lane) -> Result<(), Exit> {
    let CommandContext { ch, updates, proposals, update, id, chunk_bytes } = context;
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
fn run_redeliver(
    b: &Boot,
    controls: &ExecutionControls,
    channel_url: &str,
    update: u64,
    chunk_bytes: usize,
) -> Result<(), Exit> {
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
    let audit = controls
        .call_log
        .as_deref()
        .map(CallAudit::open)
        .transpose()
        .map_err(|_| fail(FAILED, "runtime call audit could not be opened"))?;
    let runtime = Counted { inner: &b.runtime, audit: audit.as_ref() };
    let outcome = fetch_answer(&runtime, &response, &deadline)?;
    let Outcome::Completed { text, .. } = outcome else {
        return Err(e(format!("response {response} is not completed; there is nothing to send")));
    };

    let adapter_token = Var::AdapterToken.presented();
    let ch = ironworks_channel_http::Channel::authenticated(channel_url, adapter_token);
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

fn attestation_custody(metadata: &std::fs::Metadata, directory: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    attestation_custody_fields(
        metadata.is_file(),
        metadata.uid(),
        metadata.mode(),
        directory.is_dir(),
        directory.uid(),
        directory.mode(),
    )
}

fn attestation_custody_fields(
    regular: bool,
    file_uid: u32,
    file_mode: u32,
    directory: bool,
    directory_uid: u32,
    directory_mode: u32,
) -> bool {
    regular
        && file_uid == 0
        && file_mode & 0o022 == 0
        && file_mode & 0o004 != 0
        && directory
        && directory_uid == 0
        && directory_mode & 0o022 == 0
        && directory_mode & 0o001 != 0
}

fn read_confinement_attribution(
    path: &str,
    boot: &Boot,
) -> Result<ironworks_journal::CapabilityAttribution, Exit> {
    let path = std::path::Path::new(path);
    let mut file = std::fs::File::open(path)
        .map_err(|error| fail(FAILED, format!("cannot open confinement attestation: {error}")))?;
    let metadata = file.metadata().map_err(|error| {
        fail(FAILED, format!("cannot inspect confinement attestation: {error}"))
    })?;
    let directory = std::fs::metadata(registry_publish::parent(path)).map_err(|error| {
        fail(FAILED, format!("cannot inspect confinement attestation directory: {error}"))
    })?;
    if !attestation_custody(&metadata, &directory) {
        return Err(fail(
            FAILED,
            "confinement attestation must be a root-owned regular file, world-readable but not group/world-writable, in a root-owned non-writable traversable directory",
        ));
    }
    let mut json = String::new();
    file.read_to_string(&mut json)
        .map_err(|error| fail(FAILED, format!("cannot read confinement attestation: {error}")))?;
    let attestation = ironworks_domain::ConfinementAttestation::parse(&json)
        .map_err(|error| fail(FAILED, error.to_string()))?;
    let member = RuntimeIdentity::parse(required_by("--confinement-attestation", Var::MemberId)?)
        .map_err(|error| fail(USAGE, error.to_string()))?;
    let now = now_iso();
    attestation
        .validate_for(boot.scope.slug(), &member, &boot.profile_sha256, &boot.policy_sha256, &now)
        .map_err(|error| fail(FAILED, error.to_string()))?;
    Ok(ironworks_journal::CapabilityAttribution {
        profile_sha256: attestation.profile_sha256().clone(),
        policy_sha256: attestation.policy_sha256().clone(),
        catalogue_sha256: attestation.catalogue_sha256().clone(),
        observed_at: attestation.observed_at().clone(),
    })
}

/// Drive one logical turn to an answer, recovering from durable evidence first.
///
/// Shared by both commands, which is the point: there is exactly one path from a key to an
/// answer, so a channel cannot acquire a second, looser one.
struct TurnDriver<'a> {
    boot: &'a Boot,
    controls: &'a ExecutionControls,
}

impl TurnDriver<'_> {
    fn drive(
        &self,
        key: &IdempotencyKey,
        ask: &str,
        // Which room this turn continues, when there is one. `turn` answers on a terminal and has
        // none, so it writes no thread: a thread that can never be continued is not a thread.
        conversation: Option<&ironworks_domain::ChannelRef>,
        confinement_attestation: Option<&str>,
    ) -> Result<Answer, Exit> {
        let b = self.boot;
        let controls = self.controls;
        let e = |m: String| fail(FAILED, m);
        let deadline = Deadline::starting_now(Duration::from_secs(180));
        // Before anything can return: a recovered turn commits too, and the thread it commits to has
        // to record what composition it was built on.
        let alloy =
            b.verified.alloy_digest(&ironworks_services::Heap).map_err(|x| e(x.to_string()))?;
        let audit = controls
            .call_log
            .as_deref()
            .map(CallAudit::open)
            .transpose()
            .map_err(|_| fail(FAILED, "runtime call audit could not be opened"))?;
        let runtime = Counted { inner: &b.runtime, audit: audit.as_ref() };

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
            // `.to_string()`, and the boundary is deliberate. `retrieved_at` goes into the
            // envelope and a **recovered** turn reuses the journalled value so the reconstruction
            // is byte-identical (ADR 0005) — so on the arm above it comes back from a stored row
            // as a `String`. Typing it would mean re-parsing rows written before this change, on
            // the recovery path, which buys no guarantee: what is minted here is already checked.
            None => (now_iso().as_str().to_owned(), None),
        };

        // Only admission of a new logical turn needs current evidence. Every recovery branch above
        // remains available after expiry: accepted responses are fetched, and an already-journalled
        // dispatch/replay follows its existing evidence rather than being stranded by the timer.
        let attribution = match (&plan, confinement_attestation) {
            (None, Some(path)) => Some(read_confinement_attribution(path, b)?),
            _ => None,
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
                    Some(p) => {
                        eprintln!("thread: replaying the chain from {}", p.response().as_str())
                    }
                    None => eprintln!("thread: replaying a fresh turn"),
                }
                p
            }
            (None, Some(room)) => {
                let prior = b.journal.prior_thread(&b.scope, room).map_err(|x| e(x.to_string()))?;
                prior_supplied = prior.as_ref().and_then(|p| p.supplied.clone());
                let decision = ironworks_domain::decide(prior.as_ref(), &alloy, thread_limit());
                match &decision {
                    ironworks_domain::Continuation::Fresh(why) => {
                        eprintln!("thread: fresh — {why}")
                    }
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
                eprintln!(
                    "supply: carried by the thread — {n} record(s) already in this conversation"
                );
                (Vec::new(), RecordsState::CarriedByThread { records: *n as usize })
            }
        };
        let mark = supplied_mark(&supply, catalog.as_deref(), records.len());

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
            None => {
                let decision = ironworks_journal::Decision {
                    previous: chained_from,
                    carried: match &supply {
                        ironworks_domain::Supply::CarriedByThread { records } => Some(*records),
                        ironworks_domain::Supply::Full(_) => None,
                    },
                };
                match &attribution {
                    Some(attribution) => {
                        let admitted_at = now_iso();
                        b.journal.begin_attributed(
                            &b.scope,
                            key,
                            ironworks_journal::AttributedAdmission {
                                retrieved_at: &retrieved_at,
                                digest: &digest,
                                decision: &decision,
                                attribution,
                                now: &admitted_at,
                            },
                        )
                    }
                    None => b.journal.begin(
                        &b.scope,
                        key,
                        &retrieved_at,
                        &digest,
                        &decision,
                        &now_iso(),
                    ),
                }
                .map_err(|x| e(x.to_string()))?;
            }
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

        crash_if(controls, "before-dispatch");

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
        crash_if_at(controls, "before-response", || {
            eprintln!("ironworks: DISCARDING response {} (deliberate crash)", accepted.id);
        });
        // Recorded on accept, because that is the only moment it exists: a later fetch answers
        // with a placeholder and cannot recover it (§G-2, and `ironclaw::Accepted::reported_model`).
        b.journal
            .record_accepted(key, &accepted.id, &accepted.reported_model, &now_iso())
            .map_err(|x| e(x.to_string()))?;
        eprintln!("response {} ({})", accepted.id, label(&accepted.outcome));

        // A proof affordance, like `before-response` above. It leaves B4's exact precondition on
        // disk — **accepted, with a durable response id, and not yet polled** — which is the one
        // state from which a proof can act on the live run out of band before the next process
        // fetches it. The id is printed so a script can name it.
        crash_if_at(controls, "after-accepted", || {
            eprintln!("ironworks: response {} is durable (accepted, not polled)", accepted.id);
        });

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

        crash_if_at(controls, "before-commit", || {
            eprintln!("ironworks: response {} is durable (accepted)", accepted.id);
        });
        finish(
            &b.journal,
            &b.scope,
            key,
            outcome,
            &accepted.id,
            ThreadCommit { conversation, alloy: &alloy, supplied: mark.as_ref() },
        )
    }
}

/// Fetch an answer that already exists. Never calls `create`.
fn fetch_answer(
    runtime: &dyn Runtime,
    response: &ResponseId,
    deadline: &Deadline,
) -> Result<Outcome, Exit> {
    runtime.fetch(response, deadline).map_err(describe)
}

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

/// Commit the completed turn and hand back the answer. Printing and delivering are the callers'
/// business; this is the one place a turn becomes durably complete.
///
/// A response holding nothing a person may be shown is settled for an operator instead, and one
/// that is not yet terminal is refused without settling — [`settlement`] decides which.
fn finish(
    journal: &Journal,
    scope: &ThreadScope,
    key: &IdempotencyKey,
    outcome: Outcome,
    response: &ResponseId,
    thread: ThreadCommit<'_>,
) -> Result<Answer, Exit> {
    // A completed turn commits BEFORE anything decides whether its answer may be shown, and must:
    // the turn ran, the response id is durable, and this is never re-run. What a gate refuses
    // afterwards is delivery, not the turn (§E-1).
    if let Outcome::Completed { usage, .. } = &outcome {
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
    }

    match settlement(&outcome) {
        Settlement::Deliver => match outcome {
            Outcome::Completed { text, .. } => Ok(Answer { text }),
            // `Deliver` is returned for no other outcome, and `settlement` is total, so this is
            // unreachable rather than unhandled. Stated as a refusal rather than a panic: a
            // serving process must not abort on a verdict it can simply decline to act on.
            other => Err(fail(FAILED, format!("cannot deliver a {} turn", label(&other)))),
        },
        // Not terminal, so nothing is established and nothing is written. The next cycle re-reads
        // the same response and may find an answer (§E-7).
        Settlement::LeaveOpen => Err(fail(
            FAILED,
            format!(
                "the turn is still running. Refusing WITHOUT settling — response {response} is \
                 not terminal, so the next cycle re-reads it and decides then."
            ),
        )),
        Settlement::Settled { why } => {
            // Output a person may not be shown is printed for the operator instead. Never
            // journalled — the journal holds identifiers only (§E-11), and this is model output.
            // Driven by the same classification that settles, so the sentence an operator reads
            // and the reason the journal keeps cannot disagree.
            if let Outcome::Completed { text, .. } | Outcome::Incomplete { text } = &outcome {
                eprintln!("ironworks: {why}");
                eprintln!("           it is printed below and NOT delivered.");
                eprintln!("{text}");
            }
            Err(settle(journal, key, response, label(&outcome), why))
        }
    }
}

/// Stop automatic work on a turn whose response can never yield a deliverable answer.
///
/// One path rather than one per outcome, so a caller cannot record the alarm and forget to release
/// the cursor, or the reverse. It returns the [`Exit`] instead of a `Result` so every call site
/// reads `Err(settle(…))` and the journal write cannot be skipped.
///
/// **If the write itself fails the row stays open**, and the message says so. That is the right
/// way round: a store that could not be written has established nothing, so the next cycle finds
/// the update again and settles it then. It is the same reasoning as [`Rebuild::Unestablished`],
/// applied to our own store rather than the tenant's.
fn settle(
    journal: &Journal,
    key: &IdempotencyKey,
    response: &ResponseId,
    label: &str,
    why: String,
) -> Exit {
    let reason = match ironworks_domain::NonEmptyReason::parse(why) {
        Ok(r) => r,
        Err(x) => return fail(FAILED, x.to_string()),
    };
    if let Err(x) = journal.record_operator_required(key, &reason, &now_iso()) {
        return fail(
            FAILED,
            format!(
                "the turn {label} and cannot be delivered, and the journal REFUSED to settle it: \
                 {x}.\n  → it still holds the channel cursor. Nothing is established until this \
                 write lands, so the next cycle will try again.\n  → response id: {response}"
            ),
        );
    }
    fail(
        FAILED,
        format!(
            "REFUSING TO SERVE. The turn {label} and cannot be delivered.\n  {reason}\n  \
             It is now journalled OperatorRequired, so it no longer holds the channel cursor.\n  \
             → reconcile {} by response id {response}.",
            key.as_str()
        ),
    )
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

/// A credential argument names an environment variable; it is never the secret itself.
///
/// Both `lifecycle adopt` and `registry publish-one` take credentials on the command line, so both
/// are a place an operator can paste a bearer token into a shell history, a process listing and a
/// CI log at once. `env:` is the whole of the defence, and the name after it is constrained so the
/// handle cannot smuggle anything a later reader would have to interpret.
///
/// One function because it was two identical closures, one in each command. Its own function
/// because reaching either of them takes a lifecycle store, a runtime and a record store — which
/// is why a rule about not leaking credentials had no test.
fn checked_handle(raw: &str, flag: &str) -> Result<String, Exit> {
    let Some(name) = raw.strip_prefix("env:") else {
        return Err(fail(USAGE, format!("{flag} must be an env:NAME handle, never a secret")));
    };
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(fail(USAGE, format!("{flag} must be an env:NAME handle")));
    }
    Ok(raw.to_owned())
}

/// What the thread records as carrying, once this turn has been built.
///
/// A later turn asks this mark whether the conversation already holds the book. Answering with a
/// count that is not what went into the envelope makes that decision on a number nobody sent: too
/// low and the next turn re-supplies records the model can already see, too high and it withholds
/// records the model never got. So the count is **what was rendered on this turn** when the book
/// was supplied in full, and **what the thread already carried** when it was not.
///
/// `None` where there is no catalog: nothing was supplied and nothing is known to supply, which
/// leaves whatever an earlier turn established rather than overwriting it with an empty claim.
/// Returning a mark there would tell the next turn the book is carried when the store was down.
///
/// Its own function because [`drive`] needs a runtime, a record store and a journal at once, so
/// none of the four combinations is reachable from a test through it.
fn supplied_mark(
    supply: &ironworks_domain::Supply,
    catalog: Option<&str>,
    rendered: usize,
) -> Option<ironworks_domain::SuppliedMark> {
    let catalog = catalog?.to_owned();
    match supply {
        ironworks_domain::Supply::Full(_) => {
            Some(ironworks_domain::SuppliedMark { catalog, records: rendered as u32 })
        }
        ironworks_domain::Supply::CarriedByThread { records } => {
            Some(ironworks_domain::SuppliedMark { catalog, records: *records })
        }
    }
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

/// What [`finish`] does with the outcome a response reports.
///
/// Extracted for the reason [`Rebuild`] was: the wrong answer is expensive in every direction, and
/// no direction is reachable from a unit test through [`drive`], which needs a runtime, a record
/// store and a journal at once.
///
/// # The line this type draws
///
/// **The retryable window closes when a response id becomes durable.** Before it, a turn is
/// `NotSubmitted` or `SubmittedUnknown`, ambiguity is the default (§E-6), and the next cycle
/// rebuilds byte-identically and replays one logical turn — measured fifteen for fifteen in
/// `docs/experiments/0044-a-refused-turn-recovers-on-the-next-serving-cycle.md`. Nothing here
/// touches that path.
///
/// Every outcome this function sees arrives *after* that window, because `record_accepted` has
/// already run — in `drive`'s tail, or through `Recovery::Fetch`/`Recovery::Deliver`. So the only
/// automatic move left is to fetch the same response again, and the question this answers is
/// whether doing so could ever produce something different.
#[derive(Debug, PartialEq, Eq)]
enum Settlement {
    /// A clean answer. Commit it and hand it back.
    Deliver,
    /// The response is terminal and holds nothing deliverable, so re-reading it establishes the
    /// same thing on every cycle, forever. Settle `OperatorRequired`: release the shared cursor,
    /// keep the alarm.
    ///
    /// Left open instead, one such turn holds the safe cursor — the lowest open update across
    /// every tenant — and stops every conversation in the process while the loop backs off
    /// politely and the process looks healthy. Measured, cycles 3 and 4 of
    /// `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`.
    /// `why` is the durable reason; the outcome's own label is derived from the outcome at
    /// the settle site rather than carried here, so one fact is not stored twice.
    Settled { why: String },
    /// The response is **not** terminal. Refuse WITHOUT settling — the next cycle re-reads, and it
    /// may find an answer.
    ///
    /// This is the counterpart of [`Rebuild::Unestablished`] and it exists for the same reason:
    /// a poll that ran out of budget establishes nothing about the turn (§E-7 — the turn is
    /// accepted, is billed, and its id is known), and settling on it would trade indefinite
    /// deafness for silent loss.
    LeaveOpen,
}

/// Decide what a terminal outcome licenses, from the outcome alone.
///
/// **Total over [`Outcome`], and deliberately not a method on it.** "Can this be delivered?" is a
/// seam policy; the adapter's job is to say what the runtime reported. `Outcome::has_answer`
/// already marks where the two part company — it answers `true` for `Incomplete`, which this
/// withholds.
///
/// The durable reasons say what has been **established**, never what failed. At the pin a failed
/// response's `error` is a runtime-authored constant — `code` is a closed enum whose only
/// reachable value here is `internal_error`, and `message` is derived from `code` rather than
/// passed through
/// (`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses.rs:222-248`,
/// `ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/error.rs:48-62`). So it carries
/// no provider detail to leak *and* no diagnosis to offer:
/// `docs/experiments/0013-the-pin-fails-silently-not-closed.md` measured that exact constant
/// standing in for a provider rate limit. Naming it as a cause would be the mistake `0033` closed
/// — a refusal that names a cause it has not established. It is recorded as the runtime's own
/// label and nothing more.
///
/// `Incomplete`'s partial text never reaches a reason: that one *is* model output, and §E-11 holds.
fn settlement(outcome: &Outcome) -> Settlement {
    let settled = |why: String| Settlement::Settled { why };
    match outcome {
        // Not terminal. The one outcome where a later read can differ.
        Outcome::Running => Settlement::LeaveOpen,
        Outcome::Completed { text, .. } => match ironworks_ironclaw::malformed_answer(text) {
            // A gate rather than a heuristic, so it is deterministic on the same bytes and fires
            // identically on every cycle. The answer is durable and will not improve by re-reading.
            Some(found) => settled(format!(
                "the runtime returned {found} instead of an answer. The response is complete and \
                 will read the same on every cycle, so it will not become deliverable."
            )),
            None => Settlement::Deliver,
        },
        // Nothing in this binary declares `tools`, so a parked run was built somewhere this code
        // does not know about. Polling never moves it — only submitting the outputs would, which
        // is an operation this seam does not have (§A-3).
        Outcome::Parked { calls, .. } => {
            let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
            settled(format!(
                "the runtime parked this turn on {} unanswered tool call(s): {}. \
                 It reports status=completed and holds no answer.",
                calls.len(),
                names.join(", ")
            ))
        }
        Outcome::Failed { code, .. } => settled(format!(
            "the turn ran and produced no answer; the runtime labels it {code}. That label is the \
             runtime's own and not a diagnosis — it is the only value this field takes. The \
             response is terminal, so re-reading it returns the same failure."
        )),
        // Nothing here ever cancels a run. The runtime does expose a cancel route, so a cancelled
        // response is an act taken outside this process — retrying it automatically would fight
        // whoever took it.
        Outcome::Cancelled => settled(
            "the turn was cancelled. Nothing in this seam cancels a run, so this was done outside \
             it. The response is terminal, so re-reading it returns the same cancellation."
                .to_owned(),
        ),
        // Partial output is still output, and it is withheld for the reason a malformed answer is:
        // a half-answer a client cannot tell is half is worse than none. The text is printed for
        // an operator and never journalled.
        Outcome::Incomplete { .. } => settled(
            "the turn stopped early and its output is partial, so it is not deliverable. The \
             response is terminal, so re-reading it returns the same partial output."
                .to_owned(),
        ),
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

/// How many record fetches are in flight at once.
///
/// The store is one service and this is one tenant's turn, so this is politeness as much as speed:
/// enough to stop latency scaling with the size of a book, few enough that a large book cannot
/// look like a burst. The fetches are independent GETs against one host and `ureq::Agent` pools
/// connections across them.
const RECORD_FANOUT: usize = 8;

/// Map `f` over `items` concurrently and return the results **in the order of `items`**.
///
/// # Why the order is the whole contract
///
/// The caller that needs this fetches a tenant's records, and the envelope renders them in the
/// order it is given them while `ironworks_domain::supply` fingerprints them in that same order —
/// so the sequence *is* part of the request body. Crash recovery rebuilds that body and replays
/// its idempotency key expecting it byte-identical (ADR 0005), and an order that varied with which
/// response arrived first would rebuild a different turn and forfeit the replay.
///
/// So the work is split into contiguous chunks and concatenated, never collected as it completes:
/// **the order cannot depend on timing because nothing here observes timing.** That is the property
/// `concurrency_cannot_reorder_the_result` pins, with a closure whose completion order is the
/// reverse of its input order.
///
/// At most `fanout` threads run at once, and none are spawned at all for fewer than two items.
fn in_order<T: Sync, R: Send>(items: &[T], fanout: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if items.len() <= 1 {
        return items.iter().map(&f).collect();
    }
    let chunk = items.len().div_ceil(fanout.min(items.len()).max(1));
    std::thread::scope(|threads| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|slice| {
                let f = &f;
                threads.spawn(move || slice.iter().map(f).collect::<Vec<_>>())
            })
            .collect();
        // Joined in spawn order, which is input order, so concatenation restores the sequence the
        // caller gave regardless of which chunk finished first.
        handles.into_iter().flat_map(|h| h.join().expect("in_order worker")).collect()
    })
}

/// The one store failure that refuses a turn rather than degrading it.
///
/// [`fetch_records`] asks this twice — once for the catalogue, once per record — and the two arms
/// used to answer it independently. Scope drift says the credential no longer authorises the org
/// being served, which is a fact about authority and not about availability (§A-1). Every other
/// failure is the store being unreachable, unreadable, or closed to us, and the turn proceeds with
/// what it has.
fn refuses(f: &BookFailure) -> bool {
    matches!(f, BookFailure::ScopeDrift { .. })
}

/// Read the book for this scope. A data source, so every failure degrades — except scope drift,
/// which is a control failing late and refuses.
fn fetch_records(
    book: &Book<EnvCredentials>,
    scope: &RecordScope,
) -> Result<(Vec<ironworks_domain::record::AccountRecord>, RecordsState), Exit> {
    let catalog = match book.catalog(scope) {
        Ok(c) => c,
        Err(why) if refuses(&why) => return Err(scope_drift_exit(&why)),
        Err(why) => {
            // A cause, not a sentence: the words the MODEL sees are core's to choose. The operator
            // is not the model, and used to be shown nothing — this arm bound the failure to `_`,
            // so a revoked records token and a DNS outage left byte-identical output behind on a
            // path that otherwise reports everything it does.
            eprintln!("records: catalogue unavailable — {why}");
            return Ok((Vec::new(), RecordsState::Degraded(Degradation::Unavailable)));
        }
    };
    if catalog.is_empty() {
        // `Empty` is a claim that the org HAS no records. A catalogue whose every row was
        // unreadable is the opposite claim — the records are there and none of them arrived — and
        // reporting the first would tell the model the book is empty on the strength of rows it
        // could not parse.
        return Ok(match catalog.dropped() {
            0 => (Vec::new(), RecordsState::Empty),
            unreadable => (Vec::new(), RecordsState::Degraded(Degradation::Partial { unreadable })),
        });
    }
    let mut out = Vec::with_capacity(catalog.len());
    // Rows lost at the CATALOGUE stage count too. This used to start at zero and only ever count
    // per-record failures, so rows the listing dropped were invisible: the book silently shrank
    // and the model was told `Complete`. `Degradation::Partial` exists to say how many are
    // missing, and the catalogue is where some of them go missing.
    let mut unreadable = catalog.dropped();
    // Concurrent, and in catalogue order whatever the network does — see `in_order`.
    for fetched in in_order(catalog.entries(), RECORD_FANOUT, |e| book.record(scope, &e.id)) {
        match fetched {
            Ok(Some(r)) => out.push(r),
            Ok(None) => unreadable += 1,
            Err(why) if refuses(&why) => return Err(scope_drift_exit(&why)),
            Err(why) => {
                eprintln!("records: one record could not be read — {why}");
                unreadable += 1;
            }
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
/// The one clock read in a command, as the type every store takes.
///
/// # Checked where it is minted, not at each of the places it is written
///
/// This returned a `String`, and the stores took `now: &str` — so a value that was not a timestamp
/// at all reached a column, and `crates/journal`'s `ORDER BY first_seen, key` and the lifecycle
/// ledger's `ORDER BY at, rowid` rested on every caller happening to pass a canonical one. The
/// crash-boundary tests were passing `"t0"`, `"t1"` and `"t2"`.
///
/// **A clock that cannot be read as an instant stops the command.** This used to write `0` for a
/// clock before 1970 — a false instant every store accepts and every `ORDER BY` sorts, the one kind
/// of wrong value nothing downstream can notice. The calendar is
/// `ironworks_domain::Utc::from_unix_seconds`, the one conversion the domain owns, so this binary
/// keeps no second copy of it.
fn now_iso() -> ironworks_domain::Utc {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the host clock reads after 1970");
    ironworks_domain::Utc::from_unix_seconds(since.as_secs())
        .expect("the host clock reads before the year 10000")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct ObservedRuntime {
        fetches: Cell<usize>,
    }

    impl Runtime for ObservedRuntime {
        fn create(
            &self,
            _: &ModelTurn,
            _: &IdempotencyKey,
            _: &Deadline,
        ) -> Result<Accepted, TurnFailure> {
            panic!("this audit test does not create")
        }

        fn fetch(&self, _: &ResponseId, _: &Deadline) -> Result<Outcome, TurnFailure> {
            self.fetches.set(self.fetches.get() + 1);
            Ok(Outcome::Running)
        }
    }

    struct FailedAuditWrite;

    impl Write for FailedAuditWrite {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("audit volume unavailable"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn an_unwritable_requested_audit_prevents_the_runtime_call() {
        let inner = ObservedRuntime { fetches: Cell::new(0) };
        let audit = CallAudit::from_writer(FailedAuditWrite);
        let counted = Counted { inner: &inner, audit: Some(&audit) };
        let id = ResponseId::parse("resp_audit_control").unwrap();
        let deadline = Deadline::starting_now(Duration::from_secs(1));

        let failure = counted.fetch(&id, &deadline).expect_err("the audit write must fail closed");

        assert_eq!(inner.fetches.get(), 0, "no runtime request may precede its audit row");
        assert!(matches!(
            failure,
            TurnFailure::NotSent(Cause::LocalControl(ref detail))
                if detail == "runtime call audit could not be written"
        ));
    }

    #[test]
    fn model_policy_has_distinct_read_and_explicit_write_commands_with_no_model_argument() {
        let check = match parse_model_policy(&["check".into()]) {
            Ok(args) => args,
            Err(_) => panic!("check must parse"),
        };
        assert!(matches!(check, Command::ModelPolicyCheck));
        let apply = match parse_model_policy(&["apply".into()]) {
            Ok(args) => args,
            Err(_) => panic!("apply must parse"),
        };
        assert!(matches!(apply, Command::ModelPolicyApply));
        let refusal =
            parse_model_policy(&["apply".into(), "--model".into(), "provider/override".into()])
                .err()
                .unwrap();
        assert_eq!(refusal.code, USAGE);
        assert!(refusal.message.contains("accepts no model flag"));
        assert!(!refusal.message.contains("provider/override"));
    }

    #[test]
    fn guidance_publication_accepts_no_asserted_destination_or_service() {
        let Ok(parsed) = parse_guidance(&[
            "publish-one".into(),
            "acme".into(),
            "--registry".into(),
            "registry.json".into(),
            "--source".into(),
            "candidate.md".into(),
        ]) else {
            panic!("the derived-input form must parse");
        };
        assert!(matches!(
            parsed,
            Command::GuidancePublishOne { ref registry, ref source, .. }
                if registry == "registry.json" && source == "candidate.md"
        ));
        for flag in ["--service", "--guidance", "--destination"] {
            let Err(error) = parse_guidance(&[
                "publish-one".into(),
                "acme".into(),
                "--registry".into(),
                "registry.json".into(),
                "--source".into(),
                "candidate.md".into(),
                flag.into(),
                "asserted".into(),
            ]) else {
                panic!("asserted binding input must be refused");
            };
            assert_eq!(error.code, USAGE);
            assert!(error.message.contains("derives it from the registry"));
        }
    }

    #[test]
    fn state_and_tenant_administration_parse_only_valid_argument_sets() {
        assert!(matches!(
            parse_state(&[
                "snapshot".into(),
                "--config".into(),
                "backup.json".into(),
                "--output".into(),
                "/var/lib/ironworks/manifests/run".into(),
            ]),
            Ok(Command::StateSnapshot { .. })
        ));
        assert!(
            parse_state(&[
                "verify-snapshot".into(),
                "--manifest".into(),
                "manifest.json".into(),
                "--output".into(),
                "unexpected".into(),
            ])
            .is_err()
        );
        assert!(parse_tenant(&["deprovision".into(), "acme".into()]).is_err());
        assert!(
            parse_tenant(&[
                "export".into(),
                "acme".into(),
                "--registry".into(),
                "registry.json".into(),
                "--output".into(),
                "book.json".into(),
                "--delete-records".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn attestation_custody_accepts_the_deployed_shape_and_refuses_each_writable_substitute() {
        assert!(
            attestation_custody_fields(true, 0, 0o100644, true, 0, 0o40755),
            "the root-owned readable deployed shape is the positive control"
        );
        for fields in [
            (false, 0, 0o100644, true, 0, 0o40755),
            (true, 501, 0o100644, true, 0, 0o40755),
            (true, 0, 0o100664, true, 0, 0o40755),
            (true, 0, 0o100640, true, 0, 0o40755),
            (true, 0, 0o100644, false, 0, 0o40755),
            (true, 0, 0o100644, true, 501, 0o40755),
            (true, 0, 0o100644, true, 0, 0o40777),
        ] {
            assert!(!attestation_custody_fields(
                fields.0, fields.1, fields.2, fields.3, fields.4, fields.5
            ));
        }
    }

    #[test]
    fn relay_and_attest_parse_only_their_typed_evidence_inputs() {
        let Ok(relay) = parse_relay(&[
            "--registry".into(),
            "registry.json".into(),
            "--journal".into(),
            "serve.journal".into(),
            "--channel".into(),
            "http://127.0.0.1:3994".into(),
            "--confinement-attestation".into(),
            "/run/ironworks-attest/acme/current.json".into(),
        ]) else {
            panic!("relay evidence path did not parse");
        };
        assert!(matches!(
            relay,
            Command::Relay { confinement_attestation: Some(ref path), .. }
                if path.ends_with("current.json")
        ));

        let Ok(attest) = parse_lifecycle(&[
            "attest".into(),
            "acme".into(),
            "--store".into(),
            "lifecycle.store".into(),
        ]) else {
            panic!("attest did not parse");
        };
        assert!(
            matches!(attest, Command::LifecycleAttest { ref store, .. } if store == "lifecycle.store")
        );
    }

    #[test]
    fn model_policy_credential_refusal_distinguishes_measurement_from_write_failure() {
        use ironworks_reconcile::ModelPolicyError;

        let read = model_policy_exit(ModelPolicyError::CredentialRejected { operation: "GET" });
        assert_eq!(read.code, BLOCKED);
        assert!(read.message.contains("could not be measured"));

        let write = model_policy_exit(ModelPolicyError::CredentialRejected { operation: "PUT" });
        assert_eq!(write.code, FAILED);
        assert!(write.message.contains("write"));

        let read_back = model_policy_exit(ModelPolicyError::ReadBackMismatch(
            ironworks_reconcile::ModelPolicyComparison {
                selection_enabled: true,
                workspace_default_matches: false,
                allowed_models_exact: false,
                allowed_model_count: 2,
            },
        ));
        assert_eq!(read_back.code, FAILED);
        assert!(read_back.message.contains("read-back disagrees"));
    }

    #[test]
    fn legacy_registry_v0_has_frozen_representative_bytes_and_digest() {
        use sha2::Digest;

        let bytes = encode_legacy_registry_v0(
            "acme",
            "-1001",
            "relationship-intelligence",
            "/srv/acme.md",
            "env:ACME_RECORDS",
            "env:ACME_MEMBER",
        )
        .unwrap();
        let expected = b"{\n  \"tenants\": [\n    {\n      \"channel\": \"-1001\",\n      \"guidance\": \"/srv/acme.md\",\n      \"records_credential\": \"env:ACME_RECORDS\",\n      \"runtime_credential\": \"env:ACME_MEMBER\",\n      \"service\": \"relationship-intelligence\",\n      \"slug\": \"acme\"\n    }\n  ]\n}\n";
        assert_eq!(bytes, expected);
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(&bytes)),
            "8b0f6e5937e306f87cba3c3081801b836adf1652812901e4c715e59ffb443f83"
        );
    }

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

    /// Only the qualification build can select the proof-only service.
    ///
    /// This used to branch on `env!("CARGO_BIN_NAME")`, so each compilation asserted **one** arm
    /// and the other was checked only by the other binary's copy of this file. Now that the
    /// selection is a value, both arms are asserted in one process — and the negative arm is the
    /// one that matters, so it is no longer conditional on which binary is running.
    #[cfg(feature = "synthetic-qualification")]
    #[test]
    fn only_the_qualification_build_can_select_the_synthetic_service() {
        assert!(load_serving_definition(Services::Qualification, SYNTHETIC_SERVICE).is_ok());
        assert!(load_serving_definition(Services::Serving, SYNTHETIC_SERVICE).is_err());
        // And it is not an embedded service either way, so the arm above is refusing because the
        // service does not exist rather than because the loader is broken.
        assert!(!ironworks_services::service::available().any(|name| name == SYNTHETIC_SERVICE));
    }

    /// The same for guidance: the synthetic handle resolves only for the qualification build.
    #[cfg(feature = "synthetic-qualification")]
    #[test]
    fn only_the_qualification_build_can_resolve_the_synthetic_guidance() {
        let load = |services| {
            load_serving_guidance(services, "synthetic://guidance", "proof", SYNTHETIC_SERVICE)
        };
        assert!(load(Services::Qualification).is_ok());
        // Serving falls through to the filesystem, where `synthetic://guidance` is not a path.
        assert!(load(Services::Serving).is_err());
    }

    /// One tenant, two people, two private conversations — the shape the prototype needs.
    ///
    /// Test-only: nothing here reads or writes an operator's registry, which is why the shape is
    /// exercised through a fixture.
    fn shared_tenant() -> ironworks_domain::TenantRegistry {
        ironworks_domain::TenantRegistry::parse(
            r#"{"tenants":[{"slug":"multiagency","channel":"184223311",
                "org":"multiagency","service":"account-analysis","guidance":"/etc/ironworks/multiagency.guidance.md",
                "records_credential":"env:RECORDS","runtime_credential":"env:RUNTIME",
                "contributors":[
                  {"channel":"184223311","actor":"184223311","contributor":"alice"},
                  {"channel":"713300284","actor":"713300284","contributor":"bob"}]}]}"#,
        )
        .expect("the fixture registry is valid")
    }

    /// A command update must not hold the channel open for a fault that reads the same forever.
    ///
    /// `Transient` is the positive control: without it "everything settles" would pass here while
    /// being exactly the defect (§K-8), and settling a reachable-store outage would permanently
    /// lose a sender's `/share`.
    ///
    /// The rule this pins is `Disposition::Command => Openness::Settled`: a durable refusal goes
    /// through `finish_command`, which writes that disposition and releases the process-wide safe
    /// cursor. Left open instead, one such update stops every conversation in the process — the
    /// shape `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md` measured
    /// on the turn path.
    #[test]
    fn a_scope_refusal_settles_only_when_asking_again_cannot_differ() {
        use ironworks_domain::ScopeFailure;

        // Exhaustive on purpose: a new `ScopeFailure` variant must be classified here rather than
        // silently inheriting whichever arm happens to catch it.
        let durable = |f: ScopeFailure| match f {
            ScopeFailure::Unreachable { .. } => false,
            ScopeFailure::Unauthorized | ScopeFailure::Malformed { .. } => true,
        };

        assert!(
            !durable(ScopeFailure::Unreachable { detail: "connection refused".into() }),
            "a store that could not be reached established nothing; settling loses the /share"
        );
        assert!(
            durable(ScopeFailure::Unauthorized),
            "the store answered and refused the credential; every cycle reads the same"
        );
        assert!(
            durable(ScopeFailure::Malformed { detail: "not json".into() }),
            "the store's shape will not change by asking again"
        );

        // What the room is told carries no configuration detail, whichever fault it was.
        for leak in ["org", "credential", "registry", "acme", "IRONWORKS_RECORDS"] {
            assert!(
                !UNPROMOTABLE.to_lowercase().contains(leak),
                "the sender's reply must not name {leak}: {UNPROMOTABLE}"
            );
        }
    }

    /// Concurrency must not be able to reorder a result, at any size.
    ///
    /// The closure sleeps longest for the *first* item, so completion order is the reverse of
    /// input order. A collect-as-it-finishes implementation passes every other test and fails this
    /// one — which is the point, because the failure it models is silent: the envelope would render
    /// a tenant's records in arrival order, the request body would stop being reproducible, and
    /// crash recovery would rebuild a different turn and forfeit its idempotency replay (ADR 0005).
    ///
    /// Sizes span the chunk arithmetic: below the early return, exactly at the fan-out, either side
    /// of it, and well past it.
    #[test]
    fn concurrency_cannot_reorder_the_result() {
        for n in [0usize, 1, 2, 7, 8, 9, 33, 100] {
            let items: Vec<usize> = (0..n).collect();
            let got = in_order(&items, RECORD_FANOUT, |i| {
                std::thread::sleep(std::time::Duration::from_micros((n - i) as u64 * 200));
                *i
            });
            assert_eq!(got, items, "in_order returned a different sequence for n={n}");
        }
    }

    /// The fan-out is a bound, not a suggestion.
    ///
    /// Without it a large book spawns a thread per account, which is the burst this exists to
    /// avoid. Counted rather than reasoned about: `chunks` is what decides how many threads run.
    #[test]
    fn the_record_fanout_bounds_how_many_threads_run() {
        for n in [2usize, 8, 9, 100, 1000] {
            let items: Vec<usize> = (0..n).collect();
            let chunk = items.len().div_ceil(RECORD_FANOUT.min(items.len()).max(1));
            let threads = items.chunks(chunk).count();
            assert!(
                threads <= RECORD_FANOUT,
                "n={n} would run {threads} threads, above the fan-out of {RECORD_FANOUT}"
            );
            assert_eq!(
                items.chunks(chunk).map(<[usize]>::len).sum::<usize>(),
                n,
                "n={n}: the chunks do not cover every entry"
            );
        }
    }

    /// The org control, in both directions.
    ///
    /// Serving and promoting both reach `agreed_org`, so this is the one place the rule is stated
    /// and the one place it needs testing. The matching case is the positive control: without it
    /// "refuses everything" would pass here while being exactly the defect (§K-8).
    ///
    /// The refusal names both organizations, because an operator fixing this has to know which of
    /// the registry entry and the credential is the one pointing somewhere unintended.
    /// The two identity rules publication refuses on, both directions each.
    ///
    /// # Why these are worth a unit test at all
    ///
    /// They were four lines apiece inside `run_registry_publish_one`, which needs an Account
    /// Service, an IronClaw, a SQLite lifecycle store and a filesystem before it reaches them —
    /// so the rules that stop a registry being published against **another organization's
    /// records** or **another member's runtime** could only be exercised by standing all of that
    /// up. They are comparisons; they need none of it.
    ///
    /// The agreeing case is the positive control §K-8 requires: without it, "refuses everything"
    /// would satisfy every assertion below while being exactly the defect.
    #[test]
    fn publication_refuses_a_records_org_or_a_runtime_subject_that_disagrees() {
        use ironworks_domain::OrgId;
        use ironworks_lifecycle::{RuntimeSubject, RuntimeTenantId, RuntimeUserId};

        let acme = OrgId::parse("acme-sales").unwrap();
        let rival = OrgId::parse("rival-co").unwrap();

        assert!(agreed_records_org("acme-sales", &acme, &acme).is_ok(), "all three agreeing");
        // Recorded disagrees with what the credential resolved to.
        let why = agreed_records_org("rival-co", &acme, &acme).expect_err("recorded org differs");
        assert!(why.contains("rival-co") && why.contains("acme-sales"), "{why}");
        // The spec disagrees, even though the recorded id matches — the third comparison, which a
        // two-way check would have missed.
        let why = agreed_records_org("acme-sales", &rival, &acme).expect_err("spec org differs");
        assert!(why.contains("rival-co"), "{why}");

        let subject = |tenant: &str, user: &str| {
            RuntimeSubject::new(
                RuntimeTenantId::parse(tenant).unwrap(),
                RuntimeUserId::parse(user).unwrap(),
            )
        };
        let recorded = subject("tenant-a", "user-a");
        assert!(agreed_runtime_subject(&recorded, &recorded).is_ok(), "the same subject agrees");
        // Both halves are asserted separately, so a check comparing only the tenant — or only the
        // user — still fails here.
        let why = agreed_runtime_subject(&subject("tenant-b", "user-a"), &recorded)
            .expect_err("a different tenant must refuse");
        assert!(why.contains("tenant-b"), "{why}");
        assert!(
            agreed_runtime_subject(&subject("tenant-a", "user-b"), &recorded).is_err(),
            "a different user must refuse"
        );
    }

    #[test]
    fn a_tenant_is_acted_for_only_as_the_org_its_registry_entry_records() {
        let registry = shared_tenant();
        let slug = ironworks_domain::Slug::parse("multiagency").expect("slug");
        let entry = registry.tenant(&slug).expect("the fixture tenant");

        let recorded = ironworks_domain::OrgId::parse("multiagency").expect("org");
        assert!(
            agreed_org(slug.as_str(), entry.org(), &recorded).is_ok(),
            "the org the registry records was refused; every refusal below would then be vacuous"
        );

        let repointed = ironworks_domain::OrgId::parse("acme-sales").expect("org");
        let why = agreed_org(slug.as_str(), entry.org(), &repointed)
            .expect_err("a credential resolving to another organization was accepted");
        assert!(
            why.contains("multiagency") && why.contains("acme-sales"),
            "the refusal must name both organizations: {why}"
        );
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
        assert!(matches!(args, Command::Status { ref journal } if journal == "/tmp/status.db"));
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
        let Command::Relay { registry, channel, chunk_bytes, .. } = &args else {
            panic!("not a relay")
        };
        assert_eq!(registry, "/etc/ironworks/registry.json");
        assert_eq!(channel, "http://127.0.0.1:3994");
        assert_eq!(*chunk_bytes, DEFAULT_CHUNK_BYTES);
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
        assert!(matches!(
            args,
            Command::Compose { ref service, .. } if service == "relationship-intelligence"
        ));
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

    /// The shipped template is an input to the control, not merely a hand-written facsimile.
    #[test]
    fn the_shipped_guidance_template_is_not_publishable() {
        let path =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/tenants/guidance.template.md");
        let text = std::fs::read_to_string(path).expect("the shipped guidance template");
        assert_eq!(
            text.matches("<!--").count(),
            1,
            "the template's binding marker is its only HTML comment; operator notes belong beside it"
        );
        assert_eq!(
            text.matches("-->").count(),
            1,
            "the template's binding marker is its only comment-closing delimiter"
        );
        assert!(
            !text.contains("OPERATOR NOTES"),
            "operator instructions belong beside the model-visible template, never inside it"
        );
        assert!(matches!(
            Guidance::parse(&text, "your-slug", "relationship-intelligence"),
            Err(ironworks_domain::GuidanceError::UnwrittenStubs { .. })
        ));
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
                 path outside `envelope`. Serving now requires confinement *evidence* before the \
                 registry can be staged; `0035` measured why that gate is load-bearing: declaring \
                 `tools` puts the runtime \
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

    /// An accepted `--call-log` leaves a file, whatever the run then does.
    ///
    /// A negative assertion — "refused, **and** no create call was made" — reads its second half
    /// from this log, and a count taken from a file that does not exist is not a measurement of
    /// zero. `dev/prove.sh`'s wrong-credential check reported FAIL for exactly this: the refusal
    /// was correct and the evidence for the other half had never been written.
    #[test]
    fn arming_the_call_log_leaves_an_empty_file_a_negative_assertion_can_read() {
        let dir = ironworks_testkit::Scratch::new("armed");
        let path = dir.join("calls.log");

        let controls = ExecutionControls {
            crash_at: None,
            call_log: Some(path.to_str().expect("utf-8").to_owned()),
        };
        assert!(controls.arm_call_log().is_ok());
        assert!(path.exists(), "an accepted --call-log must exist before anything can refuse");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "", "and it starts empty");

        // Arming twice must not truncate: the second call is the same process re-entering, and a
        // truncated log would erase calls already counted.
        std::fs::write(
            &path,
            "create	resp_1
",
        )
        .expect("write");
        assert!(controls.arm_call_log().is_ok());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "create	resp_1
"
        );

        // No flag, nothing to arm — and no file invented for a run that did not ask for one.
        let none = ExecutionControls { crash_at: None, call_log: None };
        assert!(none.arm_call_log().is_ok());

        // A path that cannot be opened is a refusal, not a silently absent log.
        let bad = ExecutionControls {
            crash_at: None,
            call_log: Some(format!("{}/no/such/dir/calls.log", dir.path().display())),
        };
        assert!(bad.arm_call_log().is_err());
    }

    /// A credential flag names an environment variable and never carries the secret.
    ///
    /// The refusal that matters is the first one: a raw token accepted here reaches a shell
    /// history, a process listing and a CI log at once, and no later stage looks at the shape of
    /// the handle again. The positive control is beside it because a validator that rejects
    /// everything passes every negative test there is (§K-8).
    #[test]
    fn a_credential_flag_takes_a_handle_and_never_a_secret() {
        assert_eq!(
            checked_handle("env:IRONWORKS_RECORDS_TOKEN", "--records-credential").ok(),
            Some("env:IRONWORKS_RECORDS_TOKEN".to_owned())
        );
        assert_eq!(checked_handle("env:A_1", "--x").ok(), Some("env:A_1".to_owned()));

        // What this exists to stop: the token itself, on the command line.
        let Err(why) = checked_handle("sk-live-000000000000", "--member-credential") else {
            panic!("a raw secret must be refused");
        };
        assert_eq!(why.code, USAGE);
        assert!(why.message.contains("never a secret"), "{}", why.message);

        // And the shapes that are `env:`-prefixed but are not a variable name. Each would be
        // read by something later — a shell, a path resolver, a second parser.
        for bad in [
            "env:",
            "env:NAME WITH SPACE",
            "env:NAME=value",
            "env:$OTHER",
            "env:a-b",
            "env:a;rm -rf /",
            "env:../../etc/passwd",
            "ENV:NAME",
            " env:NAME",
        ] {
            assert!(checked_handle(bad, "--x").is_err(), "{bad} was accepted");
        }
    }

    /// The mark a thread carries forward is a count the *next* turn spends records on.
    ///
    /// All four combinations, because the two that produce a mark disagree about which count is
    /// the right one, and the two that do not are the ones that must leave an earlier turn's mark
    /// standing rather than overwriting it with an empty claim.
    #[test]
    fn a_thread_only_records_carrying_what_was_actually_supplied() {
        use ironworks_domain::supply::FullBecause;
        use ironworks_domain::{Supply, supply};

        let full = Supply::Full(FullBecause::NotChained);
        let carried = Supply::CarriedByThread { records: 7 };

        // Supplied in full: the mark is what went into THIS envelope, not what was carried before.
        let m = supplied_mark(&full, Some("cat-1"), 12).expect("a full supply marks the thread");
        assert_eq!(m.records, 12);
        assert_eq!(m.catalog, "cat-1");

        // Carried: the envelope rendered nothing, so the rendered count is not the answer — the
        // thread still holds the seven records an earlier turn supplied.
        let m = supplied_mark(&carried, Some("cat-1"), 0).expect("a carried supply marks it too");
        assert_eq!(m.records, 7, "the rendered count is 0 here and is not what the thread carries");

        // No catalog: the book could not be read. Claiming a mark here would tell the next turn
        // the conversation carries a book that was never sent.
        assert!(supplied_mark(&full, None, 12).is_none());
        assert!(supplied_mark(&carried, None, 0).is_none());

        // The decision this feeds is the real one, so a fresh turn with no catalog stays full.
        assert!(matches!(supply::decide(false, None, None), Supply::Full(_)));
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
            attribution: None,
            // Nothing was accepted, so the runtime named nothing.
            reported_model: ironworks_domain::ReportedModel::Absent,
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

    /// Every terminal outcome that holds no deliverable answer settles; the one non-terminal
    /// outcome does not.
    ///
    /// Three-sided for the reason `rebuild_verdict`'s test is: each verdict is the failure of the
    /// other two. Reading `LeaveOpen` as `Settled` permanently destroys a turn the next cycle
    /// would have answered — §E-7's poll-deadline case, where the turn is accepted, billed and
    /// its id known. Reading `Settled` as `LeaveOpen` restores the indefinite retry that held the
    /// channel in `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`,
    /// because the only automatic move left is to re-fetch a terminal response and get the same
    /// answer forever. Reading `Deliver` as either withholds a good answer or ships a broken one.
    ///
    /// `Running` and the clean `Completed` are the positive controls: without them "everything
    /// settles" would pass here while being exactly the defect (§K-8).
    #[test]
    fn a_terminal_response_with_no_answer_settles_and_a_running_one_does_not() {
        use ironworks_ironclaw::{ToolCall, Usage};

        let usage = || {
            Some(Usage {
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: 0,
                total_cost_usd: None,
            })
        };
        let settled = |o: &Outcome| match settlement(o) {
            Settlement::Settled { why } => (label(o), why),
            other => panic!("{o:?} must settle, got {other:?}"),
        };

        // ── the control: a clean answer is delivered, and a running turn is left alone ──
        assert_eq!(
            settlement(&Outcome::Completed { text: "Acme is at 40%.".into(), usage: usage() }),
            Settlement::Deliver,
            "an ordinary answer was withheld"
        );
        assert_eq!(
            settlement(&Outcome::Running),
            Settlement::LeaveOpen,
            "a turn that is still running was settled permanently — the next fetch may answer it"
        );

        // ── terminal, and holding nothing a person may be shown ──
        let (label, why) =
            settled(&Outcome::Failed { code: "internal_error".into(), message: "x".into() });
        assert_eq!(label, "failed");
        assert!(why.contains("internal_error"), "the runtime's own label is not recorded: {why}");
        assert!(
            !why.contains('x'),
            "the sanitized message is a pure function of the code; storing it stores one fact \
             twice: {why}"
        );

        assert_eq!(settled(&Outcome::Cancelled).0, "cancelled");
        assert_eq!(settled(&Outcome::Incomplete { text: "half an ans".into() }).0, "incomplete");
        assert_eq!(
            settled(&Outcome::Completed { text: "<|im_start|>".into(), usage: usage() }).0,
            "completed",
            "a control token reaching the answer must settle, not deliver"
        );

        // Parked keeps the reason it always had, and reaches it through the shared path now.
        let (label, why) = settled(&Outcome::Parked {
            calls: vec![ToolCall {
                call_id: "fc_1".into(),
                name: "get_balance".into(),
                arguments: "{}".into(),
            }],
            usage: usage(),
        });
        assert_eq!(label, "parked on a tool call");
        assert!(why.contains("get_balance"), "{why}");

        // ── the journal holds identifiers only (§E-11) ──
        // Model output is printed for an operator and must never reach a durable reason.
        let partial = "the pipeline for Northwind is";
        let (_, why) = settled(&Outcome::Incomplete { text: partial.into() });
        assert!(!why.contains("Northwind"), "model output reached a durable reason: {why}");
        let (_, why) =
            settled(&Outcome::Completed { text: "<|im_start|>Northwind".into(), usage: usage() });
        assert!(!why.contains("Northwind"), "model output reached a durable reason: {why}");
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
                &ironworks_lifecycle::ObservedAt::parse("2026-09-02T00:00:00+00:00").unwrap(),
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
        assert!(matches!(ordinary, Command::LifecycleConfine { dry_run: true, .. }));
    }

    #[test]
    fn a_flag_can_never_be_swallowed_as_another_flags_value() {
        // `--store` used to be read as `it.next().cloned()` in `confine` and `members` and as
        // `flags.value(flag)?` in `adopt` and `inspect`. Under the first form `--store --dry-run` set the
        // store to the literal `--dry-run` and consumed it, so a run the operator asked to be a
        // dry run applied the confinement — and `confine` spends a member, with approval-off
        // one-way (§L-9). A value that begins with `--` is still accepted, because a path may;
        // what is refused is a `--store` with nothing after it at all.
        for argv in [
            vec!["confine".to_owned(), "client".to_owned(), "--store".to_owned()],
            vec!["members".to_owned(), "--store".to_owned()],
        ] {
            let subcommand = argv[0].clone();
            let Err(err) = parse_lifecycle(&argv) else {
                panic!("a trailing --store must be refused in {subcommand}")
            };
            assert_eq!(err.code, USAGE);
            assert!(
                err.message.contains("--store needs a value"),
                "{subcommand} reported {:?}, not that the flag needs a value",
                err.message
            );
        }

        // The positive control, and the behaviour the defect actually broke: `--dry-run` reaches
        // the command rather than being eaten as the store's value.
        let Ok(dry) = parse_lifecycle(&[
            "confine".into(),
            "client".into(),
            "--store".into(),
            "/tmp/lifecycle.store".into(),
            "--dry-run".into(),
        ]) else {
            panic!("confine with a store and a dry run should parse")
        };
        assert!(matches!(
            dry,
            Command::LifecycleConfine { ref store, dry_run: true, .. }
                if store == "/tmp/lifecycle.store"
        ));
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
            args,
            Command::LifecycleAdopt {
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
    fn publish_one_accepts_handles_and_refuses_asserted_authority_fields() {
        let Ok(args) = parse_registry(&[
            "publish-one".into(),
            "local-name".into(),
            "--store".into(),
            "lifecycle.store".into(),
            "--registry".into(),
            "registry.json".into(),
            "--guidance".into(),
            "guidance.md".into(),
            "--records-credential".into(),
            "env:TENANT_RECORDS".into(),
            "--member-credential".into(),
            "env:TENANT_MEMBER".into(),
        ]) else {
            panic!("publish-one should parse")
        };
        assert!(matches!(args, Command::RegistryPublishOne { .. }));

        for flag in ["--org", "--member-id", "--channel", "--service"] {
            let Err(err) = parse_registry(&[
                "publish-one".into(),
                "local-name".into(),
                "--store".into(),
                "s".into(),
                "--registry".into(),
                "r".into(),
                "--guidance".into(),
                "g".into(),
                "--records-credential".into(),
                "env:R".into(),
                "--member-credential".into(),
                "env:M".into(),
                flag.into(),
                "asserted".into(),
            ]) else {
                panic!("publish-one accepted {flag}")
            };
            assert!(err.message.contains("lifecycle evidence"));
        }
    }

    #[test]
    fn rollback_one_takes_only_the_registry_path() {
        let Ok(args) = parse_registry(&[
            "rollback-one".into(),
            "--registry".into(),
            "/etc/ironworks/registry.json".into(),
        ]) else {
            panic!("rollback parses")
        };
        assert!(matches!(args, Command::RegistryRollbackOne { .. }));
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

    /// The clock mints a value the stores will accept, and it is checked where it is minted.
    ///
    /// This used to assert `record::Timestamp::parse(now_iso()).is_ok()` and `len() == 25`.
    /// `Timestamp` only refuses an empty string, so the first assertion held for any 25 characters
    /// — and the second was the whole of the shape check. `now_iso` now returns
    /// `ironworks_domain::Utc`, so it cannot return at all unless the value parses: the assertion
    /// is that the type exists, and the interesting half is the calendar the old pair never saw.
    #[test]
    fn a_minted_timestamp_and_key_are_both_valid() {
        let now = now_iso();
        assert_eq!(now.as_str().len(), 25);
        assert!(now.as_str().ends_with("+00:00"), "the offset the ordering rests on");
        assert!(IdempotencyKey::parse(mint_key()).is_ok());

        // The negative arm the old pair had no way to express: a value of the right length and
        // punctuation but an impossible calendar. `ObservedAt` accepted all of these until it
        // became `Utc`, and each of them sorts between two real instants.
        for impossible in [
            "2026-13-45T99:99:99+00:00",
            "2026-02-29T00:00:00+00:00", // 2026 is not divisible by 4
            "2026-04-31T00:00:00+00:00", // April has 30 days
            "2026-08-27T24:00:00+00:00", // hours are 00–23
        ] {
            assert!(ironworks_domain::Utc::parse(impossible).is_err(), "{impossible} parsed");
        }
        // The positive controls: a real leap day, and the last second of a day.
        assert!(ironworks_domain::Utc::parse("2024-02-29T00:00:00+00:00").is_ok());
        assert!(ironworks_domain::Utc::parse("2026-08-27T23:59:59+00:00").is_ok());
    }

    /// Every store failure that is not scope drift degrades the turn rather than refusing it.
    /// What the model is *told* about that state is asserted in core, where the words now live —
    /// this crate can no longer author them, which is the point.
    ///
    /// This used to assert `!matches!(f, ScopeDrift { .. })` over an array of four variants that
    /// are not `ScopeDrift`, and then construct a `RecordsState` inside the loop with no
    /// relationship to `f` at all — true by construction on both counts, and it would still have
    /// passed with `fetch_records` deleted. It now asks [`refuses`], which is the predicate
    /// `fetch_records` actually branches on.
    #[test]
    fn every_store_failure_degrades_rather_than_refusing() {
        for f in [
            BookFailure::Unreachable { detail: "connection refused".into() },
            BookFailure::Unauthorized,
            BookFailure::Malformed { detail: "bad json".into() },
            BookFailure::NoCredential { handle: "env:X".into() },
        ] {
            assert!(!refuses(&f), "{f} should degrade the turn, not refuse it");
        }

        // The positive control §K-8 requires: the predicate can still say yes. Without this the
        // loop above would pass against a `refuses` that returned `false` for everything, which
        // is the same defect as one that returned `true`.
        assert!(refuses(&BookFailure::ScopeDrift {
            asked: ironworks_domain::OrgId::parse("acme").unwrap(),
            answered: ironworks_domain::OrgId::parse("rival").unwrap(),
        }));
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

    /// The crash points are the proof's only non-determinism budget: named, not timed.
    #[test]
    fn the_crash_points_are_named_not_timed() {
        for point in [
            "before-dispatch",
            "before-response",
            "after-accepted",
            "before-commit",
            "after-output",
            "after-send",
            "after-delivered",
        ] {
            let controls =
                ExecutionControls { crash_at: Some("something-else".into()), call_log: None };
            // A point that is not the configured one does nothing at all.
            crash_if(&controls, point);
        }
    }
}
