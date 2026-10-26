# Phase 2: Loose crowds find and lose each other

## Objective
Independent accounts discover and share leveling activity without fixed-pair choreography or a human leader.

## Implementation
- Depends on phase 1. In `src/world/view.rs`/`src/world.rs`, expose separately scoped native `World::updates` player markers, not cross-world positions, remote HP/inventory, undiscovered terrain or assumed lock tracking.
- Adapt `src/bots.rs`/`src/bots/adventures.rs`: attention/knowledge affect crowd discovery; opportunities attract more than two independent accounts.
- Permit agreed companionship without permanent leaders: join, clear, lag, leave and legitimately rejoin independently.

## Verification
- Common contract: red–green `community_crowd_` tests compare markers with native publication; cover world/session changes, unnoticed cues, unknown/ineligible teleport and private-field exclusion.
- Prepare initial activity only; run ≥3 controllers. One recovers/changes project; others continue/clear rather than freeze. Change knowledge/attention and travel availability; inspect full sequences.
- Separately observe native loose spacing and differing choices; visitor presence/exit cannot anchor the crowd. Prepared capability does not prove ordinary occurrence.
