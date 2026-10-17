## Local autonomous crews

Run `./run.sh 18923 --release`, then enter the Realm normally. Mossy leads Bayleaf/Trellis; Reed leads Juniper/Bramble. Profiles weigh recovery, resources and companions differently. They use restricted observations and ordinary movement, combat, inventory, XP, teleport and transfer rules. Uninvited humans are not permanent anchors. Native local chat `Trellis follow me` requests a temporary follow while the sender is visible.

Wizards predict visible projectile paths, lead an observed target, kite within weapon range, cast equipped spells and commit to recovery. Crews remember recent contacts, explore reachable known terrain, revisit observed Realm hunting grounds and respond to Pirate Cave calls. Visible crewmates regroup through normal teleports; separated crews can arrange a Nexus meeting through public chat. Priests cast equipped tomes for actual nearby injury, MP and accepted cooldowns.

This candidate inherits main's default-on startup; `--no-shore-bots` or explicit `"shore_bots": false` disables it. Reserve seven slots for six bots and a human; QA control remains separate. Living saves resume unchanged. After committed permanent death, a fresh character receives normal starter gear; Bayleaf/Juniper choose Priest only if that same account earned its Wizard-level-five unlock. Dead inventory is never inherited. Trade negotiation and language services remain outside this slice.

Ordinary crews earned Snake Pit clears and returns. Prepared full-backpack clears verify reward exchange and consumption; native media is prepared.

## Approved north star

OpenRotMG should feel like stepping into a busy Realm in 2012, even when you’re the only human online. Its bots are a population, not one player copied a hundred times: beginners in scrappy gear, reckless loot-chasers, quiet grinders and veterans who make dodging look easy. Their names, equipment, confidence and mistakes should fit their experience and the era. They level, trade, die, rebuild and log off. You encounter little groups on the beach, a passing godlands train, familiar names in Nexus. The world has its own momentum, with bustle and breathing room rather than bodies filling every tile.

These inhabitants should form a community you can join, not an entourage built around you. Friends stick together, strangers answer dungeon calls, leaders gather crowds, and groups scatter when things go wrong. Chat is short, messy and rooted in what’s happening: asking for gear, celebrating a drop or raging over a death. Some help you, some ignore you, some get everyone into trouble chasing loot. Conversations should change what they do, not merely decorate it. The north star is logging in without a plan and leaving with a story about who you met, what happened together and why you want to come back.

## Proposed technical direction

Run bots inside the Rust server as normal player characters with a different input controller. Reuse gameplay validation and durable inventory/death handling. Do not route production bots through the protected development HTTP controls or enable those controls in production.

```text
Flash input -> packet decoding --------------------+
                                                  v
World -> restricted player view -> bot decisions -> gameplay rules -> World
                                       ^                           |  |
                                 profile + memory                  |  +-> SQLite
                                                                   +-> Flash updates
```

Start with one small controller module, not a generic agent framework. Separate choosing a goal from executing it: score fighting, exploring, looting, recovering and following; keep a goal long enough to avoid oscillation; execute movement/aim/item use with small explicit states. Physics retains its existing 50 ms timestep. Decisions use bounded, staggered schedules and profile-dependent reaction delays.

Profiles represent experience, risk tolerance and sociability, plus coherent resources/history. Skill is not character level: a veteran can be rebuilding a level-one character. Mistakes should follow limitations and motives, not arbitrary random failures. Give controller randomness its own seeded stream.

The controller receives only observable entities/projectiles, discovered terrain, its own character, relevant chat and remembered observations. No enemy attack schedules, hidden loot or other private state. It returns intents rather than directly editing the world.

Bot social ties and conversational commitments must track real actors and actions, not invent a new formal party mechanic. A bot accepting an invitation must actually behave differently. Consider an optional language model for interpreting/free-form wording, never the combat loop or direct game-state mutations. Any language service needs explicit cost/latency limits, bounded context and schema validation; outages must be observable, not disguised with canned fallback replies.

Reserve capacity for humans. Keep human/bot counts distinguishable. Existing script-owned development/test actors are not an agreed population-bot identity; settle explicit controller classification when implementation starts, and never derive game-bot counts from enemy NPCs. Bots follow the real loot/XP rules and must not create an unlimited supply of saleable replacement gear.

## Alternatives to compare

| Approach | Tradeoff | Autonomous test |
| --- | --- | --- |
| Small scripted state machines | Quick baseline, but repeated routines may remain obvious | Fixed combat/loot/group scenarios, then repeated-run movement and timing comparisons |
| Goal scoring plus small execution states | Recommended balance of low code, explainability and varied motives | Same observation with different profiles; goal stability, seeded replay and multi-bot interaction scenarios |
| Learned behavior from recordings | Potentially useful later, but needs trustworthy action labels and harder diagnosis | Held-out recording comparisons plus the same live-game correctness scenarios; synthetic labels alone cannot establish historical authenticity |

## Research

Two research passes are complete. See `research/sample-1.md` for the initial crowd/combat/chat observations, and `research/sample-2.md` for ordinary play, recruitment, death/rejoining, completed trade sequences and counterexamples. Each report separates observations, unknowns, implications and candidate acceptance scenes. Original recording URLs and reviewed timestamps are included in the reports. Downloaded footage, extracted frames, metadata and sampling records remain private and outside this repository.

Continue with a small reviewed set of 2012 clips covering beginners, godlands trains, dungeon entrances, Nexus trading/chat and death reactions. Observe other players, not only the recorder. Record short timestamped examples: what was visible, what happened and what remains uncertain. Nearby-era footage can inform atmosphere, not certify exact 123.5.1 rules.

Extract reaction/retreat patterns, spacing, group formation/breakup, chat timing and coherent equipment/experience differences. Use recovered client definitions for item identities and permissible equipment. Do not make a large computer-vision gear-identification project a prerequisite. Expand the evidence where behavior remains unconvincing, and reserve some footage for comparison rather than tuning.

### Second pass: completed scope

The user approved the following research scope. Six recordings were inspected; findings and limits are in `research/sample-2.md`:

1. **Ordinary play and quiet time:** follow a player continuously through travel, waiting, inventory checks, recovery and solo play. What breaks up combat, and when do they seek or avoid others?
2. **Group formation and separation:** trace visible approaches, following, calls, regrouping and departures. Distinguish strangers sharing an opportunity from demonstrated conversational commitments; proximity alone is not friendship.
3. **Completed exchanges:** find native trade sequences with requests, offers, acceptance/rejection and an observable outcome. Include unanswered requests. Separate actual equipment needs from inferred motives; retain unknown gear as unknown.
4. **Death and rebuilding:** find footage spanning loss and the next character/session, including retained account resources where visible. Look for changed behavior rather than assuming a grief script or treating level one as a novice.

For each useful sequence, record source/date/build evidence, a short timestamped observation, uncertainty, a proposed behavior implication and a candidate in-game acceptance scene. A request and a nearby heal do not alone establish cause and effect. Review some longer continuous sequences so timing, pauses and transitions are not lost to sparse frames or editing. Reserve comparable examples for later evaluation rather than using every example to tune the policy.

The completed result is one second-pass report with the strongest examples and counterexamples. Next, use the reports' candidate scenes as a small behavior brief and track the shared-game dependencies before selecting implementation work. Further footage should answer a specific remaining question, such as relationships across sessions or re-equipping from storage after death. Reuse downloaded sources and link the content/mechanics owner's catalogue rather than duplicating it. Do not introduce a dataset framework, gear classifier, personality generator or training pipeline yet.

Before a bot implementation plan, recheck normal shared-game support for the behaviors selected: observable player/projectile views, relevant classes/abilities, chat, progression, transfers, inventory/economy and durable death/rebuilding. Mark each dependency verified, in progress or missing against the then-current revision. The first bot slice should exercise existing rules, not implement private substitutes for unfinished game systems.

## Completed offline spike: decisions and continuity

The user approved the proposed comparison outside the server. See `research/decision-spike.md` for its method, results, regression, tests and limits. The fixed baseline already produced profile variation; scoring added sensitivity to circumstances but needed a small switching margin. Shared memory and resource checks, not scoring, preserved continuity. Experiment code and synthetic traces remain private.

The next recommendation is a bounded partial-knowledge/time experiment, not another general footage collection or server integration. Test stale observations, absent companions, missing receipts and when commitments should be deferred or abandoned. This follow-up is not selected or authorized. The current experiment does not establish historical authenticity, movement/dodging, live social feel or perception/chat understanding.

## Verification

- **Rules:** speed/cadence/collision limits, fog/private-state isolation, exclusive ownership, inventory races, irreversible death, save/restart, and zero controller access to a human’s inputs.
- **Behavior:** danger versus loot, full inventory, lost leader, declined invitation, silent player, blocked route, death during an interaction, and realistic novice/veteran differences. Expect coherent mistakes, not universal perfect survival.
- **Population:** controlled game-clock runs across multiple seeds; check stuck actors, synchronized routines, repetitive chat, vanished groups, item creation and population drift. Control event ordering and persistence completions; a seed alone is insufficient for full-system replay.
- **Performance:** mixed human/bot load against the actual hosting limits; measure tail tick duration, queue pressure, saves and any language costs. No unverified 85-player promise.
- **Presentation:** normal Ruffle gameplay at real speed. Before/after shots show the same area empty versus inhabited; a 15-second canvas-only demo shows distinct movement, combat and a meaningful interaction. A longer uncut session tests repetition. No overlays substituting for native avatars/chat.
- **Feel:** compare unseen scenarios with reserved historical examples and get user review. Passing mechanical tests does not prove believable people.

Log meaningful decision changes with actor, observation/trigger, chosen goal/action and rejection reason. Retain enough clock/input/randomness information to reproduce a failure without generating a giant per-tick diary.

## First proof and dependencies

Aim for one human and three visibly different Wizard bots: cautious, confident and loot-greedy. They fight through normal rules, react to danger, collect available gear and exhibit different choices. Add one genuine group invitation/response if native chat is included in the first milestone. This is a prototype population, not proof of the complete north star.

At the initial inspection of server revision `c25cd6a`, the backend had a shared Wizard tutorial fixture, normal combat/inventory/death persistence and script-owned actors. At that revision, chat, XP progression, Nexus/world transfers, other classes and trading still needed game support; bot-only shortcuts must not pretend those systems exist. Recheck the live backend before planning implementation, since other owners are developing shared systems. Stage the full vision alongside those systems.

Two bounded local checks passed: the ten existing world boundary tests, and a copied-source spike that replayed movement/shooting/observation twice over forty fixed ticks without real-time sleeps. The spike also confirmed that the development observation exposes enemy scheduling, so it is not a suitable brain input unchanged. These checks do not establish bot policy, social behavior, persistence replay or production performance.

## Deferred implementation choices

1. Should the first proof emphasize believable combat, or include a small meaningful chat/group interaction immediately? Recommendation: combat plus one interaction, not sophisticated banter.
2. Should the initial population start with mixed progression/gear or earn everything from starter characters? Recommendation: starter characters for the prototype; decide any one-time population seeding before trading is enabled.

The selected cooperative extension does not approve the remaining population phases. Revisit shared gameplay dependencies before expanding it.
