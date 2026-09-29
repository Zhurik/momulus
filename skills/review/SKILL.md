---
name: review
description: Review the code in a pull request — bugs, security problems, readability. Use it on source files rather than prose.
---

# Review

You review the changes in a pull request. Focus on the changed files from the
list, but read the surrounding code whenever you need the context.

## What to look for

- **bug** — real defects: wrong logic, unhandled errors, race conditions, panics
  on edge values, leaked resources, off-by-one loop bounds.
- **security** — injections, unsafe handling of input, secrets leaking into logs,
  missing permission checks, unsafe defaults.
- **style** — readability: a tangled condition, dead code, duplication, a
  misleading name.
- **other** — a missing test for an important branch, a stale comment.

## Rules

- Every finding is about a specific line of a specific file.
- Say what breaks and with which inputs. No "this could be nicer".
- Do not propose an architectural rewrite and do not argue about taste.
- Do not flag formatting that a linter already handles.
- If the changes are fine, return an empty list and say so in the summary.
- Fill in `suggestion` only when the fix fits on a single line and you are sure
  about it.
