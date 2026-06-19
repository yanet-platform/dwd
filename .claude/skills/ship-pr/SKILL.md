---
name: ship-pr
description: >
  Ship the current work as a pull request to master end-to-end: branch if needed, commit with
  the project's conventional message format (no AI attribution), run the local CI gate, push,
  open the PR, wait for CI to go green, then squash-merge and clean up. Use when asked to
  "make a PR", "ship this", "open a PR and merge", or otherwise finish a change for review.
allowed-tools: Bash, Read, Edit, Grep, Glob
---

# ship-pr

Drive a change through the full DWD pull-request lifecycle. This repo has specific, non-default
mechanics — follow them exactly:

- **Merge is squash-only.** `merge`/`rebase` are disabled on `yanet-platform/dwd`; only
  squash-merge is allowed. The squash commit's subject is the **PR title**, so the PR title
  *is* the commit that lands on `master` — it must be a clean conventional message.
- **Two workflows gate every PR to `master`**: **CI** (`Format`, `Clippy`, `Check`,
  `MSRV (1.96)`, `Test`, `Build (no DPDK)`) and **DPDK Build** (Docker, DPDK 24.11 + `.deb`).
- **`gh` is installed and authenticated.** Use it for PR create / checks / merge.
- **Never bypass the gate to "just merge."** Never add AI attribution anywhere (a PreToolUse
  hook also enforces this).

## Commit / PR message format

`<type>: <brief>` on the subject line, a blank line, then a `<description>` of what & why.
`<type>` is a conventional type: `feat`, `fix`, `perf`, `docs`, `refactor`, `test`, `chore`,
`ci`, `build`. Imperative mood. **No** `Co-Authored-By`, no "Generated with Claude Code", no
Anthropic attribution — anywhere, in commits or PR bodies.

## Procedure

1. **Branch hygiene.** `git branch --show-current`. If it is `master`, create a feature branch
   first (`git switch -c <type>/<topic>`) — never commit directly to the default branch. If
   already on a feature branch, stay on it. Confirm there is something to ship
   (`git status --porcelain` and/or commits ahead of `origin/master`).

2. **Commit.** Stage and commit any uncommitted work using the format above. Prefer a
   descriptive body explaining the why, not just the what.

3. **Local gate** — mirror CI's native jobs, in order, stopping at the first failure:
   ```bash
   cargo fmt --all -- --check
   cargo clippy -p dwd --all-targets -- -D warnings
   cargo check -p dwd --all-targets
   cargo test -p dwd --all-targets
   cargo build -p dwd --release
   ```
   If `fmt`/`clippy` fail, fix them (`cargo fmt --all`, address warnings), then amend or add a
   follow-up commit and re-run. Do not push a red gate.

   **DPDK note:** if the diff touches DPDK code (`#[cfg(feature = "dpdk")]`,
   `dwd-core/src/worker/dpdk.rs`, the `dpdk/` crate, or the `dpdk` feature), the local gate does
   *not* cover it — the **DPDK Build** CI job will. Don't run Docker locally unless asked; just
   flag that DPDK Build is the check to watch, and that runtime DPDK / MLX5 behaviour stays
   hardware-unverifiable.

4. **Push.** `git push -u origin <branch>`.

5. **Open the PR** against `master`:
   ```bash
   gh pr create --base master --title "<type>: <brief>" --body "<description>"
   ```
   The `--title` becomes the squash commit on `master` — keep it conventional and attribution-free.
   The `--body` follows the same shape (what / why).

6. **Wait for CI.** `gh pr checks --watch` until all checks settle (covers CI + DPDK Build).
   - On failure: identify the failing job (`gh pr checks`) and pull logs
     (`gh run view --log-failed`). Summarize the cause, offer to fix it, and **do not merge.**

7. **Merge** — only when every check is green. Merging is irreversible and outward-facing, so
   **confirm with the user first** (unless they already said to merge automatically). Then:
   ```bash
   gh pr merge --squash --delete-branch
   ```
   Squash is the only allowed strategy; `--delete-branch` because the repo does not auto-delete.
   Keep the squash subject conventional (`gh` appends `(#NN)`).

8. **Post-merge.** `git switch master && git pull`. Offer to delete the local feature branch
   (`git branch -d <branch>`).

## Notes

- If `gh pr checks --watch` is unavailable or hangs, poll with `gh pr checks` periodically.
- If there is no diff vs `master`, stop and say so — nothing to ship.
- This skill does not run the DPDK or perf gates locally; those are CI's job (DPDK) or separate
  perf tooling (not yet present).
