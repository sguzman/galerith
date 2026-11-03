# Galerith — local agent stewardship

This file governs autonomous Codex iterations. Read the smallest relevant parts of the repository before editing. The project objective and acceptance criteria are established; do not redefine them to manufacture completion.

## Project contract

- Complete the native Rust port of **gallery-dl**. The **first MVP means complete parity**, not a subset of working extractors. Read `docs/ACCEPTANCE.md`, `docs/SOURCE_BASELINE.md`, `docs/PROVENANCE.md`, and the relevant part of `docs/ROADMAP.md` / `docs/progress.json`.
- `docs/progress.json` is authoritative for milestones, phases, emoji, tasks, checks, evidence, and percentages. `docs/port-matrix.json` is authoritative for pinned upstream module coverage. The corresponding Markdown files are generated; do not edit those generated outputs by hand.
- Keep the pinned upstream baseline fixed until an explicitly approved migration. Use independently written Rust implementations and reference behavior. **Never translate/copy GPL-2.0 upstream implementations into the MIT repository without resolving the license and attribution requirements in `docs/PROVENANCE.md`.** Protect secrets and private user data.
- Work on `main` by default; do not create a feature branch unless specifically requested. Respect unrelated local changes, never overwrite a dirty worktree, and never force-push.
- Prefer Rust for app code and project automation; do not add a Python or npm runtime/tooling dependency.
- **MBX is mandatory for every Rust build, test, lint, format, and run command.** Invoke Cargo through the installed MBX shim (for example, `mbx +1.99.1 test ...`). Never bypass MBX with a direct toolchain binary or by overriding `CARGO_TARGET_DIR`, `RUSTC`, `RUSTC_WRAPPER`, or `PATH`; never create an isolated temporary Cargo target. If an MBX build reports a managed target artifact is not writable, check that no build is active, inspect `mbx doctor --json` and `mbx explain --last`, and use only a documented MBX recovery command when its scope is understood (for example, `mbx clean` for this workspace). If MBX still cannot run the gate, report the exact error and stop build operations; do not fall back to direct Cargo or an alternate target.

## Autonomous iteration

When the user says **continue**, begin a **sustained autonomous work session targeting approximately 60 minutes of active engineering**, or the longest useful uninterrupted work permitted by the current Codex/session execution limits. The time target is a planning preference, not a promise that the runtime will allow an hour. **A single finished task is not a stopping condition.** Complete one meaningful, bounded check, verify and commit it, then select and implement the next feasible check **within the same response/session**. Repeat until the available execution budget is genuinely exhausted, the complete MVP is verified, the user interrupts, or no safe independent work remains. Do not ask the user to pick tasks when the ledger suffices. Do not sleep, idle, run busywork, or pad progress simply to fill a clock.

**Session duration:** A user request to continue authorizes at least 60 minutes of active project work when the runtime permits. Do not stop after one bounded check; after each verified slice, commit and push it, then select the next feasible incomplete check and continue. Stop sooner only if the user steers or cancels the work, the remaining tasks are all externally blocked, or the runtime imposes a hard limit. Provide concise progress updates during long sessions.

1. At the start of a session record the output of `git rev-parse HEAD` as the baseline SHA and retain that commit's `docs/progress.json`, and note any uncommitted changes. Do not attribute prior changes to this session.
2. Inspect only the relevant files, tests, upstream references and matrix rows. Avoid full repo dumps and huge logs. Favor work you can verify locally without live credentials, special hardware, or undocumented external dependencies.
3. Implement a meaningful vertical slice: behavior, regression tests, and documentation/fixture updates as applicable. A registered CLI option, placeholder adapter, or synthetic smoke test does **not** establish live-site/upstream parity.
4. Validate locally, fix problems where possible, and commit/push independently once verified and authorized by the existing user instructions. Preserve task IDs, check semantics, scopes and prior evidence; don't relabel or split checks just to increase the score.
5. Update accepted checks **only with evidence matching that check**. Local test results do not satisfy a check specifically requiring GitHub CI or live-site validation. Never invent CI runs, site test results, completion, or percentages. If blocked, explain the blocker and switch to a useful independent task when feasible.
6. Rebuild generated roadmap/matrix from the machine ledgers using `mbx +1.99.1 run --locked --bin galerith-roadmap`; check with `mbx +1.99.1 run --locked --bin galerith-roadmap -- --check`. Do not hand-edit generated Markdown.
7. **Keep going after a successful commit**: choose the next ledger check and implement another substantive slice. This is a loop, not a one-task-per-`continue` workflow. Prefer several independently verifiable changes over an oversized untested patch. Check remaining execution budget periodically, and reserve enough room to leave the worktree consistent and summarize accurately.
8. Only end early for an actual obstacle that prevents **all** other suitable work, a user stop, verified completion, or a real runtime/tool limit. A green test suite, a completed task, a ready next-task recommendation, or a nice-looking progress delta are **not** sufficient reasons to stop.

## Quality gates

Run before claiming a verified local change:

```sh
mbx +1.99.1 fmt --all -- --check
mbx +1.99.1 clippy --locked --all-targets -- -D warnings
mbx +1.99.1 test --locked --all-targets
mbx +1.99.1 run --locked --bin galerith-roadmap -- --check
mbx +1.99.1 build --locked --release
```

Report honestly if a gate cannot run or fails. Do not claim remote CI green until the actual corresponding commit's run is verified.

## Required end-of-session report

Report once at the end of the **multi-task work session**, not after each small accepted task. Intermediate commits are encouraged; a finished task should ordinarily lead to another task, not a final response. If the runtime forces an early stop, explain that honestly rather than claiming the hour was fulfilled.

Keep this short, with **only the milestone(s), phase(s), task(s), and extractor modules actually touched**. Retain the emoji labels already in the ledger. Do not repeat finished 100% items unless they changed during this session.

- **What changed:** short behavior-focused summary and the relevant task IDs.
- **Progress:** a visual 10-block bar and percentage for the **overall project** plus the touched milestone/phase/task entries; show the signed **percentage-point delta (pp)** from the session's recorded baseline. Show **+0.00 pp** for no accepted progress, even when code was written. Never present relative percent change as a percentage-point delta.
- Compute overall progress as the arithmetic mean of the stored integer task percentages in `docs/progress.json` (all tasks, equally weighted), before and after the session; use 2 decimal places for the detailed overall figure and delta. Milestone and phase percentages likewise average their tasks. Note that the generated roadmap rounds its displayed percentage to an integer. Do not infer precise values from its 10-block bar.
- **Evidence:** tests, actual CI run when confirmed, and changed matrix rows; distinguish synthetic verification from live-site parity.
- **Commit:** SHA and push status (or explicit reason not pushed).
- **Next:** one concrete next incomplete check or external blocker.

Example format (illustrative numbers, **not** an assertion of actual progress):

```
🖼️ Galerith overall    ███████░░░ 69.12%  (+0.21 pp)
🧬 M2 / P4 / Txxx      ████████░░ 80.00%  (+20.00 pp)
✅ Verified: local fmt / Clippy / tests / roadmap / release
📌 Commit: <SHA> (pushed to main)
➡️ Next: <one incomplete acceptance check>
```

Do not introduce new feature scope or inflate progress to make the report look better. The outcome is a correct, verifiable Rust port.
