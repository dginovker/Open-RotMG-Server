use super::*;

impl World {
    pub fn ground_damage(&mut self, id: i32, time: i32, pos: Pos) -> Result<()> {
        ensure!(
            time >= 0 && pos.x.is_finite() && pos.y.is_finite(),
            protocol("invalid ground damage report")
        );
        let p = self
            .players
            .get(&id)
            .context("ground damage player missing")?;
        // Old-position reports already drew RNG; onGoto's ACK bounds their discard: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/kabam/rotmg/messaging/impl/GameServerConnection.as#L1333-L1341
        let correcting = p.controller == Controller::Human && p.active() && p.correcting()?;
        ensure!(
            correcting || p.pos.distance(pos) <= 3.,
            protocol("ground damage position is outside the native movement window")
        );
        let c = self
            .map
            .cell(pos.x, pos.y)
            .ok_or_else(|| protocol("ground damage outside map"))?;
        let i = pos.y as usize * self.map.width as usize + pos.x as usize;
        // A native report can precede receipt of a restored-floor UPDATE; its RNG draw still occurred.
        let ground = if p.controller == Controller::Human
            && !self.content.hazards.contains_key(&c.ground)
            && self.lava_patches.contains_key(&i)
        {
            crate::dungeon::abyss::LAVA
        } else {
            c.ground
        };
        let hazard = self
            .content
            .hazards
            .get(&ground)
            .ok_or_else(|| protocol("ground report has no current or prior hazard"))?
            .clone();
        let protected = self
            .cells
            .ids(i)
            .iter()
            .any(|id| self.content.objects[&self.entities[id].kind].protects_ground);
        let p = self.players.get_mut(&id).unwrap();
        // Selected 123.5.1 Player.update requires lastDamage + 500 < currentTime per square.
        let ignored = correcting
            || self.safe
            || protected
            || !p.character.alive
            || p.paused
            || p.departing
            || p.death_pending
            || p.ground_hits
                .get(&i)
                .is_some_and(|previous| time.saturating_sub(*previous) <= 500);
        if ignored && p.controller != Controller::Human {
            return Ok(());
        }
        let damage = p.roll_between(hazard.min, hazard.max);
        if correcting {
            tracing::warn!(
                player = id,
                client_time_ms = time,
                report_x = pos.x,
                report_y = pos.y,
                player_x = p.pos.x,
                player_y = p.pos.y,
                "discarded ground damage awaiting GOTO acknowledgement"
            );
        }
        if ignored {
            return Ok(());
        }
        p.ground_hits
            .retain(|_, previous| time.saturating_sub(*previous) < 1000);
        p.ground_hits.insert(i, time);
        let damage = crate::condition::damage(damage, 0, p.conditions(self.now), true);
        p.character.hp = (p.character.hp - damage).max(0);
        if p.character.hp == 0 {
            p.death_pending = true;
            p.killer = hazard.name;
            p.bot_goal = None;
            p.bot_aim = None;
        }
        tracing::debug!(
            player = id,
            ground,
            cell = i,
            client_time_ms = time,
            world_time_ms = self.now,
            report_x = pos.x,
            report_y = pos.y,
            player_x = p.pos.x,
            player_y = p.pos.y,
            damage,
            hp = p.character.hp,
            "ground damage committed"
        );
        Ok(())
    }

    pub(super) fn ground_step(&mut self) {
        let actors: Vec<_> = self
            .players
            .iter()
            .filter(|(_, p)| {
                p.controller != Controller::Human
                    && p.character.alive
                    && !p.paused
                    && !p.departing
                    && !p.death_pending
            })
            .filter(|(_, p)| {
                self.map
                    .cell(p.pos.x, p.pos.y)
                    .is_some_and(|c| self.content.hazards.contains_key(&c.ground))
            })
            .map(|(id, p)| (*id, p.pos))
            .collect();
        for (id, pos) in actors {
            if let Err(error) = self.ground_damage(id, self.now as i32, pos) {
                tracing::error!(player=id,%error,"actor ground damage refused");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teleport_ground_reports_wait_for_own_goto_ack_without_damage_or_rng_drift() {
        let data = std::path::Path::new("data");
        let content = Arc::new(Content::load(data).unwrap());
        let mut directory = Directory::test_with_seed(data, content, 20, 537).unwrap();
        let scope = directory
            .instance("snake-pit", None, Some(DungeonReturn::Random))
            .unwrap();
        let world = directory.worlds.get_mut(&scope).unwrap();
        let (x, y) = world.map.spawns[0];
        let origin = Pos { x: x - 4., y };
        let destination = Pos { x: x + 4., y };
        for pos in [origin, destination] {
            let index = pos.y as usize * world.map.width as usize + pos.x as usize;
            Arc::make_mut(&mut world.map).cells[index].ground = crate::dungeon::abyss::LAVA;
        }
        let character: Character = serde_json::from_value(serde_json::json!({
            "schema":1,"lifetime":crate::store::FameLifetime::complete(),
            "class":0x30e,"base":[200,100,15,0,7,10,10,10],
            "id":1,"account":1,"name":"Ground","level":1,"exp":0,
            "fame":0,"hp":200,"mp":100,"revision":0,"alive":true,
            "items":[-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1]
        }))
        .unwrap();
        let mut peer = character.clone();
        peer.id = 2;
        peer.account = 2;
        peer.name = "Target".into();
        let target = world.enter(peer, None).unwrap();
        world.players.get_mut(&target).unwrap().pos = destination;
        let (packets, mut rx) = mpsc::channel(64);
        let (lost, _) = watch::channel(None);
        let id = world
            .enter(character, Some(Output { packets, lost }))
            .unwrap();
        let p = world.players.get_mut(&id).unwrap();
        p.pos = origin;
        p.known.insert(target);
        assert_eq!(p.controller, Controller::Human);
        world.ground_damage(id, 1000, origin).unwrap();
        let hp = world.players[&id].character.hp;
        assert!((141..=170).contains(&hp));
        let mut seed = world.players[&id].seed;
        let hits = world.players[&id].ground_hits.clone();
        let advance = |seed: u32| ((u64::from(seed) * 16807) % 2147483647) as u32;
        let mut observer_goto = Writer::new();
        observer_goto
            .i32(target)
            .f32(destination.x)
            .f32(destination.y);
        reconciliation::goto(
            world.players.get_mut(&id).unwrap(),
            false,
            observer_goto.0.clone(),
        );
        directory.teleport(id, target, Instant::now()).unwrap();
        let world = directory.worlds.get_mut(&scope).unwrap();
        assert_eq!(world.players[&id].pos.distance(destination), 0.);
        assert_eq!(world.players[&id].goto_acks.len(), 2);
        assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|packet| {
            packet.id == wire::GOTO && crate::wire::Reader::new(&packet.body).i32().unwrap() == id
        }));
        for time in [1501, 2102] {
            world
                .ground_damage(id, time, origin)
                .expect("in-flight ground report before GOTO ACK");
            seed = advance(seed);
            assert_eq!(world.players[&id].seed, seed);
            assert_eq!(world.players[&id].character.hp, hp);
            assert_eq!(world.players[&id].ground_hits, hits);
        }
        for (time, pos) in [
            (-1, origin),
            (2103, Pos { x: f32::NAN, y }),
            (2103, Pos { x: -1., y: -1. }),
            (2103, Pos { x, y }),
        ] {
            assert!(world.ground_damage(id, time, pos).is_err());
            assert_eq!(world.players[&id].seed, seed);
        }
        world.acknowledge_goto(id, 2200).unwrap();
        assert!(
            world.players[&id].correcting().unwrap(),
            "another player's ACK cannot release our correction"
        );
        world.ground_damage(id, 2201, origin).unwrap();
        seed = advance(seed);
        assert_eq!(world.players[&id].seed, seed);
        assert_eq!(world.players[&id].character.hp, hp);
        world.acknowledge_goto(id, 2250).unwrap();
        assert!(!world.players[&id].correcting().unwrap());
        assert!(
            world
                .ground_damage(id, 2300, origin)
                .unwrap_err()
                .to_string()
                .contains("outside the native movement window")
        );
        world.ground_damage(id, 2300, destination).unwrap();
        seed = advance(seed);
        assert_eq!(world.players[&id].seed, seed);
        assert_eq!(
            world.players[&id].character.hp,
            hp - 30 - (seed % 30) as i32
        );
        reconciliation::goto(world.players.get_mut(&id).unwrap(), false, observer_goto.0);
        assert!(
            world.ground_damage(id, 2400, origin).is_err(),
            "observer-only GOTO cannot bypass the position window"
        );
        assert_eq!(world.players[&id].seed, seed);
        world.acknowledge_goto(id, 2400).unwrap();
        world
            .correct_position(id, 2400, "test expired GOTO")
            .unwrap();
        world
            .players
            .get_mut(&id)
            .unwrap()
            .goto_acks
            .front_mut()
            .unwrap()
            .1 = Instant::now() - std::time::Duration::from_secs(6);
        assert!(
            world
                .ground_damage(id, 2500, origin)
                .unwrap_err()
                .to_string()
                .contains("GOTO acknowledgement timed out")
        );
        assert_eq!(world.players[&id].seed, seed);
        world.players.get_mut(&id).unwrap().controller = Controller::Autonomous;
        assert!(
            world
                .ground_damage(id, 2500, origin)
                .unwrap_err()
                .to_string()
                .contains("outside the native movement window")
        );
        assert_eq!(world.players[&id].seed, seed);
    }
}
