use super::*;

impl World {
    pub(super) fn record_lifetime_kill(
        &mut self,
        owner: i32,
        kind: u16,
        final_blow: bool,
    ) -> Result<()> {
        let definition = &self.content.objects[&kind];
        if definition.enemy
            && let Some(p) = self
                .players
                .get_mut(&owner)
                .filter(|p| p.character.alive && !p.death_started && !p.departing)
        {
            let mut lifetime = p.character.lifetime.clone();
            lifetime.kills = lifetime
                .kills
                .checked_add(1)
                .context("lifetime kill overflow")?;
            if definition.god {
                lifetime.god_kills = lifetime
                    .god_kills
                    .checked_add(1)
                    .context("lifetime god kill overflow")?;
            }
            if definition.cube {
                lifetime.cube_kills = lifetime
                    .cube_kills
                    .checked_add(1)
                    .context("lifetime cube kill overflow")?;
            }
            if definition.oryx && final_blow {
                lifetime.oryx_kills = lifetime
                    .oryx_kills
                    .checked_add(1)
                    .context("lifetime Oryx kill overflow")?;
            }
            p.character.lifetime = lifetime;
        }
        Ok(())
    }
    pub(super) fn damage_enemy(
        &mut self,
        owner: i32,
        target: i32,
        raw: i32,
        piercing: bool,
        effects: &[(u8, u64)],
        bullet: Option<u8>,
    ) -> Result<i32> {
        let Some(e) = self.entities.get(&target) else {
            tracing::debug!(owner, target, "damage target already removed");
            return Ok(0);
        };
        // Selected Boss Totems have no HP; zero HP must not turn an immune hit into a defeat.
        if e.kind == crate::dungeon::jungle::BOSS_TOTEM {
            return Ok(0);
        }
        let flags = e.conditions(self.now);
        // Invulnerability blocks HP loss, not projectile conditions: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/realm/entities/Enemy.cs#L93-L117
        let affects = flags
            & (crate::condition::bit(crate::condition::INVINCIBLE)
                | crate::condition::bit(crate::condition::STASIS))
            == 0;
        let damage =
            crate::condition::damage(raw, self.content.objects[&e.kind].defense, flags, piercing);
        // Evaluation shells select a fight; only the transformed boss can be defeated for rewards.
        let damage = if e.quest.is_some() && matches!(e.kind, 0x91b | 0x91f | 0x928) {
            damage.min((e.hp - 1).max(0))
        } else {
            damage
        };
        let tomb_immortal = crate::dungeon::tomb::immortal(e.kind);
        let damage = if tomb_immortal { 0 } else { damage };
        let actual = damage.min(e.hp).max(0);
        if affects {
            for (index, duration) in effects {
                self.apply_condition(target, *index, *duration)?;
            }
        }
        let e = self.entities.get_mut(&target).unwrap();
        if let Some(p) = self.players.get(&owner) {
            *e.damage.entry(p.character.id).or_default() += damage.min(e.hp).max(0);
        }
        e.hp -= damage;
        // A damaged Malphas missile ends through its telegraphed explosion, not ordinary loot/death.
        if e.kind == 0x909 {
            e.hp = e.hp.max(1);
        }
        let tomb_dying = e.hp <= 0 && matches!(e.kind, 0xd2a | 0xd2b);
        if tomb_dying {
            e.hp = 1;
            e.stage = Some(4);
            e.activated = Some(self.now);
            e.condition |= crate::condition::bit(crate::condition::INVULNERABLE);
            e.tomb
                .as_mut()
                .context("lethal worshipper state missing")?
                .initialized = true;
        }
        let lethal = e.hp <= 0 && !tomb_immortal;
        let transform =
            lethal && e.quest.is_some() && matches!(e.kind, 0x675 | 0x6c2 | 0x92a | 0x92b);
        let lantern = lethal && e.kind == crate::dungeon::davy::UNLIT && self.locker.is_some();
        let kill = lethal && !transform && !lantern;
        let kind = e.kind;

        let quest = e.kind == 0x6b1 || self.content.objects[&e.kind].quest;
        if e.kind == 0x90b {
            self.abyss_flamer_damage(target, owner, actual, lethal)?;
        }
        if let Some(p) = self.players.get_mut(&owner) {
            if damage > 0 && bullet.is_some() {
                p.character.hits += 1;
            }
            if kill {
                p.character.kills += 1;
                if quest {
                    p.character.quests += 1;
                }
            }
        }
        if kill {
            self.record_lifetime_kill(owner, self.entities[&target].kind, true)?;
        }
        let mut o = Writer::new();
        o.i32(target)
            .u8(if affects { effects.len() as u8 } else { 0 });
        if affects {
            for (index, _) in effects {
                o.u8(*index);
            }
        }
        o.i16(damage as i16)
            .u8(u8::from(kill))
            .u8(bullet.unwrap_or(0))
            .i32(if bullet.is_some() { owner } else { 0 });
        for p in self.players.values() {
            if p.known.contains(&target) {
                send(p, 47, o.0.clone());
            }
        }
        if tomb_dying {
            self.flash_inland(target, 0xffff0000, 0.1, 20.);
        }
        if lantern {
            self.light_locker_lantern(target)?;
        } else if transform {
            self.quest_lethal_transform(target)?;
        } else if kill {
            if self.name == "Realm"
                && (crate::endgame::HEROES.contains(&kind)
                    || crate::quest::root(kind)
                    || crate::quest::final_root(kind))
                && let Some(player) = self.players.get(&owner).filter(|p| {
                    p.character.alive
                        && !p.death_pending
                        && !p.departing
                        && !p.character.name.is_empty()
                })
            {
                let root = self.entities[&target].quest.unwrap_or(target);
                self.critical_killers
                    .insert(root, (kind, player.character.name.clone()));
            }
            // Terminal authority seals NEW credit sources; damage and shared XP still resolve normally.
            let killer = self
                .players
                .get(&owner)
                .filter(|p| p.character.alive && !p.death_pending && !p.departing)
                .map(|p| p.character.id);
            self.defeat_entity_with_killer(target, killer)?;
            tracing::info!(entity = target, kind, owner = owner, "enemy defeated");
        }
        Ok(actual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::{self, INVINCIBLE, INVULNERABLE, STASIS, STUNNED};

    #[test]
    fn shield_stuns_invulnerable_bosses_without_damage_or_credit() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let shield = content.get(0xa0c).unwrap().projectiles[&0].clone();
        assert_eq!(shield.effects, [(STUNNED, 3000)]);
        let map = Arc::new(Map {
            width: 40,
            height: 40,
            cells: vec![
                crate::content::Cell {
                    ground: 0x61,
                    object: None,
                    config: String::new(),
                    terrain: 0,
                    region: 0
                };
                1600
            ],
            spawns: vec![(20.5, 20.5)],
        });
        for kind in [0xd90, 0x920] {
            for protection in [
                0,
                condition::bit(INVULNERABLE),
                condition::bit(INVINCIBLE),
                condition::bit(STASIS),
                condition::bit(INVULNERABLE) | condition::bit(12),
            ] {
                let mut world = World::new(content.clone(), map.clone()).unwrap();
                world.encounter = true;
                let base = content.classes[&0x31e].caps;
                let character = serde_json::from_value(serde_json::json!({
                    "schema":1,"lifetime":crate::store::FameLifetime::complete(),
                    "class":0x31e,"base":base,"id":1,"account":1,"name":"Stun",
                    "level":20,"exp":0,"fame":0,"hp":base[0],"mp":base[1],
                    "revision":0,"alive":true,"items":[-1,0xa0c,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1]
                }))
                .unwrap();
                let player = world.enter(character, None).unwrap();
                let boss = world
                    .spawn_entity(kind, Pos { x: 22.5, y: 20.5 }, vec![])
                    .unwrap();
                let e = world.entities.get_mut(&boss).unwrap();
                e.condition = protection;
                if kind == 0x920 {
                    e.quest.set(Some(boss), boss, &mut world.quest_ids);
                }
                let hp = world.entities[&boss].hp;
                let damage = world
                    .damage_enemy(player, boss, 500, false, &shield.effects, Some(0))
                    .unwrap();
                let e = &world.entities[&boss];
                let should_stun = protection
                    & (condition::bit(INVINCIBLE) | condition::bit(STASIS) | condition::bit(12))
                    == 0;
                assert_eq!(
                    e.conditions(world.now) & condition::bit(STUNNED) != 0,
                    should_stun,
                    "boss {kind:#x}, protection {protection:#x}"
                );
                if protection != 0 {
                    assert_eq!((damage, e.hp, e.damage[&1]), (0, hp, 0));
                } else {
                    assert!(damage > 0 && e.hp < hp);
                }
                if should_stun {
                    for _ in 0..59 {
                        world.now += 50;
                        assert!(world.npc_step().iter().all(|(owner, _)| *owner != boss));
                    }
                    assert!(
                        world.entities[&boss].conditions(world.now) & condition::bit(STUNNED) != 0
                    );
                    let mut resumed = false;
                    for _ in 0..60 {
                        world.now += 50;
                        resumed |= world.npc_step().iter().any(|(owner, _)| *owner == boss);
                    }
                    assert!(resumed, "boss {kind:#x} must resume after stun expires");
                }
            }
        }
    }
}
