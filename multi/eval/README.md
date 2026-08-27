# Evaluation

Synthetic account books and cases exercise `account-analysis@1`. Run grader self-tests offline
with `python3 multi/eval/test_graders.py`; run model evaluation only against a provisioned synthetic
evaluation tenant with `multi/eval/run_eval.py --runs 2`. Review transcripts, not only scores.

Everything under `book/` is invented test data. Never substitute a real tenant record or treat an
evaluation score as a deployment, isolation, or client-usage claim.
