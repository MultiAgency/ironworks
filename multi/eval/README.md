# Evaluation

Synthetic account books and cases exercise `account-analysis@1`. Run grader self-tests offline
with `python3 multi/eval/test_graders.py`; run model evaluation only against a provisioned synthetic
evaluation tenant with `multi/eval/run_eval.py --runs 2`. Review transcripts, not only scores.

**This suite grades the service that declares it.** `run_eval.py --service <name>` composes that
service definition rather than the default, and refuses any service whose own definition does not
name `multi/eval` as its `evaluation` — including `relationship-intelligence@1`, which declares
`null` on purpose. That refusal is the point: these cases grade account qualification against a
four-tier evidence vocabulary only `ANALYST.md` defines, so running them elsewhere yields a score
that measures nothing. Do not write a suite to remove a `null`; the null is information
([`../services/README.md`](../services/README.md)). Selection is checked offline by
`test_service_selection.py`.

Everything under `book/` is invented test data. Never substitute a real tenant record or treat an
evaluation score as a deployment, isolation, or client-usage claim.
