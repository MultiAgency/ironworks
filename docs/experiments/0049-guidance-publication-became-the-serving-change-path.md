# 0049 — Guidance publication became the serving-change path

**2026-09-06.** Remediation record for `0047`, on the working tree that introduced the gate. No
runtime, model, member, channel, record service, or live host was contacted.

`0047` found that replacing the registry-named guidance file changed the next serving cycle while
the two diagnostics ran only in advisory or registry-publication paths. This change adds the
operator path that owns that replacement:

```text
ironworks guidance publish-one <slug> --registry <path> --source <path>
ironworks guidance rollback-one <slug> --registry <path>
```

`crates/cli/src/lib.rs::run_guidance_publish_one` reads the matching entry from the strict
registry and derives its service and canonical guidance destination. It accepts neither value as
an argument. It parses the candidate through `Guidance::parse`, which refuses both a surviving
comment-closing delimiter and an unwritten template stub. The refusal reports counts only.

*Citation repaired 2026-09-07: this named a `refuse_guidance_hazards` in the CLI, called beside the
parse. That function no longer exists — the refusal moved inside
`crates/domain/src/composition.rs::Guidance`, with an `UnwrittenStubs` variant on
`crates/domain/src/composition.rs::GuidanceError`. The finding is
unchanged and strengthened by the move: every caller of the parser inherits the refusal, so
publication is no longer the only path that gets it.*

`crates/cli/src/registry_publish.rs::RegistryLock::publish_artifact` performs the replacement under
the registry publication lock. It creates owner-only same-directory temporary files, syncs their
contents, syncs the directory around each rename, atomically replaces the live name, and preserves
the prior bytes in `.previous`. Rollback copies rather than consumes that artifact.

The two-sided subprocess control in `crates/cli/tests/guidance_publish.rs` passed: clean guidance
published and rolled back with mode `0600`, while both the leaked-delimiter candidate and the
stubbed candidate were refused without changing the live file. The focused run reported 2 passed,
0 failed. `dev/test-prove-host.sh` also passed its clean case and both deliberately hazardous
scorer controls. It does not establish the live host result: `dev/prove-host.sh` remains an
attended, read-only decision and was not run here.

This closes the code and documented-procedure gap found by `0047`; it does not retroactively make
that audit a deployment measurement. A direct filesystem write can still bypass the command, as
root can bypass every local policy. The supported deployment procedure no longer instructs one,
and the read-only host proof now independently composes the registry-derived live file and fails
on either warning.
