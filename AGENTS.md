# The Most Important Rules for Writing Code

Your work should be _good_. That means, in this order:
1/ Minimal LoC to do the job
2/ Testabiltiy
3/ Strong observability
4/ Proper architecture that maximizes reliability and minimizes fallbacks

# Tips on Reducing LoC

When doing any work, consider - "Can I reduce LoC in mr PR?". Here are some ways to do that:
1/ Investigate the relevant code, and consider: "If I were reimplementing this architecture knowing what I know now, what can I delete and rewrite to reduce the LoC?"
2/ Before sending the PR, consider - "What tests did I only need to get here, and what can I delete?", or "What documentation did I see that was irrelevant, and can delete?"
3/ Do not add any documentation unless explcitly asked. When writing documentation, have a subagent delete 75% of it after, and simply confirm if that captures mostly what you wanted to get across.

# Gameplay references

Before researching or implementing gameplay, read `docs/reference-sources.md`. Start from its pinned community implementations; use period documentation and footage to resolve gaps or conflicts, not to reconstruct already-documented behavior from scratch. Check target-version fit and engine semantics per feature; copied forks are not independent evidence, and reference quality is separate from licensing permission. Keep selected client definitions and project safety/durability invariants authoritative. Record unresolved differences instead of claiming 1:1 fidelity.

# Contributions

Restoration gameplay PR comments identify affected IDs (https://openrotmg.org/restoration/) and revision-bound evidence; update website records. Keep implementation and historical evidence separate. Source inspection or merging is not acceptance.

For gameplay or visual changes, capture screenshots before editing and after verification through normal client gameplay. PRs do not require video. Nonvisual changes need test evidence.

Reuse one isolated task worktree with distinct ports, state and browser profiles. Use `python3 scripts/qa.py` for builds/tests (`cargo …`), native cases (`native CASE.mjs`) and other checks (`exec -- COMMAND …`). Put `--background` before the mode for long jobs; the printed job directory supports `status`/`cancel`. See `--help` and `examples/testing/README.md`.

The runner allocates scratch and automatically cleans it after owned processes exit. Keep source/cases in the worktree; write evidence, logs, DB/state and required inputs directly to `QA_RESULTS_DIR`. Only regenerable files belong in `QA_SCRATCH_DIR`/`TMPDIR`. Do not create per-attempt source/client/cache copies, override managed Cargo storage, launch ad-hoc detached jobs or bypass runner errors. Preserve existing frozen reproductions/tool pins without migration.

Keep feature cases small and assert observed outcomes, not merely dispatched input. Keep the regression suite lean; expanding shared testing infrastructure requires explicit approval.

Native browser QA must use a private virtual display/compositor; see `examples/testing/README.md`. CDP input and page-local focus must not activate host windows or switch the user's desktops. Never use host `Page.bringToFront`, target activation or desktop input as a rendering repair.

Keep recordings and generated media outside Git. Never capture credentials, other people's tabs or the desktop. Do not change hosting, branch protections or publication permissions as part of making a demo.

