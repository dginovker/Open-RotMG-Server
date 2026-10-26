use super::*;
use std::time::Duration;

pub(super) struct VisibleProjectile {
    bullet: Bullet,
    expires: u64,
    consumed: bool,
}

// Five seconds of Arachna's webs need ~1,900 units; include each queue entry as well as its snapshots.
pub(super) const MAX_PENDING_SHOOT_UNITS: usize = 4096;

// Suspended native clients cannot acknowledge volleys while the shared game clock is paused.
pub(super) fn expect_shoot_ack(
    p: &mut Player,
    bullets: Vec<Bullet>,
    clock: &crate::clock::Clock,
) -> bool {
    if p.controller != Controller::Human {
        return true;
    }
    let pending: usize = p.shoot_acks.iter().map(|(_, shots)| 1 + shots.len()).sum();
    if pending + 1 + bullets.len() > MAX_PENDING_SHOOT_UNITS {
        let oldest = p.shoot_acks.front().and_then(|(_, shots)| shots.first());
        tracing::error!(
            character = p.character.id,
            pending_volleys = p.shoot_acks.len(),
            pending_units = pending,
            incoming_projectiles = bullets.len(),
            limit_units = MAX_PENDING_SHOOT_UNITS,
            oldest_pause_excluding_ms = p
                .shoot_acks
                .front()
                .map(|(sent, _)| clock.now().duration_since(*sent).as_millis()),
            owner = oldest.map(|b| b.owner),
            source_kind = oldest.map(|b| b.source_kind),
            born_ms = oldest.map(|b| b.born),
            last_native_move = p.last_move,
            "SHOOT acknowledgement snapshot budget exceeded"
        );
        if let Some(output) = &p.tx {
            lose(&output.lost, "shoot_ack_budget");
        }
        return false;
    }
    p.shoot_acks.push_back((clock.now(), bullets));
    true
}

impl World {
    pub(super) fn send_enemy_shot(&mut self, owner: i32, body: Vec<u8>) {
        assert!(
            body.len() == 20 || body.len() == 25,
            "invalid internal enemy volley"
        );
        let count = if body.len() == 25 { body[20] } else { 1 };
        let bullets: Vec<_> = (0..count)
            .map(|offset| {
                self.bullets
                    .iter()
                    .find(|b| {
                        !b.friendly
                            && b.born == self.now
                            && b.owner == owner
                            && b.id == body[0].wrapping_add(offset)
                    })
                    .expect("announced hostile projectile missing")
                    .clone()
            })
            .collect();
        for p in self
            .players
            .values_mut()
            .filter(|p| p.known.contains(&owner))
        {
            if expect_shoot_ack(p, bullets.clone(), &self.clock) {
                send(p, 19, body.clone());
            }
        }
    }

    pub fn acknowledge_shoot(&mut self, id: i32, time: i32) -> Result<()> {
        let p = self
            .players
            .get_mut(&id)
            .context("SHOOT ACK player missing")?;
        ensure!(time >= -1, protocol("invalid SHOOT acknowledgement time"));
        let (sent, bullets) = p
            .shoot_acks
            .pop_front()
            .ok_or_else(|| protocol("unsolicited SHOOT acknowledgement"))?;
        let age = self.clock.now().duration_since(sent);
        ensure!(
            age <= Duration::from_secs(5),
            protocol("SHOOT acknowledgement timed out")
        );
        for bullet in bullets {
            let key = (bullet.owner, bullet.id);
            // ACK and hit reports share the client FIFO, so an ID's next generation cannot steal an older report.
            if time < 0 {
                p.visible_projectiles.remove(&key);
            } else {
                let expires = self.now + bullet.spec.lifetime as u64 + 5000;
                p.visible_projectiles.insert(
                    key,
                    VisibleProjectile {
                        bullet,
                        expires,
                        consumed: false,
                    },
                );
            }
        }
        tracing::debug!(
            player = id,
            client_time = time,
            ack_pause_excluding_ms = age.as_millis(),
            pending_volleys = p.shoot_acks.len(),
            "native volley acknowledged"
        );
        Ok(())
    }

    pub fn native_player_hit(&mut self, id: i32, owner: i32, bullet: u8) -> Result<()> {
        let p = self
            .players
            .get_mut(&id)
            .context("hit report player missing")?;
        let Some(report) = p.visible_projectiles.get_mut(&(owner, bullet)) else {
            tracing::warn!(
                player = id,
                owner,
                bullet,
                "hit report rejected: projectile was not acknowledged or expired"
            );
            return Ok(());
        };
        if report.consumed || self.now > report.expires {
            tracing::warn!(
                player = id,
                owner,
                bullet,
                consumed = report.consumed,
                "duplicate or expired hit report rejected"
            );
            return Ok(());
        }
        report.consumed = true;
        let shot = report.bullet.clone();
        if self.safe || !p.character.alive || p.paused || p.departing || p.death_pending {
            tracing::debug!(
                player = id,
                owner,
                bullet,
                "reported hit ignored for inactive or safe character"
            );
            return Ok(());
        }
        self.damage_player(id, &shot, false)?;
        tracing::debug!(
            player = id,
            owner,
            bullet,
            hp = self.players[&id].character.hp,
            "client-observed projectile hit committed"
        );
        Ok(())
    }

    pub(super) fn expire_native_projectiles(&mut self) {
        let now = self.clock.now();
        for (id, p) in &mut self.players {
            p.visible_projectiles
                .retain(|_, report| self.now <= report.expires);
            if !p.departing
                && p.shoot_acks
                    .front()
                    .is_some_and(|(sent, _)| now.duration_since(*sent) > Duration::from_secs(5))
            {
                if let Some(output) = &p.tx {
                    lose(&output.lost, "shoot_ack_timeout");
                }
                p.departing = true;
                let (sent, bullets) = p.shoot_acks.front().expect("expired acknowledgement");
                tracing::error!(
                    player = id,
                    pending = p.shoot_acks.len(),
                    oldest_pause_excluding_ms = now.duration_since(*sent).as_millis(),
                    shooter = ?bullets.first().map(|b|b.owner),
                    volley = bullets.len(),
                    source_kind = bullets.first().map(|b| b.source_kind),
                    projectile = bullets.first().map(|b| b.id),
                    born_ms = bullets.first().map(|b| b.born),
                    last_native_move = p.last_move,

                    "SHOOT acknowledgement timed out; connection closed"
                );
            }
        }
    }

    pub(super) fn damage_player(&mut self, id: i32, b: &Bullet, echo: bool) -> Result<()> {
        let p = self.players.get(&id).context("projectile target missing")?;
        let flags = p.conditions(self.now);
        let affects = !crate::condition::protected(flags);
        let damage =
            crate::condition::damage(b.damage, self.defense(id)?, flags, b.spec.armor_piercing);
        if affects {
            for (index, duration) in &b.spec.effects {
                self.apply_condition(id, *index, *duration)?;
            }
        }
        let p = self.players.get_mut(&id).unwrap();
        let drained = damage.min(p.character.hp).max(0);
        p.character.hp = (p.character.hp - damage).max(0);
        if p.character.hp == 0 {
            p.death_pending = true;
            p.killer = self.content.objects[&b.source_kind].name.clone();
            p.bot_goal = None;
            p.bot_aim = None;
        }
        if echo {
            let mut out = Writer::new();
            out.i32(id).u8(if affects {
                b.spec.effects.len() as u8
            } else {
                0
            });
            if affects {
                for (index, _) in &b.spec.effects {
                    out.u8(*index);
                }
            }
            out.i16(damage as i16).u8(0).u8(b.id).i32(b.owner);
            send(p, 47, out.0);
        }
        tracing::debug!(
            player = id,
            owner = b.owner,
            bullet = b.id,
            source_kind = b.source_kind,
            damage,
            hp = p.character.hp,
            echo,
            "player damaged"
        );
        if drained > 0
            && self.content.objects[&b.source_kind].leach_health
            && self
                .entities
                .get(&b.owner)
                .is_some_and(|e| e.kind == b.source_kind && e.hp > 0)
        {
            self.heal_enemy(b.owner, b.owner, drained)?;
        }
        Ok(())
    }
}
