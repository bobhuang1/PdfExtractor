# Project rules

## GitHub Actions minutes

Actions minutes are a scarce, shared budget. Treat remote CI as opt-in, not as a
scratchpad:

- Run the relevant checks locally first. Use the narrowest command that covers
  the change, and do not push merely to find out whether CI passes.
- Do not use GitHub Actions for iteration, debugging, flaky-test diagnosis, or
  broad OS/version matrices unless the user explicitly asks for it.
- Do not trigger workflows (`gh workflow run`), re-run jobs, or dispatch a matrix
  without explicit approval.
- Do not add or extend a per-push workflow when a local check, a manual
  `workflow_dispatch`, a schedule, or a tag-triggered release would do.
- When a workflow must change, keep it lean: path filters, a `concurrency` group
  with `cancel-in-progress`, job timeouts, dependency caches, and targeted test
  commands. Avoid running the same full suite in several jobs.
- Releases stay tag-triggered or manual. Never use a release workflow as a test
  harness.

If remote CI is genuinely required, say why and get approval before spending
minutes.

## Verification

Prefer a focused local test or typecheck while developing. Run the full workspace
suite only when the change or the user calls for it, and report the checks that
actually ran, including any skipped or failed ones.
