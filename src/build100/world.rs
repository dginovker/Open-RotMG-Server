use super::{
    content::Content,
    realm_map::{Kind, Map, Region},
    rules::{
        combat::{self, Stats},
        progression::{self, EnemyDeath, Progress, RewardRecipient},
    },
    wire::{Packet, Reader, Writer},
};
use anyhow::{Context, Result, bail, ensure};
use rand::{Rng, SeedableRng, rngs::StdRng, seq::SliceRandom};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
};

#[cfg(test)]
pub const PLAYER: i32 = 1;
const SIZE: i32 = 32;
const MOVE_DISTANCE_CREDIT: f32 = 0.5;

#[derive(Debug)]
pub(crate) struct MoveBudget {
    anchor: Instant,
    validated: Instant,
    speed: f32,
    credit: f32,
    requested: f32,
}
impl std::fmt::Display for MoveBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("movement exceeds elapsed speed allowance")
    }
}
impl std::error::Error for MoveBudget {}
impl MoveBudget {
    pub(crate) fn wait_boundary(&self) -> Instant {
        // Round down to nanoseconds so scheduling cannot exceed one existing distance cap.
        self.validated
            + std::time::Duration::from_nanos(
                (f64::from(MOVE_DISTANCE_CREDIT) / f64::from(self.speed) * 1e9).floor() as u64,
            )
    }
    pub(crate) fn deadline(&self, boundary: Instant) -> Option<Instant> {
        let funded = |at: Instant| {
            self.requested
                <= self.speed * at.duration_since(self.anchor).as_secs_f32() + self.credit
        };
        if boundary <= self.validated || !funded(boundary) {
            return None;
        }
        let (mut low, mut high) = (
            0,
            u64::try_from(boundary.duration_since(self.validated).as_nanos()).ok()?,
        );
        // Find the first fundable nanosecond using the validator's exact f32 expression;
        // this predicts a wake only, never authorizes movement at that nominal time.
        while high - low > 1 {
            let middle = low + (high - low) / 2;
            if funded(self.validated + std::time::Duration::from_nanos(middle)) {
                high = middle;
            } else {
                low = middle;
            }
        }
        Some(self.validated + std::time::Duration::from_nanos(high))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Character {
    pub generation: i32,
    pub kind: i16,
    pub name: String,
    pub x: f32,
    pub y: f32,
    // Permanent bases stay durable; projecting bonuses only at use/publication prevents reconnect stacking.
    pub stats: BTreeMap<u8, i32>,
    pub alive: bool,
    #[serde(default)]
    pub tutorial_complete: bool,
    #[serde(default)]
    pub resources_version: u8,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_carry"
    )]
    pub resources_carry: Option<ResourceCarry>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_action_state"
    )]
    pub action_state: Option<ActionState>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionState {
    version: u8,
    conditions: combat::ConditionsSnapshot,
    #[serde(deserialize_with = "present_cooldown")]
    ability_used_seconds: Option<f64>,
}
fn present_action_state<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<ActionState>, D::Error> {
    ActionState::deserialize(deserializer).map(Some)
}
fn present_cooldown<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<f64>, D::Error> {
    Option::<f64>::deserialize(deserializer)
}
impl Default for ActionState {
    fn default() -> Self {
        Self {
            version: 1,
            conditions: combat::Conditions::default().snapshot(),
            ability_used_seconds: None,
        }
    }
}
impl ActionState {
    fn cooldown_expiry_seconds(&self) -> Result<Option<f64>> {
        self.ability_used_seconds
            .map(|previous| {
                let end = previous + 0.5;
                ensure!(
                    end.is_finite() && end > previous,
                    "ability cooldown overflow or lost precision"
                );
                // Addition may round down; use the first representable time accepted by cooldown_ready.
                Ok(if combat::cooldown_ready(end, Some(previous), 0.5)? {
                    end
                } else {
                    end.next_up()
                })
            })
            .transpose()
    }
    fn checked_conditions(&self) -> Result<(combat::Conditions, combat::ConditionMask)> {
        ensure!(
            self.version == 1,
            "unsupported action state version {}",
            self.version
        );
        ensure!(
            self.conditions
                .expiries
                .iter()
                .enumerate()
                .all(
                    |(i, end)| i + 1 == super::content::Condition::Invisible as usize
                        || end.is_none()
                ),
            "unsupported normal condition timer: only Invisible is executable"
        );
        let mut conditions = combat::Conditions::restore(&self.conditions)?;
        let now = self.conditions.now_seconds;
        let mask = conditions.expire(now)?;
        self.cooldown_expiry_seconds()?;
        if let Some(previous) = self.ability_used_seconds {
            ensure!(
                !combat::cooldown_ready(now, Some(previous), 0.5)?,
                "saved ability cooldown is expired"
            );
        }
        ensure!(
            mask.bits() != 0 || self.ability_used_seconds.is_some() || now == 0.,
            "empty action state has unanchored clock"
        );
        Ok((conditions, mask))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceCarry {
    pub hp_remainder: f64,
    pub mp_remainder: f64,
    pub hp_roundoff: f64,
    pub mp_roundoff: f64,
}
fn present_carry<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<ResourceCarry>, D::Error> {
    ResourceCarry::deserialize(deserializer).map(Some)
}
impl ResourceCarry {
    const ZERO: Self = Self {
        hp_remainder: 0.,
        mp_remainder: 0.,
        hp_roundoff: 0.,
        mp_roundoff: 0.,
    };
}
impl Character {
    fn progress(&self, content: &Content) -> Result<Progress> {
        let level = u8::try_from(*self.stats.get(&7).context("avatar missing level stat7")?)
            .context("avatar level outside u8 range")?;
        let progress = Progress {
            level,
            xp: *self
                .stats
                .get(&6)
                .context("avatar missing XP progress stat6")?,
            base: self
                .stats
                .iter()
                .filter(|(id, _)| matches!(id, 0 | 3 | 20 | 21 | 22 | 26 | 27 | 28))
                .map(|(&id, &value)| (id, value))
                .collect(),
        };
        progress.validate(content.get(self.kind)?)?;
        ensure!(
            self.stats.get(&5) == Some(&progression::xp_goal(level)?),
            "avatar {} generation {} XP goal stat5 mismatches level {level}",
            self.name,
            self.generation
        );
        Ok(progress)
    }
    fn award_progress(&mut self, amount: i32, content: &Content, rng: &mut StdRng) -> Result<()> {
        let current = self.progress(content)?;
        let effective = self.equipment_stats(content)?;
        let mut vitals = self.vitals(&effective)?;
        let award = progression::award_xp(&current, content.get(self.kind)?, amount, rng)?;
        vitals.hp = vitals
            .hp
            .checked_add(award.max_hp_delta)
            .context("level growth HP overflow")?;
        vitals.mp = vitals
            .mp
            .checked_add(award.max_mp_delta)
            .context("level growth MP overflow")?;
        self.stats.extend(award.progress.base);
        self.stats.extend([
            (5, progression::xp_goal(award.progress.level)?),
            (6, award.progress.xp),
            (7, i32::from(award.progress.level)),
        ]);
        self.store_vitals(vitals);
        self.vitals(&self.equipment_stats(content)?)?;
        Ok(())
    }
    fn equipment_stats(&self, content: &Content) -> Result<Stats> {
        let derive = || -> Result<Stats> {
            let class = content.get(self.kind)?;
            ensure!(class.class == "Player", "avatar class is not Player");
            ensure!(
                class.slots.len() == 12
                    && class.slots[..4].iter().all(|s| (1..=17).contains(s))
                    && class.slots[4..].iter().all(|&s| s == 0),
                "invalid class SlotTypes"
            );
            let base = Stats::from_base(
                &self
                    .stats
                    .iter()
                    .filter(|(id, _)| matches!(id, 0 | 3 | 20 | 21 | 22 | 26 | 27 | 28))
                    .map(|(&id, &value)| (id, value))
                    .collect(),
            )?;
            let inventory = (0..12)
                .map(|slot| -> Result<_> {
                    let &id = self
                        .stats
                        .get(&(8 + slot))
                        .with_context(|| format!("missing inventory slot {slot}"))?;
                    if id == -1 {
                        return Ok(None);
                    }
                    ensure!(id >= 0, "invalid item ID {id} in slot {slot}");
                    let kind = i16::try_from(id)
                        .with_context(|| format!("item ID {id} outside range in slot {slot}"))?;
                    Ok(Some(
                        content
                            .get(kind)
                            .with_context(|| format!("item ID {id} in slot {slot}"))?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            combat::equipment_stats(&base, &class.slots, &inventory)
        };
        derive().with_context(|| {
            format!(
                "avatar {} class {:#x} generation {} equipment stats",
                self.name, self.kind, self.generation
            )
        })
    }
    fn validate_current(&self, effective: &Stats) -> Result<()> {
        let hp = *self.stats.get(&1).context("avatar missing current HP")?;
        let mp = *self.stats.get(&4).context("avatar missing current MP")?;
        let (max_hp, max_mp) = (effective.get(0)?, effective.get(3)?);
        ensure!(
            (0..=max_hp).contains(&hp) && (self.alive == (hp > 0)),
            "avatar {} generation {} current HP/alive outside effective maximum: HP={hp}, alive={}, maxHP={max_hp}",
            self.name,
            self.generation,
            self.alive
        );
        ensure!(
            (0..=max_mp).contains(&mp),
            "avatar {} generation {} current MP outside effective maximum: MP={mp}, maxMP={max_mp}",
            self.name,
            self.generation
        );
        Ok(())
    }
    fn resource_conditions(&self) -> Result<combat::ConditionMask> {
        let mask = self
            .stats
            .get(&25)
            .context("avatar missing condition mask stat25")?;
        let state = self
            .action_state
            .as_ref()
            .context("normal action state missing")?;
        let (_, live) = state.checked_conditions()?;
        ensure!(
            i64::from(*mask) == i64::from(live.bits()),
            "saved condition mask stat25 mismatches live timers"
        );
        ensure!(
            self.alive || *state == ActionState::default(),
            "dead avatar has live action state"
        );
        Ok(live)
    }
    fn vitals(&self, effective: &Stats) -> Result<combat::Vitals> {
        self.validate_current(effective)?;
        self.resource_conditions()?;
        ensure!(
            self.resources_version == 1,
            "unsupported resources version {}",
            self.resources_version
        );
        let carry = self
            .resources_carry
            .context("version1 resources carry missing")?;
        let vitals = combat::Vitals {
            hp: self.stats[&1],
            mp: self.stats[&4],
            hp_remainder: carry.hp_remainder,
            mp_remainder: carry.mp_remainder,
            hp_roundoff: carry.hp_roundoff,
            mp_roundoff: carry.mp_roundoff,
        };
        vitals.validate(effective)?;
        Ok(vitals)
    }
    fn advance_resources_and_actions(&mut self, mut seconds: f64, effective: &Stats) -> Result<()> {
        let mut vitals = self.vitals(effective)?;
        if !self.alive {
            return Ok(());
        }
        let mut state = self
            .action_state
            .clone()
            .context("normal action state missing")?;
        let (mut conditions, mut mask) = state.checked_conditions()?;
        // Only one supported expiry and one cooldown can split an attached frame.
        for _ in 0..3 {
            let now = state.conditions.now_seconds;
            let boundary = conditions
                .next_expiry_seconds()
                .into_iter()
                .chain(state.cooldown_expiry_seconds()?)
                .min_by(f64::total_cmp);
            let Some(boundary) = boundary else {
                vitals = vitals.regenerate(seconds, effective, mask)?;
                state = ActionState::default();
                seconds = 0.;
                break;
            };
            let end = now + seconds;
            ensure!(
                end.is_finite() && (seconds == 0. || end > now),
                "action clock overflow or lost elapsed precision"
            );
            let next = end.min(boundary);
            let step = if end <= boundary {
                seconds
            } else {
                boundary - now
            };
            vitals = vitals.regenerate(step, effective, mask)?;
            seconds = if end <= boundary { 0. } else { seconds - step };
            mask = conditions.expire(next)?;
            state.conditions = conditions.snapshot();
            if let Some(previous) = state.ability_used_seconds
                && combat::cooldown_ready(next, Some(previous), 0.5)?
            {
                state.ability_used_seconds = None;
            }
            if mask.bits() == 0 && state.ability_used_seconds.is_none() {
                state = ActionState::default();
                vitals = vitals.regenerate(seconds, effective, mask)?;
                seconds = 0.;
                break;
            }
            if seconds == 0. {
                break;
            }
        }
        ensure!(
            seconds == 0.,
            "action frame partition exceeded supported boundaries"
        );
        state.checked_conditions()?;
        self.stats.insert(25, mask.bits() as i32);
        self.action_state = Some(state);
        self.store_vitals(vitals);
        Ok(())
    }
    fn store_vitals(&mut self, vitals: combat::Vitals) {
        self.stats.insert(1, vitals.hp);
        self.stats.insert(4, vitals.mp);
        self.resources_carry = Some(ResourceCarry {
            hp_remainder: vitals.hp_remainder,
            mp_remainder: vitals.mp_remainder,
            hp_roundoff: vitals.hp_roundoff,
            mp_roundoff: vitals.mp_roundoff,
        });
    }
    fn clear_dead_hp_carry(&mut self) {
        if self.action_state.is_some() {
            self.action_state = Some(ActionState::default());
            self.stats.insert(25, 0);
        }
        if let Some(carry) = &mut self.resources_carry {
            carry.hp_remainder = 0.;
            carry.hp_roundoff = 0.;
        }
    }
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
            tutorial_complete: false,
            resources_version: 1,
            resources_carry: Some(ResourceCarry::ZERO),
            action_state: Some(ActionState::default()),
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provenance {
    Map,
    Population,
    Summoned { source: i32 },
    Fixture,
    Loot { source: i32 },
}
#[derive(Clone)]
struct Entity {
    id: i32,
    kind: i16,
    x: f32,
    y: f32,
    stats: BTreeMap<u8, i32>,
    age: f32,
    provenance: Provenance,
}
#[derive(Clone)]
enum Flight {
    Arena,
    Normal {
        projectile: super::content::Projectile,
        angle: f64,
    },
}
impl Flight {
    fn damage(&self, raw: i32, defense: i32) -> Result<i32> {
        match self {
            Self::Arena => super::rules::defended_damage(raw, defense),
            Self::Normal { projectile, .. } => {
                combat::projectile_defended_damage(raw, defense, projectile)
            }
        }
    }
}
#[derive(Clone)]
struct Bullet {
    flight: Flight,
    owner: i32,
    id: u8,
    origin: (f32, f32),
    age: f32,
    amplitude: f32,
    frequency: f32,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    damage: i32,
    life: f32,
}
struct WeaponShot {
    id: u8,
    origin: (f32, f32),
    angle: f32,
}
#[derive(Clone)]
struct PendingWeaponBurst {
    first_received: Instant,
    due: Instant,
    weapon: super::content::Object,
    equipment: [i32; 4],
    attack: i32,
    conditions: combat::ConditionMask,
    origin: (f32, f32),
    shots: Vec<(u8, f32)>,
}
#[derive(Clone, Default)]
struct WeaponAdmission {
    // Reservations burn cadence even if incomplete; only actual funding can reanchor a completed burst.
    started: Option<Instant>,
    last_received: Option<Instant>,
    interval_ms: f64,
    pending: Option<PendingWeaponBurst>,
}
#[derive(Debug)]
pub(crate) enum TeleportDecline {
    Dead,
    TargetUnavailable,
    Paused,
    Cooldown,
}
impl std::fmt::Display for TeleportDecline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dead => "dead avatar cannot teleport",
            Self::TargetUnavailable => "teleport target is not a living owned player in this room",
            Self::Paused => "paused player cannot teleport or be a teleport target",
            Self::Cooldown => "teleport cooldown has not expired",
        })
    }
}
impl std::error::Error for TeleportDecline {}
fn teleport_unpaused(mask: combat::ConditionMask) -> Result<()> {
    if mask.contains(super::content::Condition::Paused) {
        return Err(TeleportDecline::Paused.into());
    }
    Ok(())
}

#[derive(Clone)]
pub struct Avatar {
    pub character: Character,
    move_at: Instant,
    teleport_ready: Option<Instant>,
    weapon: WeaponAdmission,
    hazard_at: f32,
    move_credit: f32,
    known_ground: BTreeSet<(i16, i16)>,
    known_objects: BTreeSet<i32>,
    enemy_view_repair: Option<i32>,
    quiver_ack: Option<i16>,
}
#[derive(Clone)]
struct ShorePirate {
    home: (f32, f32),
    next_shot: f32,
}
#[derive(Clone)]
struct ShorePopulation {
    sites: Vec<(f32, f32)>,
    actors: BTreeMap<i32, ShorePirate>,
    next_replenish: f32,
}
#[derive(Clone)]
pub struct Room {
    pub players: BTreeMap<i32, Avatar>,
    entities: BTreeMap<i32, Entity>,
    bullets: Vec<Bullet>,
    server_projectile_cursors: BTreeMap<i32, u8>,
    enemy_slows: BTreeMap<i32, f32>,
    shore: Option<ShorePopulation>,
    rng: StdRng,
    next_id: i32,
    id_limit: i32,
    pub elapsed: f32,
    pub map: Option<Arc<Map>>,
    next_gift: f32,
    shot_clock: f32,
    next_wave: f32,
    next_tick: f32,
    portal: i32,
    destination: i32,
}
// Server retirement does not prove a client's object key is retired; keep cycling per owner.
fn server_projectile_ids(
    bullets: &[Bullet],
    owner: i32,
    first: u8,
    cursor: Option<u8>,
    count: usize,
) -> Result<(Vec<u8>, u8)> {
    let capacity = usize::from(255 - first) + 1;
    ensure!(
        first > 0 && (1..=capacity).contains(&count),
        "invalid server projectile namespace/count"
    );
    // No cursor exists until this owner funds its first server shot; absence is the initial state.
    let mut next = match cursor {
        Some(next) => next,
        None => first,
    };
    ensure!(
        next >= first,
        "invalid server projectile cursor for owner {owner}"
    );
    let mut ids = Vec::with_capacity(count);
    for _ in 0..capacity {
        let id = next;
        next = if id == 255 { first } else { id + 1 };
        if !bullets.iter().any(|b| b.owner == owner && b.id == id) {
            ids.push(id);
            if ids.len() == count {
                return Ok((ids, next));
            }
        }
    }
    bail!("server projectile namespace exhausted for owner {owner}")
}
fn normal_npc_shot(
    owner: &Entity,
    angle: f32,
    content: &Content,
    bullets: &mut Vec<Bullet>,
    cursors: &mut BTreeMap<i32, u8>,
    rng: &mut StdRng,
) -> Result<Packet> {
    ensure!(
        bullets.len() < 4096,
        "build100 world projectile capacity exhausted"
    );
    let proj = content.get(owner.kind)?.normal_projectile(0)?;
    let (ids, next) =
        server_projectile_ids(bullets, owner.id, 1, cursors.get(&owner.id).copied(), 1)
            .context("normal NPC projectile live ID capacity exhausted")?;
    let damage = combat::continuous_damage(
        combat::DamageSource::Projectile(proj),
        combat::AttackScaling::Unscaled,
        rng.gen_range(0.0..1.0),
    )?;
    let b = Bullet {
        flight: Flight::Normal {
            projectile: proj.clone(),
            angle: f64::from(angle),
        },
        owner: owner.id,
        id: ids[0],
        origin: (owner.x, owner.y),
        age: 0.,
        amplitude: proj.amplitude,
        frequency: proj.frequency,
        x: owner.x,
        y: owner.y,
        vx: angle.cos() * proj.speed,
        vy: angle.sin() * proj.speed,
        damage,
        life: proj.lifetime,
    };
    let output = shot(&b, owner.kind, angle);
    bullets.push(b);
    cursors.insert(owner.id, next);
    Ok(output)
}
fn shore_target(
    players: &BTreeMap<i32, Avatar>,
    enemy: &Entity,
    home: (f32, f32),
) -> Result<Option<(i32, f32, f32)>> {
    use super::content::Condition;
    let mut nearest: Option<(f32, i32, f32, f32)> = None;
    for (&id, avatar) in players {
        let p = &avatar.character;
        if !p.alive {
            continue;
        }
        let conditions = p.resource_conditions()?;
        ensure!(
            [p.x, p.y].into_iter().all(f32::is_finite),
            "nonfinite shore target position for avatar {id}"
        );
        if conditions.contains(Condition::Paused) || conditions.contains(Condition::Invisible) {
            continue;
        }
        let d = distance(enemy.x, enemy.y, p.x, p.y);
        if d < 10.
            && distance(home.0, home.1, p.x, p.y) <= 12.
            && nearest
                .is_none_or(|(old, old_id, _, _)| d.total_cmp(&old).then(id.cmp(&old_id)).is_lt())
        {
            nearest = Some((d, id, p.x, p.y));
        }
    }
    Ok(nearest.map(|(_, id, x, y)| (id, x, y)))
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
fn healnova_recipient(alive: bool, conditions: combat::ConditionMask) -> bool {
    alive && !conditions.contains(super::content::Condition::Paused)
}

fn distance(x: f32, y: f32, xx: f32, yy: f32) -> f32 {
    (x - xx).hypot(y - yy)
}
fn projectile_cell_entry(from: (f32, f32), to: (f32, f32), cell: (f32, f32)) -> Option<f32> {
    let (mut enter, mut exit) = (0_f32, 1_f32);
    for (start, end, min) in [(from.0, to.0, cell.0), (from.1, to.1, cell.1)] {
        let delta = end - start;
        if delta == 0. {
            if !(min..min + 1.).contains(&start) {
                return None;
            }
        } else {
            let a = (min - start) / delta;
            let b = (min + 1. - start) / delta;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
        }
    }
    (enter < exit || (to.0.floor(), to.1.floor()) == cell).then_some(enter)
}
fn projectile_target_entry(
    from: (f32, f32),
    to: (f32, f32),
    center: (f32, f32),
    radius: f32,
) -> Option<f32> {
    let offset = (from.0 - center.0, from.1 - center.1);
    let c = offset.0 * offset.0 + offset.1 * offset.1 - radius * radius;
    if c < 0. {
        return Some(0.);
    }
    let delta = (to.0 - from.0, to.1 - from.1);
    let a = delta.0 * delta.0 + delta.1 * delta.1;
    let b = offset.0 * delta.0 + offset.1 * delta.1;
    let discriminant = b * b - a * c;
    if a == 0. || discriminant <= 0. {
        return None;
    }
    let enter = (-b - discriminant.sqrt()) / a;
    (0. ..=1.).contains(&enter).then_some(enter)
}
fn projectile_cover_entry(
    entities: &BTreeMap<i32, Entity>,
    content: &Content,
    bullet: &Bullet,
    from: (f32, f32),
) -> Result<Option<f32>> {
    let mut first: Option<f32> = None;
    for e in entities.values() {
        // Tutorial guns occupy their own launch cell; only other objects can stop their shots.
        if e.id != bullet.owner
            && content.get(e.kind)?.is_static
            && content.get(e.kind)?.occupy_square
            && let Some(enter) =
                projectile_cell_entry(from, (bullet.x, bullet.y), (e.x.floor(), e.y.floor()))
        {
            first = Some(first.map_or(enter, |old| old.min(enter)));
        }
    }
    Ok(first)
}
pub fn message(text: &str) -> Packet {
    packet(7, |w| {
        w.utf("*Help*").i32(-1).utf(text);
    })
}
impl Room {
    pub fn from_map(map: Arc<Map>, content: &Content, base: i32) -> Result<Self> {
        ensure!(
            base >= 1_000_000 && base + 500_000 < 1 << 24,
            "map entity partition outside 24-bit owner IDs"
        );
        let mut room = Self::new(map.seed, base, 0);
        room.id_limit = base + 500_000;
        for placement in map.objects() {
            let def = content.get(placement.kind)?;
            let mut stats = def.stats.clone();
            if let Some(&hp) = stats.get(&0) {
                stats.insert(1, hp);
            }
            if placement.kind == 0x70e {
                stats.insert(23, 0);
            }
            let id = room.allocate_entity()?;
            let (x, y) = placement.position();
            room.entities.insert(
                id,
                Entity {
                    id,
                    kind: placement.kind,
                    provenance: Provenance::Map,
                    x,
                    y,
                    stats,
                    age: 0.,
                },
            );
        }
        tracing::info!(
            seed = map.seed,
            map = map.kind.name(),
            tiles = map.tiles().len(),
            objects = map.objects().len(),
            entity_base = base,
            terrain = map
                .tile(map.spawn.0.floor() as i32, map.spawn.1.floor() as i32)?
                .region
                .terrain(),
            "build100 immutable reconstructed map ready"
        );
        if map.kind == Kind::Realm {
            // These authored preview encounters do not implement generic XML NoSpawn/Behavior semantics.
            let pirate = content.named("Pirate")?;
            ensure!(
                pirate.id == 0x600 && pirate.class == "Character" && pirate.is_enemy,
                "normal shore requires original Pirate0x600 enemy metadata"
            );
            progression::enemy_xp(pirate)?;
            pirate.normal_projectile(0)?;
            let mut sites = vec![];
            let (width, _) = map.dimensions();
            for (index, tile) in map.tiles().iter().enumerate() {
                let (x, y) = (index % usize::from(width), index / usize::from(width));
                let site = (x as f32 + 0.5, y as f32 + 0.5);
                if tile.region == Region::ShoreSand
                    && (6. ..=12.).contains(&distance(site.0, site.1, map.spawn.0, map.spawn.1))
                    && map.walkable(x as i32, y as i32)
                    && map.ground(x as i32, y as i32)?.damage.is_none()
                {
                    sites.push(site);
                }
            }
            // Placement has its own seeded ordering; construction must not consume combat/loot draws.
            sites.shuffle(&mut StdRng::seed_from_u64(map.seed));
            let mut homes: Vec<(f32, f32)> = vec![];
            for &site in &sites {
                if homes
                    .iter()
                    .all(|home| distance(site.0, site.1, home.0, home.1) >= 2.5)
                {
                    homes.push(site);
                }
                if homes.len() == 4 {
                    break;
                }
            }
            ensure!(
                homes.len() == 4,
                "normal shore seed={} spawn={:?}: need four separated static-valid ShoreSand sites in6..12, found {}",
                map.seed,
                map.spawn,
                homes.len()
            );
            room.shore = Some(ShorePopulation {
                sites,
                actors: BTreeMap::new(),
                next_replenish: 0.,
            });
        }
        room.map = Some(map);
        room.replenish_shore(content)?;
        Ok(room)
    }
    pub fn entity_partition(&self) -> i32 {
        self.id_limit - 500_000
    }
    pub fn landing(&self) -> (f32, f32) {
        self.map.as_ref().map_or((16.5, 16.5), |m| m.spawn)
    }
    pub fn description(&self, world: i32) -> (i32, i32, String) {
        self.map.as_ref().map_or(
            (32, 32, format!("Shared Fixture {world} (not Tutorial)")),
            |m| {
                let (w, h) = m.dimensions();
                (i32::from(w), i32::from(h), m.kind.name().into())
            },
        )
    }
    fn repairable_enemy(&self, viewer: i32, target: i32, content: &Content) -> Result<bool> {
        let avatar = &self.players[&viewer];
        let p = &avatar.character;
        if self.map.is_none() || !p.alive || !avatar.known_objects.contains(&target) {
            return Ok(false);
        }
        let Some(e) = self.entities.get(&target) else {
            return Ok(false);
        };
        let def = content.get(e.kind)?;
        Ok(def.is_enemy
            && def.class == "Character"
            && e.stats.get(&1).is_some_and(|&hp| hp > 0)
            && distance(p.x, p.y, e.x, e.y) <= 16.)
    }
    // Reconcile before events so neither Shoot nor Tick can refer to an unpublished owner.
    pub fn publication(
        &mut self,
        viewer: i32,
        events: &[Packet],
        content: &Content,
    ) -> Result<Vec<Packet>> {
        let map = self.map.as_ref().context("publication requires map")?;
        let p = self.character(viewer);
        let (x, y) = (p.x, p.y);
        // Native Tick replies are Moves, so reconciliation alone must not create another Tick.
        let publish_tick = events.iter().any(|p| matches!(p.id, 1 | 9 | 13))
            || (!p.alive && self.players[&viewer].known_objects.contains(&viewer));
        let (w, h) = map.dimensions();
        let mut ground = vec![];
        for yy in ((y.floor() as i32 - 16).max(0))..=(y.floor() as i32 + 16).min(i32::from(h) - 1) {
            for xx in
                ((x.floor() as i32 - 16).max(0))..=(x.floor() as i32 + 16).min(i32::from(w) - 1)
            {
                ground.push(((xx as i16, yy as i16), map.ground(xx, yy)?.id));
            }
        }
        let mut visible = BTreeMap::new();
        let mut player_stats = BTreeMap::new();
        for (&id, a) in &self.players {
            let p = &a.character;
            if (p.alive || (id == viewer && self.players[&viewer].known_objects.contains(&id)))
                && distance(x, y, p.x, p.y) <= 16.
            {
                let stats = self.wire_stats(p, content)?;
                visible.insert(id, make(id, p.kind, &p.name, p.x, p.y, &stats));
                player_stats.insert(id, stats);
            }
        }
        let entity_stats = self
            .entities
            .values()
            .map(|e| Ok((e.id, self.entity_wire_stats(e, content)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for e in self.entities.values() {
            if distance(x, y, e.x, e.y) <= 16. {
                visible.insert(e.id, make(e.id, e.kind, "", e.x, e.y, &entity_stats[&e.id]));
            }
        }
        // A known owner can shoot and retire in this frame; its shot must precede its actual Drop.
        let mut removed_owners = BTreeSet::new();
        for event in events.iter().filter(|p| p.id == 11) {
            let mut r = Reader::new(&event.body);
            let id = r.i32()?;
            r.done()?;
            if !self.players.contains_key(&id) && !self.entities.contains_key(&id) {
                removed_owners.insert(id);
            }
        }
        let repair = match self.players[&viewer].enemy_view_repair {
            Some(id) if self.repairable_enemy(viewer, id, content)? => Some(id),
            _ => None,
        };
        let avatar = self.players.get_mut(&viewer).context("viewer missing")?;
        avatar.enemy_view_repair = None;
        let mut out: Vec<_> = events
            .iter()
            .filter(|p| p.id == 1)
            .map(|p| Packet {
                id: p.id,
                body: p.body.clone(),
            })
            .collect();
        for ((xx, yy), kind) in ground {
            if avatar.known_ground.insert((xx, yy)) {
                out.push(packet(12, |w| {
                    w.i16(xx).i16(yy).u8(0).u8(kind);
                }));
            }
        }
        if let Some(id) = repair {
            // Positive HP Tick and duplicate Make cannot clear the native instance's sticky dead flag.
            out.push(drop_object(id));
            avatar.known_objects.remove(&id);
            tracing::debug!(
                viewer,
                target = id,
                "build100 enemy view repair; not authoritative resurrection"
            );
        }
        for (&id, p) in &visible {
            if !avatar.known_objects.contains(&id) {
                out.push(Packet {
                    id: p.id,
                    body: p.body.clone(),
                });
            }
        }
        for p in events {
            let mut r = Reader::new(&p.body);
            let include = match p.id {
                1 | 10 | 11 | 12 | 13 => false,
                8 => {
                    r.u8()?;
                    let owner = r.i32()?;
                    visible.contains_key(&owner)
                        || (avatar.known_objects.contains(&owner)
                            && removed_owners.contains(&owner))
                }
                18 => {
                    let owner = r.i32()?;
                    r.f32()?;
                    r.f32()?;
                    r.done()?;
                    visible.contains_key(&owner)
                }
                9 => {
                    let target = r.i32()?;
                    r.u8()?;
                    r.u8()?;
                    r.i16()?;
                    r.u8()?;
                    let owner = r.i32()?;
                    // Player.update already renders local ground damage; only authoritative HP is echoed.
                    visible.contains_key(&target) && !(target == viewer && owner == -1)
                }
                _ => true,
            };
            if include {
                out.push(Packet {
                    id: p.id,
                    body: p.body.clone(),
                });
            }
        }
        let old = std::mem::replace(&mut avatar.known_objects, visible.keys().copied().collect());
        if publish_tick {
            out.push(packet(13, |w| {
                w.i16(visible.len() as i16);
                for &id in visible.keys() {
                    if let Some(a) = self.players.get(&id) {
                        let p = &a.character;
                        status(w, id, p.x, p.y, &player_stats[&id]);
                    } else {
                        let e = &self.entities[&id];
                        status(w, id, e.x, e.y, &entity_stats[&id]);
                    }
                }
            }));
        }
        for id in old {
            if !visible.contains_key(&id) {
                out.push(drop_object(id));
            }
        }
        if !self.character(viewer).alive && visible.contains_key(&viewer) {
            out.push(drop_object(viewer));
            self.players
                .get_mut(&viewer)
                .expect("viewer")
                .known_objects
                .remove(&viewer);
        }
        Ok(out)
    }
    fn teaching_objects(&mut self, content: &Content) -> Result<()> {
        if self.map.as_ref().expect("normal map").kind != Kind::Tutorial
            || self.elapsed < self.next_gift
        {
            return Ok(());
        }
        self.next_gift = self.elapsed + 1.;
        if !self.entities.values().any(|e| e.kind == 0x806) {
            let generator = self
                .entities
                .values()
                .find(|e| e.kind == 0x807)
                .context("tutorial generator missing")?;
            let source = generator.id;
            let (x, y) = (generator.x, generator.y + 1.);
            ensure!(
                self.map
                    .as_ref()
                    .expect("normal map")
                    .walkable(x.floor() as i32, y.floor() as i32),
                "tutorial bag landing blocked"
            );
            let def = content.named("Tutorial Loot Bag")?;
            ensure!(
                def.equipment.len() == 8,
                "tutorial bag must contain eight embedded slots"
            );
            let mut stats = def.stats.clone();
            for (slot, &item) in def.equipment.iter().enumerate() {
                stats.insert(8 + slot as u8, item);
            }
            let id = self.allocate_entity()?;
            self.entities.insert(
                id,
                Entity {
                    id,
                    kind: def.id,
                    x,
                    y,
                    stats,
                    age: 0.,
                    provenance: Provenance::Summoned { source },
                },
            );
            tracing::debug!(
                bag = id,
                "build100 tutorial child bag generated; embedded teaching gift, no completion payout"
            );
        }
        let count = self.entities.values().filter(|e| e.kind == 0x801).count();
        if count < 2 {
            let hole = self
                .entities
                .values()
                .find(|e| e.kind == 0x800)
                .context("tutorial chicken hole missing")?;
            let source = hole.id;
            let (x, y) = (hole.x - 1., hole.y);
            let def = content.named("Evil Chicken")?;
            let mut stats = def.stats.clone();
            stats.insert(1, stats[&0]);
            let id = self.allocate_entity()?;
            self.entities.insert(
                id,
                Entity {
                    id,
                    kind: def.id,
                    x,
                    y,
                    stats,
                    age: 0.,
                    provenance: Provenance::Summoned { source },
                },
            );
        }
        Ok(())
    }
    fn hazards(&mut self, out: &mut Vec<Packet>) -> Result<()> {
        let map = self.map.as_ref().context("hazard map missing")?;
        for (&id, avatar) in &mut self.players {
            let p = &mut avatar.character;
            while p.alive && self.elapsed - avatar.hazard_at >= 0.5 {
                avatar.hazard_at += 0.5;
                if let Some((min, max)) = map.ground(p.x.floor() as i32, p.y.floor() as i32)?.damage
                {
                    let damage = self.rng.gen_range(min..=max);
                    let hp = (p.stats[&1] - damage).max(0);
                    p.stats.insert(1, hp);
                    out.push(damage_packet(id, -1, 0, damage));
                    if hp == 0 {
                        p.alive = false;
                        p.clear_dead_hp_carry();
                        avatar.weapon.pending = None;
                        avatar.quiver_ack = None;
                        tracing::info!(
                            avatar = id,
                            generation = p.generation,
                            "build100 ground death pending commit"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fixture_collision(&mut self, lethal: bool) {
        self.entities.retain(|_, e| e.kind != 0x500);
        let enemy = self.entities.values_mut().next().expect("fixture pirate");
        enemy.x = 18.;
        enemy.y = 16.5;
        enemy.stats.insert(1, 5);
        let enemy_id = enemy.id;
        let players: Vec<_> = self.players.keys().copied().collect();
        self.bullets.clear();
        if lethal {
            self.character_mut(players[0]).stats.insert(1, 5);
            for id in 0..2 {
                self.bullets.push(Bullet {
                    flight: Flight::Arena,
                    owner: enemy_id,
                    id,
                    origin: (16.5, 16.5),
                    age: 0.,
                    amplitude: 0.,
                    frequency: 1.,
                    x: 16.5,
                    y: 16.5,
                    vx: 0.,
                    vy: 0.,
                    damage: 5,
                    life: 1.,
                });
            }
        }
        for &owner in &players {
            self.bullets.push(Bullet {
                flight: Flight::Arena,
                owner,
                id: 255,
                origin: (18., 16.5),
                age: 0.,
                amplitude: 0.,
                frequency: 1.,
                x: 18.,
                y: 16.5,
                vx: 0.,
                vy: 0.,
                damage: 100,
                life: 1.,
            });
        }
        self.next_wave = 999.;
    }
    #[cfg(test)]
    pub(crate) fn fixture_bullets(&self) -> Vec<(i32, u8)> {
        self.bullets.iter().map(|b| (b.owner, b.id)).collect()
    }
    pub fn new(seed: u64, base: i32, destination: i32) -> Self {
        Self {
            players: BTreeMap::new(),
            entities: BTreeMap::new(),
            bullets: vec![],
            server_projectile_cursors: BTreeMap::new(),
            enemy_slows: BTreeMap::new(),
            shore: None,
            rng: StdRng::seed_from_u64(seed),
            next_id: base + 100,
            id_limit: base + 1_000_000,
            elapsed: 0.,
            map: None,
            next_gift: 1.,
            shot_clock: 0.,
            next_wave: 0.,
            next_tick: 0.2,
            portal: base + 99,
            destination,
        }
    }
    pub fn character(&self, id: i32) -> &Character {
        &self.players[&id].character
    }
    pub fn character_mut(&mut self, id: i32) -> &mut Character {
        &mut self.players.get_mut(&id).unwrap().character
    }
    pub fn attach(
        &mut self,
        id: i32,
        mut character: Character,
        content: &Content,
    ) -> Result<Vec<Packet>> {
        ensure!(!self.players.contains_key(&id), "avatar already attached");
        if self.map.is_some() {
            let effective = character.equipment_stats(content)?;
            character
                .progress(content)
                .context("invalid saved normal progression")?;
            let legacy_actions = character.action_state.is_none();
            if legacy_actions {
                ensure!(
                    character.stats.get(&25) == Some(&0),
                    "legacy action state requires zero stat25"
                );
                character.action_state = Some(ActionState::default());
            }
            let legacy = character.resources_version == 0 && character.resources_carry.is_none();
            if legacy {
                character.resources_version = 1;
                character.resources_carry = Some(ResourceCarry::ZERO);
            }
            character.vitals(&effective)?;
            if legacy_actions {
                tracing::info!(
                    avatar = id,
                    generation = character.generation,
                    "build100 known pre-cloak action state migrated to version1 empty timers; pending attach commit"
                );
            }
            if legacy {
                tracing::info!(
                    avatar = id,
                    generation = character.generation,
                    "build100 known pre-regeneration resources migrated to version1 zero carry; pending attach commit"
                );
            }
        }
        self.players.insert(
            id,
            Avatar {
                character,
                move_at: Instant::now(),
                teleport_ready: None,
                weapon: WeaponAdmission::default(),
                hazard_at: self.elapsed,
                move_credit: MOVE_DISTANCE_CREDIT,
                known_ground: BTreeSet::new(),
                known_objects: BTreeSet::new(),
                enemy_view_repair: None,
                quiver_ack: None,
            },
        );
        if self.map.is_some() {
            return Ok(vec![packet(1, |w| {
                w.i32(id);
            })]);
        }
        if self.entities.is_empty() && self.next_wave == 0. {
            self.spawn(content, &mut vec![])?;
        }
        let mut out = vec![packet(1, |w| {
            w.i32(id);
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
        for (&id, avatar) in &self.players {
            let p = &avatar.character;
            if p.alive {
                out.push(make(id, p.kind, &p.name, p.x, p.y, &p.stats));
            }
        }
        for e in self.entities.values() {
            out.push(make(e.id, e.kind, "", e.x, e.y, &e.stats));
        }
        // A registered fixture portal exercises native stat23 teardown/rejoin, not earned dungeon travel.
        out.push(make(
            self.portal,
            0x700,
            "Fixture passage",
            16.5,
            18.,
            &BTreeMap::from([(23, self.destination)]),
        ));
        out.push(message(
            "Explicit shared compatibility fixture; NOT Tutorial or a restored 2010 realm.",
        ));
        out.push(self.tick(content)?);
        Ok(out)
    }
    fn wire_stats(&self, p: &Character, content: &Content) -> Result<BTreeMap<u8, i32>> {
        let mut stats = p.stats.clone();
        if self.map.is_some() {
            let effective = p.equipment_stats(content)?;
            p.vitals(&effective)?;
            ensure!(
                !stats.contains_key(&29),
                "normal durable stats contain wire condition alias stat29"
            );
            // Original Gs._C8 dispatches wire29 to conditions; durable25 is server bookkeeping.
            let conditions = stats
                .remove(&25)
                .context("avatar missing condition mask stat25")?;
            stats.insert(29, conditions);
            stats.extend(effective.iter());
        }
        Ok(stats)
    }
    pub fn arrival(&self, id: i32, content: &Content) -> Result<Packet> {
        let p = self.character(id);
        Ok(make(
            id,
            p.kind,
            &p.name,
            p.x,
            p.y,
            &self.wire_stats(p, content)?,
        ))
    }
    pub fn detach(&mut self, id: i32) -> Vec<Packet> {
        self.players.remove(&id);
        self.server_projectile_cursors.remove(&id);
        self.bullets.retain(|b| b.owner != id);
        vec![drop_object(id)]
    }
    pub fn reachable_portal(&self, id: i32) -> Option<(i32, i32)> {
        let p = self.character(id);
        if self.map.is_some() {
            return self
                .entities
                .values()
                .find(|e| e.kind == 0x70e && p.alive && distance(p.x, p.y, e.x, e.y) <= 2.)
                .map(|e| (e.id, 0));
        }
        (p.alive && distance(p.x, p.y, 16.5, 18.) <= 2.).then_some((self.portal, self.destination))
    }
    fn allocate_entity(&mut self) -> Result<i32> {
        let id = self
            .next_id
            .checked_add(1)
            .context("build100 entity ID exhausted")?;
        ensure!(
            id < self.id_limit,
            "build100 world entity ID partition exhausted"
        );
        self.next_id = id;
        Ok(id)
    }
    fn spawn(&mut self, content: &Content, out: &mut Vec<Packet>) -> Result<()> {
        let pirate = content.named("Pirate")?;
        for (x, y) in [(21., 16.), (22., 20.), (19., 22.)] {
            let mut stats = pirate.stats.clone();
            let hp = *stats.get(&0).context("Pirate has no HP")?;
            stats.insert(1, hp);
            let id = self.allocate_entity()?;
            let e = Entity {
                id,
                kind: pirate.id,
                provenance: Provenance::Fixture,
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
    fn entity_wire_stats(&self, e: &Entity, content: &Content) -> Result<BTreeMap<u8, i32>> {
        let mut stats = e.stats.clone();
        let def = content.get(e.kind)?;
        if def.is_enemy && def.class == "Character" {
            stats.insert(
                29,
                if self
                    .enemy_slows
                    .get(&e.id)
                    .is_some_and(|remaining| *remaining > 0.)
                {
                    8
                } else {
                    0
                },
            );
        }
        Ok(stats)
    }
    fn tick(&self, content: &Content) -> Result<Packet> {
        let entity_stats = self
            .entities
            .values()
            .map(|e| Ok((e.id, self.entity_wire_stats(e, content)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let stats = self
            .players
            .iter()
            .filter(|(_, a)| a.character.alive)
            .map(|(&id, a)| Ok((id, self.wire_stats(&a.character, content)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        Ok(packet(13, |w| {
            w.i16(
                (self.players.values().filter(|a| a.character.alive).count() + self.entities.len())
                    as i16,
            );
            for (&id, avatar) in &self.players {
                let p = &avatar.character;
                if p.alive {
                    status(w, id, p.x, p.y, &stats[&id]);
                }
            }
            for e in self.entities.values() {
                status(w, e.id, e.x, e.y, &entity_stats[&e.id]);
            }
        }))
    }
    fn normal_weapon_shot(
        &mut self,
        player: i32,
        input: WeaponShot,
        weapon: &super::content::Object,
        content: &Content,
        received: Instant,
        now: Instant,
    ) -> Result<Vec<Packet>> {
        let WeaponShot { id, origin, angle } = input;
        ensure!(weapon.num_projectiles > 0, "empty weapon burst");
        ensure!(
            (1..=96).contains(&id),
            "normal native bullet ID must be in 1..=96"
        );
        let avatar = &self.players[&player];
        let effective = avatar.character.equipment_stats(content)?;
        let conditions = avatar.character.resource_conditions()?;
        let equipment = std::array::from_fn(|i| avatar.character.stats[&(8 + i as u8)]);
        let mut admission = avatar.weapon.clone();
        ensure!(
            admission.last_received.is_none_or(|last| received >= last),
            "weapon receipt clock moved backwards"
        );
        let elapsed_ms = admission
            .pending
            .as_ref()
            .map(|p| received.duration_since(p.first_received).as_secs_f64() * 1000.0);
        let expired = admission
            .pending
            .as_ref()
            .is_some_and(|p| p.shots.len() < usize::from(p.weapon.num_projectiles))
            && elapsed_ms.is_some_and(|ms| ms >= admission.interval_ms);
        tracing::debug!(
            avatar = player,
            bullet = id,
            item = weapon.id,
            dexterity = ?effective.get(28),
            attack = ?effective.get(20),
            rate_of_fire = weapon.rate_of_fire,
            conditions = conditions.bits(),
            expected_count = weapon.num_projectiles,
            previous_interval_ms = admission.interval_ms,
            elapsed_ms,
            last_received_delta_ms = admission.last_received.map(|last| received.duration_since(last).as_secs_f64()*1000.0),
            pending_count = admission.pending.as_ref().map(|p|p.shots.len()),
            pending_first = admission.pending.as_ref().and_then(|p|p.shots.first().map(|s|s.0)),
            expired,
            received_age_ms = received.elapsed().as_secs_f64()*1000.0,
            "build100 weapon admission state"
        );
        if expired {
            tracing::warn!(
                avatar = player,
                bullet = id,
                item = weapon.id,
                first_bullet = admission
                    .pending
                    .as_ref()
                    .and_then(|p| p.shots.first().map(|s| s.0)),
                interval_ms = admission.interval_ms,
                elapsed_ms,
                received_count = admission.pending.as_ref().expect("pending").shots.len(),
                expected_count = admission
                    .pending
                    .as_ref()
                    .expect("pending")
                    .weapon
                    .num_projectiles,
                "build100 incomplete weapon burst expired; discard on next valid admission, no truncated shots"
            );
            admission.pending = None;
        }
        if admission.pending.is_none() {
            ensure!(
                !conditions.contains(super::content::Condition::Stunned)
                    && !conditions.contains(super::content::Condition::Paused),
                "weapon condition rejected burst"
            );
            let interval_ms = combat::weapon_interval_ms(weapon, effective.get(28)?, conditions)?;
            let due = match admission.started {
                Some(anchor) => anchor
                    .checked_add(std::time::Duration::from_millis(interval_ms as u64))
                    .context("weapon deadline overflow")?
                    .max(now),
                None => now,
            };
            tracing::debug!(
                avatar = player,
                bullet = id,
                item = weapon.id,
                interval_ms,
                elapsed_ms,
                deficit_ms = elapsed_ms.map(|elapsed| interval_ms - elapsed),
                stunned = conditions.contains(super::content::Condition::Stunned),
                paused = conditions.contains(super::content::Condition::Paused),
                due=?due, admitted=?now,
                "build100 weapon reservation decision"
            );

            ensure!(
                weapon.root_damage.is_some(),
                "{} missing root damage",
                weapon.name
            );
            admission.interval_ms = interval_ms;
            admission.started = Some(due);
            admission.pending = Some(PendingWeaponBurst {
                first_received: received,
                due,
                weapon: weapon.clone(),
                equipment,
                attack: effective.get(20)?,
                conditions,
                origin,
                shots: Vec::with_capacity(usize::from(weapon.num_projectiles)),
            });
        }
        let pending = admission.pending.as_mut().expect("admitted burst");
        ensure!(
            pending.shots.len() < usize::from(pending.weapon.num_projectiles),
            "complete weapon burst awaiting owner release"
        );
        ensure!(
            pending.weapon.id == weapon.id && pending.equipment == equipment,
            "weapon burst equipment changed"
        );
        ensure!(pending.origin == origin, "weapon burst origin changed");
        ensure!(
            !pending.shots.iter().any(|&(old, _)| old == id),
            "duplicate weapon burst bullet ID"
        );
        if let Some(&(previous, _)) = pending.shots.last() {
            ensure!(
                id == previous % 96 + 1,
                "normal burst bullet ID must follow native modulo96 order"
            );
        }
        let count = usize::from(pending.weapon.num_projectiles);
        ensure!(
            self.bullets.len() + count <= 4096
                && self.bullets.iter().filter(|b| b.owner == player).count() + count <= 256,
            "weapon burst capacity exceeded"
        );
        pending.shots.push((id, angle));
        combat::weapon_burst_angles(
            &pending.weapon,
            &pending.shots.iter().map(|&(_, a)| a).collect::<Vec<_>>(),
        )?;
        admission.last_received = Some(received);
        if pending.shots.len() < count || now < pending.due {
            tracing::debug!(
                avatar = player,
                bullet = id,
                item = weapon.id,
                first_bullet = pending.shots.first().map(|s| s.0),
                received_count = pending.shots.len(),
                expected_count = count,
                "build100 weapon buffered; pending commit"
            );
            self.players.get_mut(&player).expect("attached").weapon = admission;
            return Ok(vec![]);
        }
        self.fund_weapon(player, admission, content, now)
    }
    pub(crate) fn weapon_deadline(&self, player: i32) -> Option<Instant> {
        self.players
            .get(&player)?
            .weapon
            .pending
            .as_ref()
            .filter(|p| p.shots.len() == usize::from(p.weapon.num_projectiles))
            .map(|p| p.due)
    }
    pub(crate) fn cancel_weapon(&mut self, player: i32) {
        if let Some(avatar) = self.players.get_mut(&player) {
            avatar.weapon.pending = None;
        }
    }
    pub(crate) fn release_weapon(
        &mut self,
        player: i32,
        content: &Content,
        now: Instant,
    ) -> Result<Vec<Packet>> {
        let admission = self
            .players
            .get(&player)
            .context("weapon owner missing")?
            .weapon
            .clone();
        self.fund_weapon(player, admission, content, now)
    }
    fn fund_weapon(
        &mut self,
        player: i32,
        mut admission: WeaponAdmission,
        content: &Content,
        now: Instant,
    ) -> Result<Vec<Packet>> {
        let avatar = self.players.get(&player).context("weapon owner missing")?;
        ensure!(avatar.character.alive, "weapon owner permanently dead");
        let pending = admission
            .pending
            .as_ref()
            .context("weapon pending burst missing")?;
        let count = usize::from(pending.weapon.num_projectiles);
        ensure!(
            pending.shots.len() == count && now >= pending.due,
            "weapon burst not eligible for funding"
        );
        let equipment = std::array::from_fn(|i| avatar.character.stats[&(8 + i as u8)]);
        ensure!(
            equipment == pending.equipment,
            "weapon burst equipment changed before funding"
        );
        let conditions = avatar.character.resource_conditions()?;
        ensure!(
            !conditions.contains(super::content::Condition::Stunned)
                && !conditions.contains(super::content::Condition::Paused),
            "weapon condition rejected funding"
        );
        let current = content.get(pending.weapon.id)?;
        current.normal_projectile(0)?;
        combat::weapon_interval_ms(
            current,
            avatar.character.equipment_stats(content)?.get(28)?,
            conditions,
        )?;
        ensure!(
            current.root_damage.is_some(),
            "weapon missing root damage before funding"
        );
        ensure!(
            self.bullets.len() + count <= 4096
                && self.bullets.iter().filter(|b| b.owner == player).count() + count <= 256,
            "weapon burst capacity exceeded"
        );
        combat::weapon_burst_angles(
            &pending.weapon,
            &pending.shots.iter().map(|s| s.1).collect::<Vec<_>>(),
        )?;
        let origin = pending.origin;
        ensure!(
            origin.0.is_finite()
                && origin.1.is_finite()
                && distance(origin.0, origin.1, avatar.character.x, avatar.character.y) < 2.,
            "weapon origin invalid before funding"
        );
        let proj = pending.weapon.normal_projectile(0)?;
        ensure!(
            pending
                .shots
                .iter()
                .all(|&(id, _)| !self.bullets.iter().any(|b| b.owner == player && b.id == id)),
            "duplicate live weapon burst bullet ID"
        );
        let mut rng = self.rng.clone();
        let mut bullets = Vec::with_capacity(count);
        let mut output = Vec::with_capacity(count);
        for &(id, angle) in &pending.shots {
            let damage = combat::continuous_damage(
                combat::DamageSource::Root(&pending.weapon),
                combat::AttackScaling::Weapon {
                    attack: pending.attack,
                    conditions: pending.conditions,
                },
                rng.gen_range(0.0..1.0),
            )?;
            let b = Bullet {
                flight: Flight::Normal {
                    projectile: proj.clone(),
                    angle: f64::from(angle),
                },
                owner: player,
                id,
                origin,
                age: 0.0,
                amplitude: proj.amplitude,
                frequency: proj.frequency,
                x: origin.0,
                y: origin.1,
                vx: angle.cos() * proj.speed,
                vy: angle.sin() * proj.speed,
                damage,
                life: proj.lifetime,
            };
            output.push(shot(&b, pending.weapon.id, angle));
            bullets.push(b);
        }
        tracing::debug!(avatar=player, due=?pending.due, funded=?now,
            lateness_ms=now.duration_since(pending.due).as_secs_f64()*1000.0,
            first_received=?pending.first_received,"build100 complete weapon funded at actual owner time");
        admission.started = Some(now);
        admission.pending = None;
        self.rng = rng;
        self.bullets.extend(bullets);
        tracing::debug!(
            avatar = player,
            weapon = equipment[0],
            bullet = output.last().and_then(|p| p.body.first()).copied(),
            first_bullet = output.first().and_then(|p| p.body.first()).copied(),
            projectiles = count,
            "build100 ordinary weapon burst complete; pending commit"
        );
        self.players.get_mut(&player).expect("attached").weapon = admission;
        Ok(output)
    }
    fn teleport(
        &mut self,
        player: i32,
        target: i32,
        content: &Content,
        now: Instant,
    ) -> Result<Vec<Packet>> {
        let caster = self.character(player);
        if self.map.is_some() {
            caster.vitals(&caster.equipment_stats(content)?)?;
        }
        if !caster.alive {
            return Err(TeleportDecline::Dead.into());
        }
        teleport_unpaused(combat::ConditionMask::from_bits(caster.stats[&25] as u32)?)?;
        if self.players[&player]
            .teleport_ready
            .is_some_and(|ready| now < ready)
        {
            return Err(TeleportDecline::Cooldown.into());
        }
        let Some(target) = self.players.get(&target).filter(|a| a.character.alive) else {
            return Err(TeleportDecline::TargetUnavailable.into());
        };
        let target = &target.character;
        if self.map.is_some() {
            target.vitals(&target.equipment_stats(content)?)?;
        }
        teleport_unpaused(combat::ConditionMask::from_bits(target.stats[&25] as u32)?)?;
        let (x, y) = (target.x, target.y);
        ensure!(
            x.is_finite() && y.is_finite(),
            "nonfinite authoritative teleport destination"
        );
        if let Some(map) = &self.map {
            let (w, h) = map.dimensions();
            ensure!(
                x >= 0.
                    && y >= 0.
                    && x < f32::from(w)
                    && y < f32::from(h)
                    && map.walkable(x.floor() as i32, y.floor() as i32),
                "illegal authoritative teleport destination"
            );
        } else {
            ensure!(
                (2.0..30.0).contains(&x) && (2.0..30.0).contains(&y),
                "teleport destination outside arena"
            );
        }
        let avatar = self.players.get_mut(&player).expect("attached caster");
        avatar.character.x = x;
        avatar.character.y = y;
        avatar.quiver_ack = None;
        // A warp must not bank idle time or regrant the attach distance credit.
        avatar.move_at = now;
        avatar.teleport_ready = Some(now + std::time::Duration::from_secs(10));
        Ok(vec![packet(18, |w| {
            w.i32(player).f32(x).f32(y);
        })])
    }
    #[cfg(test)]
    pub fn handle(&mut self, player: i32, p: Packet, content: &Content) -> Result<Vec<Packet>> {
        let now = Instant::now();
        self.handle_at(player, p, content, now, now)
    }
    pub(crate) fn handle_at(
        &mut self,
        player: i32,
        p: Packet,
        content: &Content,
        received: Instant,
        now: Instant,
    ) -> Result<Vec<Packet>> {
        ensure!(self.players.contains_key(&player), "avatar not attached");
        if p.id == 28 {
            return self.teleport(player, super::protocol::teleport(&p.body)?, content, now);
        }
        if p.id == 9 {
            ensure!(
                p.body.len() == 13,
                "native Damage9 requires exactly 13 bytes"
            );
        }
        if !self.character(player).alive {
            tracing::debug!(
                id = p.id,
                "discarding in-flight input after committed death"
            );
            return Ok(vec![]);
        }
        if self.map.is_some() {
            self.character(player)
                .vitals(&self.character(player).equipment_stats(content)?)?;
        }
        let mut r = Reader::new(&p.body);
        let mut out = vec![];
        match p.id {
            5 => {
                let (x, y) = (r.f32()?, r.f32()?);
                r.done()?;
                let owner_now = now;
                let now = Instant::now();
                let dt = now
                    .duration_since(self.players[&player].move_at)
                    .as_secs_f32();
                let from = (self.character(player).x, self.character(player).y);
                let speed = if self.map.is_some() {
                    Some(combat::movement_tiles_per_second(
                        self.character(player).equipment_stats(content)?.get(22)?,
                        combat::ConditionMask::default(),
                    )? as f32)
                } else {
                    None
                };
                let _move_trace = tracing::debug_span!(
                    target: "build100_move",
                    "MOVE validation",
                    avatar = player,
                    generation = self.character(player).generation,
                    received = ?received,
                    owner_now = ?owner_now,
                    validation_now = ?now,
                    previous_move_at = ?self.players[&player].move_at,
                    effective_speed = speed,
                    dt,
                    credit_before = self.players[&player].move_credit,
                    from_x = from.0,
                    from_y = from.1,
                    requested_x = x,
                    requested_y = y
                )
                .entered();
                let (x, y) = if let Some(map) = &self.map {
                    ensure!(x.is_finite() && y.is_finite(), "nonfinite movement");
                    let (w, h) = map.dimensions();
                    ensure!(
                        x >= 0. && y >= 0. && x < f32::from(w) && y < f32::from(h),
                        "movement outside map"
                    );
                    let speed = speed.expect("normal movement speed");
                    tracing::debug!(target: "build100_move", requested_distance=distance(x,y,from.0,from.1), allowance=speed*dt+self.players[&player].move_credit, admitted=distance(x,y,from.0,from.1)<=speed*dt+self.players[&player].move_credit, "MOVE budget decision");
                    let admitted = distance(x, y, from.0, from.1)
                        <= speed * dt + self.players[&player].move_credit;
                    if !admitted {
                        return Err(MoveBudget {
                            anchor: self.players[&player].move_at,
                            validated: now,
                            speed,
                            credit: self.players[&player].move_credit,
                            requested: distance(x, y, from.0, from.1),
                        }
                        .into());
                    }
                    if map.can_move(from, (x, y)) {
                        (x, y)
                    } else if map.can_move(from, (from.0, y)) {
                        (from.0, y)
                    } else if map.can_move(from, (x, from.1)) {
                        (x, from.1)
                    } else {
                        from
                    }
                } else {
                    ensure!(
                        (2.0..30.0).contains(&x) && (2.0..30.0).contains(&y),
                        "movement outside arena"
                    );
                    tracing::debug!(target: "build100_move", effective_speed=15_f32, requested_distance=distance(x,y,from.0,from.1), allowance=15.*dt+1., admitted=distance(x,y,from.0,from.1)<=15.*dt+1., "MOVE arena budget decision");
                    ensure!(
                        distance(x, y, from.0, from.1) <= 15. * dt + 1.,
                        "movement exceeds speed limit"
                    );
                    (x, y)
                };
                if self.map.is_some() {
                    let speed = speed.expect("normal movement speed");
                    let avatar = self.players.get_mut(&player).expect("attached avatar");
                    avatar.move_credit = (avatar.move_credit + speed * dt
                        - distance(x, y, from.0, from.1))
                    .clamp(0., MOVE_DISTANCE_CREDIT);
                }
                self.character_mut(player).x = x;
                self.character_mut(player).y = y;
                self.players
                    .get_mut(&player)
                    .expect("attached avatar")
                    .move_at = now;
                tracing::debug!(target: "build100_move", applied_x=x, applied_y=y, applied_distance=distance(x,y,from.0,from.1), credit_after=self.players[&player].move_credit, accepted_move_at=?now, "MOVE applied to candidate; not committed or installed");
            }
            8 => {
                let id = r.u8()?;
                let owner = r.i32()?;
                let sub = r.u8()?;
                let kind = r.i16()?;
                let (x, y, angle) = (r.f32()?, r.f32()?, r.f32()?);
                let _claimed_damage = r.i16()?;
                ensure!(
                    x.is_finite() && y.is_finite() && angle.is_finite(),
                    "nonfinite shot origin/angle"
                );
                r.done()?;
                let quiver =
                    self.map.is_some() && i32::from(kind) == self.character(player).stats[&9];
                ensure!(
                    owner == player
                        && (quiver || i32::from(kind) == self.character(player).stats[&8]),
                    "invalid equipped weapon shot: owner={owner} sub={sub} container={kind:#x} expected={:#x}",
                    self.character(player).stats[&8]
                );
                ensure!(
                    distance(x, y, self.character(player).x, self.character(player).y) < 2.,
                    "shot origin too far from player"
                );
                ensure!(
                    self.bullets.len() < 4096
                        && self.bullets.iter().filter(|b| b.owner == player).count() < 256
                        && !self.bullets.iter().any(|b| b.owner == player && b.id == id),
                    "duplicate or excessive bullets"
                );
                let weapon = content.get(kind)?;
                if quiver {
                    ensure!(
                        sub == 10
                            && content.named("Red Arrow")?.id == i16::from(sub)
                            && angle.abs() <= std::f32::consts::PI,
                        "invalid native quiver visual/normalized angle"
                    );
                    let (w, h) = self.map.as_ref().expect("normal map").dimensions();
                    ensure!(
                        x >= 0. && y >= 0. && x < f32::from(w) && y < f32::from(h),
                        "quiver origin outside map"
                    );
                    return self.use_ability(
                        player,
                        (1, player),
                        (x, y),
                        weapon,
                        content,
                        Some(WeaponShot {
                            id,
                            origin: (x, y),
                            angle,
                        }),
                    );
                }
                let proj = if self.map.is_some() {
                    weapon.normal_projectile(0)?
                } else {
                    weapon.arena_projectile(0)?
                };
                ensure!(
                    i16::from(sub) == content.named(&proj.visual)?.id,
                    "invalid projectile visual {sub}"
                );
                if self.map.is_some() {
                    return self.normal_weapon_shot(
                        player,
                        WeaponShot {
                            id,
                            origin: (x, y),
                            angle,
                        },
                        weapon,
                        content,
                        received,
                        now,
                    );
                }
                let attack = self.character(player).stats[&20];
                let damage =
                    super::rules::attack_damage(self.rng.gen_range(proj.min..=proj.max), attack)?;
                self.bullets.push(Bullet {
                    flight: Flight::Arena,
                    owner,
                    id,
                    origin: (x, y),
                    age: 0.,
                    amplitude: proj.amplitude,
                    frequency: proj.frequency,
                    x,
                    y,
                    vx: angle.cos() * proj.speed,
                    vy: angle.sin() * proj.speed,
                    damage,
                    life: proj.lifetime,
                });
                let b = self.bullets.last().expect("shot inserted");
                out.push(shot(b, kind, angle));
                tracing::debug!(id, kind, damage, "build100 player shot");
            }
            9 => {
                // Native collision reports are advisory; the server advances and resolves its own projectiles.
                let target = r.i32()?;
                let condition = r.u8()?;
                let _duration = r.u8()?;
                let _amount = r.i16()?;
                let bullet = r.u8()?;
                let source = r.i32()?;
                r.done()?;
                if source == player
                    && (1..=96).contains(&bullet)
                    && condition == super::content::Condition::Dead as u8
                    && self.repairable_enemy(player, target, content)?
                {
                    self.players
                        .get_mut(&player)
                        .expect("attached avatar")
                        .enemy_view_repair = Some(target);
                }
            }
            14 => {
                let a = (r.i32()?, r.u8()?, r.i16()?);
                let b = (r.i32()?, r.u8()?, r.i16()?);
                r.done()?;
                ensure!(a != b, "cannot swap slot with itself");
                ensure!(
                    self.item(player, a.0, a.1)? == i32::from(a.2)
                        && self.item(player, b.0, b.1)? == i32::from(b.2),
                    "stale inventory swap"
                );
                self.can_store(player, a.0, a.1, b.2, content)?;
                self.can_store(player, b.0, b.1, a.2, content)?;
                if self.map.is_some() {
                    let mut character = self.character(player).clone();
                    let old = character.equipment_stats(content)?;
                    let vitals = character.vitals(&old)?;
                    for (id, slot, item) in [(a.0, a.1, b.2), (b.0, b.1, a.2)] {
                        if id == player {
                            character.stats.insert(8 + slot, i32::from(item));
                        }
                    }
                    let effective = character.equipment_stats(content)?;
                    character.store_vitals(vitals.rebase(&old, &effective)?);
                    self.set_item(player, a.0, a.1, b.2);
                    self.set_item(player, b.0, b.1, a.2);
                    self.players
                        .get_mut(&player)
                        .expect("attached avatar")
                        .character = character;
                } else {
                    self.set_item(player, a.0, a.1, b.2);
                    self.set_item(player, b.0, b.1, a.2);
                }
                tracing::info!(
                    from = a.0,
                    slot = a.1,
                    to = b.0,
                    to_slot = b.1,
                    item = a.2,
                    "build100 inventory swap"
                );
                self.players.get_mut(&player).expect("attached").quiver_ack = None;
                out.push(self.tick(content)?);
            }
            19 => {
                let owner = r.i32()?;
                let slot = r.u8()?;
                let kind = r.i16()?;
                r.done()?;
                ensure!(owner == player, "cannot drop another object's item");
                ensure!(
                    kind >= 0 && self.item(player, owner, slot)? == i32::from(kind),
                    "empty or stale inventory drop"
                );
                self.can_store(player, owner, slot, kind, content)?;
                let bag_def = content.named("Loot Bag")?;
                ensure!(
                    bag_def.id == 0x500
                        && bag_def.class == "Container"
                        && bag_def.slots == vec![0; 8],
                    "inventory drop requires canonical eight-slot Loot Bag"
                );
                let mut character = self.character(player).clone();
                if self.map.is_some() {
                    let old = character.equipment_stats(content)?;
                    let vitals = character.vitals(&old)?;
                    character.stats.insert(8 + slot, -1);
                    let effective = character.equipment_stats(content)?;
                    character.store_vitals(vitals.rebase(&old, &effective)?);
                } else {
                    character.stats.insert(8 + slot, -1);
                }
                let id = self.allocate_entity()?;
                let mut stats = bag_def.stats.clone();
                for bag_slot in 8..16 {
                    stats.insert(bag_slot, if bag_slot == 8 { i32::from(kind) } else { -1 });
                }
                let bag = Entity {
                    id,
                    kind: bag_def.id,
                    x: character.x,
                    y: character.y,
                    stats,
                    age: 0.,
                    provenance: Provenance::Loot { source: player },
                };
                out.push(make(id, bag.kind, "", bag.x, bag.y, &bag.stats));
                self.entities.insert(id, bag);
                self.players
                    .get_mut(&player)
                    .expect("attached avatar")
                    .character = character;
                self.players.get_mut(&player).expect("attached").quiver_ack = None;
                out.push(self.tick(content)?);
                tracing::info!(
                    world_partition = self.entity_partition(),
                    avatar = player,
                    slot,
                    item = kind,
                    bag = id,
                    "build100 inventory drop pending owner commit"
                );
            }
            15 => {
                let owner = r.i32()?;
                let slot = r.u8()?;
                let target = r.i32()?;
                let (x, y) = (r.f32()?, r.f32()?);
                r.done()?;
                ensure!(owner == player, "cannot use another object's item");
                let kind = i16::try_from(self.item(player, owner, slot)?)?;
                let item = content.get(kind)?;
                if self.map.is_some() {
                    ensure!(x.is_finite() && y.is_finite(), "nonfinite item cursor");
                    if item.name != "Health Potion" {
                        out.extend(self.use_ability(
                            player,
                            (slot, target),
                            (x, y),
                            item,
                            content,
                            None,
                        )?);
                        out.push(self.tick(content)?);
                        return Ok(out);
                    }
                    ensure!(
                        item.activations.len() == 1
                            && item.effects.is_empty()
                            && matches!(
                                item.activations[0].action()?,
                                super::content::Action::IncrementStat { stat: 1, .. }
                            ),
                        "unsupported normal Health Potion semantics"
                    );
                }
                let activation = item.single_activation()?;
                match activation.name.as_str() {
                    "IncrementStat" if item.name == "Health Potion" => {
                        ensure!(
                            activation.attributes.get("stat").map(String::as_str) == Some("1"),
                            "Health Potion must increment HP"
                        );
                        let amount: i32 = activation
                            .attributes
                            .get("amount")
                            .context("IncrementStat missing amount")?
                            .parse()?;
                        let hp = self.character(player).stats[&1];
                        let mut character = self.character(player).clone();
                        let effective = if self.map.is_some() {
                            Some(character.equipment_stats(content)?)
                        } else {
                            None
                        };
                        let max = match &effective {
                            Some(stats) => stats.get(0)?,
                            None => character.stats[&0],
                        };
                        ensure!(hp < max, "health is already full");
                        let healed = hp
                            .checked_add(amount)
                            .context("health potion HP overflow")?;
                        if let Some(old) = &effective {
                            let vitals = character.vitals(old)?.heal(
                                amount,
                                old,
                                character.resource_conditions()?,
                            )?;
                            if item.consumable {
                                character.stats.insert(8 + slot, -1);
                            }
                            let new = character.equipment_stats(content)?;
                            character.store_vitals(vitals.rebase(old, &new)?);
                        } else {
                            character.stats.insert(1, healed.min(max));
                        }
                        self.players
                            .get_mut(&player)
                            .expect("attached avatar")
                            .character = character;
                    }
                    "BulletNova" => {
                        let proj = item.arena_projectile(0)?;
                        ensure!(
                            distance(x, y, self.character(player).x, self.character(player).y)
                                < 15.,
                            "spell target too distant"
                        );
                        ensure!(x.is_finite() && y.is_finite(), "nonfinite spell target");
                        let ids: Vec<u8> = (0..=255)
                            .filter(|id| {
                                !self
                                    .bullets
                                    .iter()
                                    .any(|b| b.owner == player && b.id == *id)
                            })
                            .take(20)
                            .collect();
                        ensure!(
                            ids.len() == 20 && self.bullets.len() + 20 <= 4096,
                            "projectile capacity exceeded"
                        );
                        for (index, id) in ids.into_iter().enumerate() {
                            let angle = index as f32 * std::f32::consts::TAU / 20.;
                            let damage = self.rng.gen_range(proj.min..=proj.max);
                            let b = Bullet {
                                flight: Flight::Arena,
                                owner: player,
                                id,
                                origin: (x, y),
                                age: 0.,
                                amplitude: proj.amplitude,
                                frequency: proj.frequency,
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
                    name => bail!("build100 activation {name} is not implemented"),
                }
                if item.consumable && !(self.map.is_some() && item.name == "Health Potion") {
                    if self.map.is_some() {
                        let mut character = self.character(player).clone();
                        let old = character.equipment_stats(content)?;
                        let vitals = character.vitals(&old)?;
                        character.stats.insert(8 + slot, -1);
                        let new = character.equipment_stats(content)?;
                        character.store_vitals(vitals.rebase(&old, &new)?);
                        self.players
                            .get_mut(&player)
                            .expect("attached avatar")
                            .character = character;
                    } else {
                        self.set_item(player, owner, slot, -1);
                    }
                }
                tracing::info!(
                    kind,
                    hp = self.character(player).stats[&1],
                    "build100 item used"
                );
                out.push(self.tick(content)?);
            }
            7 => {
                r.utf()?;
                r.i32()?;
                let text = r.utf()?;
                r.done()?;
                ensure!(text.len() <= 256, "chat too long");
                out.push(message(&format!("{}: {text}", self.character(player).name)));
            }
            super::protocol::PONG => {
                super::protocol::pong(&p.body)?;
            }
            _ => bail!("build100 packet {} is not implemented", p.id),
        }
        Ok(out)
    }
    fn use_ability(
        &mut self,
        player: i32,
        selection: (u8, i32),
        cursor: (f32, f32),
        item: &super::content::Object,
        content: &Content,
        native: Option<WeaponShot>,
    ) -> Result<Vec<Packet>> {
        use super::content::{Action, Condition};
        let (slot, target) = selection;
        ensure!(slot == 1, "normal ability requires equipped slot1");
        ensure!(
            target == player,
            "normal self condition requires target=owner"
        );
        let mut character = self.character(player).clone();
        ensure!(character.alive, "dead avatar cannot use ability");
        let effective = character.equipment_stats(content)?;
        let mut vitals = character.vitals(&effective)?;
        ensure!(
            item.class == "Equipment"
                && !item.consumable
                && item.slot_type == Some(content.get(character.kind)?.slots[1]),
            "normal ability must be equipped nonconsumable matching slot type"
        );
        ensure!(
            !item.activations.is_empty(),
            "unsupported normal ability semantics: no activations"
        );
        let first_action = item.activations[0].action()?;
        let quiver = if matches!(first_action, Action::Shoot) {
            ensure!(
                character.kind == 0x307,
                "unsupported normal quiver: requires Archer"
            );
            let projectile = item.quiver_projectile()?;
            let avatar = &self.players[&player];
            if native.is_none() {
                ensure!(
                    avatar.quiver_ack == Some(item.id),
                    "unsupported normal quiver acknowledgement: no matching paid shot"
                );
                self.players.get_mut(&player).expect("attached").quiver_ack = None;
                return Ok(vec![]);
            }
            ensure!(
                avatar.quiver_ack.is_none(),
                "outstanding quiver acknowledgement"
            );
            ensure!(
                avatar.weapon.pending.is_none(),
                "quiver overlaps reserved weapon burst"
            );
            let input = native.as_ref().expect("native quiver");
            ensure!(
                (1..=96).contains(&input.id),
                "normal native bullet ID must be in 1..=96"
            );
            Some(projectile)
        } else {
            ensure!(native.is_none(), "unsupported native ability shot");
            None
        };
        let heal = if matches!(item.id, 0xa31..=0xa33)
            || matches!(first_action, Action::HealNova { .. })
        {
            ensure!(
                character.kind == 0x310
                    && matches!(item.id, 0xa31..=0xa33)
                    && item.activations.len() == 1
                    && item.effects.is_empty(),
                "unsupported normal HealNova: requires ordinary Priest spell, one activation and no root effects"
            );
            let Action::HealNova { range, healing } = first_action else {
                bail!("unsupported normal Priest spell action: requires HealNova");
            };
            Some((range, healing))
        } else {
            None
        };
        let nova = if matches!(first_action, Action::BulletNova) {
            ensure!(
                character.kind == 0x30e && item.activations.len() == 1 && item.effects.is_empty(),
                "unsupported normal Wizard BulletNova: requires one activation and no root effects"
            );
            let map = self.map.as_ref().context("normal ability map missing")?;
            let (w, h) = map.dimensions();
            ensure!(
                cursor.0.is_finite()
                    && cursor.1.is_finite()
                    && cursor.0 >= 0.
                    && cursor.1 >= 0.
                    && cursor.0 < f32::from(w)
                    && cursor.1 < f32::from(h),
                "normal spell cursor outside map"
            );
            ensure!(
                distance(cursor.0, cursor.1, character.x, character.y) < 15.,
                "spell target too distant"
            );
            let projectile = item.normal_projectile(0)?;
            // Native prediction owns 1..96; server Nova must not collide with those client object keys.
            let (ids, next_cursor) = server_projectile_ids(
                &self.bullets,
                player,
                97,
                self.server_projectile_cursors.get(&player).copied(),
                20,
            )
            .context("normal Nova projectile capacity exceeded")?;
            ensure!(
                self.bullets.len() + 20 <= 4096
                    && self.bullets.iter().filter(|b| b.owner == player).count() + 20 <= 256,
                "normal Nova projectile capacity exceeded"
            );
            Some((projectile, ids, next_cursor))
        } else if heal.is_some() || quiver.is_some() {
            None
        } else {
            for activation in &item.activations {
                ensure!(
                    matches!(activation.action()?, Action::ConditionEffectSelf),
                    "unsupported normal ability semantics: {}",
                    activation.name
                );
            }
            ensure!(
                !item.effects.is_empty()
                    && item
                        .effects
                        .iter()
                        .all(|effect| effect.condition == Condition::Invisible
                            && effect.duration_seconds.is_finite()
                            && effect.duration_seconds > 0.),
                "unsupported normal ability effects: only positive root Invisible is executable"
            );
            None
        };
        let mut state = character
            .action_state
            .clone()
            .context("normal action state missing")?;
        let (mut conditions, _) = state.checked_conditions()?;
        let now = state.conditions.now_seconds;
        let (now_ms, previous_ms) = match state.ability_used_seconds {
            Some(previous) => ((now - previous) * 1000., Some(0.)),
            None => (now * 1000., None),
        };
        ensure!(
            combat::ability_ready(
                item,
                vitals.mp,
                effective.get(3)?,
                character.resource_conditions()?,
                now_ms,
                previous_ms
            )?,
            "normal ability insufficient MP or cooldown not ready"
        );
        let mut healed = BTreeMap::new();
        let mut hp_delta = 0_i64;
        if let Some((range, amount)) = heal {
            let map = self.map.as_ref().context("normal heal map missing")?;
            let (width, height) = map.dimensions();
            for (&id, avatar) in &self.players {
                let peer = &avatar.character;
                ensure!(
                    peer.x.is_finite()
                        && peer.y.is_finite()
                        && peer.x >= 0.
                        && peer.y >= 0.
                        && peer.x < f32::from(width)
                        && peer.y < f32::from(height),
                    "normal heal authoritative position outside map: avatar {id}"
                );
                if peer.alive {
                    self.wire_stats(peer, content)?;
                }
            }
            for (&id, avatar) in &self.players {
                let mut peer = avatar.character.clone();
                if !peer.alive || distance(character.x, character.y, peer.x, peer.y) >= range {
                    continue;
                }
                let stats = peer.equipment_stats(content)?;
                let before = peer.vitals(&stats)?;
                let mask = peer.resource_conditions()?;
                if !healnova_recipient(peer.alive, mask) {
                    continue;
                }
                let after = before.heal(amount, &stats, mask)?;
                hp_delta += i64::from(after.hp - before.hp);
                peer.store_vitals(after);
                peer.vitals(&stats)?;
                healed.insert(id, peer);
            }
            // Merge the caster's heal before payment so its recipient copy cannot refund MP.
            if let Some(caster) = healed.get(&player) {
                character = caster.clone();
                vitals = character.vitals(&effective)?;
            }
        }
        let mut rng = self.rng.clone();
        let mut bullets = vec![];
        let mut next_projectile_cursor = None;
        if let Some((projectile, ids, next_cursor)) = nova {
            next_projectile_cursor = Some(next_cursor);
            for (index, id) in ids.into_iter().enumerate() {
                let angle = (index as f64 * std::f64::consts::TAU / 20.) as f32;
                let damage = combat::continuous_damage(
                    combat::DamageSource::Projectile(projectile),
                    combat::AttackScaling::Unscaled,
                    rng.gen_range(0.0..1.0),
                )?;
                bullets.push(Bullet {
                    flight: Flight::Normal {
                        projectile: projectile.clone(),
                        angle: f64::from(angle),
                    },
                    owner: player,
                    id,
                    origin: cursor,
                    age: 0.,
                    amplitude: projectile.amplitude,
                    frequency: projectile.frequency,
                    x: cursor.0,
                    y: cursor.1,
                    vx: angle.cos() * projectile.speed,
                    vy: angle.sin() * projectile.speed,
                    damage,
                    life: projectile.lifetime,
                });
            }
        }
        if let Some(projectile) = quiver {
            let input = native.as_ref().expect("native quiver");
            let damage = combat::continuous_damage(
                combat::DamageSource::Root(item),
                combat::AttackScaling::Unscaled,
                rng.gen_range(0.0..1.0),
            )?;
            bullets.push(Bullet {
                flight: Flight::Normal {
                    projectile: projectile.clone(),
                    angle: f64::from(input.angle),
                },
                owner: player,
                id: input.id,
                origin: input.origin,
                age: 0.,
                amplitude: projectile.amplitude,
                frequency: projectile.frequency,
                x: input.origin.0,
                y: input.origin.1,
                vx: input.angle.cos() * projectile.speed,
                vy: input.angle.sin() * projectile.speed,
                damage,
                life: projectile.lifetime,
            });
        }
        let mut mask = character.resource_conditions()?;
        if quiver.is_none() {
            for _ in &item.activations {
                for effect in &item.effects {
                    mask = conditions.refresh(effect, now)?;
                }
            }
        }
        state.conditions = conditions.snapshot();
        state.ability_used_seconds = Some(now);
        state.checked_conditions()?;
        vitals.mp -= item.mp_cost.context("normal ability missing root MpCost")?;
        character.store_vitals(vitals);
        character.stats.insert(25, mask.bits() as i32);
        character.action_state = Some(state);
        character.vitals(&effective)?;
        if heal.is_some() {
            self.wire_stats(&character, content)?;
        }
        let selected = healed.len();
        healed.insert(player, character);
        for (id, character) in healed {
            self.players
                .get_mut(&id)
                .expect("staged attached avatar")
                .character = character;
        }
        self.rng = rng;
        let mut out = vec![];
        for bullet in &bullets {
            let Flight::Normal { angle, .. } = bullet.flight else {
                unreachable!("normal ability projectile")
            };
            out.push(shot(bullet, item.id, angle as f32));
        }
        self.bullets.extend(bullets);
        if quiver.is_some() {
            self.players.get_mut(&player).expect("attached").quiver_ack = Some(item.id);
        }
        if let Some(next) = next_projectile_cursor {
            self.server_projectile_cursors.insert(player, next);
        }
        tracing::info!(
            avatar = player,
            item = item.id,
            mp = vitals.mp,
            payment = item.mp_cost,
            selected,
            hp_delta,
            mask = mask.bits(),
            "build100 paid normal ability pending commit"
        );
        Ok(out)
    }
    fn item(&self, player: i32, id: i32, slot: u8) -> Result<i32> {
        if id == player {
            ensure!(slot < 12, "invalid character slot");
            self.players[&player]
                .character
                .stats
                .get(&(8 + slot))
                .copied()
                .with_context(|| format!("missing character inventory slot {slot}"))
        } else {
            let e = self.entities.get(&id).context("missing loot container")?;
            ensure!(
                slot < 8 && matches!(e.kind, 0x500 | 0x806),
                "invalid bag slot"
            );
            ensure!(
                distance(
                    e.x,
                    e.y,
                    self.players[&player].character.x,
                    self.players[&player].character.y
                ) < 2.,
                "loot container too far away"
            );
            e.stats
                .get(&(8 + slot))
                .copied()
                .with_context(|| format!("missing loot container {id} slot {slot}"))
        }
    }
    fn can_store(
        &self,
        player: i32,
        id: i32,
        slot: u8,
        kind: i16,
        content: &Content,
    ) -> Result<()> {
        self.item(player, id, slot)?;
        if self.map.is_some() && kind != -1 {
            ensure!(kind >= 0, "invalid inventory item ID {kind}");
            let item = content.get(kind)?;
            ensure!(item.class == "Equipment", "inventory item is not Equipment");
            ensure!(
                item.slot_type.is_some_and(|s| (1..=17).contains(&s)),
                "inventory item missing/invalid SlotType"
            );
        }
        if id == player && slot < 4 && kind != -1 {
            let required = content.get(self.players[&player].character.kind)?.slots[slot as usize];
            ensure!(
                content.get(kind)?.slot_type == Some(required),
                "item does not fit equipment slot"
            );
        }
        Ok(())
    }
    fn set_item(&mut self, player: i32, id: i32, slot: u8, kind: i16) {
        if id == player {
            self.character_mut(player)
                .stats
                .insert(8 + slot, i32::from(kind));
        } else {
            self.entities
                .get_mut(&id)
                .expect("validated container")
                .stats
                .insert(8 + slot, i32::from(kind));
        }
    }
    fn normal_rewards(
        &mut self,
        enemy: &Entity,
        owner: i32,
        content: &Content,
        out: &mut Vec<Packet>,
    ) -> Result<()> {
        let source = content.get(enemy.kind)?;
        let death = EnemyDeath {
            // A live Room's entity partition identifies the instance; recipients come only from that Room.
            instance: self.entity_partition(),
            position: (enemy.x, enemy.y),
            ordinary_enemy: source.is_enemy && source.class == "Character",
            player_attributed: self.players.contains_key(&owner),
            summoned: matches!(enemy.provenance, Provenance::Summoned { .. }),
        };
        if !death.ordinary_enemy
            || !death.player_attributed
            || !matches!(enemy.provenance, Provenance::Map | Provenance::Population)
        {
            return Ok(());
        }
        let amount = progression::enemy_xp(source)?;
        let mut recipients = vec![];
        for (&id, avatar) in &self.players {
            let p = &avatar.character;
            if progression::reward_eligible(
                RewardRecipient {
                    instance: self.entity_partition(),
                    position: (p.x, p.y),
                    living: p.alive,
                    paused: p
                        .resource_conditions()?
                        .contains(super::content::Condition::Paused),
                },
                death,
            )? {
                recipients.push(id);
            }
        }
        let declared = source
            .loot
            .iter()
            .filter(|entry| matches!(entry, super::content::Loot::Item { .. }))
            .count();
        let bags_needed = i32::try_from(declared.div_ceil(8))?;
        ensure!(
            self.next_id
                .checked_add(bags_needed)
                .is_some_and(|last| last < self.id_limit),
            "normal loot exceeds world entity ID capacity"
        );
        let bag_def = content.named("Loot Bag")?;
        ensure!(
            bag_def.id == 0x500 && bag_def.class == "Container" && bag_def.slots == vec![0; 8],
            "normal loot requires canonical eight-slot Loot Bag"
        );
        // Loot validates every declaration before drawing; the owner installs this RNG only after SQL commit.
        let bags = progression::roll_loot(source, content, &mut self.rng)?;
        for id in recipients {
            self.players
                .get_mut(&id)
                .expect("reward recipient")
                .character
                .award_progress(amount, content, &mut self.rng)?;
        }
        for items in bags {
            let id = self.allocate_entity()?;
            let mut stats = bag_def.stats.clone();
            for (slot, item) in items.into_iter().enumerate() {
                stats.insert(8 + slot as u8, item.map_or(-1, i32::from));
            }
            let bag = Entity {
                id,
                kind: bag_def.id,
                x: enemy.x,
                y: enemy.y,
                stats,
                age: 0.,
                provenance: Provenance::Loot { source: enemy.id },
            };
            out.push(make(id, bag.kind, "", bag.x, bag.y, &bag.stats));
            self.entities.insert(id, bag);
        }
        tracing::info!(
            enemy = enemy.id,
            kind = enemy.kind,
            owner,
            xp = amount,
            "build100 normal enemy rewards pending owner commit"
        );
        Ok(())
    }
    fn replenish_shore(&mut self, content: &Content) -> Result<()> {
        let Some(shore) = &mut self.shore else {
            return Ok(());
        };
        shore.actors.retain(|id, _| self.entities.contains_key(id));
        if self.elapsed < shore.next_replenish {
            return Ok(());
        }
        let next = self.elapsed + 10.;
        ensure!(
            next.is_finite() && next > self.elapsed,
            "shore replenishment clock overflow/lost precision"
        );
        shore.next_replenish = next;
        ensure!(
            shore.actors.len() <= 4,
            "shore Pirate population exceeds four"
        );
        let missing = 4 - shore.actors.len();
        let mut homes: Vec<_> = shore.actors.values().map(|b| b.home).collect();
        let mut picks = vec![];
        for &site in &shore.sites {
            if picks.len() == missing {
                break;
            }
            if homes
                .iter()
                .any(|home| distance(site.0, site.1, home.0, home.1) < 2.5)
                || self.players.values().any(|a| {
                    a.character.alive && distance(site.0, site.1, a.character.x, a.character.y) < 3.
                })
            {
                continue;
            }
            let mut overlaps = false;
            for e in self.entities.values() {
                if content.get(e.kind)?.class == "Character"
                    && e.stats.get(&1).is_some_and(|hp| *hp > 0)
                    && distance(site.0, site.1, e.x, e.y) < 1.3
                {
                    overlaps = true;
                    break;
                }
            }
            if overlaps
                || picks
                    .iter()
                    .any(|p: &(f32, f32)| distance(site.0, site.1, p.0, p.1) < 1.3)
            {
                continue;
            }
            homes.push(site);
            picks.push(site);
        }
        if picks.len() < missing {
            tracing::debug!(
                missing,
                selected = picks.len(),
                reason = "occupied shore sites",
                "normal Pirate replenishment deferred"
            );
        }
        let def = content.get(0x600)?;
        let hp = *def
            .stats
            .get(&0)
            .context("normal shore Pirate missing MaxHP")?;
        ensure!(
            hp > 0 && def.stats.get(&21).is_some_and(|d| *d >= 0),
            "normal shore Pirate invalid HP/DEF"
        );
        for home in picks {
            let id = self.allocate_entity()?;
            let mut stats = def.stats.clone();
            stats.insert(1, hp);
            self.entities.insert(
                id,
                Entity {
                    id,
                    kind: def.id,
                    x: home.0,
                    y: home.1,
                    stats,
                    age: 0.,
                    provenance: Provenance::Population,
                },
            );
            self.shore
                .as_mut()
                .expect("shore population")
                .actors
                .insert(
                    id,
                    ShorePirate {
                        home,
                        next_shot: self.elapsed,
                    },
                );
        }
        Ok(())
    }
    fn advance_shore(&mut self, dt: f32, content: &Content, out: &mut Vec<Packet>) -> Result<()> {
        let Some(shore) = &mut self.shore else {
            return Ok(());
        };
        shore.actors.retain(|id, _| self.entities.contains_key(id));
        let map = self.map.as_ref().context("shore map missing")?;
        for (&id, brain) in &mut shore.actors {
            let e = self.entities.get_mut(&id).context("shore actor missing")?;
            ensure!(
                e.kind == 0x600
                    && e.provenance == Provenance::Population
                    && e.stats.get(&1).is_some_and(|hp| *hp > 0),
                "invalid live shore actor {id}"
            );
            ensure!(
                [e.x, e.y, brain.home.0, brain.home.1, brain.next_shot]
                    .into_iter()
                    .all(f32::is_finite),
                "nonfinite shore actor position/timer {id}"
            );
            let target = if distance(e.x, e.y, brain.home.0, brain.home.1) > 12. {
                None
            } else {
                shore_target(&self.players, e, brain.home)?
            };
            // Original Follow1/Wander.4 use a selected later-engine unit bridge, not recovered July AI.
            let (goal, speed, stop) = match target {
                Some((_, x, y)) => ((x, y), 6.29, 1.),
                None => (brain.home, 2.96, 0.),
            };
            let from = (e.x, e.y);
            let d = distance(from.0, from.1, goal.0, goal.1);
            if d > stop {
                let factor = if self
                    .enemy_slows
                    .get(&id)
                    .is_some_and(|remaining| *remaining > 0.)
                {
                    0.5
                } else {
                    1.
                };
                let step = (speed * factor * dt).min(d - stop);
                let to = (
                    from.0 + (goal.0 - from.0) / d * step,
                    from.1 + (goal.1 - from.1) / d * step,
                );
                let next = if map.can_move(from, to) {
                    to
                } else if map.can_move(from, (from.0, to.1)) {
                    (from.0, to.1)
                } else if map.can_move(from, (to.0, from.1)) {
                    (to.0, from.1)
                } else {
                    from
                };
                // Axis sliding can leave the home disk even when the intended segment stays inside it.
                if target.is_none() || distance(next.0, next.1, brain.home.0, brain.home.1) <= 12. {
                    e.x = next.0;
                    e.y = next.1;
                }
            }
            if let Some((_, x, y)) = target
                && distance(e.x, e.y, x, y) < 3.
                && self.elapsed >= brain.next_shot
            {
                let next = self.elapsed + 2.5;
                ensure!(
                    next.is_finite() && next > self.elapsed,
                    "shore attack clock overflow/lost precision"
                );
                out.push(normal_npc_shot(
                    e,
                    (y - e.y).atan2(x - e.x),
                    content,
                    &mut self.bullets,
                    &mut self.server_projectile_cursors,
                    &mut self.rng,
                )?);
                brain.next_shot = next;
            }
        }
        Ok(())
    }
    pub fn advance(&mut self, elapsed_seconds: f64, content: &Content) -> Result<Vec<Packet>> {
        ensure!(
            elapsed_seconds.is_finite()
                && elapsed_seconds >= 0.
                && elapsed_seconds <= f64::from(f32::MAX),
            "invalid world elapsed seconds {elapsed_seconds}"
        );
        // Geometry retains its f32 step; resource carry must use the original elapsed precision.
        let dt = elapsed_seconds as f32;
        ensure!(
            (self.elapsed + dt).is_finite() && (self.shot_clock + dt).is_finite(),
            "world elapsed clock overflow"
        );
        if self.map.is_some() {
            for avatar in self.players.values_mut() {
                let p = &mut avatar.character;
                let effective = p.equipment_stats(content)?;
                p.advance_resources_and_actions(elapsed_seconds, &effective)?;
            }
        }
        self.elapsed += dt;
        self.shot_clock += dt;
        let mut out = vec![];
        let fire = self.shot_clock >= 1.;
        if fire {
            self.shot_clock = 0.;
        }
        let chicken_home = self
            .entities
            .values()
            .find(|e| e.kind == 0x800)
            .map(|e| (e.x, e.y));
        for e in self.entities.values_mut() {
            e.age += dt;
            if let Some(map) = &self.map {
                if e.kind == 0x801 {
                    let angle = (e.id as f32 + self.elapsed * 0.4).sin() * std::f32::consts::TAU;
                    let factor = if self
                        .enemy_slows
                        .get(&e.id)
                        .is_some_and(|remaining| *remaining > 0.)
                    {
                        0.5
                    } else {
                        1.
                    };
                    let to = (
                        e.x + angle.cos() * dt * 0.4 * factor,
                        e.y + angle.sin() * dt * 0.4 * factor,
                    );
                    if chicken_home.is_some_and(|home| distance(to.0, to.1, home.0, home.1) <= 5.)
                        && map.can_move((e.x, e.y), to)
                    {
                        e.x = to.0;
                        e.y = to.1;
                    }
                }
                if !(0x802..=0x805).contains(&e.kind) || e.age < 3. {
                    continue;
                }
            } else if e.kind == 0x500 {
                continue;
            }
            let Some(p) = self
                .players
                .values()
                .map(|a| &a.character)
                .filter(|p| p.alive)
                .min_by(|a, b| {
                    distance(e.x, e.y, a.x, a.y).total_cmp(&distance(e.x, e.y, b.x, b.y))
                })
            else {
                continue;
            };
            let d = distance(e.x, e.y, p.x, p.y);
            if self.map.is_none() && d > 1. && d < 10. {
                let step = dt.min(d - 1.);
                e.x += (p.x - e.x) / d * step;
                e.y += (p.y - e.y) / d * step;
            }
            if (self.map.is_some() || fire) && d < if self.map.is_some() { 20. } else { 3.6 } {
                if self.map.is_some() {
                    e.age %= 3.;
                    let angle = match e.kind {
                        0x802 => 90_f32,
                        0x803 => 270.,
                        0x804 => 0.,
                        0x805 => 180.,
                        _ => unreachable!(),
                    }
                    .to_radians();
                    out.push(normal_npc_shot(
                        e,
                        angle,
                        content,
                        &mut self.bullets,
                        &mut self.server_projectile_cursors,
                        &mut self.rng,
                    )?);
                    continue;
                }
                ensure!(
                    self.bullets.len() < 4096,
                    "build100 world projectile capacity exhausted"
                );
                let proj = content.get(e.kind)?.arena_projectile(0)?;
                let angle = (p.y - e.y).atan2(p.x - e.x);
                let id = (self.elapsed as u32 % 256) as u8;
                let b = Bullet {
                    flight: Flight::Arena,
                    owner: e.id,
                    id,
                    origin: (e.x, e.y),
                    age: 0.,
                    amplitude: proj.amplitude,
                    frequency: proj.frequency,
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
        self.advance_shore(dt, content, &mut out)?;
        // Positive-on-entry slows the whole AI step; hits below affect the next advance.
        self.enemy_slows.retain(|id, remaining| {
            *remaining = (*remaining - dt).max(0.);
            *remaining > 0. && self.entities.contains_key(id)
        });
        let mut hits = vec![];
        for b in &mut self.bullets {
            let from = (b.x, b.y);
            let duration = b.age + b.life;
            b.age += dt;
            ensure!(b.age.is_finite(), "projectile age overflow");
            match &b.flight {
                Flight::Normal { projectile, angle } => {
                    ensure!(self.map.is_some(), "normal projectile in arena");
                    let Some((x, y)) = combat::projectile_position(
                        projectile,
                        (f64::from(b.origin.0), f64::from(b.origin.1)),
                        *angle,
                        b.id,
                        f64::from(b.age),
                    )?
                    else {
                        b.life = -1.;
                        continue;
                    };
                    b.x = x as f32;
                    b.y = y as f32;
                    ensure!(
                        b.x.is_finite() && b.y.is_finite(),
                        "projectile position outside world f32 domain"
                    );
                }
                Flight::Arena => {
                    ensure!(self.map.is_none(), "arena projectile in normal map");
                    b.life -= dt;
                    if b.life <= 0. {
                        continue;
                    }
                    b.x = b.origin.0 + b.vx * b.age;
                    b.y = b.origin.1 + b.vy * b.age;
                    if b.amplitude != 0. {
                        let phase = if b.id % 2 == 0 {
                            0.
                        } else {
                            std::f32::consts::PI
                        };
                        let offset = b.amplitude
                            * (phase + b.age / duration * b.frequency * std::f32::consts::TAU)
                                .sin();
                        let speed = b.vx.hypot(b.vy);
                        ensure!(speed > 0., "sinusoidal projectile requires positive speed");
                        b.x -= offset * b.vy / speed;
                        b.y += offset * b.vx / speed;
                    }
                }
            }
            let target = if self.map.is_some() {
                let cover = projectile_cover_entry(&self.entities, content, b, from)?;
                let mut first: Option<(f32, i32)> = None;
                let mut consider = |id, x, y, radius| {
                    if let Some(enter) = projectile_target_entry(from, (b.x, b.y), (x, y), radius)
                        && cover.is_none_or(|at| enter <= at)
                        && first.is_none_or(|(at, _)| enter < at)
                    {
                        first = Some((enter, id));
                    }
                };
                if self.players.contains_key(&b.owner) {
                    for e in self.entities.values() {
                        let def = content.get(e.kind)?;
                        if def.is_enemy
                            && def.class == "Character"
                            && e.stats.get(&1).is_some_and(|hp| *hp > 0)
                        {
                            consider(e.id, e.x, e.y, 0.65);
                        }
                    }
                } else {
                    for (&id, avatar) in self.players.iter().filter(|(_, a)| a.character.alive) {
                        let p = &avatar.character;
                        consider(id, p.x, p.y, 0.5);
                    }
                }
                if cover.is_some() {
                    b.life = -1.;
                }
                first.map(|(_, id)| id)
            } else if self.players.contains_key(&b.owner) {
                self.entities
                    .values()
                    .find(|e| e.kind != 0x500 && distance(e.x, e.y, b.x, b.y) < 0.65)
                    .map(|e| e.id)
            } else {
                self.players
                    .iter()
                    .find(|(_, a)| {
                        a.character.alive && distance(a.character.x, a.character.y, b.x, b.y) < 0.5
                    })
                    .map(|(&id, _)| id)
            };
            if let Some(target) = target {
                hits.push((target, b.owner, b.id, b.damage, b.flight.clone()));
                b.life = -1.;
            }
        }
        self.bullets.retain(|b| b.life > 0.);
        for (target, owner, id, raw, flight) in hits {
            if let Some(avatar) = self.players.get_mut(&target) {
                let p = &mut avatar.character;
                if !p.alive {
                    continue;
                }
                let defense = if self.map.is_some() {
                    p.equipment_stats(content)?.get(21)?
                } else {
                    p.stats[&21]
                };
                let damage = flight.damage(raw, defense)?;
                let hp = (p.stats[&1] - damage).max(0);
                p.stats.insert(1, hp);
                out.push(damage_packet(target, owner, id, damage));
                if hp == 0 {
                    p.alive = false;
                    p.clear_dead_hp_carry();
                    avatar.weapon.pending = None;
                    avatar.quiver_ack = None;
                    tracing::info!(
                        avatar = target,
                        generation = p.generation,
                        "build100 permanent death pending commit"
                    );
                    out.push(self.tick(content)?);
                    out.push(drop_object(target));
                }
            } else if let Some(e) = self.entities.get_mut(&target) {
                let damage =
                    flight.damage(raw, *e.stats.get(&21).context("enemy Defense missing")?)?;
                let hp = e.stats.get_mut(&1).context("enemy HP missing")?;
                *hp = hp.checked_sub(damage).context("enemy damage HP overflow")?;
                let slow = match &flight {
                    Flight::Normal { projectile, .. } => projectile
                        .effects
                        .iter()
                        .find(|effect| effect.condition == super::content::Condition::Slowed),
                    Flight::Arena => None,
                };
                if let Some(effect) = slow {
                    self.enemy_slows.insert(target, effect.duration_seconds);
                    out.push(packet(9, |w| {
                        w.i32(target)
                            .u8(super::content::Condition::Slowed as u8)
                            .u8(effect.duration_seconds as u8)
                            .i16(damage as i16)
                            .u8(id)
                            .i32(owner);
                    }));
                } else {
                    out.push(damage_packet(target, owner, id, damage));
                }
                if e.stats[&1] <= 0 {
                    self.enemy_slows.remove(&target);
                    let e = self.entities.remove(&target).expect("enemy present");
                    out.push(drop_object(target));
                    if self.map.is_some() {
                        self.normal_rewards(&e, owner, content, &mut out)?;
                        continue;
                    }
                    for avatar in self.players.values_mut() {
                        let p = &mut avatar.character;
                        if !p.alive || distance(p.x, p.y, e.x, e.y) >= 16. {
                            continue;
                        }
                        *p.stats.get_mut(&6).context("player XP missing")? += 5;
                        while p.stats[&7] < 20 && p.stats[&6] >= p.stats[&5] {
                            for &(stat, min, max, cap) in &content.get(p.kind)?.growth {
                                let value =
                                    p.stats.get_mut(&stat).context("growth stat missing")?;
                                *value = (*value + self.rng.gen_range(min..=max)).min(cap);
                            }
                            *p.stats.get_mut(&7).expect("level initialized") += 1;
                            *p.stats.get_mut(&5).expect("XP threshold initialized") +=
                                p.stats[&7] * 50;
                            p.stats.insert(1, p.stats[&0]);
                            p.stats.insert(4, p.stats[&3]);
                        }
                    }
                    let id = self.allocate_entity()?;
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
                        provenance: Provenance::Loot { source: target },
                        x: e.x,
                        y: e.y,
                        stats,
                        age: 0.,
                    };
                    out.push(make(id, bag.kind, "", bag.x, bag.y, &bag.stats));
                    self.entities.insert(id, bag);
                    tracing::info!(
                        target,
                        bag = id,
                        "build100 enemy death pending commit; fixture guaranteed potion"
                    );
                }
            }
        }
        let expired: Vec<_> = self
            .entities
            .values()
            .filter(|e| {
                matches!(e.kind, 0x500 | 0x806)
                    && (e.age > 60. || (8..16).all(|s| e.stats[&s] == -1))
            })
            .map(|e| e.id)
            .collect();
        for id in expired {
            self.entities.remove(&id);
            out.push(drop_object(id));
        }
        if self.map.is_none()
            && self.players.values().any(|a| a.character.alive)
            && self.elapsed >= self.next_wave
            && !self.entities.values().any(|e| e.kind != 0x500)
        {
            self.spawn(content, &mut out)?;
        }
        if self.map.is_some() {
            self.teaching_objects(content)?;
            self.hazards(&mut out)?;
        }
        self.enemy_slows
            .retain(|id, _| self.entities.contains_key(id));
        self.replenish_shore(content)?;
        self.server_projectile_cursors.retain(|owner, _| {
            self.players.get(owner).is_some_and(|a| a.character.alive)
                || self.entities.contains_key(owner)
        });
        if self.elapsed >= self.next_tick {
            self.next_tick += 0.2;
            out.push(self.tick(content)?);
        }
        Ok(out)
    }
}
fn shot(b: &Bullet, kind: i16, angle: f32) -> Packet {
    packet(8, |w| {
        w.u8(b.id)
            .i32(b.owner)
            // Native player input carries a visual ID, but server output selects the XML projectile index.
            .u8(0)
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
#[path = "move_budget_tests.rs"]
mod move_budget_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::content::{Object, Projectile};
    pub(super) fn normal_flight(speed: f32, lifetime: f32) -> Flight {
        Flight::Normal {
            projectile: Projectile {
                visual: "Missile".into(),
                min: 1,
                max: 1000,
                speed,
                lifetime,
                amplitude: 0.,
                frequency: 1.,
                wavy: false,
                continues: false,
                effects: vec![],
                features: vec![],
            },
            angle: 0.,
        }
    }
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
                    projectiles: vec![],
                    activations: vec![],
                    root_damage: None,
                    rate_of_fire: None,
                    mp_cost: None,
                    num_projectiles: 1,
                    arc_gap_radians: std::f32::consts::PI / 16.0,
                    bonuses: vec![],
                    effects: vec![],
                    xp_multiplier: 1.0,
                    loot: vec![],
                    consumable: false,
                    occupy_square: false,
                    is_static: false,
                    is_enemy: id == 0x600,
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
        e.projectiles.push((
            0,
            Projectile {
                visual: "Missile".into(),
                min: 5,
                max: 5,
                speed: 6.,
                lifetime: 0.6,
                amplitude: 0.,
                frequency: 1.,
                wavy: false,
                continues: false,
                effects: vec![],
                features: vec![],
            },
        ));
        let w = objects.get_mut(&0xa97).unwrap();
        w.slot_type = Some(17);
        w.projectiles.push((
            0,
            Projectile {
                visual: "Missile".into(),
                min: 20,
                max: 20,
                speed: 13.,
                lifetime: 1.,
                amplitude: 0.,
                frequency: 1.,
                wavy: false,
                continues: false,
                effects: vec![],
                features: vec![],
            },
        ));
        let p = objects.get_mut(&0xa22).unwrap();
        p.slot_type = Some(10);
        p.activations = vec![crate::content::Activation {
            name: "IncrementStat".into(),
            attributes: BTreeMap::from([
                ("stat".into(), "1".into()),
                ("amount".into(), "1000".into()),
            ]),
        }];
        p.consumable = true;
        Content { objects }
    }
    fn room() -> (Content, Room) {
        let c = content();
        let p = Character::new(&c, 0x30e, "Test".into(), 1).unwrap();
        let mut r = Room::new(42, 0, 2);
        r.attach(PLAYER, p, &c).unwrap();
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
        r.handle(PLAYER, fire(1, PLAYER, 0xa97), &c).unwrap();
        for _ in 0..8 {
            r.advance(0.05, &c).unwrap();
        }
        assert!(!r.entities.contains_key(&101));
        assert_eq!(r.character(PLAYER).stats[&6], 5);
        let bag = r.entities.values().find(|e| e.kind == 0x500).unwrap();
        assert_eq!(bag.stats[&8], 0xa22);
    }
    #[test]
    fn forged_damage_cannot_kill_or_reward() {
        let (c, mut r) = room();
        let old = r.entities[&101].stats[&1];
        r.handle(PLAYER, damage_packet(101, PLAYER, 1, 32000), &c)
            .unwrap();
        assert_eq!(r.entities[&101].stats[&1], old);
        assert_eq!(r.character(PLAYER).stats[&6], 0);
    }
    #[test]
    fn rejects_spoofed_shots_and_inventory() {
        let (c, mut r) = room();
        assert!(r.handle(PLAYER, fire(1, 999, 0xa97), &c).is_err());
        assert!(r.handle(PLAYER, fire(1, PLAYER, 0xa22), &c).is_err());
        assert!(r.can_store(PLAYER, PLAYER, 0, 0xa22, &c).is_err());
        assert!(r.item(PLAYER, 123, 0).is_err());
        assert!(r.item(PLAYER, PLAYER, 12).is_err());
    }
    #[test]
    fn potion_once_and_postdeath_inputs_cannot_resurrect() {
        let (c, mut r) = room();
        r.character_mut(PLAYER).stats.insert(1, 25);
        let use_item = || {
            packet(15, |w| {
                w.i32(PLAYER).u8(4).i32(-1).f32(16.5).f32(16.5);
            })
        };
        r.handle(PLAYER, use_item(), &c).unwrap();
        assert_eq!(r.character(PLAYER).stats[&1], 100);
        assert_eq!(r.character(PLAYER).stats[&12], -1);
        assert!(r.handle(PLAYER, use_item(), &c).is_err());
        r.character_mut(PLAYER).alive = false;
        r.character_mut(PLAYER).stats.insert(1, 0);
        r.handle(PLAYER, use_item(), &c).unwrap();
        assert_eq!(r.character(PLAYER).stats[&1], 0);
    }
    #[test]
    fn projectile_death_is_once_only_and_emits_native_death() {
        let (c, mut r) = room();
        r.character_mut(PLAYER).stats.insert(1, 5);
        r.entities.clear();
        r.next_wave = 999.;
        r.bullets.push(Bullet {
            flight: Flight::Arena,
            owner: 100,
            id: 1,
            origin: (16.5, 16.5),
            age: 0.,
            amplitude: 0.,
            frequency: 1.,
            x: 16.5,
            y: 16.5,
            vx: 0.,
            vy: 0.,
            damage: 5,
            life: 1.,
        });
        let packets = r.advance(0.05, &c).unwrap();
        assert!(!r.character(PLAYER).alive);
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
                provenance: Provenance::Fixture,
                x: 17.,
                y: 16.5,
                stats,
                age: 0.,
            },
        );
        r.character_mut(PLAYER).stats.insert(12, -1);
        let swap = || {
            packet(14, |w| {
                w.i32(200).u8(0).i16(0xa22).i32(PLAYER).u8(4).i16(-1);
            })
        };
        r.handle(PLAYER, swap(), &c).unwrap();
        assert_eq!(r.item(PLAYER, PLAYER, 4).unwrap(), 0xa22);
        assert_eq!(r.item(PLAYER, 200, 0).unwrap(), -1);
        assert!(r.handle(PLAYER, swap(), &c).is_err());
        r.entities.get_mut(&200).unwrap().x = 25.;
        assert!(r.item(PLAYER, 200, 0).is_err());
    }
    #[test]
    fn historical_contracts_server_shoot_uses_index_zero_not_visual() {
        let bullet = Bullet {
            flight: Flight::Arena,
            owner: 101,
            id: 7,
            origin: (18., 16.),
            age: 0.,
            amplitude: 0.,
            frequency: 1.,
            x: 18.,
            y: 16.,
            vx: 6.,
            vy: 0.,
            damage: 5,
            life: 0.6,
        };
        let packet = shot(&bullet, 0x600, 0.);
        assert_eq!(packet.id, 8);
        // Source-format golden with synthetic fields, not captured native bytes.
        assert_eq!(
            packet.body,
            [
                7, 0, 0, 0, 101, 0, 6, 0, 0x41, 0x90, 0, 0, 0x41, 0x80, 0, 0, 0, 0, 0, 0, 0, 5
            ]
        );
    }

    #[test]
    #[ignore = "requires private qualified assets: BUILD100_CLIENT_DIR; run explicitly with --include-ignored"]
    fn historical_contracts_real_wizard_starter_follows_original_sinusoid() {
        let path = std::path::PathBuf::from(
            std::env::var("BUILD100_CLIENT_DIR").expect("BUILD100_CLIENT_DIR required"),
        );
        let c = Content::load(&path).unwrap();
        let character = Character::new(&c, 0x30e, "Phase".into(), 1).unwrap();
        assert_eq!(character.stats[&8], 0xa97);
        let projectile = c.get(0xa97).unwrap().arena_projectile(0).unwrap();
        assert_eq!(
            (
                projectile.amplitude,
                projectile.frequency,
                projectile.speed,
                projectile.lifetime
            ),
            (0.5, 2., 13., 1.)
        );
        let visual = u8::try_from(c.named(&projectile.visual).unwrap().id).unwrap();
        assert_ne!(visual, 0);
        let mut r = Room::new(42, 0, 2);
        r.attach(PLAYER, character, &c).unwrap();
        r.entities.clear();
        let fire = |id, subtype| {
            packet(8, |w| {
                w.u8(id)
                    .i32(PLAYER)
                    .u8(subtype)
                    .i16(0xa97)
                    .f32(16.8)
                    .f32(16.5)
                    .f32(0.)
                    .i16(32767);
            })
        };
        assert!(r.handle(PLAYER, fire(2, 0), &c).is_err());
        r.handle(PLAYER, fire(0, visual), &c).unwrap();
        r.handle(PLAYER, fire(1, visual), &c).unwrap();
        r.advance(0.125, &c).unwrap();
        assert_eq!(r.bullets.len(), 2);
        for (b, y) in r.bullets.iter().zip([17., 16.]) {
            assert!((b.x - 18.425).abs() < 0.0001);
            assert!((b.y - y).abs() < 0.0001);
        }
        r.advance(0.25, &c).unwrap();
        assert_eq!(r.bullets.len(), 2);
        for (b, y) in r.bullets.iter().zip([16., 17.]) {
            assert!((b.x - 21.675).abs() < 0.0001);
            assert!((b.y - y).abs() < 0.0001);
        }
        r.advance(0.625, &c).unwrap();
        assert!(r.bullets.is_empty());
    }

    #[test]
    fn historical_contracts_room_applies_original_attack_and_defense() {
        let (c, mut r) = room();
        r.entities.retain(|&id, _| id == 101);
        let e = r.entities.get_mut(&101).unwrap();
        e.x = 18.;
        e.y = 16.5;
        e.stats.insert(1, 100);
        r.character_mut(PLAYER).stats.insert(20, 75);
        r.handle(PLAYER, fire(1, PLAYER, 0xa97), &c).unwrap();
        for _ in 0..8 {
            r.advance(0.05, &c).unwrap();
        }
        assert_eq!(r.entities[&101].stats[&1], 60);
        r.entities.clear();
        r.character_mut(PLAYER).stats.insert(21, 80);
        r.character_mut(PLAYER).stats.insert(1, 100);
        r.bullets.push(Bullet {
            flight: Flight::Arena,
            owner: 999,
            id: 2,
            origin: (r.character(PLAYER).x, r.character(PLAYER).y),
            age: 0.,
            amplitude: 0.,
            frequency: 1.,
            x: r.character(PLAYER).x,
            y: r.character(PLAYER).y,
            vx: 0.,
            vy: 0.,
            damage: 100,
            life: 1.,
        });
        let packets = r.advance(0.05, &c).unwrap();
        assert_eq!(r.character(PLAYER).stats[&1], 75);
        let damage = packets.iter().find(|p| p.id == 9).unwrap();
        let mut reader = Reader::new(&damage.body);
        assert_eq!(reader.i32().unwrap(), PLAYER);
        assert_eq!((reader.u8().unwrap(), reader.u8().unwrap()), (0, 0));
        assert_eq!(reader.i16().unwrap(), 25);
        assert_eq!(reader.u8().unwrap(), 2);
        assert_eq!(reader.i32().unwrap(), 999);
        reader.done().unwrap();
    }
    #[test]
    fn compatibility_xp_levels_with_client_growth_caps() {
        let (c, mut r) = room();
        r.character_mut(PLAYER).stats.insert(6, 45);
        r.entities.retain(|&id, _| id == 101);
        let e = r.entities.get_mut(&101).unwrap();
        e.x = 18.;
        e.y = 16.5;
        r.handle(PLAYER, fire(1, PLAYER, 0xa97), &c).unwrap();
        for _ in 0..8 {
            r.advance(0.05, &c).unwrap();
        }
        assert_eq!(r.character(PLAYER).stats[&7], 2);
        assert_eq!(r.character(PLAYER).stats[&0], 125);
        assert_eq!(r.character(PLAYER).stats[&5], 150);
    }
}

#[cfg(test)]
#[path = "teleport_runtime_tests.rs"]
pub(crate) mod teleport_runtime_tests;

#[cfg(test)]
#[path = "realm_entry_world_tests.rs"]
mod realm_entry_tests;

#[cfg(test)]
#[path = "inventory_drop_tests.rs"]
mod inventory_drop_tests;

#[cfg(test)]
#[path = "equipment_runtime_tests.rs"]
mod equipment_runtime_tests;

#[cfg(test)]
#[path = "resources_runtime_tests.rs"]
mod resources_runtime_tests;

#[cfg(test)]
#[path = "cloak_runtime_tests.rs"]
mod cloak_runtime_tests;

#[cfg(test)]
#[path = "weapon_runtime_tests.rs"]
mod weapon_runtime_tests;

#[cfg(test)]
#[path = "progression_runtime_tests.rs"]
pub(crate) mod progression_runtime_tests;

#[cfg(test)]
#[path = "spell_runtime_tests.rs"]
pub(crate) mod spell_runtime_tests;

#[cfg(test)]
#[path = "shore_runtime_tests.rs"]
pub(crate) mod shore_runtime_tests;

#[cfg(test)]
#[path = "healnova_runtime_tests.rs"]
mod healnova_runtime_tests;

#[cfg(test)]
#[path = "prediction_runtime_tests.rs"]
pub(crate) mod prediction_runtime_tests;

#[cfg(test)]
#[path = "quiver_runtime_tests.rs"]
pub(crate) mod quiver_runtime_tests;
