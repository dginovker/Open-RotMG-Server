# Phase 3: Supplies, possessions and private time matter

## Objective
An outing develops into a real equipment/supply/storage cycle with consequential choices and legitimate quiet activity.

## Implementation
- Depends on phase 1. Extend `src/bots/progression.rs`, `src/bots/pursuits.rs` and `src/bots/runtime.rs`: supplies, unfamiliar-item assessment, withdrawal/re-equipping and full-storage choices via ordinary actions/revision-checked Store commits.
- Evaluate held/observed goods for current use, future characters and exchange. Mistaken beliefs cannot bypass unlocks; wealth is not skill.
- Bound inspection/organization/trying-equipment with reassessment and a next purpose; no extra capacity, arbitrary pauses or parallel inventory.

## Verification
- Common contract: red–green `community_resources_` cases cover upgrade, potion↔item reconsideration, familiarity under danger, cross-class retention, locked class, full backpack/vault, stale stock/revision and death. Audit sources/sinks; forbid replacement-gear farming.
- Controllers acquire/assess goods, store/withdraw/make room, then continue with changed holdings. Include an unsatisfied preference and sensible alternative; no post-setup fixture direction.
- Native inventory/Vault plus commits show private inspection and subsequent use. Prepared wealth cannot replace the final campaign's earned resource cycle.
