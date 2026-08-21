# Agent spec — the fleet's handoff format

MultiAgency grows itself: when a client (or the team) needs a new agent, this **one
artifact** flows through three stages — emitted by a client-facing Multi agent, designed
into a persona, then built — while the crew coordinates the flow in **Multron's** internal
group. No agent calls another; a human carries the spec between stages and approves each
step. The harness isolates instances, so the human (and this shared artifact) is the bus —
deliberately. The stages below describe the work, not a roster: an operator does all three.

## The spec

```
AGENT SPEC
  name:       <agent's name, e.g. "Acme Concierge">   (persona name; default "Multi")
  slug:       <infra slug, e.g. acme>                 (→ container ironclaw-<slug>, host <slug>.<your-domain>)
  audience:   <who it serves: this client's staff? external customers? contributors?>
  purpose:    <one line — what this agent is for>
  boundaries: <what it must NOT do; confidentiality scope; when to escalate to a human>
  tone:       <voice: how much character, formality, language>
  tools:      <anything beyond chat it needs — default: none>
  origin:     <where this came from — usually a Multron work order for <client>>
```

## The pipeline (who does what)

1. **Multi agent (intake).** During client onboarding, if the client needs their own
   agent, the client-facing Multi agent ends with an AGENT SPEC — `audience` / `purpose` /
   `boundaries` drawn from the discovery. A draft a human reviews (alongside the work
   order). The crew picks it up in **Multron's** internal group and drives it forward.
2. **Design.** Takes the spec and drafts the agent's persona in the house
   shape (identity → setting → what it does → human-in-the-loop → privacy → voice),
   honoring `boundaries` and `tone`. Result: a new `agent/identity/<Name>.md`. A human
   approves it.
3. **Build.** Takes the spec + persona and drafts the provision command:
   ```
   PROVISION_FROM_ENV=1 \
   PERSONA_SOURCE=agent/identity/<Name>.md AGENT_NAME="<name>" PURPOSE="<purpose>" \
   TELEGRAM_BOT_TOKEN="$(cat ~/.agency/<slug>.token)" TELEGRAM_BOT_USERNAME=<bot> \
     ./provision-agent.sh "<slug>"
   ```
   `PROVISION_FROM_ENV=1` is required because the command passes `PERSONA_SOURCE`; without it
   the script's inherit-guard refuses the run (`provision-agent.sh` :44/:61).
   A human runs it, then `./deploy/doctor.sh <slug>` to verify.

## The rule that keeps it safe

Every stage **drafts**; a **human** carries the spec forward and approves. No agent
provisions, edits, or messages another — that would concentrate fleet-wide power behind a
prompt-injectable chat, the exact risk the isolation model exists to prevent. The spec is
what lets three isolated agents behave like one pipeline without ever touching each other.
