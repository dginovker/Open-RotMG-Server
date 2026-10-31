use super::*;
use crate::wire::Reader;
use tokio::sync::broadcast;

const BUFFER: usize = 256;

pub(crate) fn advance_cursor(last: &mut Option<u32>, packet: &Packet) -> Result<bool> {
    ensure!(
        packet.id == 250 && matches!(packet.body.len(), 21 | 29),
        "invalid native cursor packet"
    );
    let tick = u32::from_be_bytes(packet.body[8..12].try_into()?);
    if last
        .is_some_and(|prior| tick.wrapping_sub(prior) == 0 || tick.wrapping_sub(prior) >= 1 << 31)
    {
        return Ok(false);
    }
    *last = Some(tick);
    Ok(true)
}
pub(crate) struct Attachment {
    pub world: i32,
    pub account: i32,
    pub player: i32,
    pub character: i32,
    pub map: Vec<u8>,
    pub snapshot: Vec<Packet>,
    pub receiver: broadcast::Receiver<Packet>,
}

pub(super) fn publish(p: &Player, id: u8, body: &[u8]) {
    if matches!(
        id,
        wire::UPDATE
            | wire::NEW_TICK
            | 19
            | 74
            | 13
            | 4
            | wire::GOTO
            | wire::SHOW_EFFECT
            | wire::PLAY_SOUND
            | wire::TEXT
            | 34
    ) && let Some(sender) = &p.spectators
        && sender.receiver_count() > 0
    {
        let _ = sender.send(Packet {
            id,
            body: body.to_vec(),
        });
    }
}

impl Handle {
    pub(crate) async fn perspective_visible(
        &self,
        source: (i32, i32),
        target: (i32, i32),
    ) -> Result<bool> {
        let Some(slot) = self.routing.player(source.0)? else {
            return Ok(false);
        };
        self.request(
            &slot.mailbox,
            "world",
            "perspective",
            |world| world.performance.get_mut(),
            move |world| Ok(world.perspective_visible(source, target)),
        )
        .await
    }
    #[cfg(test)]
    pub(crate) async fn spectator_cursor(
        &self,
        world: i32,
        player: i32,
        account: i32,
        character: i32,
    ) -> Result<Option<Packet>> {
        Ok(self
            .spectator_inputs(world, player, account, character, 1)
            .await?
            .map(|mut packets| packets.remove(0)))
    }
    pub(crate) async fn spectator_inputs(
        &self,
        world: i32,
        player: i32,
        account: i32,
        character: i32,
        version: u8,
    ) -> Result<Option<Vec<Packet>>> {
        ensure!(matches!(version, 1 | 2), "invalid spectator input version");
        let Some(slot) = self.routing.player(player)? else {
            return Ok(None);
        };
        if slot.id != world {
            return Ok(None);
        }
        self.request(
            &slot.mailbox,
            "world",
            "spectator_cursor",
            |w| w.performance.get_mut(),
            move |w| {
                let Some(p) = w.players.get(&player) else {
                    return Ok(None);
                };
                if p.character.account != account
                    || p.character.id != character
                    || p.controller != Controller::Autonomous
                    || !p.active()
                {
                    return Ok(None);
                }
                let Some(inputs) = &p.inputs else {
                    return Ok(None);
                };
                let Some(cursor) = inputs.cursor else {
                    return Ok(None);
                };
                ensure!(
                    cursor.x.is_finite()
                        && cursor.y.is_finite()
                        && p.pos.x.is_finite()
                        && p.pos.y.is_finite(),
                    "nonfinite spectator cursor"
                );
                let mut out = Writer::new();
                out.i32(player)
                    .i32(character)
                    .i32(w.tick as u32 as i32)
                    .f32(cursor.x)
                    .f32(cursor.y)
                    .u8(u8::from(inputs.fire && !p.pending));
                if version == 2 {
                    out.f32(p.pos.x).f32(p.pos.y);
                }
                let mut packets = vec![Packet {
                    id: 250,
                    body: out.0,
                }];
                if version == 2 {
                    let draft = &inputs.composition;
                    let (phase, text) = if p.pending {
                        (0, "")
                    } else {
                        (draft.phase, draft.text.as_str())
                    };
                    ensure!(
                        phase <= 2
                            && (phase != 0 || text.is_empty())
                            && text.encode_utf16().count() <= 128,
                        "invalid spectator composition"
                    );
                    let mut out = Writer::new();
                    out.i32(player)
                        .i32(character)
                        .i32(w.tick as u32 as i32)
                        .u8(phase)
                        .utf(text);
                    packets.push(Packet {
                        id: 251,
                        body: out.0,
                    });
                }
                Ok(Some(packets))
            },
        )
        .await
    }
    pub(crate) async fn spectate(&self, player: i32, account: i32) -> Result<Option<Attachment>> {
        let Some(slot) = self.routing.player(player)? else {
            return Ok(None);
        };
        let scope = slot.id;
        self.request(
            &slot.mailbox,
            "world",
            "spectate",
            |w| w.performance.get_mut(),
            move |w| {
                let Some(p) = w.players.get(&player) else {
                    return Ok(None);
                };
                ensure!(
                    p.character.account == account && p.controller == Controller::Autonomous,
                    "spectator identity is not an autonomous bot"
                );
                if !p.character.alive || p.death_pending || p.departing {
                    return Ok(None);
                }
                w.attach_spectator(player, scope).map(Some)
            },
        )
        .await
    }
}

impl World {
    pub(super) fn perspective_visible(&self, source: (i32, i32), target: (i32, i32)) -> bool {
        let (Some(own), Some(other)) = (self.players.get(&source.0), self.players.get(&target.0))
        else {
            return false;
        };
        let live_bot = |player: &Player, account| {
            player.character.account == account
                && player.controller == Controller::Autonomous
                && player.character.alive
                && !player.death_pending
                && !player.departing
                && player.persistence_failure.is_none()
        };
        source.0 != target.0
            && live_bot(own, source.1)
            && live_bot(other, target.1)
            && own.known.contains(&target.0)
    }
    pub(super) fn attach_spectator(&mut self, id: i32, world: i32) -> Result<Attachment> {
        let p = self
            .players
            .get_mut(&id)
            .context("spectator bot left world")?;
        let receiver = p
            .spectators
            .get_or_insert_with(|| broadcast::channel(BUFFER).0)
            .subscribe();
        let character = p.character.id;
        let mut packets = Vec::new();
        let tiles: Vec<_> = p.tiles.iter().copied().collect();
        for chunk in tiles.chunks(4096) {
            let mut out = Writer::new();
            out.i16(chunk.len() as i16);
            for &(x, y) in chunk {
                out.i16(x)
                    .i16(y)
                    .u8(self.map.cells[y as usize * self.map.width as usize + x as usize].ground);
            }
            out.i16(0).i16(0);
            packets.push(Packet {
                id: wire::UPDATE,
                body: out.0,
            });
        }
        let mut ids: Vec<_> = self.players[&id]
            .known
            .iter()
            .copied()
            .filter(|entity| {
                self.players.contains_key(entity) || self.entities.contains_key(entity)
            })
            .collect();
        ensure!(
            ids.contains(&id),
            "spectator bot has not received its initial update"
        );
        ids.sort_by_key(|entity| (*entity != id, *entity));
        for chunk in ids.chunks(256) {
            let mut out = Writer::new();
            out.i16(0).i16(chunk.len() as i16);
            for &entity in chunk {
                let kind = self
                    .players
                    .get(&entity)
                    .map(|p| p.character.class)
                    .or_else(|| self.entities.get(&entity).map(|e| e.kind))
                    .context("spectator known object is missing")?;
                out.i16(kind as i16);
                out.0.extend(self.stats(entity, id)?);
            }
            out.i16(0);
            packets.push(Packet {
                id: wire::UPDATE,
                body: out.0,
            });
        }
        let mut quest = Writer::new();
        quest.i32(
            self.players[&id]
                .quest
                .filter(|q| ids.contains(q))
                .unwrap_or(-1),
        );
        packets.push(Packet {
            id: 34,
            body: quest.0,
        });
        Ok(Attachment {
            world,
            account: self.players[&id].character.account,
            player: id,
            character,
            map: self.map_info(),
            snapshot: packets,
            receiver,
        })
    }
}

pub(crate) struct Projection {
    own: i32,
    player_types: BTreeMap<i32, bool>,
}
impl Projection {
    pub(crate) fn new(own: i32) -> Self {
        Self {
            own,
            player_types: BTreeMap::new(),
        }
    }
    fn status(&self, r: &mut Reader<'_>, out: &mut Writer) -> Result<()> {
        let actor = r.i32()?;
        out.i32(actor).f32(r.f32()?).f32(r.f32()?);
        let count = r.i16()?;
        ensure!(count >= 0, "negative spectator stat count");
        let mut fields = Writer::new();
        let mut kept = 0;
        for _ in 0..count {
            let key = r.u8()?;
            let mut field = Writer::new();
            field.u8(key);
            if matches!(key, 31 | 62) {
                field.utf(&r.utf()?);
            } else {
                field.i32(r.i32()?);
            }
            let public = matches!(key,0..=7 | 20..=22 | 26..=33 | 46..=53 | 56..=58 | 62..=65);
            let player = self.player_types.get(&actor).copied();
            let appearance = player == Some(false) && matches!(key, 34 | 36..=37 | 40..=45 | 55);
            let inventory =
                (8..=19).contains(&key) && (actor == self.own || key <= 11 && player == Some(true));
            // Container contents and human backpacks never become public merely because the bot received them.
            if public || appearance || inventory {
                fields.0.extend(field.0);
                kept += 1;
            }
        }
        out.i16(kept);
        out.0.extend(fields.0);
        Ok(())
    }
    pub(crate) fn packet(&mut self, packet: Packet, content: &Content) -> Result<Option<Packet>> {
        let mut r = Reader::new(&packet.body);
        let mut out = Writer::new();
        match packet.id {
            wire::UPDATE => {
                let tiles = r.i16()?;
                ensure!(tiles >= 0, "negative spectator tile count");
                out.i16(tiles);
                for _ in 0..tiles {
                    out.i16(r.i16()?).i16(r.i16()?).u8(r.u8()?);
                }
                let objects = r.i16()?;
                ensure!(objects >= 0, "negative spectator object count");
                out.i16(objects);
                for _ in 0..objects {
                    let kind = r.i16()? as u16;
                    // NEW_TICK omits types; merchant appearance and player inventory have different public fields.
                    let mut peek = Reader::new(r.remaining());
                    let id = peek.i32()?;
                    self.player_types
                        .insert(id, content.get(kind)?.class == "Player");
                    out.i16(kind as i16);
                    self.status(&mut r, &mut out)?;
                }
                let drops = r.i16()?;
                ensure!(drops >= 0, "negative spectator drop count");
                out.i16(drops);
                for _ in 0..drops {
                    let id = r.i32()?;
                    self.player_types.remove(&id);
                    out.i32(id);
                }
            }
            wire::NEW_TICK => {
                out.i32(r.i32()?).i32(r.i32()?);
                let count = r.i16()?;
                ensure!(count >= 0, "negative spectator tick count");
                out.i16(count);
                for _ in 0..count {
                    self.status(&mut r, &mut out)?;
                }
            }
            wire::TEXT => {
                r.utf()?;
                let actor = r.i32()?;
                r.i32()?;
                r.u8()?;
                let recipient = r.utf()?;
                r.utf()?;
                r.utf()?;
                r.done()?;
                return Ok(
                    (recipient.is_empty() && self.player_types.get(&actor) == Some(&true))
                        .then_some(packet),
                );
            }
            19 | 74 | 13 | 4 | wire::GOTO | wire::SHOW_EFFECT | wire::PLAY_SOUND | 34 => {
                return Ok(Some(packet));
            }
            _ => return Ok(None),
        }
        r.done()?;
        Ok(Some(Packet {
            id: packet.id,
            body: out.0,
        }))
    }
}

#[cfg(test)]
#[test]
fn unwatched_bot_ticks_are_not_encoded_and_the_first_watched_tick_is_complete() {
    let (mut world, id) = super::movement_tests::fixture(0x18, false);
    world.players.get_mut(&id).unwrap().controller = Controller::Autonomous;
    world.updates(id, true).unwrap();
    for _ in 0..6 {
        world.step();
    }
    assert_eq!(world.players[&id].tick_stats.elided, 0, "unwatched tick was encoded");
    let mut attachment = world.attach_spectator(id, 1).unwrap();
    let ticks: Vec<_> = (0..4)
        .flat_map(|_| {
            world.step();
            std::iter::from_fn(|| attachment.receiver.try_recv().ok()).collect::<Vec<_>>()
        })
        .filter(|packet| packet.id == wire::NEW_TICK)
        .collect();
    assert!(!ticks.is_empty(), "watched bot received no NEW_TICK");
    let mut r = Reader::new(&ticks[0].body);
    r.i32().unwrap();
    r.i32().unwrap();
    let mut own = None;
    for _ in 0..r.i16().unwrap() {
        let start = r.remaining();
        let entity = r.i32().unwrap();
        r.take(8).unwrap();
        for _ in 0..r.i16().unwrap() {
            if matches!(r.u8().unwrap(), 31 | 62) {
                r.utf().unwrap();
            } else {
                r.i32().unwrap();
            }
        }
        if entity == id {
            own = Some(start[..start.len() - r.remaining().len()].to_vec());
        }
    }
    r.done().unwrap();
    assert_eq!(own.unwrap(), world.stats(id, id).unwrap(), "first watched tick omitted stats");
}
