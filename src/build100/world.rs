use super::{
    content::Content,
    wire::{Packet, Reader, Writer},
};
use anyhow::{Context, Result, bail, ensure};
use rand::{Rng, SeedableRng, rngs::StdRng};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Instant};

pub const PLAYER: i32 = 1;
const SIZE: i32 = 32;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Character {
    pub generation: i32,
    pub kind: i16,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub stats: BTreeMap<u8, i32>,
    pub alive: bool,
}
impl Character {
    pub fn new(content: &Content, kind: i16, name: String, generation: i32) -> Result<Self> {
        let def = content.get(kind)?;
        ensure!(
            def.class == "Player" && def.equipment.len() == 12,
            "not a playable class"
        );
        ensure!(
            !name.is_empty() && name.len() <= 16 && name.bytes().all(|b| b.is_ascii_alphanumeric()),
            "name must be 1–16 ASCII letters/digits"
        );
        let mut stats = def.stats.clone();
        let hp = *stats.get(&0).context("class missing maximum HP")?;
        let mp = *stats.get(&3).context("class missing maximum MP")?;
        stats.extend([(1, hp), (2, 100), (4, mp), (5, 50), (6, 0), (7, 1), (25, 0)]);
        for (i, &item) in def.equipment.iter().enumerate() {
            stats.insert(8 + i as u8, item);
        }
        Ok(Self {
            generation,
            kind,
            name,
            x: 16.5,
            y: 16.5,
            stats,
            alive: true,
        })
    }
}
#[derive(Clone)]
struct Entity {
    id: i32,
    kind: i16,
    x: f32,
    y: f32,
    stats: BTreeMap<u8, i32>,
    age: f32,
}
struct Bullet {
    owner: i32,
    id: u8,
    visual: u8,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    damage: i32,
    life: f32,
}
pub struct Room {
    pub character: Character,
    entities: BTreeMap<i32, Entity>,
    bullets: Vec<Bullet>,
    rng: StdRng,
    next_id: i32,
    elapsed: f32,
    shot_clock: f32,
    next_wave: f32,
    next_tick: f32,
    move_at: Instant,
}
fn packet(id: u8, f: impl FnOnce(&mut Writer)) -> Packet {
    let mut w = Writer::new();
    f(&mut w);
    Packet { id, body: w.0 }
}
fn status(w: &mut Writer, id: i32, x: f32, y: f32, stats: &BTreeMap<u8, i32>) {
    w.i32(id).f32(x).f32(y).i16(stats.len() as i16);
    for (&k, &v) in stats {
        w.u8(k).i32(v);
    }
}
fn make(id: i32, kind: i16, name: &str, x: f32, y: f32, stats: &BTreeMap<u8, i32>) -> Packet {
    packet(10, |w| {
        w.i16(kind).utf(name);
        status(w, id, x, y, stats);
    })
}
fn drop_object(id: i32) -> Packet {
    packet(11, |w| {
        w.i32(id);
    })
}
fn distance(x: f32, y: f32, xx: f32, yy: f32) -> f32 {
    (x - xx).hypot(y - yy)
}
pub fn message(text: &str) -> Packet {
    packet(7, |w| {
        w.utf("*Help*").i32(-1).utf(text);
    })
}
impl Room {
    pub fn new(character: Character, seed: u64) -> Self {
        Self {
            character,
            entities: BTreeMap::new(),
            bullets: vec![],
            rng: StdRng::seed_from_u64(seed),
            next_id: 100,
            elapsed: 0.,
            shot_clock: 0.,
            next_wave: 0.,
            next_tick: 0.2,
            move_at: Instant::now(),
        }
    }
    pub fn enter(&mut self, content: &Content) -> Result<Vec<Packet>> {
        let mut out = vec![packet(1, |w| {
            w.i32(PLAYER);
        })];
        for x in 0..SIZE {
            for y in 0..SIZE {
                out.push(packet(12, |w| {
                    w.i16(x as i16).i16(y as i16).u8(0).u8(
                        if (2..30).contains(&x) && (2..30).contains(&y) {
                            0x46
                        } else {
                            0x32
                        },
                    );
                }));
            }
        }
        let p = &self.character;
        out.push(make(PLAYER, p.kind, &p.name, p.x, p.y, &p.stats));
        out.push(message("Build 100 compatibility arena: reconstructed solo loop, not a faithful 2010 realm. WASD move, click shoot, 1 potion. Defeat pirates and drag bag loot into inventory."));
        self.spawn(content, &mut out)?;
        out.push(self.tick());
        Ok(out)
    }
    fn spawn(&mut self, content: &Content, out: &mut Vec<Packet>) -> Result<()> {
        let pirate = content.named("Pirate")?;
        for (x, y) in [(21., 16.), (22., 20.), (19., 22.)] {
            let mut stats = pirate.stats.clone();
            let hp = *stats.get(&0).context("Pirate has no HP")?;
            stats.insert(1, hp);
            self.next_id += 1;
            let e = Entity {
                id: self.next_id,
                kind: pirate.id,
                x,
                y,
                stats,
                age: 0.,
            };
            out.push(make(e.id, e.kind, "", x, y, &e.stats));
            self.entities.insert(e.id, e);
        }
        self.next_wave = self.elapsed + 8.;
        Ok(())
    }
    fn tick(&self) -> Packet {
        packet(13, |w| {
            w.i16((1 + self.entities.len()) as i16);
            let p = &self.character;
            status(w, PLAYER, p.x, p.y, &p.stats);
            for e in self.entities.values() {
                status(w, e.id, e.x, e.y, &e.stats);
            }
        })
    }
    pub fn handle(&mut self, p: Packet, content: &Content) -> Result<Vec<Packet>> {
        if !self.character.alive {
            tracing::debug!(
                id = p.id,
                "discarding in-flight input after committed death"
            );
            return Ok(vec![]);
        }
        let mut r = Reader::new(&p.body);
        let mut out = vec![];
        match p.id {
            5 => {
                let (x, y) = (r.f32()?, r.f32()?);
                r.done()?;
                ensure!(
                    (2.0..30.0).contains(&x) && (2.0..30.0).contains(&y),
                    "movement outside arena"
                );
                let now = Instant::now();
                let dt = now.duration_since(self.move_at).as_secs_f32();
                ensure!(
                    distance(x, y, self.character.x, self.character.y) <= 15. * dt + 1.,
                    "movement exceeds speed limit"
                );
                self.character.x = x;
                self.character.y = y;
                self.move_at = now;
            }
            8 => {
                let id = r.u8()?;
                let owner = r.i32()?;
                let sub = r.u8()?;
                let kind = r.i16()?;
                let (x, y, angle) = (r.f32()?, r.f32()?, r.f32()?);
                let _claimed_damage = r.i16()?;
                r.done()?;
                ensure!(
                    owner == PLAYER && i32::from(kind) == self.character.stats[&8],
                    "invalid equipped weapon shot: owner={owner} sub={sub} container={kind:#x} expected={:#x}",
                    self.character.stats[&8]
                );
                ensure!(
                    distance(x, y, self.character.x, self.character.y) < 2.,
                    "shot origin too far from player"
                );
                ensure!(
                    self.bullets.len() < 256
                        && !self.bullets.iter().any(|b| b.owner == PLAYER && b.id == id),
                    "duplicate or excessive bullets"
                );
                let proj = content
                    .get(kind)?
                    .projectile
                    .as_ref()
                    .context("weapon has no projectile")?;
                ensure!(
                    i16::from(sub) == content.named(&proj.visual)?.id,
                    "invalid projectile visual {sub}"
                );
                let attack = self.character.stats[&20] as f32;
                let damage = (self.rng.gen_range(proj.min..=proj.max) as f32
                    * (0.5 + attack / 75. * 2.))
                    .round() as i32;
                self.bullets.push(Bullet {
                    owner,
                    id,
                    visual: sub,
                    x,
                    y,
                    vx: angle.cos() * proj.speed,
                    vy: angle.sin() * proj.speed,
                    damage,
                    life: proj.lifetime,
                });
                tracing::debug!(id, kind, damage, "build100 player shot");
            }
            9 => {
                // Native collision reports are advisory; the server advances and resolves its own projectiles.
                r.i32()?;
                r.u8()?;
                r.u8()?;
                r.i16()?;
                r.u8()?;
                r.i32()?;
                r.done()?;
            }
            14 => {
                let a = (r.i32()?, r.u8()?, r.i16()?);
                let b = (r.i32()?, r.u8()?, r.i16()?);
                r.done()?;
                ensure!(a != b, "cannot swap slot with itself");
                ensure!(
                    self.item(a.0, a.1)? == i32::from(a.2)
                        && self.item(b.0, b.1)? == i32::from(b.2),
                    "stale inventory swap"
                );
                self.can_store(a.0, a.1, b.2, content)?;
                self.can_store(b.0, b.1, a.2, content)?;
                self.set_item(a.0, a.1, b.2);
                self.set_item(b.0, b.1, a.2);
                tracing::info!(
                    from = a.0,
                    slot = a.1,
                    to = b.0,
                    to_slot = b.1,
                    item = a.2,
                    "build100 inventory swap"
                );
                out.push(self.tick());
            }
            15 => {
                let owner = r.i32()?;
                let slot = r.u8()?;
                let _target = r.i32()?;
                let (x, y) = (r.f32()?, r.f32()?);
                r.done()?;
                ensure!(owner == PLAYER, "cannot use another object's item");
                let kind = i16::try_from(self.item(owner, slot)?)?;
                let item = content.get(kind)?;
                match item
                    .activation
                    .as_ref()
                    .context("item cannot be activated")?
                {
                    (name, amount) if name == "IncrementStat" && item.name == "Health Potion" => {
                        let hp = self.character.stats[&1];
                        let max = self.character.stats[&0];
                        ensure!(hp < max, "health is already full");
                        self.character.stats.insert(1, (hp + amount).min(max));
                    }
                    (name, _) if name == "BulletNova" => {
                        let proj = item
                            .projectile
                            .as_ref()
                            .context("spell missing projectile")?;
                        ensure!(
                            distance(x, y, self.character.x, self.character.y) < 15.,
                            "spell target too distant"
                        );
                        for id in 0..20 {
                            let angle = id as f32 * std::f32::consts::TAU / 20.;
                            let damage = self.rng.gen_range(proj.min..=proj.max);
                            let b = Bullet {
                                owner: PLAYER,
                                id,
                                visual: u8::try_from(content.named(&proj.visual)?.id)?,
                                x,
                                y,
                                vx: angle.cos() * proj.speed,
                                vy: angle.sin() * proj.speed,
                                damage,
                                life: proj.lifetime,
                            };
                            out.push(shot(&b, kind, angle));
                            self.bullets.push(b);
                        }
                    }
                    (name, _) => bail!("build100 activation {name} is not implemented"),
                }
                if item.consumable {
                    self.set_item(owner, slot, -1);
                }
                tracing::info!(kind, hp = self.character.stats[&1], "build100 item used");
                out.push(self.tick());
            }
            7 => {
                r.utf()?;
                r.i32()?;
                let text = r.utf()?;
                r.done()?;
                ensure!(text.len() <= 256, "chat too long");
                out.push(message(&format!("{}: {text}", self.character.name)));
            }
            23 => {
                r.i32()?;
                r.done()?;
            }
            _ => bail!("build100 packet {} is not implemented", p.id),
        }
        Ok(out)
    }
    fn item(&self, id: i32, slot: u8) -> Result<i32> {
        if id == PLAYER {
            ensure!(slot < 12, "invalid character slot");
            Ok(self.character.stats[&(8 + slot)])
        } else {
            let e = self.entities.get(&id).context("missing loot container")?;
            ensure!(slot < 8 && e.kind == 0x500, "invalid bag slot");
            ensure!(
                distance(e.x, e.y, self.character.x, self.character.y) < 2.,
                "loot container too far away"
            );
            Ok(e.stats[&(8 + slot)])
        }
    }
    fn can_store(&self, id: i32, slot: u8, kind: i16, content: &Content) -> Result<()> {
        self.item(id, slot)?;
        if id == PLAYER && slot < 4 && kind != -1 {
            let required = content.get(self.character.kind)?.slots[slot as usize];
            ensure!(
                content.get(kind)?.slot_type == Some(required),
                "item does not fit equipment slot"
            );
        }
        Ok(())
    }
    fn set_item(&mut self, id: i32, slot: u8, kind: i16) {
        if id == PLAYER {
            self.character.stats.insert(8 + slot, i32::from(kind));
        } else {
            self.entities
                .get_mut(&id)
                .expect("validated container")
                .stats
                .insert(8 + slot, i32::from(kind));
        }
    }
    pub fn advance(&mut self, dt: f32, content: &Content) -> Result<Vec<Packet>> {
        if !self.character.alive {
            return Ok(vec![]);
        }
        self.elapsed += dt;
        self.shot_clock += dt;
        let mut out = vec![];
        let p = &self.character;
        let fire = self.shot_clock >= 1.;
        if fire {
            self.shot_clock = 0.;
        }
        for e in self.entities.values_mut() {
            e.age += dt;
            if e.kind == 0x500 {
                continue;
            }
            let d = distance(e.x, e.y, p.x, p.y);
            if d > 1. && d < 10. {
                let step = dt.min(d - 1.);
                e.x += (p.x - e.x) / d * step;
                e.y += (p.y - e.y) / d * step;
            }
            if fire && d < 3.6 {
                let proj = content
                    .get(e.kind)?
                    .projectile
                    .as_ref()
                    .context("enemy missing projectile")?;
                let angle = (p.y - e.y).atan2(p.x - e.x);
                let id = (self.elapsed as u32 % 256) as u8;
                let b = Bullet {
                    owner: e.id,
                    id,
                    visual: u8::try_from(content.named(&proj.visual)?.id)?,
                    x: e.x,
                    y: e.y,
                    vx: angle.cos() * proj.speed,
                    vy: angle.sin() * proj.speed,
                    damage: proj.min,
                    life: proj.lifetime,
                };
                out.push(shot(&b, e.kind, angle));
                self.bullets.push(b);
            }
        }
        let mut hits = vec![];
        for b in &mut self.bullets {
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            b.life -= dt;
            if b.life < 0. {
                continue;
            }
            let target = if b.owner == PLAYER {
                self.entities
                    .values()
                    .find(|e| e.kind != 0x500 && distance(e.x, e.y, b.x, b.y) < 0.65)
                    .map(|e| e.id)
            } else if distance(self.character.x, self.character.y, b.x, b.y) < 0.5 {
                Some(PLAYER)
            } else {
                None
            };
            if let Some(target) = target {
                hits.push((target, b.owner, b.id, b.damage));
                b.life = -1.;
            }
        }
        self.bullets.retain(|b| b.life > 0.);
        for (target, owner, id, raw) in hits {
            if !self.character.alive {
                break;
            }
            if target == PLAYER {
                let damage = (raw - self.character.stats[&21]).max((raw + 6) / 7);
                let hp = (self.character.stats[&1] - damage).max(0);
                self.character.stats.insert(1, hp);
                out.push(damage_packet(target, owner, id, damage));
                tracing::info!(damage, hp, "build100 player damaged");
                if hp == 0 {
                    self.character.alive = false;
                    out.push(self.tick());
                    out.push(drop_object(PLAYER));
                    tracing::info!(
                        generation = self.character.generation,
                        "build100 permanent death"
                    );
                }
            } else if let Some(e) = self.entities.get_mut(&target) {
                let damage = (raw - e.stats[&21]).max(1);
                *e.stats.get_mut(&1).context("enemy HP missing")? -= damage;
                out.push(damage_packet(target, owner, id, damage));
                if e.stats[&1] <= 0 {
                    let e = self.entities.remove(&target).expect("enemy present");
                    out.push(drop_object(target));
                    *self
                        .character
                        .stats
                        .get_mut(&6)
                        .context("player XP missing")? += 5;
                    while self.character.stats[&7] < 20
                        && self.character.stats[&6] >= self.character.stats[&5]
                    {
                        for &(stat, min, max, cap) in &content.get(self.character.kind)?.growth {
                            let value = self
                                .character
                                .stats
                                .get_mut(&stat)
                                .context("growth stat missing")?;
                            *value = (*value + self.rng.gen_range(min..=max)).min(cap);
                        }
                        *self.character.stats.get_mut(&7).expect("level initialized") += 1;
                        *self
                            .character
                            .stats
                            .get_mut(&5)
                            .expect("XP threshold initialized") += self.character.stats[&7] * 50;
                        self.character.stats.insert(1, self.character.stats[&0]);
                        self.character.stats.insert(4, self.character.stats[&3]);
                        tracing::info!(
                            level = self.character.stats[&7],
                            "build100 compatibility progression level up"
                        );
                    }
                    self.next_id += 1;
                    let id = self.next_id;
                    let mut stats = BTreeMap::from([(2, 75)]);
                    for slot in 8..16 {
                        stats.insert(
                            slot,
                            if slot == 8 {
                                i32::from(content.named("Health Potion")?.id)
                            } else {
                                -1
                            },
                        );
                    }
                    let bag = Entity {
                        id,
                        kind: 0x500,
                        x: e.x,
                        y: e.y,
                        stats,
                        age: 0.,
                    };
                    out.push(make(id, bag.kind, "", bag.x, bag.y, &bag.stats));
                    self.entities.insert(id, bag);
                    tracing::info!(
                        target,
                        xp = self.character.stats[&6],
                        bag = id,
                        "build100 enemy killed; compatibility-arena guaranteed potion"
                    );
                }
            }
        }
        let expired: Vec<_> = self
            .entities
            .values()
            .filter(|e| e.kind == 0x500 && (e.age > 60. || (8..16).all(|s| e.stats[&s] == -1)))
            .map(|e| e.id)
            .collect();
        for id in expired {
            self.entities.remove(&id);
            out.push(drop_object(id));
        }
        if self.character.alive
            && self.elapsed >= self.next_wave
            && !self.entities.values().any(|e| e.kind != 0x500)
        {
            self.spawn(content, &mut out)?;
        }
        if self.character.alive && self.elapsed >= self.next_tick {
            self.next_tick += 0.2;
            out.push(self.tick());
        }
        Ok(out)
    }
}
fn shot(b: &Bullet, kind: i16, angle: f32) -> Packet {
    packet(8, |w| {
        w.u8(b.id)
            .i32(b.owner)
            .u8(b.visual)
            .i16(kind)
            .f32(b.x)
            .f32(b.y)
            .f32(angle)
            .i16(b.damage as i16);
    })
}
fn damage_packet(target: i32, owner: i32, id: u8, damage: i32) -> Packet {
    packet(9, |w| {
        w.i32(target)
            .u8(0)
            .u8(0)
            .i16(damage as i16)
            .u8(id)
            .i32(owner);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Object, Projectile};
    pub fn content() -> Content {
        let mut objects = BTreeMap::new();
        for (id, name, class) in [
            (0x30e, "Wizard", "Player"),
            (0x600, "Pirate", "Character"),
            (0x500, "Loot Bag", "Container"),
            (0xa22, "Health Potion", "Equipment"),
            (0xa97, "Energy Staff", "Equipment"),
            (41, "Missile", "Projectile"),
        ] {
            objects.insert(
                id,
                Object {
                    id,
                    name: name.into(),
                    class: class.into(),
                    stats: BTreeMap::new(),
                    growth: vec![],
                    slots: vec![],
                    equipment: vec![],
                    slot_type: None,
                    projectile: None,
                    activation: None,
                    consumable: false,
                },
            );
        }
        let p = objects.get_mut(&0x30e).unwrap();
        p.stats = BTreeMap::from([(0, 100), (3, 100), (20, 12), (21, 0), (22, 10)]);
        p.growth = vec![(0, 25, 25, 1000)];
        p.slots = vec![17, 11, 14, 9];
        p.equipment = vec![0xa97, -1, -1, -1, 0xa22, -1, -1, -1, -1, -1, -1, -1];
        let e = objects.get_mut(&0x600).unwrap();
        e.stats = BTreeMap::from([(0, 5), (21, 0)]);
        e.projectile = Some(Projectile {
            visual: "Missile".into(),
            min: 5,
            max: 5,
            speed: 6.,
            lifetime: 0.6,
        });
        let w = objects.get_mut(&0xa97).unwrap();
        w.slot_type = Some(17);
        w.projectile = Some(Projectile {
            visual: "Missile".into(),
            min: 20,
            max: 20,
            speed: 13.,
            lifetime: 1.,
        });
        let p = objects.get_mut(&0xa22).unwrap();
        p.slot_type = Some(10);
        p.activation = Some(("IncrementStat".into(), 1000));
        p.consumable = true;
        Content { objects }
    }
    fn room() -> (Content, Room) {
        let c = content();
        let p = Character::new(&c, 0x30e, "Test".into(), 1).unwrap();
        let mut r = Room::new(p, 42);
        r.enter(&c).unwrap();
        (c, r)
    }
    fn fire(id: u8, owner: i32, kind: i16) -> Packet {
        packet(8, |w| {
            w.u8(id)
                .i32(owner)
                .u8(41)
                .i16(kind)
                .f32(16.8)
                .f32(16.5)
                .f32(0.)
                .i16(32767);
        })
    }
    #[test]
    fn real_layout_and_server_authoritative_damage() {
        let (c, mut r) = room();
        r.entities.retain(|&id, _| id == 101);
        let e = r.entities.get_mut(&101).unwrap();
        e.x = 18.;
        e.y = 16.5;
        r.handle(fire(1, PLAYER, 0xa97), &c).unwrap();
        for _ in 0..8 {
            r.advance(0.05, &c).unwrap();
        }
        assert!(!r.entities.contains_key(&101));
        assert_eq!(r.character.stats[&6], 5);
        let bag = r.entities.values().find(|e| e.kind == 0x500).unwrap();
        assert_eq!(bag.stats[&8], 0xa22);
    }
    #[test]
    fn forged_damage_cannot_kill_or_reward() {
        let (c, mut r) = room();
        let old = r.entities[&101].stats[&1];
        r.handle(damage_packet(101, PLAYER, 1, 32000), &c).unwrap();
        assert_eq!(r.entities[&101].stats[&1], old);
        assert_eq!(r.character.stats[&6], 0);
    }
    #[test]
    fn rejects_spoofed_shots_and_inventory() {
        let (c, mut r) = room();
        assert!(r.handle(fire(1, 999, 0xa97), &c).is_err());
        assert!(r.handle(fire(1, PLAYER, 0xa22), &c).is_err());
        assert!(r.can_store(PLAYER, 0, 0xa22, &c).is_err());
        assert!(r.item(123, 0).is_err());
        assert!(r.item(PLAYER, 12).is_err());
    }
    #[test]
    fn potion_once_and_postdeath_inputs_cannot_resurrect() {
        let (c, mut r) = room();
        r.character.stats.insert(1, 25);
        let use_item = || {
            packet(15, |w| {
                w.i32(PLAYER).u8(4).i32(-1).f32(16.5).f32(16.5);
            })
        };
        r.handle(use_item(), &c).unwrap();
        assert_eq!(r.character.stats[&1], 100);
        assert_eq!(r.character.stats[&12], -1);
        assert!(r.handle(use_item(), &c).is_err());
        r.character.alive = false;
        r.character.stats.insert(1, 0);
        r.handle(use_item(), &c).unwrap();
        assert_eq!(r.character.stats[&1], 0);
    }
    #[test]
    fn projectile_death_is_once_only_and_emits_native_death() {
        let (c, mut r) = room();
        r.character.stats.insert(1, 5);
        r.entities.clear();
        r.next_wave = 999.;
        r.bullets.push(Bullet {
            owner: 100,
            id: 1,
            visual: 41,
            x: 16.5,
            y: 16.5,
            vx: 0.,
            vy: 0.,
            damage: 5,
            life: 1.,
        });
        let packets = r.advance(0.05, &c).unwrap();
        assert!(!r.character.alive);
        assert!(packets.iter().any(|p| p.id == 11));
        assert!(r.advance(0.05, &c).unwrap().is_empty());
    }
    #[test]
    fn loot_swap_is_conservative_and_range_checked() {
        let (c, mut r) = room();
        let stats = (8..16)
            .map(|s| (s, if s == 8 { 0xa22 } else { -1 }))
            .collect();
        r.entities.insert(
            200,
            Entity {
                id: 200,
                kind: 0x500,
                x: 17.,
                y: 16.5,
                stats,
                age: 0.,
            },
        );
        r.character.stats.insert(12, -1);
        let swap = || {
            packet(14, |w| {
                w.i32(200).u8(0).i16(0xa22).i32(PLAYER).u8(4).i16(-1);
            })
        };
        r.handle(swap(), &c).unwrap();
        assert_eq!(r.item(PLAYER, 4).unwrap(), 0xa22);
        assert_eq!(r.item(200, 0).unwrap(), -1);
        assert!(r.handle(swap(), &c).is_err());
        r.entities.get_mut(&200).unwrap().x = 25.;
        assert!(r.item(200, 0).is_err());
    }
    #[test]
    fn compatibility_xp_levels_with_client_growth_caps() {
        let (c, mut r) = room();
        r.character.stats.insert(6, 45);
        r.entities.retain(|&id, _| id == 101);
        let e = r.entities.get_mut(&101).unwrap();
        e.x = 18.;
        e.y = 16.5;
        r.handle(fire(1, PLAYER, 0xa97), &c).unwrap();
        for _ in 0..8 {
            r.advance(0.05, &c).unwrap();
        }
        assert_eq!(r.character.stats[&7], 2);
        assert_eq!(r.character.stats[&0], 125);
        assert_eq!(r.character.stats[&5], 150);
    }
}
