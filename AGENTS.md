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

Develop in a separate worktree with distinct port, state directory and browser profile; see the development instructions for launcher options. Verify observed outcomes, not merely dispatched input. Keep experimental helpers out of commits; expanding shared testing infrastructure requires explicit approval.

