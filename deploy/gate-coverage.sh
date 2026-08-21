#!/usr/bin/env bash
# Refuse content that no gate has seen.
#
# WHY THIS EXISTS. Every other check here derives its file set from the index, from a
# checkout, or from history, so they share one blind spot: untracked-but-not-ignored content
# passes all of them by being INVISIBLE rather than by being clean. A whole subtree can arrive
# as one line of `git status` and be linted by nothing.
#
# THE RULE. Every path is either TRACKED (and therefore gated) or IGNORED (and therefore a
# recorded decision not to ship it). Anything else is a third state nobody chose.
#
# The escape hatch is .gitignore, which is the right place for it: "we deliberately do not
# ship this" belongs in a reviewed file, not in whoever-remembered's head.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# One worktree walk, reused below — the count and the grouping both read from this file.
others="$(mktemp)"; trap 'rm -f "$others"' EXIT
git ls-files --others --exclude-standard -z > "$others"
n="$(tr '\0' '\n' < "$others" | grep -c . || true)"

if [ "$n" -eq 0 ]; then
  echo "gate-coverage: OK — every path is tracked or ignored"
  exit 0
fi

echo "gate-coverage: $n path(s) are neither tracked nor ignored — no gate has inspected them:" >&2
echo >&2
# Group by the first two path components so a large new subtree reads as one actionable
# line ("deploy/<subtree>/  18 files") rather than 900 lines or a useless "deploy/".
tr '\0' '\n' < "$others" \
  | awk -F/ '{ k = (NF > 2 ? $1"/"$2"/" : (NF > 1 ? $1"/"$2 : $1)); c[k]++ }
             END { for (x in c) printf "    %-44s %d file(s)\n", x, c[x] }' \
  | sort >&2
echo >&2
cat >&2 <<'MSG'
  Each one needs a decision, not a default:
    git add <path>              ship it — every gate then covers it
    echo <path> >> .gitignore   do not ship it — the reason belongs in a comment there

  Build output (target/, *.wasm) is already ignored; if something here looks like build
  output, the ignore rule is missing rather than the file being wrong.
MSG
exit 1
