use super::*;
use crate::content::Object;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct TeleportReceipt {
    pub player: i32,
    pub character: i32,
    pub world: i32,
    pub target: i32,
    pub position: Pos,
    pub tick: u64,
}

pub(super) fn ready(p: &Player) -> bool {
    p.active()
        && p.character.hp > 0
        && !p.pending
        && p.tx
            .as_ref()
            .is_none_or(|tx| !tx.lost.borrow().is_some() && !tx.packets.is_closed())
}

pub(super) fn effect(kind: u8, target: i32, position: Pos) -> Vec<u8> {
    let mut out = Writer::new();
    out.u8(kind)
        .i32(target)
        .f32(position.x)
        .f32(position.y)
        .f32(0.)
        .f32(0.)
        .u32(0xffffffff);
    out.0
}

impl Directory {
    pub fn teleport(&mut self, id: i32, target: i32, received: Instant) -> Result<TeleportReceipt> {
        let scope = self.location(id)?;
        let world = self.resolve(Target::Player(id))?;
        let position = world.teleport_destination(id, target)?;
        let account = world.players[&id].character.account;
        if let Some(last) = self.teleports.get(&account) {
            ensure!(
                received
                    .checked_duration_since(*last)
                    .context("teleport receive clock regressed")?
                    >= Duration::from_secs(10),
                controller("Too soon to teleport again; wait ten seconds.")
            );
        }
        self.teleports
            .retain(|_, last| received.saturating_duration_since(*last) < Duration::from_secs(10));
        ensure!(
            self.teleports.len() < 4096,
            controller("Teleport rate registry is full.")
        );
        let world = self.resolve(Target::Player(id))?;
        let movement_received = received
            .checked_duration_since(world.attack_epoch)
            .context("teleport predates world epoch")?
            .as_millis() as u64;
        let p = world.players.get_mut(&id).unwrap();
        p.pos = position;
        p.movement_credit = 0.78;
        // Server-driven motors budget against simulation time, not native packet arrival time.
        p.last_move_at = if p.controller == Controller::Human {
            movement_received
        } else {
            world.now
        };
        let receipt = TeleportReceipt {
            player: id,
            character: p.character.id,
            world: scope,
            target,
            position,
            tick: world.tick,
        };
        world.updates(id, false)?;
        let mut out = Writer::new();
        out.i32(id).f32(position.x).f32(position.y);
        for (receiver, p) in world
            .players
            .iter_mut()
            .filter(|(_, p)| ready(p) && p.known.contains(&id))
        {
            reconciliation::goto(p, *receiver == id, out.0.clone());
            send(p, wire::SHOW_EFFECT, effect(2, id, position));
        }
        tracing::info!(player=id, account, target, x=position.x, y=position.y, world=%world.name, "player teleported");
        world
            .players
            .get_mut(&id)
            .unwrap()
            .character
            .lifetime
            .teleported = true;
        self.teleports.insert(account, received);
        Ok(receipt)
    }
}

impl World {
    pub(super) fn allow_teleport(&self) -> bool {
        // Abyss dungeon anchoring relies on player teleports: https://anticosmic.blogspot.com/2012/04/fame-farming.html#tunnel_rat
        matches!(
            self.name.as_str(),
            // Build 123.0.0 exposes Nexus Teleport: https://www.youtube.com/watch?v=Ex_Ix3dlB4U&t=299
            "Nexus"
                | "Realm"
                | "Pirate Cave"
                | "Snake Pit"
                | "Sprite World"
                | "Undead Lair"
                | "Abyss of Demons"
        )
    }

    fn teleport_destination(&self, id: i32, target: i32) -> Result<Pos> {
        ensure!(
            self.allow_teleport(),
            controller("Cannot teleport in this world.")
        );
        let p = self
            .players
            .get(&id)
            .context("Teleport sender is no longer in this world.")?;
        ensure!(
            p.persistence_failure.is_none(),
            "Busy, dead, departing or quarantined players cannot teleport."
        );
        ensure!(
            ready(p),
            controller("Busy, dead, departing or quarantined players cannot teleport.")
        );
        ensure!(
            !p.correcting()?,
            controller("Position correction awaits GOTO acknowledgement.")
        );
        ensure!(!p.paused, controller("Cannot teleport while paused."));
        ensure!(id != target, controller("You are already at yourself."));
        let q = self
            .players
            .get(&target)
            .ok_or_else(|| controller("Teleport target is not a live player in this world."))?;
        ensure!(
            ready(q),
            controller("Teleport target is busy, dead, departing or quarantined.")
        );
        ensure!(!q.paused, controller("Cannot teleport to a paused player."));
        ensure!(
            q.conditions(self.now) & crate::condition::bit(crate::condition::INVISIBLE) == 0,
            controller("Cannot teleport to an invisible player.")
        );
        ensure!(
            p.known.contains(&target),
            controller("Teleport target has not been observed.")
        );
        ensure!(
            q.pos.x.is_finite() && q.pos.y.is_finite() && self.map.cell(q.pos.x, q.pos.y).is_some(),
            "Teleport target's position is blocked."
        );
        ensure!(
            self.player_position_valid(None, q.pos),
            controller("Teleport target's position is blocked.")
        );
        Ok(q.pos)
    }

    // The original server heals on a 1s cadence: White Fountain HealPlayer(5, 1000, 100).
    pub(super) fn fountain_heal(&mut self) {
        const RANGE: f32 = 5.;
        const AMOUNT: i32 = 100;
        if self.fountains.is_empty() || self.now < self.next_fountain_heal {
            return;
        }
        self.next_fountain_heal = self.now + 1000;
        let fountains = self.fountains.clone();
        let mut healed = Vec::new();
        for (fountain, pos) in fountains {
            for (id, p) in self.players.iter_mut() {
                if !ready(p)
                    || p.pos.distance(pos) >= RANGE
                    || p.conditions(self.now) & crate::condition::bit(crate::condition::SICK) != 0
                {
                    continue;
                }
                let cap = p
                    .effective(&self.content, self.now)
                    .expect("validated player equipment and base stats")[0];
                let amount = (cap - p.character.hp).clamp(0, AMOUNT);
                if amount == 0 {
                    continue;
                }
                p.character.hp += amount;
                healed.push((*id, amount, p.pos, fountain));
            }
        }
        for (id, amount, pos, fountain) in healed {
            let mut out = Writer::new();
            out.i32(id).utf(&format!("+{amount}")).u32(0xff00ff00);
            for p in self
                .players
                .values()
                .filter(|p| ready(p) && p.known.contains(&id) && p.known.contains(&fountain))
            {
                send(p, wire::NOTIFICATION, out.0.clone());
                send(p, wire::SHOW_EFFECT, effect(1, id, pos));
                send(p, wire::SHOW_EFFECT, effect(7, fountain, pos));
            }
            tracing::info!(
                player = id,
                fountain,
                amount,
                x = pos.x,
                y = pos.y,
                world = %self.name,
                "fountain healed player"
            );
        }
    }

    pub(super) fn consumable_heal_nova(
        &mut self,
        owner: i32,
        amount: i32,
        range: f32,
    ) -> Result<()> {
        let p = self
            .players
            .get(&owner)
            .context("committed healer disappeared")?;
        if !ready(p) {
            return Ok(());
        }
        let position = p.pos;
        let mut healed = Vec::new();
        for (id, q) in self.players.iter_mut().filter(|(_, q)| {
            ready(q)
                && q.pos.distance(position) < range
                && q.conditions(self.now) & crate::condition::bit(crate::condition::SICK) == 0
        }) {
            let cap = q.effective(&self.content, self.now)?[0];
            let before = q.character.hp;
            q.character.hp = (q.character.hp + amount).min(cap);
            healed.push((*id, q.character.hp - before));
        }
        for (target, amount) in healed {
            self.consumable_restored(target, amount, 0xff00ff00);
        }
        for q in self
            .players
            .values()
            .filter(|q| ready(q) && q.known.contains(&owner))
        {
            send(
                q,
                wire::SHOW_EFFECT,
                effect(5, owner, Pos { x: range, y: 0. }),
            );
        }
        tracing::info!(owner, amount, range, "committed Snake Oil healing applied");
        Ok(())
    }

    pub(super) fn heal_ability(
        &mut self,
        id: i32,
        item: &Object,
        amount: i32,
        range: f32,
    ) -> Result<()> {
        let p = self.players.get(&id).context("healer missing")?;
        let position = p.pos;
        let healing = self
            .players
            .iter()
            .filter(|(_, q)| {
                ready(q)
                    && q.pos.distance(position) < range
                    && q.conditions(self.now) & crate::condition::bit(crate::condition::SICK) == 0
            })
            .map(|(id, q)| {
                Ok((
                    *id,
                    (q.effective(&self.content, self.now)?[0] - q.character.hp)
                        .max(0)
                        .min(amount),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let combat = self.entities.iter().any(|(enemy_id, enemy)| {
            enemy.hp > 0
                && self.content.objects[&enemy.kind].enemy
                && healing.iter().any(|(target, amount)| {
                    *amount > 0
                        && self.players[target].known.contains(enemy_id)
                        && self.players[target].pos.distance(enemy.pos) <= 15.
                })
        });
        for (target, amount) in &healing {
            self.players.get_mut(target).unwrap().character.hp += amount;
            if *amount > 0 {
                let mut out = Writer::new();
                out.i32(*target).utf(&format!("+{amount}")).u32(0xff00ff00);
                // Known includes distant minimap peers, not just observers of local healing.
                for p in self.players.values().filter(|p| {
                    ready(p)
                        && p.known.contains(target)
                        && p.pos.distance(self.players[target].pos) <= 20.
                }) {
                    send(p, wire::NOTIFICATION, out.0.clone());
                    send(p, wire::SHOW_EFFECT, effect(1, *target, position));
                }
            }
        }
        for p in self
            .players
            .values()
            .filter(|p| ready(p) && p.known.contains(&id) && p.pos.distance(position) <= 20.)
        {
            send(p, wire::SHOW_EFFECT, effect(5, id, Pos { x: range, y: 0. }));
        }
        tracing::info!(
            player = id,
            character = self.players[&id].character.id,
            world = %self.name,
            time_ms = self.now,
            combat,
            item = item.kind,
            mana = item.mana_cost,
            range,
            ?healing,
            "area ability healing applied"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nexus_keeps_player_teleport_permission() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let d = Directory::test_with_seed(std::path::Path::new("data"), content, 20, 616).unwrap();
        assert!(d.worlds[&-2].allow_teleport());
    }

    #[tokio::test]
    async fn abyss_teleport_advertises_and_uses_the_guarded_player_path() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
        let mut d =
            Directory::test_with_seed(std::path::Path::new("data"), content, 20, 578).unwrap();
        let world = d
            .instance("abyss-of-demons", None, Some(DungeonReturn::Random))
            .unwrap();
        let mut ids = Vec::new();
        let (packets, mut inbox) = mpsc::channel(64);
        let (lost, _) = watch::channel(None);
        let mut output = Some(Output { packets, lost });
        for role in ["sender", "target"] {
            let account = store.list(format!("abyss-tp-{role}")).await.unwrap();
            let ch = store
                .create(account.id, d.content.get(crate::content::WIZARD).unwrap())
                .await
                .unwrap();
            ids.push(
                d.worlds
                    .get_mut(&world)
                    .unwrap()
                    .enter(ch, output.take())
                    .unwrap(),
            );
        }
        let (sender, target) = (ids[0], ids[1]);
        let received;
        let destination;
        {
            let w = d.worlds.get_mut(&world).unwrap();
            let (x, y) = w.map.spawns[0];
            w.players.get_mut(&sender).unwrap().pos = Pos {
                x: x - 1.,
                y: y - 1.,
            };
            destination = Pos {
                x: x + 1.,
                y: y + 1.,
            };
            w.players.get_mut(&target).unwrap().pos = destination;
            w.updates(sender, false).unwrap();
            assert!(w.players[&sender].known.contains(&target));
            received = w.attack_epoch + Duration::from_secs(1);
        }
        let account = d.worlds[&world].players[&sender].character.account;
        let receipt = d.teleport(sender, target, received).unwrap();
        assert_eq!(receipt.world, world);
        assert!(receipt.position.distance(destination) < 0.001);
        assert!(d.player_view(sender).unwrap().map.allow_teleport);
        {
            let w = d.worlds.get_mut(&world).unwrap();
            let bytes = w.map_info();
            let mut packet = wire::Reader::new(&bytes);
            packet.i32().unwrap();
            packet.i32().unwrap();
            packet.utf().unwrap();
            packet.i32().unwrap();
            packet.i32().unwrap();
            assert_eq!(packet.u8().unwrap(), 1);
            assert!(w.players[&sender].character.lifetime.teleported);
            assert_eq!(w.players[&sender].goto_acks.len(), 1);
            assert!(std::iter::from_fn(|| inbox.try_recv().ok()).any(|p| p.id == wire::GOTO));
            w.acknowledge_goto(sender, 0).unwrap();
        }
        assert!(
            d.teleport(sender, target, received + Duration::from_secs(1))
                .unwrap_err()
                .to_string()
                .contains("Too soon")
        );
        assert_eq!(d.teleports[&account], received);
        d.worlds
            .get_mut(&world)
            .unwrap()
            .players
            .get_mut(&target)
            .unwrap()
            .paused = true;
        assert!(
            d.teleport(sender, target, received + Duration::from_secs(11))
                .unwrap_err()
                .to_string()
                .contains("paused")
        );
        assert_eq!(d.teleports[&account], received);
        for name in ["Ocean Trench", "Tomb of the Ancients", "Oryx's Castle"] {
            d.worlds.get_mut(&world).unwrap().name = name.into();
            assert!(!d.player_view(sender).unwrap().map.allow_teleport);
            assert!(
                d.teleport(sender, target, received + Duration::from_secs(11))
                    .unwrap_err()
                    .to_string()
                    .contains("Cannot teleport in this world")
            );
            assert_eq!(d.teleports[&account], received);
        }
    }
}
