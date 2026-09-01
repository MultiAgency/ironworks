#!/usr/bin/env python3
"""Run the answer-quality evaluation against a live instance, through the REAL seam.

    # 1. seed the eval book into its own org (see README)
    # 2. run:
    IRONCLAW_API=http://127.0.0.1:3020 \
    EVAL_IRONCLAW_TOKEN=<sealed member token> EVAL_ACCOUNT_TOKEN=<org token> \
      python3 multi/eval/run_eval.py

Options:
    --service NAME  which service definition to compose and grade (default: the seam's
                    DEFAULT_SERVICE). A service is refused unless its own definition declares
                    THIS suite as its `evaluation` — see `resolve_service` for why.
    --case <id>     run one case
    --runs N        run each case N times (consistency: same book, same question)
    --json PATH     write the full transcript + verdicts

This drives `context_ingress.turn` — the same code path a client's Telegram message takes —
so a pass here is evidence about the product, not about a harness. Answers are graded by
multi/eval/cases.py, whose graders are themselves self-tested (test_graders.py).
"""
import argparse, json, os, pathlib, sys, collections

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "seam"))

from cases import CASES, PASS, FAIL, REVIEW

# BELOW THE IMPORT, NOT ABOVE IT, and that is load-bearing rather than tidiness. Ruff's E402
# exempts `sys.path` manipulation preceding a module-level import, which is why that import needs
# no suppression. A plain assignment above it is NOT exempt and silently ends the exemption: these
# two lines sat above the import for one afternoon and put an E402 on a line nobody had touched
# since it was written. Anything that is not `sys.path` setup belongs down here.
ROOT = pathlib.Path(__file__).resolve().parents[2]
SUITE_DIR = pathlib.Path(__file__).resolve().parent


def resolve_service(name=None):
    """The service this suite is allowed to grade, or exit saying why it is not.

    THIS IS WHAT MAKES `evaluation` MEAN SOMETHING. Every definition declares an `evaluation`
    path — `"multi/eval"` for account-analysis, `null` for relationship-intelligence — and until
    now no code could act on it: this runner composed the DEFAULT service unconditionally, so the
    field was a claim about coverage that nothing checked and adding a second graded service
    meant editing the runner rather than the manifest.

    Two refusals, both fail-closed:

      * `evaluation: null` means nothing measures this service's answers. That null is
        INFORMATION — `multi/services/README.md` argues at length against writing a suite to fill
        one — so grading it here anyway would manufacture exactly the coverage the null denies.
      * an `evaluation` pointing somewhere else means this suite does not claim to cover that
        service. `multi/eval/` grades account-qualification cases whose evidence-discipline
        grader greps a four-tier vocabulary only `ANALYST.md` defines; run it against a service
        that declares a different objective and every case measures nothing.
    """
    import services as svc
    name = name or svc.DEFAULT_SERVICE
    try:
        d = svc.load_service(name)
    except svc.ServiceError as e:
        # No "Known: ..." appended here — `load_service`'s own message already lists them, and a
        # second copy is a second thing to keep in step.
        sys.exit(str(e))
    declared = d["evaluation"]
    if declared is None:
        sys.exit(
            f"service {name!r} declares \"evaluation\": null — nothing measures its answers, and "
            "that is deliberate.\nDo not point this suite at it to remove the null: multi/eval/ "
            "grades account-qualification cases and would report a score that means nothing.\n"
            f"Its responsibility is: {d['responsibility']}")
    if (ROOT / declared).resolve() != SUITE_DIR:
        sys.exit(
            f"service {name!r} declares its evaluation suite as {declared!r}, which is not this "
            f"one ({SUITE_DIR.relative_to(ROOT)}).\nRun the suite that claims to cover it, or fix "
            "the definition — a suite grading a service it does not claim is not evidence.")
    return d


def build_thread(service=None):
    import context_ingress as ing
    from persona import compose_service_persona
    d = resolve_service(service)
    guidance = SUITE_DIR / "eval.guidance.md"
    if not guidance.is_file():
        sys.exit(f"missing {guidance} — the eval client needs guidance like any other client "
                 "(composition fails closed by design)")
    missing = [v for v in ("EVAL_IRONCLAW_TOKEN", "EVAL_ACCOUNT_TOKEN") if not os.environ.get(v)]
    if missing:
        sys.exit(f"missing {' and '.join(missing)} — the eval org's own credentials. "
                 "Provision it and seed the book first (see multi/verify/README.md). "
                 "Never point this at a real client's tokens.")
    client = ing.ClientConfig(
        slug="eval",
        ironclaw_token=os.environ["EVAL_IRONCLAW_TOKEN"],
        account_token=os.environ["EVAL_ACCOUNT_TOKEN"],
        name="Evaluation Org",
        # `compose_service_persona`, not the default-service alias: the service is now chosen.
        # A guidance file whose first-line marker names a different service (or names none, which
        # pins the default) is REFUSED inside load_guidance — the same two-agreeing-edits rule
        # that stops a registry tenant landing on the wrong composition. Grading a service
        # therefore needs its guidance to say so too, which is the correct amount of friction.
        persona=compose_service_persona(d, str(guidance), "eval"),
    )
    return ing, ing.Thread(client=client)


def run_case(ing, thread, case, tally, results, width=20, run=None):
    """One case end to end: turn, grade, tally, record, print. Shared with adversarial.py,
    which re-implemented this loop while already importing build_thread from here (and had
    drifted: a dict tally instead of a Counter, and a different field width).

    A dead turn is a real RESULT, not a crash — it grades FAIL and the sweep continues.
    """
    try:
        answer, supplied = ing.turn(thread, case["q"])
    except Exception as e:
        verdict, why, answer, supplied = FAIL, f"turn raised {type(e).__name__}: {e}", "", []
    else:
        verdict, why = case["grader"](answer)
    tally[verdict] += 1
    row = dict(case=case["id"], verdict=verdict, why=why, question=case["q"],
               expect=case["expect"], supplied=list(supplied or []), answer=answer)
    if run is not None:
        row = dict(run=run, **row)
    results.append(row)
    mark = {PASS: "PASS", FAIL: "FAIL", REVIEW: "????"}[verdict]
    print(f"  [{mark}] {case['id']:{width}} {why}")
    return verdict


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--service",
                    help="which service definition to compose and grade. Defaults to the "
                         "seam's DEFAULT_SERVICE. A service whose definition declares a "
                         "different evaluation suite, or null, is refused rather than "
                         "graded by cases that do not claim to cover it.")
    ap.add_argument("--case")
    ap.add_argument("--runs", type=int, default=1)
    ap.add_argument("--json")
    ap.add_argument("--isolate", action="store_true",
                    help="fresh thread per case. Default is one shared thread, which is what a "
                         "real Telegram room looks like — but it confounds measurement, because "
                         "the seam injects context ONCE and later cases then answer partly from "
                         "thread history rather than a fresh retrieval. Use --isolate to test "
                         "per-question capability; use the default to test the lived experience.")
    args = ap.parse_args()

    cases = [c for c in CASES if not args.case or c["id"] == args.case]
    if not cases:
        sys.exit(f"no case matching {args.case!r}. Known: {[c['id'] for c in CASES]}")

    ing, thread = build_thread(args.service)
    results, tally = [], collections.Counter()

    for run in range(1, args.runs + 1):
        if args.runs > 1:
            print(f"\n{'='*70}\nRUN {run}/{args.runs}   (fresh thread — no carry-over between runs)\n{'='*70}")
            _, thread = build_thread(args.service)
        for c in cases:
            if args.isolate:
                _, thread = build_thread(args.service)
            run_case(ing, thread, c, tally, results, width=20, run=run)

    total = sum(tally.values())
    print(f"\n{'-'*70}")
    print(f"  {tally[PASS]}/{total} pass · {tally[FAIL]} fail · {tally[REVIEW]} need human review")
    if tally[REVIEW]:
        print("  REVIEW means the grader could not tell — read those answers yourself.")
    if args.runs > 1:
        by_case = collections.defaultdict(set)
        for r in results:
            by_case[r["case"]].add(r["verdict"])
        unstable = [c for c, v in by_case.items() if len(v) > 1]
        print(f"  consistency: {len(by_case) - len(unstable)}/{len(by_case)} cases gave the same "
              f"verdict every run" + (f"; UNSTABLE: {unstable}" if unstable else ""))

    if args.json:
        pathlib.Path(args.json).write_text(json.dumps(results, indent=1))
        print(f"  transcript written to {args.json}")

    sys.exit(1 if tally[FAIL] else 0)


if __name__ == "__main__":
    main()
