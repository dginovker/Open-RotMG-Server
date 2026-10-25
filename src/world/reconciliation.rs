use super::*;
use std::time::Duration;

const ACK_LIMIT: usize = 64;
const ACK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub enum NativeInput {
    Applied,
    DiscardedUntilGotoAck,
}

#[derive(Debug)]
pub struct RejectedNativeShot(pub String);
impl std::fmt::Display for RejectedNativeShot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub(super) fn attack_cooldown(
    p: &Player,
    content: &Content,
    now: u64,
    item_rate: f32,
) -> Result<f32> {
    let rate = if p.conditions(now) & crate::condition::bit(crate::condition::DAZED) != 0 {
        0.0015
    } else {
        let base = 0.0015 + (0.008 - 0.0015) * p.effective(content, now)?[5] as f64 / 75.;
        base * if p.conditions(now) & crate::condition::bit(crate::condition::BERSERK) != 0 {
            1.5
        } else {
            1.
        }
    };
    // Ruffle's integer attack period must not accumulate fractional server charges.
    Ok((1. / (rate * f64::from(item_rate))).floor() as f32)
}

pub(super) fn goto(p: &mut Player, own: bool, body: Vec<u8>) {
    let Some(output) = &p.tx else { return };
    if p.goto_acks.len() == ACK_LIMIT {
        tracing::error!(
            character = p.character.id,
            "GOTO acknowledgement backlog exceeded"
        );
        output.lost.send_replace(true);
        return;
    }
    // ACK has no object ID, so other players' GOTOs must share the same FIFO.
    p.goto_acks.push_back((own, Instant::now()));
    send(p, wire::GOTO, body);
}

impl Player {
    pub(super) fn advertised_weapon(&self, kind: u16) -> bool {
        self.weapon_ticks
            .iter()
            .any(|(_, k, _)| *k == i32::from(kind))
    }
    // NEW_TICK ACK precedes applying DEX, so in-flight cadence needs ACK-bounded views: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/kabam/rotmg/messaging/impl/GameServerConnection.as#L1182-L1194
    pub(super) fn advertised_dex(&self, current: i32) -> i32 {
        self.weapon_ticks
            .iter()
            .fold(current, |dex, (_, _, past)| dex.max(*past))
    }
    pub(super) fn remember_weapon_tick(&mut self, dex: i32) {
        if self.controller != Controller::Human {
            return;
        }
        if self.weapon_ticks.len() == ACK_LIMIT {
            tracing::error!(
                character = self.character.id,
                "tick acknowledgement backlog exceeded"
            );
            if let Some(output) = &self.tx {
                output.lost.send_replace(true);
            }
            return;
        }
        self.weapon_ticks
            .push_back((self.tick, self.character.items[0], dex));
    }
    pub(super) fn correcting(&self) -> Result<bool> {
        ensure!(self.active(), "inactive player cannot reconcile position");
        if let Some((_, sent)) = self.goto_acks.iter().find(|(own, _)| *own) {
            ensure!(
                sent.elapsed() <= ACK_TIMEOUT,
                crate::server::lifecycle::protocol("GOTO acknowledgement timed out")
            );
            return Ok(true);
        }
        Ok(false)
    }
    fn consume_discarded_shot(&mut self, id: i32, time: i32, bullet: u8, count: i32) {
        self.discarded_attack = Some((time, count + 1));
        // Ruffle allocated these bullet IDs before receiving the correction; authority stays unchanged.
        self.next_bullet = (bullet + 1) % 128;
        tracing::warn!(
            player = id,
            client_time = time,
            bullet,
            "discarded SHOOT across position correction"
        );
    }
}

impl World {
    pub fn acknowledge_tick(&mut self, id: i32, tick: i32) -> Result<()> {
        let p = self
            .players
            .get_mut(&id)
            .context("tick ACK player missing")?;
        ensure!(
            tick >= 0 && tick < p.tick,
            crate::server::lifecycle::protocol("unsolicited tick acknowledgement")
        );
        // NEW_TICK queues MOVE before applying inventory; earlier SHOOTs still use the old weapon.
        p.weapon_ticks.retain(|(sent, _, _)| *sent >= tick);
        Ok(())
    }
    pub(super) fn validate_native_position(&self, id: i32, pos: Pos, time: i32) -> Result<()> {
        let p = self.players.get(&id).context("native player missing")?;
        ensure!(p.persistence_failure.is_none(), "inactive native character");
        ensure!(
            p.active(),
            crate::server::lifecycle::controller("inactive native character")
        );
        ensure!(
            time >= 0 && time >= p.last_move,
            crate::server::lifecycle::controller("native position clock regressed")
        );
        ensure!(
            pos.x.is_finite() && pos.y.is_finite(),
            crate::server::lifecycle::controller("non-finite native position")
        );
        ensure!(
            self.player_step_valid(p.pos, pos),
            crate::server::lifecycle::controller("native position is outside walkable terrain")
        );
        Ok(())
    }

    pub fn acknowledge_autonomous_goto(&mut self, id: i32) -> Result<()> {
        ensure!(
            self.players
                .get(&id)
                .context("GOTO ACK player missing")?
                .controller
                == Controller::Autonomous,
            "GOTO ACK requires autonomous ownership"
        );
        self.acknowledge_goto(
            id,
            self.now
                .try_into()
                .context("autonomous GOTO clock overflow")?,
        )
    }

    pub fn acknowledge_goto(&mut self, id: i32, time: i32) -> Result<()> {
        let p = self
            .players
            .get_mut(&id)
            .context("GOTO ACK player missing")?;
        let (own, sent) = *p.goto_acks.front().ok_or_else(|| {
            crate::server::lifecycle::protocol("unsolicited GOTO acknowledgement")
        })?;
        ensure!(
            time >= 0,
            crate::server::lifecycle::protocol("negative GOTO acknowledgement time")
        );
        ensure!(
            sent.elapsed() <= ACK_TIMEOUT,
            crate::server::lifecycle::protocol("GOTO acknowledgement timed out")
        );
        if own {
            ensure!(
                time >= p.last_move,
                crate::server::lifecycle::protocol("GOTO acknowledgement clock regressed")
            );
            // The native client ACKs with lastUpdate_, then moves to the supplied position.
            p.last_move = time;
        }
        p.goto_acks.pop_front();
        tracing::debug!(player = id, client_time = time, own, "GOTO acknowledged");
        Ok(())
    }

    fn validate_discarded_shot(&self, id: i32, time: i32, bullet: u8, kind: u16) -> Result<i32> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            time >= 0 && time >= p.last_attack,
            crate::server::lifecycle::protocol("invalid correcting SHOOT time")
        );
        ensure!(
            p.character.items[0] == i32::from(kind) || p.advertised_weapon(kind),
            crate::server::lifecycle::protocol("weapon does not match equipped item")
        );
        ensure!(
            bullet == p.next_bullet,
            crate::server::lifecycle::protocol("unexpected player bullet counter")
        );
        let (last, count) = p
            .discarded_attack
            .unwrap_or((p.last_attack, p.attack_count));
        ensure!(
            time >= last,
            crate::server::lifecycle::protocol("discarded attack clock regressed")
        );
        let item = self.content.get(kind)?;
        let count = if time == last { count } else { 0 };
        if time != last {
            let dex = p.advertised_dex(p.effective(&self.content, self.now)?[5]);
            let rate = 0.0015 + 0.0065 * dex as f64 / 75.;
            let cooldown = (1. / (rate * f64::from(item.rate))).floor() as f32;
            ensure!(
                i64::from(time) - i64::from(last) >= cooldown as i64 - 2,
                crate::server::lifecycle::protocol("discarded attack too fast")
            );
        }
        ensure!(
            count < item.shots,
            crate::server::lifecycle::protocol("too many discarded projectiles in attack")
        );
        Ok(count)
    }

    fn consume_native_discard(
        &mut self,
        id: i32,
        time: i32,
        bullet: u8,
        kind: u16,
        count: i32,
    ) -> Result<()> {
        let spec = self
            .content
            .get(kind)?
            .projectiles
            .get(&0)
            .context("weapon has no projectile zero")?;
        let p = self.players.get_mut(&id).context("player missing")?;
        // Ruffle allocated the shot before the correction, even though no authoritative bullet survives.
        p.roll_damage(spec);
        p.consume_discarded_shot(id, time, bullet, count);
        Ok(())
    }
    pub fn correct_native_shot(
        &mut self,
        id: i32,
        time: i32,
        bullet: u8,
        kind: u16,
        reason: &str,
    ) -> Result<()> {
        let count = self.validate_discarded_shot(id, time, bullet, kind)?;
        self.correct_position(id, time, reason)?;
        self.consume_native_discard(id, time, bullet, kind, count)
    }

    pub(super) fn discard_correcting_move(&self, id: i32, pos: Pos, time: i32) -> Result<bool> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            time >= 0 && pos.x.is_finite() && pos.y.is_finite(),
            crate::server::lifecycle::protocol("invalid correcting MOVE")
        );
        let correcting = p.correcting()?;
        if correcting {
            tracing::warn!(
                player = id,
                client_time = time,
                "discarded MOVE awaiting GOTO acknowledgement"
            );
        }
        Ok(correcting)
    }

    pub(super) fn discard_correcting_shot(
        &mut self,
        id: i32,
        time: i32,
        bullet: u8,
        kind: u16,
        start: Pos,
        angle: f32,
    ) -> Result<bool> {
        let p = self.players.get(&id).context("player missing")?;
        let correcting = p.correcting()?;
        let continuing = p.discarded_attack.is_some_and(|(last, _)| time == last);
        if let Some((last, _)) = p.discarded_attack {
            ensure!(
                time >= last,
                crate::server::lifecycle::protocol("discarded attack clock regressed")
            );
        }
        if !correcting && !continuing {
            return Ok(false);
        }
        ensure!(
            start.x.is_finite() && start.y.is_finite() && angle.is_finite(),
            crate::server::lifecycle::protocol("invalid correcting SHOOT")
        );
        let count = self.validate_discarded_shot(id, time, bullet, kind)?;
        self.consume_native_discard(id, time, bullet, kind, count)?;
        Ok(true)
    }

    pub(super) fn expire_goto_acks(&mut self) {
        for (id, p) in &mut self.players {
            if !p.departing
                && p.goto_acks
                    .front()
                    .is_some_and(|(_, sent)| sent.elapsed() > ACK_TIMEOUT)
            {
                if let Some(output) = &p.tx {
                    output.lost.send_replace(true);
                }
                p.departing = true;
                tracing::error!(
                    player = id,
                    "GOTO acknowledgement timed out; connection closed"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_dexterity_drop_reconciles_only_until_tick_ack_without_extra_damage() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let content = Arc::new(Content::load(&data).unwrap());
        let map = Arc::new(Map::load(&data.join("nexus.wmap"), &content).unwrap());
        let mut world = World::new(content, map).unwrap();
        let character: Character = serde_json::from_value(serde_json::json!({
            "schema":1,"lifetime":crate::store::FameLifetime::complete(),
            "class":0x31d,"base":[200,100,15,0,7,10,10,10],
            "id":1,"account":1,"name":"Cadence","level":1,"exp":0,
            "fame":0,"hp":200,"mp":100,"revision":0,"alive":true,
            "items":[0xa00,-1,-1,0xacc,-1,-1,-1,-1,-1,-1,-1,-1]
        }))
        .unwrap();
        let id = world.enter(character, None).unwrap();
        world.players.get_mut(&id).unwrap().controller = Controller::Human;
        world.step();
        world.step();
        let pos = world.players[&id].pos;
        let start = Pos {
            x: pos.x + 0.3,
            y: pos.y,
        };
        let shot = |world: &mut World, time, bullet| {
            let received = world.clock.now();
            world.native_shoot_received(id, (time, received), bullet, 0xa00, start, 0.)
        };
        assert_eq!(shot(&mut world, 1000, 1).unwrap(), NativeInput::Applied);
        world.players.get_mut(&id).unwrap().character.items[3] = -1;
        assert!(world.shoot(id, 1350, 2, 0xa00, start, 0.).is_err());
        let correcting = world.validate_discarded_shot(id, 1350, 2, 0xa00);
        let seed = world.players[&id].seed;
        assert_eq!(
            shot(&mut world, 1350, 2).expect("old advertised DEX still predicts a 326-ms attack"),
            NativeInput::Applied
        );
        assert_eq!(correcting.unwrap(), 0);
        assert_eq!(world.players[&id].next_bullet, 3);
        assert_eq!(
            world.players[&id].seed,
            ((u64::from(seed) * 16807) % 2147483647) as u32
        );
        assert_eq!(world.players[&id].character.shots, 1);
        assert_eq!(
            world
                .bullets
                .iter()
                .filter(|bullet| bullet.owner == id)
                .count(),
            1
        );
        assert!(world.players[&id].attack_suppressed.is_some());
        world.step();
        world.step();
        let tick = world.players[&id].tick - 1;
        world.acknowledge_tick(id, tick).unwrap();
        assert!(world.validate_discarded_shot(id, 1700, 3, 0xa00).is_err());
        let error = shot(&mut world, 1700, 3).unwrap_err();
        assert!(error.to_string().contains("attack too fast"));
        assert_eq!(world.players[&id].next_bullet, 3);
        assert_eq!(shot(&mut world, 1800, 3).unwrap(), NativeInput::Applied);
        assert_eq!(world.players[&id].character.shots, 2);
        assert!(
            shot(&mut world, 1800, 4)
                .unwrap_err()
                .to_string()
                .contains("too many projectiles")
        );
    }
}
