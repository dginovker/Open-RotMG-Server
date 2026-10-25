# The community we are building

Planning only at `5bdf25d`: no implementation, runtime, provider use, downloads, population expansion, publication or deployment. `docs/bots/research/owner-culture.md` supplies C01–C14 owner readings and uncertainties, not verified mechanics. Acceptance below is proposed, not achieved.

## What logging in should feel like

You arrive in a world where other people already seem to have things to do. Nobody needed you to start their outing, make them trade, give them equipment or tell them where to go. Joining can change a particular person's plans; leaving does not stop the world.

A new-looking character can confidently find a crowd, while another struggles despite comparable gear. People fight together without moving as one organism. Someone stops to read a drop; the group stretches or separates. A Priest may help, someone may ask for spare equipment, and others continue their own business. Quiet stretches, mistakes and failed attempts belong in the same world as competence and generosity.

In Nexus, not everybody immediately runs back into combat. Someone wants to exchange a held item, rejects an unsuitable offer, makes room in the Vault and returns to the meeting place. Another has kept an item for a future character; somebody else gives a spare away or declines a gift. A good deal can lead to a brief guild message and private equipment organization. These conversations have material consequences, not just plausible wording.

A death really removes a character and their possessions, but not necessarily the account's connection to someone who helped them. The replacement has a reason to rebuild or seek that acquaintance, through legitimate information and travel. At shared events, people advance cautiously, react differently to opportunities and danger, then inspect loot, ask, give, linger or leave at different times.

These are overlapping lives with breathing room, not a scene order, permanent roles, demographic model or script reenacting recordings.

## What would convince us—and what would not

| Experience | Visible evidence we need | Reject the imitation if… |
| --- | --- | --- |
| Different people — C02/C05/C06/C09 | Comparable resources reveal different knowledge, attention, execution and risk: discovery, item assessment and retreat differ. Sensible upgrades coexist with reconsidered swaps or overload/late escape; experienced rebuilding differs from novice play. | Same choices with different delays; randomly disabled actions called novice play; gear/level determines skill. |
| A loose outing — C02/C10 | At least three independent participants can share activity without exclusive-pair choreography. When one pauses or leaves, others can continue, clear remaining enemies, wait or pursue something else; reunion needs real information. | All followers stop, turn, teleport or change destination together; a human is required to gather them; a missed contact grants hidden coordinates. |
| A material life — C04/C05/C09 | Supply use, unfamiliar-item assessment, a meaningful upgrade/reconsideration, storage pressure and a subsequent choice are visible parts of an outing. A retained cross-class item has a real account interest; private inspection leads somewhere rather than freezing forever. | Gear is granted, unused items vanish conveniently, banking only dumps everything, or meaningless idle timers are presented as thought. |
| A marketplace — C07/C08 | Own wants lead to inspected/rejected offers or real exchanges, including a capacity-driven bank trip and rendezvous. Distinguish agreed terms, partial delivery and settlement: the owner's honored 8+8+1 reading is an example, not universal pricing. | Timed universal advertising; promises counted as payment; junk exchanges for metrics; invented stock/counterparty; silent installment default. |
| Selective help and acquaintance — C03/C04/C10 | Help/gifts are possible but not universal. A real interaction affects later willingness or choice of company. A replacement may seek a known account while retaining only legitimate account resources. Refusals and ignored requests can end normally. | Every request succeeds, friendship is proximity or a predetermined crew, a dead inventory returns, or every death produces the same scripted reunion. |
| Nexus and private downtime — C03/C08/C09 | People can wait near a genuinely full entrance, advertise or reconsider while waiting; inspect, organize or celebrate privately; then continue with a reason. The population is not permanently concentrated in one place. | Stillness is either always treated as failure or inserted as cosmetic noise; capacity is bypassed; a connected observer is counted as zero humans. |
| Shared event culture — C11–C14 | An ordinary world progression creates a shared opportunity. Individuals cautiously join or decline, use supported encounter knowledge, and split into different post-fight activities. Loot speech and redistribution follow real outcomes. | The test teleports actors into a staged finale and calls it autonomous progression; everyone rushes/loots/leaves identically; a decorative pool or fake reward stands in for shared mechanics. |

The existing six-account cohort cannot prove crowded-Realm density; expansion and production changes remain unselected.

## How these situations arise

Account interests—loadout/supplies, future characters, activity/company, wanted goods, acquaintances—select activities from legitimate knowledge and means. Results satisfy, interrupt or invalidate them; transfer does not reset personality.

```text
own PlayerView + received native Output
  -> account mind: interests, knowledge, bounded factual experience
  -> activity: objective, next step, resume/abandon/completion conditions
  -> existing travel / combat / recovery / inventory skills
  -> ordinary guarded game actions -> committed outcomes -> mind update
```

Retain game-control skills; replace upper flat arbitration where needed. Persist account interests, not character/session actions, receivers or volatile deadlines. Death removes character holdings; restart cannot revive packets. Memories are observations, not inventory or an omniscient registry.

Bounded attention/familiarity explains mistakes with ordinary consequences, not illegal actions or random incompetence. Language expresses supported intentions, never creates facts/assets. Preserve default-off NVIDIA/verified-zero-cost approval, budgets and observable failure; no canned/paid success fallback.

## Common acceptance contract

Every phase uses this contract; contextual controller/native checks happen locally, not only in phase 8. Before future gameplay work, use `docs/reference-sources.md`; copy all four `examples/testing/README.md` recipes into scratch. For each phase's `community_*` filter, first run `cargo test --locked -j2 FILTER -- --list` and require nonzero matching tests; then `cargo test --locked -j2 FILTER -- --test-threads=2`, retaining red-before-green evidence.

Use only protected controls (`src/server/testing.rs`, `scripts/game.mjs`) and the existing native bridge, never Playwright/new general QA infrastructure. Inspect capabilities/catalog; preserve one controller per actor. Use bounded `time` goals (≤600000 ms), `observe`/`inspect`/`trace` and committed outcomes; no invented market/Cellar/Lab goals. Successful bounded `run` means elapsed time, not culture success.

### 1. Does it behave correctly in context?

Prepare **starting context only**, then run actual controllers through ordinary World/Store authority. Hidden evaluation state must not reach bots. Declare visible preconditions before running:

- **Crowd:** published cues, independent participants, one legitimate interruption. Compare continuing, clearing, waiting and losing contact without test-driving followers.
- **Trade:** held goods, independent wants, normal communication/reach/capacity. Require useful exchange, unsuitable offer, departure and interrupted installments. Durable receipts prove settlement, not accept clicks/revision alone; refusal/default needs explicit remaining-balance resolution.
- **Gift/help:** available spare or equipped ready heal, observed/communicated need, reachable recipient. Include acceptance, refusal and competing own need; verify inventory or HP/MP effects, not animation.
- **Reunion:** prior observed interaction and committed death. Replacement uses current observations, published contact or remembered meetings, not hidden coordinates. Include unavailable acquaintance and independent continuation.
- **Waiting/hazards:** real entrance capacity or registered encounter/status/remedy; normally published changes must alter behavior, not merely return API success.

Change resources, knowledge, audience, partner availability or risk; appropriate behavior must change. Vary names/locations/items to defeat clip scripts. Prepared capability is **not spontaneous occurrence**.

### 2. Does ordinary play produce an interleaved community?

Use six fresh ordinary accounts: **zero connected Human controllers throughout**, no injected assets/friends/outcomes, assigned stories or human plans. Initial budget: three sixty-minute game-clock runs, seeds 7/42/101; retain timestamped health/controller/observation traces and all attempts. Declare extensions first; seeds cannot fix asynchronous ordering.

Require linked discovery/combat/resource outings with interruptions, separation and subsequent choices; independently initiated material social/economic interactions; meaningful quiet periods. Trace want/opportunity → action → outcome, not isolated screenshots. Repeatedly ignoring available market/help preconditions fails behavior, not scheduling.

Full entrance, death/reunion and specific pools still need contextual tests if absent here; report natural absence as unobserved. Earned event progression is mandatory for self-sustaining event culture; prepared finales cannot substitute. Repetitive combat/template invitations fail the community milestone despite passing API tests.

### 3. Does it look like the owner's community?

Separately join through a normal client, observe/interact/refuse and leave. Even a motionless native observer is Human. Review whole sequences, failures and quiet time against owner windows and the table: distinguishable continuities, consequential interactions and life continuing after departure, not highlights.

Native evidence must show rendered loose crowds/chat, trade UI/private messages/settled holdings/Vault, selective help/refusal, replacement/reunion and cautious event advance/aftermath. Pair frames with ordered transitions/receipts; stills prove neither timing, motives nor unattended operation. Use a private display and existing bridge, register every native participant for coordinated Ruffle/page/server pause; lease expiry is terminal. Never activate host windows. Live review or ordered native evidence suffices; video is optional. An unrelated visitor cannot see another account's private Vault: verify that bot activity through its own session outputs, decisions and committed state. Do not take over a bot or present a human's Vault screenshot as its behavior. User authenticity review remains a separate gate, not a likeness score.

## Getting there

Dependencies: phase 1 account continuity/cognition → 2 crowds and 3 resources → 4 markets → 5 social/Nexus life; 6 shared Lab mechanics → 7 events using earlier behavior; 8 integrates all. No phase alone establishes a community. Phase 5 proposes one protected, pre-enrollment capacity setup for its waiting scenario; this expands shared testing controls and requires separate explicit approval before implementation. No general QA framework is proposed.

World/Store authority, combat, storage, native trade/chat and parts of Castle/Oryx/Cellar exist; autonomous adapters/policy/continuity remain incomplete. Native minimap markers exceed current bot perception (phase 2). Lab portal `0x890`/boss `0x976` definitions are not a registered playable dungeon (phase 6). Resolve pool/Cellar/loot and contact-publication ambiguities through selected definitions and pinned references.

Prior guarded-gift/focused-regression spikes and static Lab findings are supporting evidence, not native/community acceptance; do not sum overlapping counts or rerun exploration/stopped video/model studies. Retain account/session/activity/trigger/result/end-reason traces. No invented economy or omniscient coordinates.
