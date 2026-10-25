# Player-visible state

`Directory::player_view(actor)` returns a read-only typed snapshot of the current world, actor, account and character, with simulation tick/time. Missing actors return an error, not an invented living character. It is an internal data boundary, not an in-process isolation sandbox or production HTTP endpoint.

Own state includes HP/MP, level/experience, equipped and carried items, inventory revision, base stats and effective stats in `content::STATS` order. Lifecycle reports pending operations, death, departure, quarantine and whether the ordinary output sink is live, lost or detached. Other players expose public level and equipped slots, never carried inventory. Containers must be published and accessible through normal ownership/visibility rules.

Map dimensions/name are native MAPINFO facts. Tiles contain only previously delivered ground and its movement multiplier. Visible objects supply their public blocking property. The own quest includes only its already-published native target. There is no whole-map collision mask, undiscovered terrain, enemy schedule, goal, damage RNG or hit ledger. Projectile snapshots contain observed trajectory/spec facts within sight, conservatively excluding shots at or before the shooter's latest discovery tick. Missing or expired observations are not proof that a route is clear.

## Results and stream lifetime

Use the existing ordered, bounded recipient `Output` stream for actual TEXT, effects, acknowledgments, transaction results and lifecycle publication. Queue failure always marks `Output.lost`, even without a watch subscriber. A lost sink is not a complete conversation or transaction record. Snapshots do not reconstruct missed messages or fulfilled promises.

Bind the output receiver to its original world/actor/character session. Transfer, reconnect, restart and replacement require a fresh receiver context. Drain or discard old-session events without attributing them to the new actor. Actor numbers are not durable transport identities across server restarts.

Gameplay application, persistence and delivery are different outcomes. GOTOACK is a client acknowledgment, not proof of an unrelated action. TRADEDONE success follows durable exchange. A revision and current holdings confirm state, but a revision change alone does not prove a particular payment. Prepared inventory plans, transfer grants and death work are not completed outcomes; consume their existing published completion or explicit failure.

`enter(tx=None)` remains legacy script-owned QA and discards ordinary outputs. It is not autonomous-bot enrollment. A separately selected controller must obtain its own recipient sink and explicit input ownership without loosening the protected human-input guard. The development observation still exposes privileged outer fields and must not become a bot perception interface.
