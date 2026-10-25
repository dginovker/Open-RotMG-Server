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
        let affects = !crate::condition::protected(flags);
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
