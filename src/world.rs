use crate::server::lifecycle::{CloseReason, controller, protocol};
use crate::{
    content::{Content, Map, PARALYZED, Projectile, SLOWED, SPEEDY, STUNNED},
    store::{AccountLists, AccountPresentation, Character, LevelAssist, Vault},
    wire::{self, Packet, Writer},
};
use anyhow::{Context, Result, bail, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicI32, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};
mod abilities;
pub(crate) mod spectator;

mod cell_index;
mod charges;
pub(crate) mod chat;
#[cfg(test)]
mod community_postclear_audit;

mod companions;
mod consumables;
mod davy;
mod dungeon_entry;
mod endgame;
mod endgame_ai;
mod enemy_damage;
mod enemy_shots;
mod entity_store;
mod execution;
mod families;
mod fauna;
mod fauna_births;
mod fauna_special;
mod ghost_ship;
mod gods;
mod ground_dye;

mod gold;
mod golems;

mod ground;

mod guilds;
mod lab;

mod lair;
mod landmark_ai;

mod lost_lands;
mod manor;
mod manor_entry;
mod midlands;

mod named_births;
mod names;
mod ocean;
mod ocean_setpiece;

mod population;
mod population_density;
#[cfg(test)]
mod population_reproduction_tests;

mod realm_events;
mod realm_voice;

mod realm_portal;
mod realm_quests;

mod realm_setpieces;
mod skull_erosion;

mod spider;

mod tick_stats;
mod tomb;
mod tracked;

pub(crate) use chat::{chat_error, chat_info, chat_private, validate as validate_chat};
pub use endgame::ForcedTransfer;
mod abyss;

mod beach;
mod coil;
mod jungle;

mod cooperative;

mod mechanics;

mod native_hits;

mod npc_attacks;
mod npc_queries;

mod reconciliation;

mod resurrection;
mod snake;

pub use reconciliation::{NativeInput, RejectedNativeShot};
mod progression;

pub(crate) mod shop;
mod trade;
pub(crate) mod view;

pub use trade::{TradeAction, TradePlan};
#[derive(Clone)]
pub struct Output {
    pub packets: mpsc::Sender<Packet>,
    pub lost: watch::Sender<Option<&'static str>>,
}
// The first cause names the disconnect; later failures of a closing connection are its consequences.
pub(crate) fn lose(lost: &watch::Sender<Option<&'static str>>, cause: &'static str) {
    lost.send_if_modified(|current| current.is_none() && current.replace(cause).is_none());
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}
impl Pos {
    pub fn distance(self, p: Self) -> f32 {
        ((self.x - p.x).powi(2) + (self.y - p.y).powi(2)).sqrt()
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Following {
    #[default]
    Unknown,
    Acquired,
    Resting,
}
#[derive(Clone)]
pub struct Entity {
    pub id: i32,
    pub kind: u16,
    pub pos: Pos,
    home: Pos,
    state: entity_store::EntityState,
}
#[derive(Clone, Debug, Serialize)]
pub struct EntityData {
    pub name: String,
    pub hp: i32,
    pub size: i32,
    pub condition: i32,
    statuses: BTreeMap<u8, (u64, u64)>,
    pub items: Vec<i32>,
    #[serde(skip)]
    drop_slots: BTreeSet<usize>,
    pub extra_stats: Vec<(u8, i32)>,
    bag_owners: Vec<i32>,
    pub reserved: bool,
    published: bool,
    damage: BTreeMap<i32, i32>,
    goal: Option<Pos>,
    #[serde(skip)]
    motion: Option<crate::encounter::Motion>,
    #[serde(skip)]
    following: Following,
    #[serde(skip)]
    lair_escort: Option<lair::Escort>,
    #[serde(skip)]
    tomb: Option<tomb::State>,
    #[serde(skip)]
    follow_remaining: u64,
    #[serde(skip)]
    strict_initialized: bool,
    #[serde(skip)]
    charge: Option<charges::Charge>,
    #[serde(skip)]
    script: Option<landmark_ai::Script>,
    #[serde(skip)]
    abyss: Option<abyss::Runtime>,
    last_step: Pos,
    phase: f32,
    orbit: Option<realm_events::Orbit>,
    next_shot: u64,
    #[serde(skip)]
    attacks: crate::encounter::attacks::Clocks,
    activated: Option<u64>,
    stage: Option<u64>,
    parent: tracked::Tracked<i32>,
    camp: Option<usize>,
    #[serde(skip)]
    population_slot: Option<usize>,
    quest: Option<i32>,
    expires: tracked::Tracked<u64>,
    next_aux: u64,
    blocked_since: Option<u64>,
    reaction_until: u64,
    bullet: u8,
}
impl std::fmt::Debug for Entity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("Entity");
        out.field("id", &self.id);
        out.field("kind", &self.kind);
        out.field("name", &self.name);
        out.field("pos", &self.pos);
        out.field("hp", &self.hp);
        out.field("size", &self.size);
        out.field("condition", &self.condition);
        out.field("statuses", &self.statuses);
        out.field("items", &self.items);
        out.field("drop_slots", &self.drop_slots);
        out.field("extra_stats", &self.extra_stats);
        out.field("bag_owners", &self.bag_owners);
        out.field("reserved", &self.reserved);
        out.field("published", &self.published);
        out.field("damage", &self.damage);
        out.field("home", &self.home);
        out.field("goal", &self.goal);
        out.field("motion", &self.motion);
        out.field("following", &self.following);
        out.field("tomb", &self.tomb);
        out.field("charge", &self.charge);
        out.field("script", &self.script);
        out.field("last_step", &self.last_step);
        out.field("phase", &self.phase);
        out.field("orbit", &self.orbit);
        out.field("next_shot", &self.next_shot);
        out.field("attacks", &self.attacks);
        out.field("activated", &self.activated);
        out.field("stage", &self.stage);
        out.field("parent", &self.parent);
        out.field("camp", &self.camp);
        out.field("population_slot", &self.population_slot);
        out.field("quest", &self.quest);
        out.field("expires", &self.expires);
        out.field("next_aux", &self.next_aux);
        out.field("blocked_since", &self.blocked_since);
        out.field("reaction_until", &self.reaction_until);
        out.field("bullet", &self.bullet);
        out.finish()
    }
}
impl Serialize for Entity {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("Entity", 31)?;
        out.serialize_field("id", &self.id)?;
        out.serialize_field("kind", &self.kind)?;
        out.serialize_field("name", &self.name)?;
        out.serialize_field("pos", &self.pos)?;
        out.serialize_field("hp", &self.hp)?;
        out.serialize_field("size", &self.size)?;
        out.serialize_field("condition", &self.condition)?;
        out.serialize_field("statuses", &self.statuses)?;
        out.serialize_field("items", &self.items)?;
        out.serialize_field("extra_stats", &self.extra_stats)?;
        out.serialize_field("bag_owners", &self.bag_owners)?;
        out.serialize_field("reserved", &self.reserved)?;
        out.serialize_field("published", &self.published)?;
        out.serialize_field("damage", &self.damage)?;
        out.serialize_field("home", &self.home)?;
        out.serialize_field("goal", &self.goal)?;
        out.serialize_field("last_step", &self.last_step)?;
        out.serialize_field("phase", &self.phase)?;
        out.serialize_field("orbit", &self.orbit)?;
        out.serialize_field("next_shot", &self.next_shot)?;
        out.serialize_field("activated", &self.activated)?;
        out.serialize_field("stage", &self.stage)?;
        out.serialize_field("tomb", &self.tomb)?;
        out.serialize_field("parent", &self.parent)?;
        out.serialize_field("camp", &self.camp)?;
        out.serialize_field("quest", &self.quest)?;
        out.serialize_field("expires", &self.expires)?;
        out.serialize_field("next_aux", &self.next_aux)?;
        out.serialize_field("blocked_since", &self.blocked_since)?;
        out.serialize_field("reaction_until", &self.reaction_until)?;
        out.serialize_field("bullet", &self.bullet)?;
        out.end()
    }
}
impl Entity {
    fn conditions(&self, now: u64) -> i32 {
        self.condition | crate::condition::mask(&self.statuses, now)
    }
    fn accessible(&self, account: i32) -> bool {
        if !self.published {
            return false;
        }
        if !self.bag_owners.is_empty() {
            return self.bag_owners.contains(&account);
        }
        match self.extra_stats.iter().find(|(stat, _)| *stat == 54) {
            Some((_, owner)) if *owner >= 0 => *owner == account,
            _ => true,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Controller {
    Human,
    Qa,
    Autonomous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttackSuppression {
    EffectiveCooldown,
}
struct ObservedMotion {
    position: Pos,
    time: i32,
    velocity: Pos,
    at: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProgressAnchor {
    exp: i32,
    level: i32,
}
impl ProgressAnchor {
    fn of(ch: &Character) -> Self {
        Self {
            exp: ch.exp,
            level: ch.level,
        }
    }
}
struct QueuedExperience {
    amount: i32,
    anchor: ProgressAnchor,
    // A pending inventory commit can change the batch's starting boost state.
    assists: [Vec<LevelAssist>; 2],
}

pub struct Player {
    spectators: Option<tokio::sync::broadcast::Sender<Packet>>,
    dungeon_entries: VecDeque<view::ObservedDungeonEntry>,
    pub character: Character,
    pub pos: Pos,
    pub tx: Option<Output>,
    pub departing: bool,
    account_lists: AccountLists,
    pub(crate) presentation: AccountPresentation,
    paused: bool,
    pub(crate) controller: Controller,
    pub(crate) native_update_acked: bool,
    pub known: BTreeSet<i32>,
    tick_stats: tick_stats::Cache,
    quest: Option<i32>,
    qualified_quest: Option<i32>,
    committed_lifetime: crate::store::FameLifetime,
    statuses: BTreeMap<u8, (u64, u64)>,
    lab_conditions: i32,
    known_since: BTreeMap<i32, u64>,
    shoot_acks: VecDeque<(Instant, Vec<Bullet>)>,
    weapon_ticks: VecDeque<(i32, i32, i32)>,
    visible_projectiles: BTreeMap<(i32, u8), native_hits::VisibleProjectile>,
    predicted_deaths: BTreeMap<i32, (u8, Option<u64>)>,
    ground_hits: BTreeMap<usize, i32>,
    oxygen: i32,
    tiles: BTreeSet<(i16, i16)>,
    tile_updates: Vec<(i16, i16, u8)>,
    tile_scan: Option<(i32, i32)>,
    view_tiles: std::cell::RefCell<view::Terrain>,
    pub death_started: bool,
    death_lifetime: Option<crate::store::FameLifetime>,
    departure_snapshot: Option<Character>,
    killer: String,
    movement_credit: f32,
    hp_fraction: f32,
    mp_fraction: f32,
    pub pending: bool,
    pub experience_queued: i32,
    experience_queue: Option<QueuedExperience>,
    progress_snapshot: Option<Character>,
    pub(crate) level_assists_prepared: Vec<LevelAssist>,
    clears_queued: [u64; crate::dungeon::DUNGEONS.len()],
    pub(crate) progress_started: bool,
    pub death_pending: bool,
    persistence_failure: Option<String>,
    pub bot_goal: Option<Pos>,
    pub bot_aim: Option<f32>,
    pub next_bullet: u8,
    server_bullet: u8,
    seed: u32,
    last_attack: i32,
    last_attack_at: u64,
    attack_credit: f32,
    attack_count: i32,
    attack_suppressed: Option<AttackSuppression>,
    last_move: i32,
    last_move_at: u64,
    motion: ObservedMotion,
    goto_acks: VecDeque<(bool, Instant)>,
    discarded_attack: Option<(i32, i32)>,
    tick: i32,
    last_motion: Option<(f32, u64)>,
    ability_ready_ms: u64,
    boosts: Vec<consumables::Boost>,
    last_companion: Option<u64>,
    ability_shot: Option<(u16, i32, bool, i32)>,
}
impl Player {
    fn acknowledge_lifetime(&mut self, committed: crate::store::FameLifetime) -> Result<()> {
        committed.follows(&self.committed_lifetime)?;
        self.character.lifetime.follows(&self.committed_lifetime)?;
        self.character.lifetime.merge(&committed)?;
        // Later live facts remain different from this acknowledged lower bound and must still save.
        self.committed_lifetime = committed;
        Ok(())
    }
    pub(crate) fn queue_experience(
        &mut self,
        amount: i32,
        killer: Option<i32>,
        positive_damage: bool,
    ) -> Result<()> {
        ensure!(
            amount >= 0 && killer.is_none_or(|id| id > 0),
            "invalid experience source for character {}",
            self.character.id
        );
        let anchor = if self.progress_started {
            ProgressAnchor::of(
                self.progress_snapshot
                    .as_ref()
                    .context("pending progress lacks its prepared experience anchor")?,
            )
        } else {
            ProgressAnchor::of(&self.character)
        };
        crate::store::level_start(anchor.level)?;
        ensure!(anchor.exp >= 0, "negative experience anchor");
        let queue = self
            .experience_queue
            .get_or_insert_with(|| QueuedExperience {
                amount: 0,
                anchor,
                assists: [Vec::new(), Vec::new()],
            });
        ensure!(
            queue.amount == self.experience_queued && queue.anchor == anchor,
            "experience queue mismatch: character={} scalar={} sources={} queued_anchor={:?} current_anchor={:?}",
            self.character.id,
            self.experience_queued,
            queue.amount,
            queue.anchor,
            anchor
        );
        let next = queue
            .amount
            .checked_add(amount)
            .context("experience queue overflow")?;
        for (mode, assists) in queue.assists.iter_mut().enumerate() {
            let multiplier = (mode + 1) as u64;
            let before = u64::try_from(anchor.exp)?
                .checked_add(
                    u64::try_from(queue.amount)?
                        .checked_mul(multiplier)
                        .context("experience projection overflow")?,
                )
                .context("experience projection overflow")?;
            let after = u64::try_from(anchor.exp)?
                .checked_add(
                    u64::try_from(next)?
                        .checked_mul(multiplier)
                        .context("experience projection overflow")?,
                )
                .context("experience projection overflow")?;
            for level in anchor.level + 1..=20 {
                let threshold = u64::try_from(crate::store::level_start(level)?)?;
                if before < threshold
                    && threshold <= after
                    && positive_damage
                    && let Some(giver) = killer.filter(|giver| *giver != self.character.id)
                {
                    assists.push(LevelAssist {
                        reached_level: u8::try_from(level)?,
                        giver,
                    });
                }
            }
            ensure!(
                assists.len() <= 19,
                "experience projection exceeds lifetime levels"
            );
        }
        queue.amount = next;
        self.experience_queued = next;
        Ok(())
    }
    fn owes_credit(&self, giver: i32) -> bool {
        self.level_assists_prepared.iter().any(|a| a.giver == giver)
            || self
                .experience_queue
                .as_ref()
                .is_some_and(|q| q.assists.iter().flatten().any(|a| a.giver == giver))
    }
    fn record_motion(&mut self, position: Pos, time: i32, at: u64, maximum_speed: f32) {
        let elapsed = i64::from(time) - i64::from(self.motion.time);
        if elapsed == 0 {
            return;
        }
        let mut velocity = Pos { x: 0., y: 0. };
        if (1..=1000).contains(&elapsed) {
            velocity = Pos {
                x: (position.x - self.motion.position.x) * 1000. / elapsed as f32,
                y: (position.y - self.motion.position.y) * 1000. / elapsed as f32,
            };
            // Teleports and discontinuities are observations, not physical velocities to extrapolate.
            if velocity.x.hypot(velocity.y) > maximum_speed * 1.2 {
                velocity = Pos { x: 0., y: 0. };
            }
        }
        self.motion = ObservedMotion {
            position,
            time,
            velocity,
            at,
        };
    }
    fn quarantine(&mut self, id: i32, error: String) {
        self.persistence_failure = Some(error);
        self.pending = true;
        self.bot_goal = None;
        self.bot_aim = None;
        if let Some(output) = &self.tx {
            lose(&output.lost, "persistence_quarantine");
        }
        // Quarantine remains authoritative even when its diagnostic sink fails.
        let error = self
            .persistence_failure
            .as_ref()
            .expect("recorded persistence failure");
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tracing::error!(player=id,account=self.character.account,%error,"character quarantined; avatar and reservations retained");
        }))
        .is_err()
        {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr(),
                "quarantine diagnostic sink panicked; player={id} account={} error={error}",
                self.character.account
            );
        }
    }
    fn rewards_queued(&self) -> bool {
        self.experience_queued != 0
            || self.experience_queue.is_some()
            || self.clears_queued != [0; crate::dungeon::DUNGEONS.len()]
    }
    fn progress_queued(&self) -> bool {
        self.character.lifetime != self.committed_lifetime || self.rewards_queued()
    }
    fn active(&self) -> bool {
        self.character.alive
            && !self.death_pending
            && !self.departing
            && self.persistence_failure.is_none()
    }
    pub(crate) fn native_time(&self) -> i32 {
        self.last_move
    }
    fn validate_progress_reservation(&self, snapshot: &Character) -> Result<()> {
        ensure!(
            self.pending
                && self.progress_started
                && self.character.id == snapshot.id
                && self.character.account == snapshot.account
                && self.character.revision == snapshot.revision
                && self.progress_snapshot.as_ref() == Some(snapshot),
            "progress reservation changed"
        );
        Ok(())
    }
    pub(crate) fn native_pause_progress(&self) -> Result<Option<&Character>> {
        ensure!(
            self.active() && self.character.hp > 0,
            "native pause player is not active: alive={} hp={} departing={} death_pending={} persistence_failure={:?}",
            self.character.alive,
            self.character.hp,
            self.departing,
            self.death_pending,
            self.persistence_failure
        );
        if self.pending || self.progress_started {
            ensure!(
                self.pending && self.progress_started,
                "native pause has non-progress pending or malformed progress flags: pending={} progress_started={}",
                self.pending,
                self.progress_started
            );
            let snapshot = self
                .progress_snapshot
                .as_ref()
                .context("native pause ordinary progress snapshot missing")?;
            self.validate_progress_reservation(snapshot)
                .context("native pause ordinary progress snapshot mismatch")?;
            ensure!(
                snapshot.alive,
                "native pause progress snapshot is not living"
            );
            Ok(Some(snapshot))
        } else {
            ensure!(
                self.progress_snapshot.is_none() && self.level_assists_prepared.is_empty(),
                "native pause has an unowned progress snapshot"
            );
            Ok(None)
        }
    }
    pub(crate) fn native_transferring(&self) -> bool {
        self.departing && self.persistence_failure.is_none()
    }
    pub(crate) fn native_ready(&self, clock_paused: bool) -> bool {
        self.native_update_acked && (clock_paused || self.last_move > 0)
    }
    fn roll_damage(&mut self, spec: &Projectile) -> i32 {
        self.roll_between(spec.min, spec.max)
    }
    fn roll_between(&mut self, min: i32, max: i32) -> i32 {
        if min == max {
            min
        } else {
            self.seed = ((self.seed as u64 * 16807) % 2147483647) as u32;
            min + (self.seed % (max - min) as u32) as i32
        }
    }
    fn speed(&self, content: &Content, now: u64) -> Result<f32> {
        let condition = self.conditions(now);
        Ok(if condition & PARALYZED != 0 {
            0.
        } else if condition & SLOWED != 0 {
            0.004
        } else {
            (0.004 + (0.0096 - 0.004) * self.effective(content, now)?[4] as f32 / 75.)
                * if condition & SPEEDY != 0 { 1.5 } else { 1. }
        })
    }
}
#[derive(Clone)]
pub struct Bullet {
    pub owner: i32,
    pub source_kind: u16,
    pub id: u8,
    pub friendly: bool,
    pub start: Pos,
    pub pos: Pos,
    pub angle: f32,
    pub born: u64,
    pub damage: i32,
    pub spec: Projectile,
    pub hit: BTreeSet<i32>,
    pub hit_enemy: bool,
}
impl Player {
    pub(crate) fn conditions(&self, now: u64) -> i32 {
        self.lab_conditions | crate::condition::mask(&self.statuses, now)
    }
}
impl Bullet {
    pub fn position(&self, now: u64) -> Pos {
        let (x, y) = self
            .spec
            .offset(self.id, self.angle, (now - self.born) as f32);
        Pos {
            x: self.start.x + x,
            y: self.start.y + y,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Slot {
    pub object: i32,
    pub slot: usize,
    pub kind: i32,
}
#[derive(Clone, Copy)]
pub struct PortalPlan {
    locked: Option<i32>,
    opened: i32,
}
impl PortalPlan {
    fn lifetime(self) -> u64 {
        // Trapped's Create uses 20 seconds; preserve the existing Cellar's 120-second policy.
        if self.locked.is_some() { 2400 } else { 400 }
    }
}
#[derive(Clone)]
pub struct InventoryPlan {
    ground_dye: Option<ground_dye::Reservation>,
    pub player: i32,
    pub items: Vec<i32>,
    pub revision: i32,
    pub account: i32,
    pub character: i32,
    pub bags: Vec<(i32, Vec<i32>)>,
    pub effects: Vec<crate::content::ConsumableEffect>,
    pub stat_growth: Option<(Character, Slot, usize, i32)>,
    pub potion: Option<Slot>,
    pub appearance: Option<(Slot, u32, u32)>,
    pub xp_boost: Option<Slot>,
    pub(crate) companion: Option<companions::Plan>,
    pub vaults: Vec<Vault>,
    pub(crate) vault_purchase: Option<shop::VaultPurchase>,
    pub dropped: Option<i32>,
    pub unlock: Option<PortalPlan>,
    pub collected: Option<u16>,
    pub purchase: Option<(i32, crate::store::Purchase)>,
    loot_slot: Option<(i32, usize)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Action {
    Move { x: f32, y: f32 },
    Shoot { angle: f32 },
    Stop,
    Use { slot: usize, x: f32, y: f32 },
    Swap { from: Slot, to: Slot },
    Drop { slot: Slot },
}

struct Spawn {
    kind: u16,
    pos: Pos,
    entity: Option<i32>,
    camp: Option<usize>,
}
struct Admission {
    account: i32,
    character: i32,
    expires: u64,
    consumed: bool,
}
pub struct World {
    pub content: Arc<Content>,
    pub map: Arc<Map>,
    navigation_geometry: Box<std::cell::RefCell<view::GeometryCache>>,
    pub(crate) realm_layout: Option<String>,
    performance: std::cell::RefCell<crate::performance::Window>,
    pub entities: entity_store::Entities,
    actor_ids: BTreeSet<i32>,
    // Pool effects and Beach announcements must not scan every actor each tick.
    world_effect_ids: BTreeSet<i32>,
    // Superset of actors that ever received an expiry or parent; mechanic_expiry rechecks the exact predicate.
    timed_ids: BTreeSet<i32>,
    gun_ids: BTreeSet<i32>,
    pentaract_ids: BTreeSet<i32>,
    // Terminal deadlines run in every world, including Realms without any Flamers.
    abyss_flamer_ids: BTreeSet<i32>,
    // Only fully dormant ordinary population can leave this set; clocks and unclassified births stay global.
    npc_global_ids: BTreeSet<i32>,
    non_population_ids: BTreeSet<i32>,
    quests_by_level: BTreeMap<Option<i32>, BTreeSet<i32>>,
    pub players: BTreeMap<i32, Player>,
    pub guild: Option<(i32, i32)>,
    pub bullets: Vec<Bullet>,
    // Collision callbacks can emit while the original batch is outside this vector.
    detached_bullets: usize,
    grenades: Vec<mechanics::Grenade>,
    fauna_births: Vec<fauna_births::Birth>,
    named_births: Vec<named_births::Birth>,
    lab_tosses: Vec<lab::Toss>,
    manor_births: Vec<manor::Toss>,
    lava_patches: BTreeMap<usize, (u64, u8)>,
    locker: Option<davy::Locker>,
    ability_state: abilities::State,
    companions: companions::State,
    finale: endgame_ai::Actors,
    cells: cell_index::CellIndex,
    fountains: Vec<(i32, Pos)>,
    next_fountain_heal: u64,
    rng: rand::rngs::StdRng,
    pub now: u64,
    last_npc_step: u64,
    attack_epoch: Instant,
    clock: crate::clock::Clock,
    new_tick_at: Instant,
    pub(crate) prepared_seed: Option<u64>,
    pub tick: u64,
    ids: Arc<AtomicI32>,
    pub name: String,
    background: i32,
    pub safe: bool,
    pub owner: Option<i32>,
    return_route: Option<DungeonReturn>,
    pub max_players: usize,
    admissions: BTreeMap<Vec<u8>, Admission>,
    encounter: bool,
    population: Vec<Spawn>,
    camps: Vec<population::Camp>,
    population_regions: [population_density::Region; 13],
    population_counts: [usize; 13],
    population_next: u64,
    god_reproduction: BTreeMap<i32, u64>,
    setpieces: Vec<realm_setpieces::Area>,
    event_sites: Vec<realm_setpieces::EventSite>,
    portal_requests: Vec<(u16, Pos)>,
    critical_deaths: Vec<i32>,
    critical_killers: BTreeMap<i32, (u16, String)>,
    event_reserves: Vec<realm_events::ReservedEvent>,
    event_waiting: Option<i32>,
    event_choice: Option<u16>,
    event_rng: rand::rngs::StdRng,
    admission_open: bool,
    bag_expiry: BTreeMap<i32, u64>,
    cleared: bool,
    clear_exit: Option<i32>,
    vaults: BTreeMap<i32, Vault>,
    closed_vaults: BTreeMap<i32, i32>,
    trades: BTreeMap<i32, trade::Trade>,
    trade_requests: BTreeMap<i32, (i32, u64)>,
}
fn allocate_id(ids: &AtomicI32) -> Result<i32> {
    let mut id = ids.load(Ordering::Relaxed);
    loop {
        let next = id
            .checked_add(1)
            .context("entity identity space exhausted")?;
        match ids.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(id),
            Err(current) => id = current,
        }
    }
}
#[derive(Clone, Copy)]
enum Target {
    World(i32),
    Player(i32),
}
const TRANSFER_REVOKED: u8 = 1;
const TRANSFER_WRITE: u8 = 2;
const TRANSFER_DEPARTED: u8 = 4;
const TRANSFER_CONSUMED: u8 = 8;
const TRANSFER_ADMITTED: u8 = 16;
const TRANSFER_RESERVED: u8 = 32;
const TRANSFER_RETIRED: u8 = 64;
#[derive(Clone)]
struct TransferAuthority(Arc<AtomicU8>);
impl TransferAuthority {
    fn transition(&self, set: u8, required: u8, forbidden: u8) -> Result<()> {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                (state & TRANSFER_REVOKED == 0
                    && state & required == required
                    && state & forbidden == 0)
                    .then_some(state | set)
            })
            .map(|_| ())
            .map_err(|_| protocol("transfer authority revoked or already used"))
    }
    fn state(&self) -> u8 {
        self.0.load(Ordering::SeqCst)
    }
}
pub(crate) struct TransferReservation {
    key: Vec<u8>,
    authority: TransferAuthority,
    armed: bool,
}
impl TransferReservation {
    pub(crate) fn key(&self) -> &[u8] {
        &self.key
    }
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
    pub(crate) fn revoke(&self) {
        if self.armed {
            self.authority
                .0
                .fetch_or(TRANSFER_REVOKED, Ordering::SeqCst);
        }
    }
}
impl Drop for TransferReservation {
    fn drop(&mut self) {
        self.revoke();
    }
}
pub(crate) struct TransferArrival {
    key: Vec<u8>,
    authority: TransferAuthority,
}
struct ExitIntent {
    source: i32,
    portal: i32,
    player: i32,
    dungeon: crate::dungeon::HistoricalDungeon,
}
struct Grant {
    entry_portal: Option<i32>,
    entry_witness: Option<dungeon_entry::Witness>,
    exit: Option<ExitIntent>,
    authority: TransferAuthority,
    origin: Option<i32>,
    account: i32,
    character: i32,
    destination: i32,
    key_time: i32,
    expires: u64,
}
struct Portal {
    origin: i32,
    destination: Option<i32>,
    expires: u64,
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum DungeonReturn {
    Realm(i32),
    Random,
}
pub struct Directory {
    pub clock: crate::clock::Clock,
    pub worlds: execution::Worlds,
    templates: BTreeMap<String, Arc<Map>>,
    realms: Vec<crate::terrain::realms::Source>,
    realm_rng: rand::rngs::StdRng,
    content: Arc<Content>,
    ids: Arc<AtomicI32>,
    grants: BTreeMap<Vec<u8>, Grant>,
    portals: BTreeMap<i32, Portal>,
    chat_budgets: BTreeMap<i32, chat::ChatBudget>,
    guild_invites: BTreeMap<i32, guilds::Invitation>,
    endgame: endgame::RealmDirectory,
    teleports: BTreeMap<i32, Instant>,
    idle: BTreeMap<i32, u64>,
    next_world: i32,
    pub tick: u64,
    pub max_tick_us: u64,
    diagnostic_ticks: std::collections::VecDeque<(Instant, u64, u64)>,
    performance: crate::performance::Window,
    attribution: crate::performance::attribution::Attribution,
    max_players: usize,
    prepared_seed: Option<u64>,
}
impl Directory {
    pub fn bug_context(&self, player: i32) -> Result<serde_json::Value> {
        let world_id = self.location(player)?;
        let (total_players, total_entities, total_projectiles) = self
            .worlds
            .summaries()
            .fold((0, 0, 0), |(p, e, b), (_, w)| {
                (p + w.players.len(), e + w.entities, b + w.bullets)
            });
        let world = self.worlds.get(&world_id).context("bug world missing")?;
        let p = world.players.get(&player).context("bug player missing")?;
        let ticks = self.diagnostic_ticks.iter().filter(|(t, _, _)| t.elapsed().as_secs() <= 120).map(|(time, duration, late)| serde_json::json!({"age_ms": time.elapsed().as_millis(), "execution_us": duration, "scheduling_lateness_us": late})).collect::<Vec<_>>();
        Ok(
            serde_json::json!({"world_id": world_id, "world_name": world.name, "player_id": player,
            "character_id": p.character.id, "player_name": p.character.name, "class": p.character.class, "level": p.character.level,
            "position": {"x": p.pos.x, "y": p.pos.y}, "visible_entities": p.known.len(),
            "world_players": world.players.len(), "world_entities": world.entities.len(), "world_projectiles": world.bullets.len(),
            "native_acknowledgements": {"weapon_tick_records":p.weapon_ticks.len(),"goto":p.goto_acks.len(),"shoot":p.shoot_acks.len(),
                "oldest_goto_ms":p.goto_acks.front().map(|(_,at)|at.elapsed().as_millis()),
                "oldest_shoot_pause_excluding_ms":p.shoot_acks.front().map(|(at,_)|world.clock.now().duration_since(*at).as_millis())},
            "tick_interval_ms": 50, "recent_ticks": ticks, "tick": self.tick,
            "total_worlds": self.worlds.len(), "total_players": total_players,
            "total_entities": total_entities, "total_projectiles": total_projectiles, "recent_tick_scope":"coordinator"}),
        )
    }
    pub fn announce(&self, text: &str) -> usize {
        self.worlds
            .values()
            .map(|w| w.broadcast_text(chat::ANNOUNCEMENT, text))
            .sum()
    }
    pub fn competitive_characters(&self) -> Vec<Character> {
        self.worlds
            .values()
            .flat_map(|w| w.players.values())
            .filter(|p| p.controller == Controller::Human && p.persistence_failure.is_none())
            .map(|p| p.character.clone())
            .collect()
    }
    pub fn health(&self, started: Instant) -> serde_json::Value {
        let worlds: Vec<_> = self.worlds.summaries().collect();
        let players: usize = worlds.iter().map(|(_, w)| w.players.len()).sum();
        let qa_actors = worlds
            .iter()
            .flat_map(|(_, w)| w.players.values())
            .filter(|p| p.controller == Controller::Qa)
            .count();
        let bots = worlds
            .iter()
            .flat_map(|(_, w)| w.players.values())
            .filter(|p| p.controller == Controller::Autonomous)
            .count();
        serde_json::json!({
            "status":"development","uptime_seconds":started.elapsed().as_secs(),
            "worlds":worlds.len(),"players":players-qa_actors-bots,"bots":bots,"qa_actors":qa_actors,
            "entities":worlds.iter().map(|(_,w)|w.entities).sum::<usize>(),
            "projectiles":worlds.iter().map(|(_,w)|w.bullets).sum::<usize>(),
            "tick":self.tick,"max_tick_us":self.max_tick_us,"tick_scope":"coordinator, not world simulation",
            "world_progress":worlds.iter().map(|(_,w)|serde_json::json!({"tick":w.tick,"simulation_ms":w.now,"published_age_ms":w.published.elapsed().as_millis()})).collect::<Vec<_>>(),
            "paused":self.clock.paused(),"process":std::process::id(),"revision":option_env!("GITHUB_SHA"),
        })
    }

    #[cfg(test)]
    pub(crate) fn test_with_seed(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
    ) -> Result<Self> {
        Self::fixture(path, content, max_players, seed, None)
    }

    #[cfg(test)]
    fn fixture(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        prepared_seed: Option<u64>,
    ) -> Result<Self> {
        // Restored Grove populations need room alongside both finite 65-tile graveyards.
        let map = Arc::new(crate::terrain::generate(&content, seed, 1024)?);
        Self::configured(
            path,
            content,
            max_players,
            seed,
            prepared_seed,
            vec![crate::terrain::realms::Source::Fixture(map)],
            2,
        )
        .with_context(|| format!("test Realm fixture map/layout seed {seed}"))
    }
    pub fn with_seed(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        realm_count: usize,
    ) -> Result<Self> {
        Self::configured(
            path,
            content,
            max_players,
            seed,
            None,
            crate::terrain::realms::sources(path)?,
            realm_count,
        )
    }
    pub(crate) fn prepared(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        realm_count: usize,
    ) -> Result<Self> {
        Self::configured(
            path,
            content,
            max_players,
            seed,
            Some(seed),
            crate::terrain::realms::sources(path)?,
            realm_count,
        )
    }
    fn configured(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        prepared_seed: Option<u64>,
        realms: Vec<crate::terrain::realms::Source>,
        realm_count: usize,
    ) -> Result<Self> {
        ensure!((2..=3).contains(&realm_count), "realm_count must be 2 or 3");
        ensure!(!realms.is_empty(), "Realm layout pool is empty");
        crate::endgame::validate(&content)?;
        let mut templates = BTreeMap::new();
        for name in [
            "tutorial",
            "kitchen",
            "nexus",
            "nexus-limbo",
            "vault",
            crate::endgame::CASTLE,
            crate::endgame::CHAMBER,
            crate::endgame::CELLAR,
            "guild-hall-1",
            "guild-hall-2",
            "guild-hall-3",
            "guild-hall-4",
        ] {
            templates.insert(
                name.to_string(),
                Arc::new(Map::load(&path.join(format!("{name}.wmap")), &content)?),
            );
        }
        let mut d = Self {
            clock: crate::clock::Clock::default(),
            worlds: execution::Worlds::new(),
            templates,
            realms,
            realm_rng: rand::rngs::StdRng::seed_from_u64(seed),
            content,
            ids: Arc::new(AtomicI32::new(1000)),
            grants: BTreeMap::new(),
            portals: BTreeMap::new(),
            chat_budgets: BTreeMap::new(),
            guild_invites: BTreeMap::new(),
            endgame: endgame::RealmDirectory::default(),
            teleports: BTreeMap::new(),
            idle: BTreeMap::new(),
            next_world: 1,
            tick: 0,
            max_tick_us: 0,
            diagnostic_ticks: std::collections::VecDeque::new(),
            performance: crate::performance::Window::default(),
            attribution: crate::performance::attribution::Attribution::default(),
            max_players,
            prepared_seed,
        };
        for (id, template, name, safe) in [
            (-1, "tutorial", "Tutorial", false),
            (-2, "nexus", "Nexus", true),
            (-3, "realm-shore", "Realm", false),
            (-4, "nexus-limbo", "Nexus Limbo", true),
        ] {
            let world = d.make(template, name, safe, None)?;
            d.worlds.insert(id, world);
        }
        for slot in 0..realm_count {
            let origin = if slot == 0 {
                -3
            } else {
                let id = d.next_world;
                let world = d.make("realm-shore", "Realm", false, None)?;
                d.worlds.insert(id, world);
                d.next_world += 1;
                id
            };
            d.bind_realm_heroes(origin, d.worlds[&origin].hero_identities())?;
            let name =
                (seed as usize % realm_portal::NAMES.len() + slot) % realm_portal::NAMES.len();
            let portal = d.install_realm_portal(slot)?;
            let record = d.endgame.realms.get_mut(&origin).unwrap();
            record.name = name;
            record.portal = Some(portal);
        }
        d.sync_realm_portal();
        Ok(d)
    }
    fn make(
        &mut self,
        template: &str,
        name: &str,
        safe: bool,
        owner: Option<i32>,
    ) -> Result<World> {
        let layout =
            (template == "realm-shore").then(|| self.realm_rng.gen_range(0..self.realms.len()));
        let mut world = World::with_ids(
            self.content.clone(),
            if let Some(index) = layout {
                self.realms[index].open(&self.content)?
            } else if template == "pirate-cave" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::pirate::generate(&self.content, seed)?)
            } else if template == "abyss-of-demons" {
                let seed = match self.prepared_seed {
                    Some(seed) => seed ^ self.next_world as u64,
                    None => rand::random(),
                };
                Arc::new(crate::dungeon::abyss::generate(&self.content, seed)?)
            } else if template == "manor-of-the-immortals" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::manor::generate(&self.content, seed)?)
            } else if template == "beachzone" {
                Arc::new(crate::dungeon::beach::generate(&self.content)?)
            } else if template == "ocean-trench" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::trench::generate(&self.content, seed)?)
            } else if template == "forbidden-jungle" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::jungle::generate(&self.content, seed)?)
            } else if template == "spider-den" {
                let seed = match self.prepared_seed {
                    Some(seed) => seed ^ self.next_world as u64,
                    None => rand::random(),
                };
                Arc::new(crate::dungeon::spider::generate(&self.content, seed)?)
            } else if template == "mad-lab" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::lab::generate(&self.content, seed)?)
            } else if template == "tomb-of-the-ancients" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::tomb::generate(&self.content, seed)?)
            } else if template == "snake-pit" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::snake_pit(&self.content, seed)?)
            } else if template == "sprite-world" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::sprite::generate(&self.content, seed)?)
            } else if template == crate::dungeon::davy::TEMPLATE {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::davy::generate(&self.content, seed)?)
            } else if template == "undead-lair" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::undead::generate(&self.content, seed)?)
            } else {
                self.templates
                    .get(template)
                    .context("world template missing")?
                    .clone()
            },
            self.ids.clone(),
        )?;
        world.realm_layout = layout.map(|index| self.realms[index].name().to_owned());
        world.clock = self.clock.clone();
        world.attack_epoch = self.clock.now();
        world.new_tick_at = self.clock.now();
        #[cfg(test)]
        if layout.is_some_and(|index| {
            matches!(
                &self.realms[index],
                crate::terrain::realms::Source::Fixture(_)
            )
        }) {
            // Reproduce fixture populations without granting prepared-world authority or consuming layout RNG.
            let seed = self.realm_rng.clone().r#gen::<u64>();
            world.rng = rand::rngs::StdRng::seed_from_u64(seed);
            world.event_rng = rand::rngs::StdRng::seed_from_u64(seed ^ 0x7265616c6d);
            tracing::debug!(seed, "test Realm population seeded");
        }
        if let Some(seed) = self.prepared_seed {
            let seed = seed ^ self.next_world as u64;
            world.rng = rand::rngs::StdRng::seed_from_u64(seed);
            world.event_rng = rand::rngs::StdRng::seed_from_u64(seed ^ 0x7265616c6d);
            world.prepared_seed = Some(seed);
        }
        world.name = name.into();
        // Preserve the blue Nexus/Vault backdrop, not dungeon void: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/realm/worlds/Vault.cs
        world.background = if matches!(template, "nexus" | "vault") {
            2
        } else {
            0
        };
        world.safe = safe;
        world.owner = owner;
        world.max_players = self.max_players;
        world.encounter = template == "realm-shore"
            || template == crate::endgame::CASTLE
            || crate::dungeon::DUNGEONS
                .iter()
                .chain(crate::endgame::DUNGEONS)
                .any(|(_, name, _, _)| *name == template);
        if template == "realm-shore" {
            let started = Instant::now();
            let groves = self.realms[layout.context("Realm layout selection missing")?].groves()?;
            world.populate_authored_groves(&groves)?;
            // No period numeric rarity survives; keep this a rare inland encounter, not modern fixed Reef spawns.
            let beach_bum = world.rng.gen_ratio(1, 200);
            world.populate_tomb_entry()?;
            world.populate_coil_snakes()?;
            if beach_bum {
                world.populate_beach_bum()?;
            }
            world.populate_realm_heroes()?;
            world.populate_named_quests(!groves.is_empty())?;
            world.populate_realm_events()?;
            world.populate_hermit()?;
            world.populate_manor_entry()?;
            world.populate_ghost_ship()?;
            world.populate_lost_lands()?;
            world.populate_realm_population()?;
            // Finite crystals must stay outside ordinary respawns: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/setpieces/SetPieces.cs
            let crystal = world.place_named_setpiece(0x935, &[1])?;
            world.spawn_entity(0x935, crystal, vec![])?;
            world.reserve_realm_events()?;
            tracing::info!(
                layout = world.realm_layout,
                entities = world.entities.len(),
                actors = world.actor_ids.len(),
                elapsed_ms = started.elapsed().as_millis(),
                "Realm simulation populated"
            );
        }
        world.prepare_endgame()?;
        if template == crate::dungeon::davy::TEMPLATE {
            world.prepare_locker()?;
        }
        if template == "sprite-world" {
            let exits: Vec<_> = world
                .entities
                .values()
                .filter(|e| e.kind == 0x70e)
                .map(|e| e.id)
                .collect();
            ensure!(
                exits.len() == 1,
                "Sprite World requires one authored completion exit"
            );
            world.entities.get_mut(&exits[0]).unwrap().published = false;
            world.clear_exit = Some(exits[0]);
        }
        if world.encounter {
            let kinds: BTreeSet<_> = world
                .entities
                .values()
                .filter(|e| {
                    let object = &world.content.objects[&e.kind];
                    object.enemy && (object.class != "Wall" || object.passive_destructible())
                })
                .map(|e| e.kind)
                .collect();
            for kind in kinds {
                if !world.content.objects[&kind].passive_destructible() {
                    crate::encounter::validate(&world.content, kind)?;
                }
                crate::loot::validate(&world.content, kind)?;
            }
        }
        for kind in [
            0x173, 0x653, 0x918, 0x919, 0xd07, 0xd08, 0xd09, 0xd0a, 0xdaa, 0xdab, 0xdac, 0xdad,
            0xdae, 0xdaf, 0xdb0, 0xdb1, 0xdb2,
        ] {
            crate::encounter::validate(&world.content, kind)?;
            crate::loot::validate(&world.content, kind)?;
        }
        Ok(world)
    }
    fn resolve(&mut self, target: Target) -> Result<&mut World> {
        match target {
            Target::World(id) => self.worlds.get_mut(&id).context("world no longer exists"),
            Target::Player(id) => self
                .worlds
                .values_mut()
                .find(|w| w.players.contains_key(&id))
                .context("player missing from world directory"),
        }
    }
    pub(crate) fn character_transferring(&self, character: i32) -> bool {
        self.grants.values().any(|g| {
            g.character == character
                && g.authority.state() & (TRANSFER_REVOKED | TRANSFER_CONSUMED) == 0
        })
    }
    pub fn location(&self, player: i32) -> Result<i32> {
        self.worlds
            .summaries()
            .find(|(_, w)| w.players.contains_key(&player))
            .map(|(id, _)| *id)
            .context("player missing from world directory")
    }
    pub(crate) fn available_realms(&self) -> Vec<(i32, usize)> {
        self.endgame
            .realms
            .iter()
            .filter_map(|(id, record)| {
                let world = self.worlds.summary(id).expect("Realm metadata missing");
                (record.enabled && world.admission_open && world.occupancy < world.max_players)
                    .then_some((*id, world.occupancy))
            })
            .collect()
    }
    pub fn initial(&self, game: i32) -> Result<i32> {
        let destination = match game {
            0 | -2 => -2,
            -1 => -1,
            -3 => self
                .available_realms()
                .choose(&mut rand::thread_rng())
                .map(|(id, _)| *id)
                .ok_or_else(|| controller("no open Realm has capacity"))?,
            -4 => -4,
            _ => {
                return Err(anyhow::anyhow!(CloseReason::Protocol)
                    .context(format!("world entry {game} requires a transfer grant")));
            }
        };
        ensure!(
            self.worlds.contains_key(&destination),
            "entry world missing"
        );
        ensure!(
            self.worlds[&destination].admission_open,
            anyhow::anyhow!(CloseReason::Controller).context("Realm is closed")
        );
        Ok(destination)
    }
    pub(crate) fn prepared_instance_with_seed(&mut self, template: &str, seed: u64) -> Result<i32> {
        ensure!(
            self.prepared_seed.is_some(),
            "prepared encounter requires isolated test state"
        );
        let previous = self.prepared_seed;
        self.prepared_seed = Some(seed ^ self.next_world as u64);
        let result = self.prepared_instance(template);
        self.prepared_seed = previous;
        result
    }
    pub(crate) fn prepared_instance(&mut self, template: &str) -> Result<i32> {
        ensure!(
            self.prepared_seed.is_some(),
            "prepared encounter requires isolated test state"
        );
        if template != crate::endgame::CASTLE {
            return self.instance(template, None, Some(DungeonReturn::Random));
        }
        let world = self.make(template, "Oryx's Castle", false, None)?;
        let id = self.next_world;
        self.next_world = id
            .checked_add(1)
            .context("world identity space exhausted")?;
        self.worlds.insert(id, world);
        self.idle.insert(id, self.tick);
        Ok(id)
    }
    fn reclaimable_world(&self, id: i32) -> bool {
        if id <= 0 {
            return false;
        }
        let world = self
            .worlds
            .summary(&id)
            .expect("reclaimable world metadata missing");
        world.players.is_empty()
            && world.bags_empty
            && world.admissions_empty
            && !self.endgame.references_world(id)
            && !self
                .grants
                .values()
                .any(|g| g.destination == id && g.authority.state() & TRANSFER_REVOKED == 0)
            && !self
                .portals
                .values()
                .any(|p| p.origin == id || p.destination == Some(id))
    }
    pub(crate) fn instance(
        &mut self,
        template: &str,
        account: Option<i32>,
        return_route: Option<DungeonReturn>,
    ) -> Result<i32> {
        ensure!(
            account.is_none() || return_route.is_none(),
            "private world cannot have a dungeon return route"
        );
        ensure!(
            return_route.is_some()
                || !crate::dungeon::DUNGEONS
                    .iter()
                    .any(|(_, name, _, _)| *name == template),
            "dungeon requires an explicit return route"
        );
        ensure!(
            matches!((template, account), ("vault", Some(_)) | ("kitchen", None))
                || (account.is_none()
                    && crate::dungeon::DUNGEONS
                        .iter()
                        .chain(crate::endgame::DUNGEONS)
                        .any(|(_, name, _, _)| *name == template)),
            "invalid private/dungeon instance policy"
        );
        if let Some(account) = account
            && let Some((id, _)) = self.worlds.iter().find(|(_, w)| w.owner == Some(account))
        {
            return Ok(*id);
        }
        let mut world = self.make(
            template,
            if template == "vault" {
                "Vault"
            } else if template == "kitchen" {
                "Kitchen"
            } else {
                crate::dungeon::DUNGEONS
                    .iter()
                    .chain(crate::endgame::DUNGEONS)
                    .find(|(_, name, _, _)| *name == template)
                    .context("dungeon registration missing")?
                    .2
            },
            template == "vault",
            account,
        )?;
        // Entrances expire before their dungeons; preserve RECONNECT's exact world identity: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/kabam/rotmg/messaging/impl/incoming/Reconnect.as
        world.return_route = return_route;
        let id = self.next_world;
        self.next_world = id
            .checked_add(1)
            .context("world identity space exhausted")?;
        self.worlds.insert(id, world);
        self.idle.insert(id, self.tick);
        tracing::info!(
            world = id,
            template,
            account,
            return_route = ?return_route,
            "world instance created"
        );
        Ok(id)
    }
    fn dungeon_return_from(&self, origin: i32) -> Result<DungeonReturn> {
        let world = &self.worlds[&origin];
        if world.name == "Realm" {
            Ok(DungeonReturn::Realm(origin))
        } else if crate::dungeon::DUNGEONS
            .iter()
            .any(|(_, _, name, _)| *name == world.name)
        {
            world
                .return_route
                .context("parent dungeon has no return route")
        } else {
            Ok(DungeonReturn::Random)
        }
    }
    fn dungeon_realm(&self, world: &World) -> Result<i32> {
        match world.return_route.context("dungeon has no return route")? {
            DungeonReturn::Random => self.initial(-3),
            DungeonReturn::Realm(id) => {
                ensure!(
                    self.endgame.realms.get(&id).is_some_and(|r| r.enabled)
                        && self.worlds.get(&id).is_some_and(|w| w.admission_open),
                    controller("This dungeon's Realm has closed. Use Nexus.")
                );
                Ok(id)
            }
        }
    }
    pub fn destination(&mut self, player: i32, portal: Option<i32>) -> Result<(i32, i32, i32)> {
        let origin = self.location(player)?;
        let world = &self.worlds[&origin];
        let p = &world.players[&player];
        ensure!(
            p.character.alive && p.character.hp > 0 && !p.death_pending && !p.departing,
            protocol("character cannot transfer")
        );
        let (account, character) = (p.character.account, p.character.id);
        let kind = if let Some(id) = portal {
            let e = world
                .entities
                .get(&id)
                .ok_or_else(|| protocol("portal missing"))?;
            ensure!(e.published, protocol("portal not published"));
            ensure!(p.pos.distance(e.pos) <= 2., protocol("portal out of reach"));
            e.kind
        } else {
            0x712
        };
        let destination = match kind {
            0x712 if origin == -2 && portal.is_some() => {
                let (id, record) = self
                    .endgame
                    .realms
                    .iter()
                    .find(|(_, record)| record.portal == portal)
                    .ok_or_else(|| protocol("stale named Realm portal"))?;
                ensure!(
                    record.enabled && self.worlds[id].admission_open,
                    controller("Realm is closed")
                );
                *id
            }
            0x703 if matches!(world.name.as_str(), "Snake Pit" | "Abyss of Demons") => {
                self.dungeon_realm(world)?
            }
            0x703 | 0x70d | 0x712 | 0x71d | crate::dungeon::tomb::EXIT => -2,
            0x704 | 0x70e => self.dungeon_realm(world)?,
            0x71c => self.initial(-3)?,
            // Kitchen destination: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/realm/entities/player/Player.cs#L387-L416
            // Reconstruction: a fresh Kitchen per use; Trapped instead caches the first Kitchen on the portal and reuses it.
            0x71e => self.instance("kitchen", None, None)?,
            0x720 => self.instance("vault", Some(account), None)?,
            0x72f => {
                let guild = p
                    .presentation
                    .guild
                    .as_ref()
                    .ok_or_else(|| protocol("You are not in a guild."))?
                    .id;
                *self
                    .worlds
                    .iter()
                    .find(|(_, w)| w.guild.is_some_and(|(id, _)| id == guild))
                    .context("Guild Hall has not been initialized.")?
                    .0
            }
            _ if portal.is_some_and(|id| self.portals.contains_key(&id)) => {
                let binding = self
                    .portals
                    .get(&portal.context("dungeon portal identity missing")?)
                    .context("dropped portal has no live instance binding")?;
                ensure!(
                    binding.origin == origin && binding.expires > self.tick,
                    protocol("dungeon portal binding expired or belongs to another world")
                );
                binding
                    .destination
                    .ok_or_else(|| protocol("This dungeon is not implemented yet."))?
            }
            _ => {
                return Err(anyhow::anyhow!(CloseReason::Protocol)
                    .context(format!("unsupported portal destination {kind:#x}")));
            }
        };
        ensure!(
            destination != origin,
            protocol("already in destination world")
        );
        Ok((account, character, destination))
    }
    pub fn issue(
        &mut self,
        account: i32,
        character: i32,
        destination: i32,
    ) -> Result<(i32, Vec<u8>, String)> {
        self.retire_transfer_grants();
        ensure!(
            self.grants.len() < 64,
            anyhow::anyhow!(CloseReason::Controller).context("transfer grant limit reached")
        );
        let world = self
            .worlds
            .get(&destination)
            .context("transfer destination missing")?;
        ensure!(
            world.owner.is_none_or(|owner| owner == account),
            "private destination belongs to another account"
        );
        ensure!(
            world.admission_open,
            anyhow::anyhow!(CloseReason::Controller).context("Realm is closed")
        );
        ensure!(
            world.occupancy() < world.max_players,
            anyhow::anyhow!(CloseReason::Controller).context("transfer destination full")
        );
        ensure!(
            !self.worlds.values().any(|w| w
                .admissions
                .values()
                .any(|a| a.account == account && a.expires > w.now)),
            anyhow::anyhow!(CloseReason::Controller)
                .context("account already has a reserved admission")
        );
        let origin = self
            .worlds
            .iter()
            .find(|(_, w)| {
                w.players
                    .values()
                    .any(|p| p.character.account == account && p.character.id == character)
            })
            .map(|(id, _)| *id);
        let reserved = self.endgame.forced.iter().any(|(id, route)| {
            self.worlds
                .get(&route.origin)
                .and_then(|w| w.players.get(id))
                .is_some_and(|p| p.character.account == account && p.character.id == character)
        });
        // Queued parties can exceed the live-grant bound; ordinary transfers must not starve them.
        ensure!(
            reserved || self.grants.len() + self.endgame.forced.len() < 64,
            controller("transfer grant capacity reserved for collective departures")
        );
        let name = world.name.clone();
        let mut key = vec![0; 16];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
        let key_time = i32::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        )?;
        let world = self
            .worlds
            .get_mut(&destination)
            .context("transfer destination missing")?;
        world.admissions.insert(
            key.clone(),
            Admission {
                account,
                character,
                expires: world.now + 120000,
                consumed: false,
            },
        );
        self.grants.insert(
            key.clone(),
            Grant {
                entry_portal: None,
                entry_witness: None,
                exit: None,
                authority: TransferAuthority(Arc::new(AtomicU8::new(0))),
                origin,
                account,
                character,
                destination,
                key_time,
                expires: self.tick + 2400,
            },
        );
        tracing::info!(
            account,
            character,
            destination,
            expires_tick = self.tick + 2400,
            "transfer grant issued"
        );
        Ok((key_time, key, name))
    }
    pub(crate) fn bind_exit(&mut self, id: i32, portal: Option<i32>, key: &[u8]) -> Result<()> {
        let Some(portal) = portal else { return Ok(()) };
        self.grants
            .get_mut(key)
            .context("portal grant missing")?
            .entry_portal = Some(portal);
        let source = self.location(id)?;
        let world = &self.worlds[&source];
        let player = &world.players[&id];
        let grant = self.grants.get(key).context("earned exit grant missing")?;
        if self.worlds[&grant.destination].name != "Realm" {
            return Ok(());
        }
        let Some(dungeon) = crate::dungeon::HistoricalDungeon::from_world(&world.name) else {
            return Ok(());
        };
        let entity = world
            .entities
            .get(&portal)
            .context("earned exit portal missing")?;
        ensure!(
            player.character.alive
                && player.character.hp > 0
                && !player.death_pending
                && !player.departing
                && entity.published
                && player.pos.distance(entity.pos) <= 2.
                && (matches!(entity.kind, 0x704 | 0x70e | 0x71c)
                    || entity.kind == 0x703
                        && matches!(world.name.as_str(), "Snake Pit" | "Abyss of Demons")),
            "earned exit source portal changed"
        );
        ensure!(
            grant.origin == Some(source)
                && grant.account == player.character.account
                && grant.character == player.character.id
                && grant.expires > self.tick
                && grant.authority.state() & (TRANSFER_REVOKED | TRANSFER_CONSUMED) == 0
                && grant.exit.is_none(),
            "earned exit grant binding changed"
        );
        let grant = self.grants.get_mut(key).unwrap();
        grant.exit = Some(ExitIntent {
            source,
            portal,
            player: id,
            dungeon,
        });
        Ok(())
    }
    pub(crate) fn reserve_transfer(&mut self, key: Vec<u8>) -> Result<TransferReservation> {
        let grant = self
            .grants
            .get(&key)
            .context("transfer reservation missing")?;
        grant
            .authority
            .transition(TRANSFER_RESERVED, 0, TRANSFER_RESERVED | TRANSFER_CONSUMED)?;
        Ok(TransferReservation {
            key,
            authority: grant.authority.clone(),
            armed: true,
        })
    }
    pub(crate) fn accept_source_write(&mut self, id: i32, key: &[u8]) -> Result<()> {
        self.validate_source_exit(id, key)?;
        let grant = self.grants.get(key).context("source write grant missing")?;
        // This transition owns the uncancellable write; cancellation cannot promise to undo accepted SQL.
        grant.authority.transition(
            TRANSFER_WRITE,
            TRANSFER_RESERVED,
            TRANSFER_WRITE | TRANSFER_DEPARTED | TRANSFER_CONSUMED,
        )
    }
    pub(crate) fn retire_replacement(
        &mut self,
        arrival: TransferArrival,
        account: i32,
        character: i32,
        destination: i32,
    ) -> Result<()> {
        let grant = self
            .grants
            .get(&arrival.key)
            .context("replacement source grant missing")?;
        ensure!(
            Arc::ptr_eq(&arrival.authority.0, &grant.authority.0)
                && grant.account == account
                && grant.character == character
                && grant.destination == destination
                && grant.expires > self.tick
                && grant.exit.is_none(),
            "replacement source grant binding changed"
        );
        grant.authority.transition(
            TRANSFER_RETIRED,
            TRANSFER_CONSUMED | TRANSFER_DEPARTED,
            TRANSFER_ADMITTED | TRANSFER_RETIRED,
        )
    }
    pub(crate) fn admit_transfer(
        &mut self,
        arrival: TransferArrival,
        account: i32,
        character: i32,
        destination: i32,
    ) -> Result<()> {
        let grant = self
            .grants
            .get(&arrival.key)
            .context("arrival grant missing")?;
        ensure!(
            Arc::ptr_eq(&arrival.authority.0, &grant.authority.0)
                && grant.account == account
                && grant.character == character
                && grant.destination == destination
                && grant.expires > self.tick,
            "arrival grant changed or expired"
        );
        ensure!(
            self.worlds
                .get(&destination)
                .is_some_and(|w| w.admission_open),
            "arrival destination unavailable"
        );
        grant.authority.transition(
            TRANSFER_ADMITTED,
            TRANSFER_CONSUMED,
            TRANSFER_ADMITTED | TRANSFER_RETIRED,
        )
    }
    fn validate_exit_destination(&self, grant: &Grant) -> Result<()> {
        let destination = self
            .worlds
            .get(&grant.destination)
            .context("source transfer destination missing")?;
        ensure!(
            destination.admission_open,
            "source transfer destination unavailable"
        );
        if grant.exit.is_some() {
            ensure!(
                destination.name == "Realm" && destination.players.len() < destination.max_players,
                "earned exit destination is full or unavailable"
            );
        }
        Ok(())
    }
    fn validate_source_exit(
        &self,
        id: i32,
        key: &[u8],
    ) -> Result<Option<crate::dungeon::HistoricalDungeon>> {
        let source = self.location(id)?;
        let p = &self.worlds[&source].players[&id];
        let grant = self
            .grants
            .get(key)
            .context("source transfer grant missing")?;
        ensure!(
            grant.account == p.character.account
                && grant.character == p.character.id
                && grant.authority.state()
                    & (TRANSFER_REVOKED | TRANSFER_WRITE | TRANSFER_CONSUMED | TRANSFER_DEPARTED)
                    == 0
                && grant.authority.state() & TRANSFER_RESERVED != 0
                && grant.expires > self.tick,
            "source transfer grant stale or foreign"
        );
        self.validate_exit_destination(grant)?;
        match &grant.exit {
            None => Ok(None),
            Some(exit) => {
                ensure!(
                    exit.source == source
                        && exit.player == id
                        && p.character.alive
                        && !p.death_pending
                        && self.worlds[&source]
                            .entities
                            .get(&exit.portal)
                            .is_some_and(|e| e.published && e.pos.distance(p.pos) <= 2.),
                    "earned exit source changed"
                );
                Ok(Some(exit.dungeon))
            }
        }
    }
    pub(crate) fn begin_source_departure(
        &mut self,
        id: i32,
        key: Option<&[u8]>,
    ) -> Result<Option<Character>> {
        let source = self.location(id)?;
        if let Some(key) = key {
            let p = &self.worlds[&source].players[&id];
            if p.death_pending || !p.character.alive {
                let grant = self
                    .grants
                    .get_mut(key)
                    .context("dying source transfer grant missing")?;
                grant.authority.transition(
                    0,
                    0,
                    TRANSFER_WRITE | TRANSFER_DEPARTED | TRANSFER_CONSUMED,
                )?;
                grant.exit = None;
            }
        }
        let exit = key
            .map(|key| self.validate_source_exit(id, key))
            .transpose()?
            .flatten();
        let w = self.worlds.get_mut(&source).unwrap();
        let Some(mut snapshot) = w.begin_departure(id)? else {
            return Ok(None);
        };
        if let Some(exit) = exit {
            snapshot.lifetime.realm_exits.insert(exit);
        }
        w.players.get_mut(&id).unwrap().departure_snapshot = Some(snapshot.clone());
        Ok(Some(snapshot))
    }
    pub(crate) fn finish_source_departure(
        &mut self,
        id: i32,
        snapshot: Character,
        key: Option<Vec<u8>>,
    ) -> Result<()> {
        let source = self.location(id)?;
        ensure!(
            self.worlds[&source].players[&id]
                .departure_snapshot
                .as_ref()
                == Some(&snapshot),
            "source departure snapshot changed"
        );
        if let Some(key) = key.as_ref() {
            let grant = self
                .grants
                .get(key)
                .context("committed source transfer grant missing")?;
            self.validate_exit_destination(grant)?;
            let grant = self
                .grants
                .get_mut(key)
                .context("committed source transfer grant missing")?;
            ensure!(
                grant.account == snapshot.account
                    && grant.character == snapshot.id
                    && grant.authority.state() & (TRANSFER_REVOKED | TRANSFER_CONSUMED) == 0
                    && grant.authority.state() & TRANSFER_DEPARTED == 0,
                "committed source transfer binding changed"
            );
            grant.authority.transition(
                TRANSFER_DEPARTED,
                TRANSFER_WRITE,
                TRANSFER_DEPARTED | TRANSFER_CONSUMED,
            )?;
        }
        if let Some(key) = key.as_ref() {
            let witness = self.capture_dungeon_entry(id, key);
            self.grants
                .get_mut(key)
                .context("departed grant missing")?
                .entry_witness = witness;
        }
        self.worlds.get_mut(&source).unwrap().finish_departure(id)
    }
    pub fn consume(
        &mut self,
        account: i32,
        destination: i32,
        key_time: i32,
        key: Vec<u8>,
    ) -> Result<(i32, TransferArrival)> {
        let grant = self
            .grants
            .get(&key)
            .ok_or_else(|| protocol("unknown or already used transfer grant"))?;
        ensure!(
            grant.authority.state() & (TRANSFER_REVOKED | TRANSFER_CONSUMED) == 0,
            protocol("transfer grant already used")
        );
        ensure!(
            grant.authority.state() & TRANSFER_RESERVED == 0
                || grant.authority.state() & TRANSFER_DEPARTED != 0,
            protocol("earned exit source has not retired")
        );
        ensure!(
            grant.expires > self.tick,
            protocol("transfer grant expired")
        );
        ensure!(
            grant.account == account
                && grant.destination == destination
                && grant.key_time == key_time,
            protocol("transfer grant binding mismatch")
        );
        ensure!(
            self.worlds.contains_key(&destination),
            protocol("transfer destination expired")
        );
        ensure!(
            self.worlds[&destination].admission_open,
            anyhow::anyhow!(CloseReason::Controller).context("Realm is closed")
        );
        ensure!(
            !self.worlds.values().any(|w| {
                w.players.values().any(|p| {
                    p.character.account == grant.account && p.character.id == grant.character
                })
            }),
            controller("transfer origin still owns the character; durable departure incomplete")
        );
        let character = grant.character;
        let world = self
            .worlds
            .get_mut(&destination)
            .context("transfer destination missing")?;
        let admission = world
            .admissions
            .get_mut(&key)
            .context("transfer admission expired or cancelled")?;
        ensure!(
            admission.account == account
                && admission.character == character
                && admission.expires > world.now
                && !admission.consumed,
            "transfer admission binding/expiry changed"
        );
        admission.consumed = true;
        // HELLO consumes the key before LOAD enters the world; keep its capacity reservation until then.
        grant.authority.transition(
            TRANSFER_CONSUMED,
            if grant.authority.state() & TRANSFER_RESERVED != 0 {
                TRANSFER_DEPARTED
            } else {
                0
            },
            TRANSFER_CONSUMED,
        )?;
        let arrival = TransferArrival {
            key: key.clone(),
            authority: grant.authority.clone(),
        };
        if destination > 0 {
            self.idle.insert(destination, self.tick);
        }
        tracing::info!(account, character, destination, "transfer grant consumed");
        Ok((character, arrival))
    }
    pub(crate) fn cancel_admission(&mut self, account: i32, key: Vec<u8>) -> Result<()> {
        if let Some(grant) = self.grants.get(&key) {
            ensure!(
                grant.account == account,
                "cannot cancel another account's grant"
            );
        }
        for world in self.worlds.values_mut() {
            if let Some(admission) = world.admissions.get(&key) {
                ensure!(
                    admission.account == account,
                    "cannot cancel another account's admission"
                );
                world.admissions.remove(&key);
            }
        }
        self.grants.remove(&key);
        Ok(())
    }

    fn drop_dungeon_portal(&mut self, origin: i32, kind: u16, pos: Pos) -> Result<i32> {
        let definition = crate::dungeon::DUNGEONS
            .iter()
            .find(|(portal, _, _, _)| *portal == kind)
            .context("unregistered earned dungeon portal")?;
        ensure!(
            self.worlds[&origin].name == "Realm" && self.worlds[&origin].walkable(pos),
            "dungeon portal must be earned on the walkable Realm"
        );
        let destination = self.instance(definition.1, None, Some(DungeonReturn::Realm(origin)))?;
        let id = self
            .worlds
            .get_mut(&origin)
            .unwrap()
            .spawn_clear_portal(kind, pos, true)?;
        self.portals.insert(
            id,
            Portal {
                origin,
                destination: Some(destination),
                // The May 2012 Sprite World guide specifies 15 seconds, unlike newer references.
                expires: self.tick + if kind == 0x70c { 300 } else { 600 },
            },
        );
        tracing::info!(
            portal = id,
            kind,
            origin,
            destination,
            x = self.worlds[&origin].entities[&id].pos.x,
            y = self.worlds[&origin].entities[&id].pos.y,
            requested_x = pos.x,
            requested_y = pos.y,
            displacement = pos.distance(self.worlds[&origin].entities[&id].pos),
            "earned dungeon portal dropped"
        );
        Ok(id)
    }
    fn retire_transfer_grants(&mut self) {
        self.retire_grants(|_, _| {});
    }
    fn retire_grants(&mut self, after: impl Fn(&mut execution::Worlds, i32)) {
        let mut retired = Vec::new();
        self.grants.retain(|key, g| {
            let keep = g.expires > self.tick
                && g.authority.state() & TRANSFER_REVOKED == 0
                && !(g.authority.state() & TRANSFER_CONSUMED != 0
                    && self
                        .worlds
                        .summary(&g.destination)
                        .expect("grant destination metadata missing")
                        .players
                        .values()
                        .any(|p| p.account == g.account && p.character == g.character));
            if !keep {
                retired.push((g.destination, key.clone()));
            }
            keep
        });
        for (id, key) in retired {
            if let Some(world) = self.worlds.get_mut(&id) {
                world.admissions.remove(&key);
                after(&mut self.worlds, id);
            }
        }
    }
    #[cfg(test)]
    fn step(&mut self) {
        self.step_using(|worlds, tick| {
            for (id, world) in worlds {
                let _context = crate::supervision::world(*id, tick);
                world.step();
                world.report_performance(*id);
            }
        });
    }
    #[cfg(test)]
    fn step_using(&mut self, simulate: impl FnOnce(&mut execution::Worlds, u64)) {
        self.tick += 1;
        self.retire_transfer_grants();
        simulate(&mut self.worlds, self.tick);
        for (id, world) in &mut self.worlds {
            if *id > 0 && !self.endgame.realms.contains_key(id) {
                if world.players.is_empty() {
                    self.idle.entry(*id).or_insert(self.tick);
                } else {
                    self.idle.remove(id);
                }
            }
        }
        let ready: Vec<_> = self.worlds.keys().copied().collect();
        self.coordinate(&ready);
    }
    fn coordinator_step(&mut self) {
        self.tick += 1;
        self.retire_grants(|worlds, id| worlds.release_world(&id));
        for (id, world) in self.worlds.summaries() {
            if *id > 0 && !self.endgame.realms.contains_key(id) {
                if world.players.is_empty() {
                    self.idle.entry(*id).or_insert(self.tick);
                } else {
                    self.idle.remove(id);
                }
            }
        }
        self.coordinate(&[]);
    }
    // Another world's completion or a timer must not publish this world's queued effects early.
    fn coordinate(&mut self, ready: &[i32]) {
        self.endgame_step(ready);
        self.sync_realm_portal();
        self.worlds.release_world(&-2);
        let requesting: Vec<_> = self
            .worlds
            .summaries()
            .filter(|(id, w)| ready.contains(id) && w.portals_pending)
            .map(|(id, _)| *id)
            .collect();
        let requests: Vec<_> = requesting
            .into_iter()
            .flat_map(|id| {
                std::mem::take(
                    &mut self
                        .worlds
                        .get_mut(&id)
                        .expect("portal source missing")
                        .portal_requests,
                )
                .into_iter()
                .map(move |pos| (id, pos))
            })
            .collect();
        for (origin, (kind, pos)) in requests {
            if let Err(error) = self.drop_dungeon_portal(origin, kind, pos) {
                tracing::error!(origin,kind,x=pos.x,y=pos.y,%error,"earned dungeon portal creation failed");
            }
        }
        let expired_portals: Vec<_> = self
            .portals
            .iter()
            .filter(|(_, binding)| binding.expires <= self.tick)
            .map(|(id, binding)| (*id, binding.origin))
            .collect();
        for (id, origin) in expired_portals {
            let world = self.worlds.get_mut(&origin).expect("portal origin exists");
            if world.entities.get(&id).is_none_or(|e| !e.reserved) {
                world.remove_entity(id);
                self.portals.remove(&id);
                tracing::info!(portal = id, origin, "dungeon portal expired");
            }
            self.worlds.release_world(&origin);
        }
        let expired: Vec<_> = self
            .idle
            .iter()
            .filter(|(id, since)| self.tick - **since > 6000 && self.reclaimable_world(**id))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.worlds.get(&id).expect("idle world missing");
            if !self.reclaimable_world(id) {
                tracing::debug!(world = id, "idle world changed before reclamation");
                self.worlds.release_world(&id);
                continue;
            }
            self.worlds.remove(&id);
            self.idle.remove(&id);
            tracing::info!(world = id, "idle world removed");
        }
        self.worlds.routing.tick.store(self.tick, Ordering::Release);
    }
}
type Reply = Box<dyn FnOnce() + Send>;
struct Job<S> {
    body: Box<dyn FnOnce(&mut S) -> Reply + Send>,
    sample: Box<crate::performance::attribution::Sample>,
    enqueued: Instant,
}
impl<S> Job<S> {
    fn new(
        operation: &'static str,
        send_wait: Duration,
        body: Box<dyn FnOnce(&mut S) -> Reply + Send>,
    ) -> Self {
        let mut sample = crate::performance::attribution::Sample::new(operation);
        sample.stages[0] = send_wait;
        Self {
            body,
            sample,
            enqueued: Instant::now(),
        }
    }
}
fn execute_directory_job(directory: &mut Directory, mut job: Job<Directory>) {
    let _context =
        crate::performance::attribution::Context::enter(job.sample.operation, job.sample.id);
    job.sample.stages[1] = job.enqueued.elapsed();
    let execution = crate::performance::attribution::Execution::start();
    let work = crate::performance::Measure::start();
    let reply = (job.body)(directory);
    let work = work.finish();
    job.sample.stages[4] = work.wall;
    job.sample.cpu = work.cpu;
    job.sample.tick = directory.tick;
    let publish = crate::performance::Measure::start();
    directory
        .worlds
        .routing
        .tick
        .store(directory.tick, Ordering::Release);
    directory.worlds.release();
    let publish = publish.finish();
    job.sample.stages[5] = publish.wall;
    job.sample.publish_cpu = publish.cpu;
    let dispatch = Instant::now();
    reply();
    job.sample.stages[6] = dispatch.elapsed();
    job.sample.execution = execution.map(crate::performance::attribution::Execution::finish);
    job.sample.incident(None);
    directory.attribution.record(job.sample);
    if let Some(attribution) = directory.attribution.report() {
        crate::performance::attribution::Attribution::emit(attribution, None);
    }
}
async fn service_queued_jobs(directory: &mut Directory, rx: &mut mpsc::Receiver<Job<Directory>>) {
    for _ in 0..16 {
        // Serial packet handlers must receive their replies before they can enqueue the next acknowledgement.
        tokio::task::yield_now().await;
        let Ok(job) = rx.try_recv() else { break };
        execute_directory_job(directory, job);
    }
}
#[cfg(test)]
type LifecycleJobGate = Arc<
    std::sync::Mutex<
        Option<(
            usize,
            bool,
            bool,
            oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
>;
#[derive(Clone)]
pub struct Handle {
    #[cfg(test)]
    lifecycle_lost_reply: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    lifecycle_job_gate: LifecycleJobGate,
    tx: mpsc::Sender<Job<Directory>>,
    routing: Arc<execution::Routing>,
    target: Target,
    pub clock: crate::clock::Clock,
    pub supervisor: crate::supervision::Supervisor,
}
impl Handle {
    #[cfg(test)]
    pub fn spawn(directory: Directory) -> Self {
        Self::supervised(directory, crate::supervision::Supervisor::default())
    }
    pub fn supervised(
        mut directory: Directory,
        supervisor: crate::supervision::Supervisor,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel::<Job<Directory>>(256);
        let clock = directory.clock.clone();
        let routing = directory.worlds.routing.clone();
        directory
            .worlds
            .start(clock.clone(), supervisor.clone(), tx.downgrade())
            .expect("world actors could not start");
        // Preserve the incident component consumed by shutdown and account-fence readers.
        supervisor.spawn_finite_thread("world", move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
            runtime.block_on(async move {
            let mut directory = directory;
            let mut timer = tokio::time::interval(std::time::Duration::from_millis(50));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    scheduled = timer.tick() => {
                        if directory.clock.paused() { continue; }
                        let mut sample=crate::performance::attribution::Sample::new("coordinator_tick");
                        let _context=crate::performance::attribution::Context::enter(sample.operation,sample.id);
                        let late=scheduled.elapsed().as_micros() as u64;
                        let stamp=crate::performance::attribution::Execution::start();
                        let execution=crate::performance::Measure::start();
                        let measuring=crate::performance::Measure::start();
                        let start=Instant::now(); directory.coordinator_step();
                        let us=start.elapsed().as_micros() as u64;
                        directory.max_tick_us=directory.max_tick_us.max(us);
                        let measured=measuring.finish();
                        sample.tick=directory.tick;
                        directory.performance.completed("coordination_step", measured);
                        directory.performance.record("scheduling_lateness", std::time::Duration::from_micros(late));
                        directory.diagnostic_ticks.push_back((start, us, late));
                        while directory.diagnostic_ticks.front().is_some_and(|(t, _, _)| t.elapsed().as_secs() > 120) { directory.diagnostic_ticks.pop_front(); }
                        if us>25_000 {tracing::warn!(tick=directory.tick,elapsed_us=us,"slow world coordination tick");}
                        if directory.tick.is_multiple_of(100) { directory.max_tick_us=0; }
                        if let Some(timings)=directory.performance.report() {
                            tracing::info!(tick=directory.tick,worlds=directory.worlds.len(),
                                players=directory.worlds.summaries().map(|(_,w)|w.players.len()).sum::<usize>(),
                                entities=directory.worlds.summaries().map(|(_,w)|w.entities).sum::<usize>(),
                                actors=directory.worlds.summaries().map(|(_,w)|w.actors).sum::<usize>(),
                                scope="coordinator only; per-world simulation is reported separately",
                                diagnostic_tick_samples=directory.diagnostic_ticks.len(),queued_jobs=rx.len(),
                                memory=%crate::performance::memory(),timings=%timings,"directory health");
                        }
                        // Over-budget ticks must not repeatedly win select and starve queued input or lease renewals.
                        let execution=execution.finish();
                        sample.stages[4]=execution.wall;
                        sample.cpu=execution.cpu;
                        let publish=crate::performance::Measure::start();
                        directory.worlds.release();
                        let publish=publish.finish();
                        sample.stages[5]=publish.wall;
                        sample.publish_cpu=publish.cpu;
                        sample.execution=stamp.map(crate::performance::attribution::Execution::finish);
                        sample.incident(None);
                        directory.attribution.record(sample);
                        if let Some(attribution)=directory.attribution.report() {
                            crate::performance::attribution::Attribution::emit(attribution,None);
                        }
                        service_queued_jobs(&mut directory, &mut rx).await;
                    },
                    job=rx.recv()=>match job{Some(f)=>execute_directory_job(&mut directory,f),None=>break}
                }
            }
            Ok(())
            })
        }).expect("directory worker could not start");
        Self {
            #[cfg(test)]
            lifecycle_lost_reply: Default::default(),
            #[cfg(test)]
            lifecycle_job_gate: Default::default(),
            tx,
            routing,
            target: Target::World(-1),
            clock,
            supervisor,
        }
    }
    pub fn in_world(&self, id: i32) -> Self {
        Self {
            #[cfg(test)]
            lifecycle_lost_reply: self.lifecycle_lost_reply.clone(),
            #[cfg(test)]
            lifecycle_job_gate: self.lifecycle_job_gate.clone(),
            tx: self.tx.clone(),
            routing: self.routing.clone(),
            target: Target::World(id),
            clock: self.clock.clone(),
            supervisor: self.supervisor.clone(),
        }
    }
    pub fn for_player(&self, id: i32) -> Self {
        Self {
            #[cfg(test)]
            lifecycle_lost_reply: self.lifecycle_lost_reply.clone(),
            #[cfg(test)]
            lifecycle_job_gate: self.lifecycle_job_gate.clone(),
            tx: self.tx.clone(),
            routing: self.routing.clone(),
            target: Target::Player(id),
            clock: self.clock.clone(),
            supervisor: self.supervisor.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn lifecycle_test_pause_job(
        &self,
        call: usize,
        before: bool,
    ) -> (oneshot::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.lifecycle_test_gate(call, before, false)
    }
    #[cfg(test)]
    fn lifecycle_test_gate(
        &self,
        call: usize,
        before: bool,
        request_only: bool,
    ) -> (oneshot::Receiver<()>, std::sync::mpsc::Sender<()>) {
        assert!(call > 0);
        let (entered, ready) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        assert!(
            self.lifecycle_job_gate
                .lock()
                .unwrap()
                .replace((call, before, request_only, entered, wait))
                .is_none()
        );
        (ready, release)
    }

    pub async fn directory<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Directory) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.request(
            &self.tx,
            "directory",
            std::any::type_name_of_val(&f),
            |d| &mut d.performance,
            f,
        )
        .await
    }
    async fn request<S: Send + 'static, T: Send + 'static>(
        &self,
        sender: &mpsc::Sender<Job<S>>,
        actor: &'static str,
        operation: &'static str,
        timings: fn(&mut S) -> &mut crate::performance::Window,
        f: impl FnOnce(&mut S) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.supervisor.ensure_running()?;
        let (tx, rx) = oneshot::channel();
        let request = crate::performance::REQUEST.try_with(Arc::clone).ok();
        let trace = request.as_ref().and_then(|r| {
            let mut r = r.lock().expect("request timings poisoned");
            r.calls += 1;
            r.native_control
                .as_ref()
                .map(|trace| (trace.clone(), r.calls))
        });
        let record = move |stage, receiver_closed| {
            if let Some((trace, call)) = &trace {
                trace.record(*call, stage, receiver_closed);
            }
        };
        let start = Instant::now();
        record("capacity_wait", tx.is_closed());
        let capacity_started = Instant::now();
        let permit = sender
            .reserve()
            .await
            .map_err(|_| anyhow::anyhow!("{actor} request queue stopped"))?;
        let send_wait = capacity_started.elapsed();
        record("capacity_acquired", tx.is_closed());
        let queued = Instant::now();
        let work_request = request.clone();
        let work_record = record.clone();
        #[cfg(test)]
        let lost = self.lifecycle_lost_reply.clone();
        #[cfg(test)]
        let mut gate = {
            let mut gate = self.lifecycle_job_gate.lock().unwrap();
            if let Some((count, _, request_only, _, _)) = gate.as_mut()
                && (!*request_only || request.is_some())
            {
                *count -= 1;
            }
            if gate.as_ref().is_some_and(|(count, _, _, _, _)| *count == 0) {
                gate.take()
            } else {
                None
            }
        };
        permit.send(Job::new(
            operation,
            send_wait,
            Box::new(move |d: &mut S| -> Reply {
                let queue_wait = queued.elapsed();
                work_record("started", tx.is_closed());
                timings(d).record("job_send_wait", send_wait);
                timings(d).record("job_wait", queue_wait);
                let started = crate::performance::Measure::start();
                #[cfg(test)]
                if gate.as_ref().is_some_and(|(_, before, _, _, _)| *before) {
                    let (_, _, _, entered, wait) = gate.take().unwrap();
                    let _ = entered.send(());
                    if wait.recv().is_err() {
                        return Box::new(move || {
                            let _ = tx.send(Err(anyhow::anyhow!("lifecycle World gate abandoned")));
                        });
                    }
                }
                let result = f(d);
                #[cfg(test)]
                if let Some((_, _, _, entered, wait)) = gate {
                    let _ = entered.send(());
                    if wait.recv().is_err() {
                        return Box::new(move || {
                            let _ = tx.send(Err(anyhow::anyhow!("lifecycle World gate abandoned")));
                        });
                    }
                }

                let measurement = started.finish();
                timings(d).completed("job", measurement);
                if let Some(r) = work_request {
                    let mut r = r.lock().expect("request timings poisoned");
                    r.completed += 1;
                    r.send_wait += send_wait;
                    r.queued += queue_wait;
                    r.work += measurement.wall;
                    if let Some(cpu) = measurement.cpu {
                        *r.cpu.get_or_insert(Duration::ZERO) += cpu;
                    }
                }
                work_record("completed", tx.is_closed());
                #[cfg(test)]
                if lost.swap(false, Ordering::SeqCst) {
                    return Box::new(|| {});
                }
                Box::new(move || {
                    let receiver_closed = tx.send(result).is_err();
                    work_record("result_send", receiver_closed);
                })
            }),
        ));
        // The receiver cannot be dropped until this poll yields.
        record("enqueued", false);
        let result = rx.await.with_context(|| format!("{actor} worker failed"));
        if let Some(r) = request {
            r.lock().expect("request timings poisoned").round_trip += start.elapsed();
        }
        result?
    }
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut World) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.call_named(std::any::type_name_of_val(&f), move |world, _| f(world))
            .await
    }
    async fn call_scoped<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut World, i32) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.call_named(std::any::type_name_of_val(&f), f).await
    }
    async fn call_named<T: Send + 'static>(
        &self,
        operation: &'static str,
        f: impl FnOnce(&mut World, i32) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let target = self.target;
        let slot = self.routing.resolve(target)?;
        let scope = slot.id;
        self.request(
            &slot.mailbox,
            "world",
            operation,
            |world| world.performance.get_mut(),
            move |world| {
                if let Target::Player(player) = target {
                    ensure!(
                        world.players.contains_key(&player),
                        "player missing from world actor"
                    );
                }
                f(world, scope)
            },
        )
        .await
    }
    pub(crate) async fn registered_world<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut World) -> Result<T> + Send + 'static,
    ) -> Result<Option<T>> {
        let Target::World(scope) = self.target else {
            bail!("world enumeration requires an explicit world target")
        };
        self.directory(move |d| d.worlds.get_mut(&scope).map(f).transpose())
            .await
    }
    pub(crate) fn worlds(&self) -> Vec<Self> {
        self.routing
            .ids()
            .into_iter()
            .map(|id| self.in_world(id))
            .collect()
    }
}
pub(crate) fn send(p: &Player, id: u8, body: Vec<u8>) {
    spectator::publish(p, id, &body);
    let packet_bytes = body.len();
    if let Some(tx) = &p.tx
        && let Err(error) = tx.packets.try_send(Packet { id, body })
    {
        lose(
            &tx.lost,
            if tx.packets.is_closed() {
                "output_queue_closed"
            } else {
                "output_queue_full"
            },
        );
        let queue_state = match error {
            mpsc::error::TrySendError::Full(_) => "full",
            mpsc::error::TrySendError::Closed(_) => "closed",
        };
        tracing::warn!(
            character = p.character.id,
            packet = id,
            packet_bytes,
            queue_state,
            queue_capacity = tx.packets.max_capacity(),
            "outbound game packet rejected; closing connection"
        );
    }
}
fn stat(out: &mut Writer, id: u8, v: i32) {
    out.u8(id).i32(v);
}
fn text_stat(out: &mut Writer, id: u8, v: &str) {
    out.u8(id).utf(v);
}
impl World {
    #[cfg(test)]
    pub fn new(content: Arc<Content>, map: Arc<Map>) -> Result<Self> {
        Self::with_ids(content, map, Arc::new(AtomicI32::new(1000)))
    }
    pub(crate) fn with_ids(
        content: Arc<Content>,
        map: Arc<Map>,
        ids: Arc<AtomicI32>,
    ) -> Result<Self> {
        let mut entities = entity_store::Entities::default();
        let mut scenery_templates = BTreeMap::new();
        let mut actor_ids = BTreeSet::new();
        let mut quests_by_level: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let mut fountains = Vec::new();
        let mut cells = cell_index::CellIndex::new(map.width as usize, map.height as usize);
        for (y, row) in map.cells.chunks(map.width as usize).enumerate() {
            for (x, c) in row.iter().enumerate() {
                if let Some(kind) = c.object {
                    let id = allocate_id(&ids)?;
                    let o = content.get(kind)?;
                    let pos = Pos {
                        x: x as f32 + 0.5,
                        y: y as f32 + 0.5,
                    };
                    let mut e = Entity {
                        id,
                        kind,
                        pos,
                        home: pos,
                        state: entity_store::EntityState::Owned(Box::new(EntityData {
                            name: if o.class == "Portal" {
                                o.name.clone()
                            } else {
                                String::new()
                            },
                            goal: None,
                            motion: None,
                            following: Following::Unknown,
                            lair_escort: None,
                            tomb: tomb::State::for_kind(kind),
                            follow_remaining: 0,
                            strict_initialized: false,
                            charge: None,
                            script: None,
                            abyss: None,
                            last_step: Pos { x: 0., y: 0. },
                            phase: 0.,
                            orbit: None,
                            hp: o.hp,
                            size: o.size,
                            condition: crate::dungeon::manor::initial_condition(kind)
                                | crate::dungeon::lab::initial_condition(kind)
                                | crate::dungeon::abyss::initial_condition(kind)
                                | crate::dungeon::spider::initial_condition(kind)
                                | crate::dungeon::snake::initial_condition(kind)
                                | crate::dungeon::jungle::initial_condition(kind)
                                | crate::dungeon::davy::initial_condition(kind)
                                | crate::dungeon::tomb::initial_condition(kind)
                                | if o.stasis_immune {
                                    crate::condition::bit(crate::condition::STASIS_IMMUNE)
                                } else {
                                    0
                                },
                            statuses: BTreeMap::new(),
                            items: o.equipment.clone(),
                            drop_slots: BTreeSet::new(),
                            extra_stats: if o.class == "Portal" {
                                vec![(37, 1)]
                            } else {
                                vec![]
                            },
                            bag_owners: Vec::new(),
                            reserved: false,
                            published: true,
                            damage: BTreeMap::new(),
                            next_shot: 0,
                            attacks: Default::default(),
                            activated: None,
                            stage: None,
                            parent: Default::default(),
                            camp: None,
                            population_slot: None,
                            quest: None,
                            expires: Default::default(),
                            next_aux: 0,
                            blocked_since: None,
                            reaction_until: 0,
                            bullet: 0,
                        })),
                    };
                    for setting in c.config.split(';').filter(|s| !s.is_empty()) {
                        let (k, v) = setting.split_once(':').context("malformed map setting")?;
                        match k {
                            "name" => e.name = v.into(),
                            "size" => e.size = v.parse()?,
                            "eff" | "conn" | "mtype" | "mcount" | "mtime" | "nstar" => {
                                let value = if let Some(hex) = v.strip_prefix("0x") {
                                    u32::from_str_radix(hex, 16)? as i32
                                } else {
                                    v.parse()?
                                };
                                if k == "eff" {
                                    e.condition |= value;
                                    continue;
                                }
                                let stat = match k {
                                    "conn" => 41,
                                    "mtype" => 34,
                                    "mcount" => 42,
                                    "mtime" => 43,
                                    // NameChanger is not a SellableObject in 123.5.1.
                                    "nstar" => 55,
                                    _ => unreachable!(),
                                };
                                e.extra_stats.push((stat, value));
                            }
                            _ => bail!("unsupported map setting {k}"),
                        }
                    }
                    if o.class == "Merchant" {
                        shop::configure(&mut e, &content)?;
                    }
                    if o.name == "White Fountain" {
                        fountains.push((id, pos));
                    }
                    let actor = !o.passive_destructible()
                        && (o.enemy
                            || o.quest
                            || !o.static_object
                            || matches!(kind, 0x173 | 0xd7d | 0x802..=0x805));
                    cells.insert(y * map.width as usize + x, id, actor);
                    if actor {
                        actor_ids.insert(id);
                    }
                    if o.quest {
                        quests_by_level.entry(o.quest_level).or_default().insert(id);
                    }
                    if !actor
                        && !o.destructible
                        && o.class == "GameObject"
                        && o.name != "White Fountain"
                    {
                        e.share(&mut scenery_templates, &c.config);
                    }
                    entities.insert(id, e);
                }
            }
        }
        let gun_ids = actor_ids
            .iter()
            .copied()
            .filter(|id| (0x802..=0x805).contains(&entities[id].kind))
            .collect();
        let mut world = Self {
            content,
            map,
            navigation_geometry: Box::default(),
            realm_layout: None,
            timed_ids: BTreeSet::new(),
            gun_ids,
            abyss_flamer_ids: actor_ids
                .iter()
                .copied()
                .filter(|id| entities[id].kind == 0x90b)
                .collect(),
            pentaract_ids: entities
                .values()
                .filter(|e| e.kind == 0xd5f)
                .map(|e| e.id)
                .collect(),
            world_effect_ids: actor_ids
                .iter()
                .copied()
                .filter(|id| Self::has_world_effect(entities[id].kind))
                .collect(),
            entities,
            non_population_ids: actor_ids.clone(),
            npc_global_ids: actor_ids.clone(),
            actor_ids,
            quests_by_level,
            players: BTreeMap::new(),
            guild: None,
            bullets: Vec::new(),
            detached_bullets: 0,
            grenades: Vec::new(),
            fauna_births: Vec::new(),
            named_births: Vec::new(),
            lab_tosses: Vec::new(),
            manor_births: Vec::new(),
            lava_patches: BTreeMap::new(),
            locker: None,
            ability_state: abilities::State::default(),
            companions: companions::State::default(),
            finale: endgame_ai::Actors::default(),
            cells,
            fountains,
            next_fountain_heal: 0,
            rng: rand::rngs::StdRng::from_entropy(),
            now: 0,
            last_npc_step: 0,
            performance: std::cell::RefCell::default(),
            attack_epoch: Instant::now(),
            clock: crate::clock::Clock::default(),
            new_tick_at: Instant::now(),
            prepared_seed: None,
            tick: 0,
            ids,
            name: "Development tutorial".into(),
            background: 0,
            safe: false,
            owner: None,
            return_route: None,
            max_players: 85,
            admissions: BTreeMap::new(),
            encounter: false,
            population: vec![],
            camps: vec![],
            population_regions: std::array::from_fn(|_| population_density::Region::default()),
            population_counts: [0; 13],
            population_next: 10000,
            god_reproduction: BTreeMap::new(),
            setpieces: vec![],
            event_sites: vec![],
            portal_requests: vec![],
            critical_deaths: vec![],
            critical_killers: BTreeMap::new(),
            event_reserves: vec![],
            event_waiting: None,
            event_choice: None,
            event_rng: rand::rngs::StdRng::from_entropy(),
            admission_open: true,
            bag_expiry: BTreeMap::new(),
            cleared: false,
            clear_exit: None,
            vaults: BTreeMap::new(),
            closed_vaults: BTreeMap::new(),
            trades: BTreeMap::new(),
            trade_requests: BTreeMap::new(),
        };
        world.jungle_initialize()?;
        world.tomb_bindings()?;
        Ok(world)
    }
    pub fn settled(&self) -> Result<bool> {
        for p in self.players.values() {
            ensure!(
                p.persistence_failure.is_none(),
                "character {} is quarantined",
                p.character.id
            );
        }
        Ok(self.players.values().all(|p| {
            !p.pending
                && !p.progress_queued()
                && !p.progress_started
                && !p.death_pending
                && !p.departing
        }))
    }
    pub fn load_vaults(&mut self, vaults: Vec<Vault>) -> Result<()> {
        let account = self.owner.context("Vault world has no owner")?;
        ensure!(
            self.name == "Vault"
                && !vaults.is_empty()
                && vaults.len() <= crate::store::VAULT_CHESTS as usize
                && vaults.iter().all(|v| v.account == account
                    && (0..crate::store::VAULT_CHESTS).contains(&v.chest)
                    && v.items.len() == 8
                    && v.items.iter().all(|i| (-1..=65535).contains(i))
                    && v.revision >= 0),
            "invalid private vault state"
        );
        let count = vaults.len();
        let by_chest: BTreeMap<_, _> = vaults.into_iter().map(|v| (v.chest, v)).collect();
        ensure!(
            by_chest.len() == count && by_chest.contains_key(&0),
            "duplicate or missing initial vault chest"
        );
        if !self.vaults.is_empty() {
            ensure!(
                self.vaults.len() == by_chest.len()
                    && self.vaults.iter().all(|(id, v)| {
                        by_chest.get(&v.chest).is_some_and(|saved| {
                            saved.account == v.account
                                && saved.revision == v.revision
                                && saved.items == v.items
                                && self.entities[id].items == saved.items
                        })
                    }),
                "loaded vault disagrees with durable identity or revision"
            );
            return Ok(());
        }
        let (x, y) = self.map.spawns[0];
        let spawn = Pos { x, y };
        let mut cells = self
            .map
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.region == 12)
            .map(|(i, _)| Pos {
                x: (i % self.map.width as usize) as f32 + 0.5,
                y: (i / self.map.width as usize) as f32 + 0.5,
            })
            .collect::<Vec<_>>();
        // Stable current identities; the period evidence does not establish equal-distance ordering.
        cells.sort_by(|a, b| {
            a.distance(spawn)
                .total_cmp(&b.distance(spawn))
                .then(a.y.total_cmp(&b.y))
                .then(a.x.total_cmp(&b.x))
        });
        ensure!(
            cells.len() == crate::store::VAULT_CHESTS as usize,
            "Vault map must have exactly twenty chest cells"
        );
        for (chest, pos) in cells.into_iter().enumerate() {
            let chest = chest as i32;
            if let Some(vault) = by_chest.get(&chest) {
                let id = self.spawn_entity(0x504, pos, vault.items.clone())?;
                self.entities
                    .get_mut(&id)
                    .unwrap()
                    .extra_stats
                    .push((54, account));
                self.vaults.insert(id, vault.clone());
            } else {
                let id = self.spawn_entity(0x505, pos, Vec::new())?;
                // Native stat 54 casts to Container; sellers are owned by this private world's identity map.
                self.entities.get_mut(&id).unwrap().extra_stats.extend([
                    (36, crate::store::VAULT_PRICE),
                    (40, 0),
                    (45, 0),
                ]);
                self.closed_vaults.insert(id, chest);
            }
        }
        tracing::info!(
            account,
            owned = self.vaults.len(),
            closed = self.closed_vaults.len(),
            "private vault loaded"
        );
        Ok(())
    }
    fn bind_vaults(&self, plan: &mut InventoryPlan) -> Result<()> {
        for (id, items) in &plan.bags {
            let e = self.entities.get(id).context("vault source disappeared")?;
            if let Some(vault) = self.vaults.get(id) {
                ensure!(
                    e.kind == 0x504
                        && self.owner == Some(plan.account)
                        && vault.account == plan.account
                        && vault.items == e.items
                        && e.extra_stats.contains(&(54, plan.account)),
                    "vault source ownership or contents changed"
                );
                let mut vault = vault.clone();
                vault.items = items.clone();
                plan.vaults.push(vault);
            } else {
                ensure!(e.kind != 0x504, "container has no durable chest identity");
            }
        }
        Ok(())
    }
    pub fn spawn_entity(&mut self, kind: u16, pos: Pos, items: Vec<i32>) -> Result<i32> {
        ensure!(self.walkable(pos), "entity spawn is blocked");
        self.insert_entity(kind, pos, items)
    }
    fn insert_entity(&mut self, kind: u16, pos: Pos, items: Vec<i32>) -> Result<i32> {
        ensure!(
            pos.x.is_finite() && pos.y.is_finite() && self.map.cell(pos.x, pos.y).is_some(),
            "entity spawn outside map"
        );
        let definition = self.content.get(kind)?;
        let id = allocate_id(&self.ids)?;
        let extra_stats = if definition.class == "Portal" {
            vec![(37, 1)]
        } else {
            vec![]
        };
        self.entities.insert(
            id,
            Entity {
                id,
                kind,
                pos,
                home: pos,
                state: entity_store::EntityState::Owned(Box::new(EntityData {
                    name: if definition.class == "Portal" {
                        definition.name.clone()
                    } else {
                        String::new()
                    },
                    goal: None,
                    motion: None,
                    following: Following::Unknown,
                    lair_escort: None,
                    tomb: tomb::State::for_kind(kind),
                    follow_remaining: 0,
                    strict_initialized: false,
                    charge: None,
                    script: None,
                    abyss: None,
                    last_step: Pos { x: 0., y: 0. },
                    phase: 0.,
                    orbit: None,
                    hp: definition.hp,
                    size: definition.size,
                    condition: crate::dungeon::manor::initial_condition(kind)
                        | crate::dungeon::lab::initial_condition(kind)
                        | crate::dungeon::abyss::initial_condition(kind)
                        | crate::dungeon::spider::initial_condition(kind)
                        | crate::dungeon::snake::initial_condition(kind)
                        | crate::dungeon::jungle::initial_condition(kind)
                        | crate::dungeon::davy::initial_condition(kind)
                        | crate::dungeon::tomb::initial_condition(kind)
                        | (if kind == 0x91a {
                            crate::condition::bit(crate::condition::INVULNERABLE)
                        } else {
                            0
                        })
                        | (if definition.stasis_immune {
                            crate::condition::bit(crate::condition::STASIS_IMMUNE)
                        } else {
                            0
                        }),

                    statuses: BTreeMap::new(),
                    items,
                    drop_slots: BTreeSet::new(),
                    extra_stats,
                    bag_owners: Vec::new(),
                    reserved: false,
                    published: true,
                    damage: BTreeMap::new(),
                    next_shot: 0,
                    attacks: Default::default(),
                    activated: None,
                    stage: None,
                    parent: Default::default(),
                    camp: None,
                    population_slot: None,
                    quest: None,
                    expires: Default::default(),
                    next_aux: 0,
                    blocked_since: None,
                    reaction_until: 0,
                    bullet: 0,
                })),
            },
        );
        self.index_actor(id);
        Ok(id)
    }
    fn index_actor(&mut self, id: i32) {
        let e = &self.entities[&id];
        let kind = e.kind;
        let definition = &self.content.objects[&kind];
        let cell = e.pos.y.floor() as usize * self.map.width as usize + e.pos.x.floor() as usize;
        let actor = !definition.passive_destructible();
        self.cells.insert(cell, id, actor);
        if e.parent.is_some() || e.expires.is_some() {
            self.timed_ids.insert(id);
        }
        if !actor {
            return;
        }
        self.actor_ids.insert(id);
        if Self::has_world_effect(kind) {
            self.world_effect_ids.insert(id);
        }
        if (0x802..=0x805).contains(&kind) {
            self.gun_ids.insert(id);
        }
        if kind == 0xd5f {
            self.pentaract_ids.insert(id);
        }
        if kind == 0x90b {
            self.abyss_flamer_ids.insert(id);
        }
        self.npc_global_ids.insert(id);
        self.non_population_ids.insert(id);
        if definition.quest {
            self.quests_by_level
                .entry(definition.quest_level)
                .or_default()
                .insert(id);
        }
    }
    fn spawn_exit(&mut self, kind: u16, loot: Pos) -> Result<i32> {
        self.spawn_clear_portal(kind, loot, false)
    }
    fn spawn_clear_portal(&mut self, kind: u16, loot: Pos, keep_origin: bool) -> Result<i32> {
        // Keep the reference death point unless loot/portal clearance requires separation: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/behaviors/DropPortalOnDeath.cs#L42
        let pos = std::iter::once(loot)
            .filter(|_| keep_origin)
            .chain((0..8).map(|i| {
                let angle = i as f32 * std::f32::consts::TAU / 8.;
                Pos {
                    x: loot.x + 2. * angle.cos(),
                    y: loot.y + 2. * angle.sin(),
                }
            }))
            .find(|pos| {
                self.walkable(*pos)
                    && self
                        .entities_near(*pos, 2.)
                        .filter(|e| {
                            matches!(
                                self.content.objects[&e.kind].class.as_str(),
                                "Portal" | "Container"
                            )
                        })
                        .all(|e| e.pos.distance(*pos) > 1.5)
            })
            .context("portal has no walkable position clear of loot and interactions")?;
        self.spawn_entity(kind, pos, vec![])
    }
    fn remove_entity(&mut self, id: i32) -> Entity {
        self.actor_ids.remove(&id);
        self.world_effect_ids.remove(&id);
        self.timed_ids.remove(&id);
        self.gun_ids.remove(&id);
        self.pentaract_ids.remove(&id);
        self.abyss_flamer_ids.remove(&id);
        self.npc_global_ids.remove(&id);
        self.non_population_ids.remove(&id);
        self.god_reproduction.remove(&id);
        self.ability_state.objects.remove(&id);
        let entity = self.entities.remove(&id).expect("indexed entity missing");
        let definition = &self.content.objects[&entity.kind];
        if definition.quest {
            let ids = self
                .quests_by_level
                .get_mut(&definition.quest_level)
                .expect("quest index missing");
            assert!(ids.remove(&id), "indexed quest missing");
            if ids.is_empty() {
                self.quests_by_level.remove(&definition.quest_level);
            }
        }
        if let Some(slot) = entity.population_slot {
            let terrain = self.population_terrain(&self.population[slot]) as usize;
            assert_eq!(
                self.population[slot].entity,
                Some(id),
                "population slot ownership changed"
            );
            self.population[slot].entity = None;
            self.population_counts[terrain] = self.population_counts[terrain]
                .checked_sub(1)
                .expect("population count underflow");
        }
        self.orphan_satellites(&entity);
        self.orphan_abyss(&entity);
        assert!(!entity.reserved, "cannot remove a reserved container");
        self.bag_expiry.remove(&id);
        if let Some(camp) = entity.camp {
            tracing::debug!(
                camp,
                entity = id,
                kind = entity.kind,
                "family member removed"
            );
        }
        let cell = entity.pos.y as usize * self.map.width as usize + entity.pos.x as usize;
        assert!(
            self.cells.remove(cell, id),
            "entity absent from spatial index"
        );
        entity
    }
    pub fn map_info(&self) -> Vec<u8> {
        let mut o = Writer::new();
        o.i32(self.map.width)
            .i32(self.map.height)
            .utf(&self.name)
            .u32(1)
            .i32(self.background)
            .u8(u8::from(self.allow_teleport()))
            .u8(1)
            .i16(0)
            .i16(0);
        o.0
    }
    pub(crate) fn occupancy(&self) -> usize {
        self.players.len() + self.reserved_slots()
    }
    #[cfg(test)]
    pub fn enter(&mut self, ch: Character, tx: Option<Output>) -> Result<i32> {
        // Combat and inventory fixtures need nearby, repeatable starting positions.
        let (x, y) = self.map.spawns[self.players.len() % self.map.spawns.len()];
        self.enter_at(
            ch,
            tx,
            Some(Pos { x, y }),
            AccountPresentation {
                guild: None,
                credits: 100,
                fame: 0,
                best_fame: BTreeMap::new(),
            },
        )
    }

    pub fn enter_at(
        &mut self,
        ch: Character,
        tx: Option<Output>,
        position: Option<Pos>,
        presentation: AccountPresentation,
    ) -> Result<i32> {
        self.enter_admitted(ch, tx, position, presentation, None)
    }
    pub(crate) fn reserved_slots(&self) -> usize {
        self.admissions
            .values()
            .filter(|a| a.expires > self.now)
            .count()
    }
    pub(crate) fn enter_admitted(
        &mut self,
        ch: Character,
        tx: Option<Output>,
        position: Option<Pos>,
        presentation: AccountPresentation,
        admission: Option<Vec<u8>>,
    ) -> Result<i32> {
        if let Some(key) = &admission {
            let reserved = self
                .admissions
                .get(key)
                .context("entry admission missing")?;
            ensure!(
                reserved.account == ch.account
                    && reserved.character == ch.id
                    && reserved.consumed
                    && reserved.expires > self.now,
                "entry admission binding/expiry changed"
            );
        }
        if let Some(pos) = position {
            ensure!(
                pos.x.is_finite() && pos.y.is_finite() && self.walkable(pos),
                "prepared position is blocked or outside the world"
            );
        }
        ensure!(
            self.admission_open,
            anyhow::anyhow!(CloseReason::Controller).context("Realm is closed")
        );
        ensure!(ch.alive && ch.hp > 0, "dead character");
        let dex = ch.effective(&self.content)?[5];
        ensure!(
            self.owner.is_none_or(|owner| owner == ch.account),
            "private world belongs to another account"
        );
        ensure!(
            self.guild
                .is_none_or(|(id, _)| presentation.guild.as_ref().is_some_and(|m| m.id == id)),
            "Guild Hall membership has been revoked."
        );
        ensure!(
            self.occupancy() < self.max_players + usize::from(admission.is_some()),
            "development world full"
        );
        ensure!(
            !self
                .players
                .values()
                .any(|p| p.character.account == ch.account),
            "account already in this world"
        );
        let id = allocate_id(&self.ids)?;
        let pos = if let Some(pos) = position {
            pos
        } else {
            let &(x, y) = self
                .map
                .spawns
                .choose(&mut self.rng)
                .context("world has no spawn cells")?;
            Pos { x, y }
        };
        let char_id = ch.id;
        if let Some(kind) = ch.pet_kind {
            self.attach_companion(id, kind, pos)?;
        }
        self.players.insert(
            id,
            Player {
                spectators: None,
                dungeon_entries: VecDeque::new(),
                weapon_ticks: VecDeque::from([(-1, ch.items[0], dex)]),
                committed_lifetime: ch.lifetime.clone(),
                character: ch,
                pos,
                departing: false,
                account_lists: AccountLists::default(),
                presentation,
                paused: false,
                native_update_acked: false,
                controller: if tx.is_none() {
                    Controller::Qa
                } else {
                    Controller::Human
                },
                tx,
                known: BTreeSet::new(),
                tick_stats: tick_stats::Cache::default(),
                quest: None,
                qualified_quest: None,
                statuses: BTreeMap::new(),
                lab_conditions: 0,
                known_since: BTreeMap::new(),
                shoot_acks: VecDeque::new(),
                visible_projectiles: BTreeMap::new(),
                predicted_deaths: BTreeMap::new(),
                ground_hits: BTreeMap::new(),
                oxygen: if self.name == "Ocean Trench" { 100 } else { -1 },
                tiles: BTreeSet::new(),
                tile_updates: vec![],
                tile_scan: None,
                view_tiles: std::cell::RefCell::default(),
                death_started: false,
                death_lifetime: None,
                departure_snapshot: None,
                killer: String::new(),
                movement_credit: 0.78,
                hp_fraction: 0.,
                mp_fraction: 0.,
                pending: false,
                experience_queued: 0,
                experience_queue: None,
                progress_snapshot: None,
                level_assists_prepared: Vec::new(),
                clears_queued: [0; crate::dungeon::DUNGEONS.len()],
                progress_started: false,
                death_pending: false,
                persistence_failure: None,
                bot_goal: None,
                bot_aim: None,
                next_bullet: 1,
                server_bullet: 128,
                seed: 1,
                last_attack: -1000,
                last_attack_at: 0,
                attack_credit: 1000.,
                attack_count: 0,
                attack_suppressed: None,
                last_move: 0,
                last_move_at: self.now,
                motion: ObservedMotion {
                    position: pos,
                    time: 0,
                    velocity: Pos { x: 0., y: 0. },
                    at: self.now,
                },
                goto_acks: VecDeque::new(),
                discarded_attack: None,
                tick: 0,
                last_motion: None,
                ability_ready_ms: 0,
                boosts: Vec::new(),
                last_companion: None,
                ability_shot: None,
            },
        );
        if let Some(key) = admission {
            self.admissions.remove(&key);
        }
        let mut o = Writer::new();
        o.i32(id).i32(char_id);
        send(&self.players[&id], wire::CREATE_SUCCESS, o.0);
        self.updates(id, true)?;
        if self.name == "Realm" {
            let welcome =
                chat::text_packet("#Oryx the Mad God", -1, 0, "You are food for my minions!");
            send(&self.players[&id], welcome.id, welcome.body);
        }
        self.locker_key_ui(id);
        tracing::info!(
            player = id,
            character = char_id,
            world = %self.name,
            x = pos.x,
            y = pos.y,
            "player entered development world"
        );
        Ok(id)
    }

    pub fn leave(&mut self, id: i32) -> Result<Character> {
        self.clear_abilities(id);
        self.cancel_trade(id, "Trade cancelled by departure.");
        let p = self.players.get(&id).context("player not found")?;
        ensure!(
            !p.pending && !p.death_pending,
            "inventory or death operation pending"
        );
        self.clear_companions(id);
        let p = self.players.remove(&id).unwrap();
        self.bullets.retain(|b| b.owner != id);
        tracing::info!(player = id, character = p.character.id, "player left");
        Ok(p.character)
    }
    pub fn stats(&self, id: i32, recipient: i32) -> Result<Vec<u8>> {
        let mut o = Writer::new();
        if let Some(p) = self.players.get(&id) {
            let values = p.effective(&self.content, self.now)?;
            o.i32(id)
                .f32(p.pos.x)
                .f32(p.pos.y)
                .i16(45 + 4 * i16::from(id == recipient));
            for (k, v) in [
                (0, values[0]),
                (1, p.character.hp),
                (2, 100),
                (3, values[1]),
                (4, p.character.mp),
                (5, crate::store::experience_goal(p.character.level)?),
                (
                    6,
                    p.character.exp - crate::store::level_start(p.character.level)?,
                ),
                (7, p.character.level),
            ] {
                stat(&mut o, k, v);
            }
            for (i, item) in p.character.items.iter().enumerate() {
                stat(&mut o, 8 + i as u8, *item);
            }
            for (k, v) in [
                (20, values[2]),
                (21, values[3]),
                (22, values[4]),
                (26, values[6]),
                (27, values[7]),
                (28, values[5]),
                (
                    29,
                    p.conditions(self.now) | if p.paused { 1 << 20 } else { 0 },
                ),
                (30, p.presentation.stars()),
            ] {
                stat(&mut o, k, v);
            }
            text_stat(&mut o, 31, &p.character.name);
            stat(&mut o, 32, p.character.tex1 as i32);
            stat(&mut o, 33, p.character.tex2 as i32);
            stat(&mut o, 38, p.character.account);
            if id == recipient {
                stat(&mut o, 35, p.presentation.credits);
                stat(&mut o, 39, p.presentation.fame);
                stat(&mut o, 64, p.oxygen);
                stat(&mut o, 65, i32::from(p.character.xp_boost.active));
            }
            for (i, boost) in [46, 47, 48, 49, 50, 53, 51, 52].into_iter().enumerate() {
                stat(&mut o, boost, values[i] - p.character.base[i]);
            }
            stat(
                &mut o,
                56,
                i32::from(crate::store::name_chosen(
                    p.character.account,
                    &p.character.name,
                )),
            );
            stat(&mut o, 57, p.character.fame);
            stat(
                &mut o,
                58,
                p.presentation
                    .next_quest(p.character.class, p.character.fame),
            );
            text_stat(
                &mut o,
                62,
                p.presentation
                    .guild
                    .as_ref()
                    .map_or("", |g| g.name.as_str()),
            );
            stat(
                &mut o,
                63,
                p.presentation.guild.as_ref().map_or(0, |g| g.rank),
            );
        } else {
            let e = self.entities.get(&id).context("entity missing")?;
            o.i32(id).f32(e.pos.x).f32(e.pos.y).i16(
                (4 + usize::from(!e.name.is_empty()) + e.items.len() + e.extra_stats.len()) as i16,
            );
            stat(&mut o, 0, self.content.get(e.kind)?.hp);
            stat(&mut o, 1, e.hp);
            stat(&mut o, 2, e.size);
            stat(&mut o, 29, e.conditions(self.now));
            if !e.name.is_empty() {
                text_stat(&mut o, 31, &e.name);
            }
            for (i, item) in e.items.iter().enumerate() {
                stat(&mut o, 8 + i as u8, *item);
            }
            for (id, value) in &e.extra_stats {
                stat(&mut o, *id, *value);
            }
        }
        Ok(o.0)
    }
    pub(crate) fn apply_condition(&mut self, id: i32, index: u8, duration_ms: u64) -> Result<()> {
        ensure!(
            (1..=28).contains(&index) && duration_ms > 0,
            "invalid condition application"
        );
        let deadline = self
            .now
            .checked_add(duration_ms)
            .context("condition deadline overflow")?;
        let timers = if let Some(p) = self.players.get_mut(&id) {
            if index == crate::condition::QUIET {
                p.character.mp = 0;
                p.mp_fraction = 0.;
            }
            &mut p.statuses
        } else {
            let e = self
                .entities
                .get_mut(&id)
                .context("condition target missing")?;
            if index == crate::condition::STASIS {
                if e.conditions(self.now) & crate::condition::bit(crate::condition::STASIS_IMMUNE)
                    != 0
                {
                    return Ok(());
                }
                e.statuses
                    .insert(crate::condition::STASIS_IMMUNE, (deadline, deadline + 3000));
            }
            if index == crate::condition::STUNNED
                && e.conditions(self.now) & crate::condition::bit(12) != 0
            {
                tracing::debug!(entity = id, index, "condition blocked by Stun Immunity");
                return Ok(());
            }
            &mut e.statuses
        };
        timers
            .entry(index)
            .and_modify(|old| *old = (self.now, old.1.max(deadline)))
            .or_insert((self.now, deadline));
        tracing::debug!(
            entity = id,
            condition = index,
            deadline,
            "timed condition applied"
        );
        Ok(())
    }
    fn defense(&self, id: i32) -> Result<i32> {
        let p = &self.players[&id];
        Ok(p.effective(&self.content, self.now)?[3])
    }
    fn quest_target(&self, id: i32) -> Option<i32> {
        if self.safe {
            return None;
        }
        let player = &self.players[&id];
        let level = player.character.level;
        let mut lower = self
            .quests_by_level
            .range(Some(i32::MIN)..=Some(level))
            .rev()
            .peekable();
        let mut upper = self
            .quests_by_level
            .range((
                std::ops::Bound::Excluded(Some(level)),
                std::ops::Bound::Unbounded,
            ))
            .peekable();
        let levels = std::iter::from_fn(|| match (lower.peek(), upper.peek()) {
            (Some((a, _)), Some((b, _)))
                if a.map(|a| a.abs_diff(level)) <= b.map(|b| b.abs_diff(level)) =>
            {
                lower.next()
            }
            (Some(_), None) => lower.next(),
            _ => upper.next(),
        });
        let levels = self
            .quests_by_level
            .get_key_value(&None)
            .into_iter()
            .chain(levels);
        let mut best: Option<&Entity> = None;
        let mut difference = u32::MAX;
        for (&quest_level, ids) in levels {
            let delta = quest_level.map_or(0, |quest_level| quest_level.abs_diff(level));
            if delta > difference {
                break;
            }
            // Equally suitable levels on both sides must compete on distance, not iteration order.
            for id in ids {
                let e = self.entities.get(id).expect("indexed quest missing");
                let (min, max) = self.content.objects[&e.kind].quest_window;
                if e.hp <= 0
                    || !e.accessible(player.character.account)
                    || level < i32::from(min)
                    || level > i32::from(max)
                {
                    continue;
                }
                if best.is_none_or(|b| {
                    e.pos
                        .distance(player.pos)
                        .total_cmp(&b.pos.distance(player.pos))
                        .then_with(|| e.id.cmp(&b.id))
                        .is_lt()
                }) {
                    best = Some(e);
                    difference = delta;
                }
            }
        }
        best.map(|e| e.id)
    }
    fn broadcast_text(&self, name: &str, text: &str) -> usize {
        let packet = chat::text_packet(name, -1, 0, text);
        let mut recipients = 0;
        for p in self
            .players
            .values()
            .filter(|p| !p.departing && p.persistence_failure.is_none())
        {
            send(p, packet.id, packet.body.clone());
            recipients += 1;
        }
        recipients
    }
    fn visible(&self, id: i32) -> BTreeSet<i32> {
        self.visible_with(id, self.quest_target(id))
    }
    fn visible_with(&self, id: i32, quest: Option<i32>) -> BTreeSet<i32> {
        let pos = self.players[&id].pos;
        self.entities_near(pos, 20.)
            .filter(|e| e.accessible(self.players[&id].character.account))
            .map(|e| e.id)
            .chain(quest)
            .chain(
                self.players
                    .iter()
                    .filter(|(_, p)| p.pos.distance(pos) < 20.)
                    .map(|(i, _)| *i),
            )
            .collect()
    }
    fn player_markers(&self, id: i32) -> impl Iterator<Item = (i32, &Player)> {
        self.players
            .iter()
            .filter(move |(peer, _)| **peer != id && self.players[&id].known.contains(peer))
            .map(|(peer, player)| (*peer, player))
    }
    pub fn updates(&mut self, id: i32, initial: bool) -> Result<()> {
        let started = crate::performance::Measure::start();
        // Nothing below changes quest eligibility, so one evaluation serves visibility and the quest arrow.
        let quest = self.quest_target(id);
        let mut visible = self.visible_with(id, quest);
        // The native minimap tracks players across this world, including outside explored terrain.
        visible.extend(self.players.keys().copied());
        let previous = &self.players[&id].known;
        let mut added: Vec<i32> = visible.difference(previous).copied().collect();
        // Private-container owner stats dereference the native map's self player.
        added.sort_by_key(|entity| (*entity != id, *entity));
        let dropped: Vec<i32> = previous.difference(&visible).copied().collect();
        let pos = self.players[&id].pos;
        let mut tiles = Vec::new();
        let center = (pos.x as i32, pos.y as i32);
        // Discovered tiles only grow, so rescanning the same centre cannot find anything new.
        let rescan = initial || self.players[&id].tile_scan != Some(center);
        for y in ((pos.y as i32 - 20).max(0))..=(pos.y as i32 + 20).min(self.map.height - 1) {
            for x in ((pos.x as i32 - 20).max(0))..=(pos.x as i32 + 20).min(self.map.width - 1) {
                let dx = x - pos.x as i32;
                let dy = y - pos.y as i32;
                if rescan
                    && dx * dx + dy * dy <= 20 * 20
                    && !self.players[&id].tiles.contains(&(x as i16, y as i16))
                {
                    tiles.push((
                        x as i16,
                        y as i16,
                        self.map.cells[(y * self.map.width + x) as usize].ground,
                    ));
                }
            }
        }
        self.players.get_mut(&id).unwrap().tile_scan = Some(center);
        let changed_tiles = self.players[&id].tile_updates.len();
        if initial
            || !tiles.is_empty()
            || changed_tiles > 0
            || !added.is_empty()
            || !dropped.is_empty()
        {
            let mut o = Writer::new();
            o.i16(i16::try_from(tiles.len() + changed_tiles)?);
            let p = self.players.get_mut(&id).unwrap();
            for &(x, y, ground) in &tiles {
                p.tiles.insert((x, y));
                p.character.tiles = p
                    .character
                    .tiles
                    .checked_add(1)
                    .context("exploration counter overflow")?;
                if p.character.alive && !p.death_started && !p.departing {
                    p.character.lifetime.tiles = p
                        .character
                        .lifetime
                        .tiles
                        .checked_add(1)
                        .context("lifetime exploration overflow")?;
                }
                o.i16(x).i16(y).u8(ground);
            }
            if !tiles.is_empty() {
                let adding = crate::performance::Measure::start();
                p.view_tiles
                    .borrow_mut()
                    .discover(&self.map, &self.content, &tiles, p.tiles.len());
                self.performance.borrow_mut().work("terrain_update", adding);
            }
            for (x, y, ground) in std::mem::take(&mut p.tile_updates) {
                o.i16(x).i16(y).u8(ground);
            }
            o.i16(added.len() as i16);
            for entity in added {
                let kind = if self.players.contains_key(&entity) {
                    self.players[&entity].character.class
                } else {
                    self.entities[&entity].kind
                };
                o.i16(kind as i16);
                o.0.extend(self.stats(entity, id)?);
            }
            o.i16(dropped.len() as i16);
            for entity in dropped {
                o.i32(entity);
            }
            send(&self.players[&id], wire::UPDATE, o.0);
        }
        let p = self.players.get_mut(&id).unwrap();
        p.known_since.retain(|entity, _| visible.contains(entity));
        for entity in &visible {
            p.known_since.entry(*entity).or_insert(self.now);
        }
        p.known = visible;
        p.tick_stats.retain(&p.known);
        let quest_identity = quest.map(|id| {
            let e = &self.entities[&id];
            if crate::quest::root(e.kind) || crate::quest::final_root(e.kind) || e.kind == 0x6c2 {
                e.quest.unwrap_or(id)
            } else {
                id
            }
        });
        let p = self.players.get_mut(&id).unwrap();
        p.qualified_quest = quest_identity;
        if initial || p.quest != quest {
            p.quest = quest;
            let mut out = Writer::new();
            out.i32(quest.unwrap_or(-1));
            send(p, 34, out.0);
            tracing::debug!(player = id, quest, "native quest target updated");
        }
        self.performance.borrow_mut().work("visibility", started);
        Ok(())
    }

    fn validate_move_at(&self, id: i32, pos: Pos, time: i32, received: u64) -> Result<f32> {
        ensure!(
            pos.x.is_finite() && pos.y.is_finite(),
            "non-finite movement"
        );
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.active(),
            "dead, departing or quarantined player cannot move"
        );
        ensure!(time >= 0, "negative movement time");
        let elapsed = if p.last_move == 0 {
            100.
        } else {
            (i64::from(time) - i64::from(p.last_move)) as f32
        };
        ensure!(
            elapsed >= 0.,
            "movement time reversed: client_time={time} previous_client_time={}",
            p.last_move
        );
        let cell = self
            .map
            .cell(p.pos.x, p.pos.y)
            .context("player outside map")?;
        let speed = p.speed(&self.content, self.now)? * self.content.grounds[&cell.ground];
        let push = self.content.pushes[&cell.ground];
        let distance = p.pos.distance(pos);
        let input_distance = |delta| {
            p.pos.distance(Pos {
                x: pos.x - push.0 * delta,
                y: pos.y - push.1 * delta,
            })
        };
        ensure!(
            p.conditions(self.now) & PARALYZED == 0 || distance <= 0.001,
            "paralyzed movement"
        );
        ensure!(
            input_distance(elapsed) <= speed * elapsed + 0.3,
            "movement exceeds speed: from=({}, {}) to=({}, {}) distance={distance} allowed={} speed={speed} client_elapsed_ms={elapsed}",
            p.pos.x,
            p.pos.y,
            pos.x,
            pos.y,
            speed * elapsed + 0.3
        );
        let server_elapsed = received
            .checked_sub(p.last_move_at)
            .context("movement server clock regressed")?;
        let credit = (p.movement_credit + speed * server_elapsed as f32).min(speed * 350. + 0.3);
        let charged = input_distance((server_elapsed as f32).min(350.));
        tracing::debug!(
            player = id,
            client_time = time,
            received,
            ground = cell.ground,
            speed,
            client_elapsed_ms = elapsed,
            server_elapsed_ms = server_elapsed,
            previous_credit = p.movement_credit,
            credit,
            charged,
            from_x = p.pos.x,
            from_y = p.pos.y,
            to_x = pos.x,
            to_y = pos.y,
            "native motion budget"
        );
        ensure!(
            charged <= credit + 0.001,
            "movement exceeds server time budget: distance={distance} credit={credit} server_elapsed_ms={server_elapsed} client_elapsed_ms={elapsed} previous_credit={}",
            p.movement_credit
        );
        let samples = (distance / 0.1).ceil().max(1.) as usize;
        ensure!(
            (1..=samples).all(|i| {
                let f = i as f32 / samples as f32;
                self.player_step_valid(
                    p.pos,
                    Pos {
                        x: p.pos.x + (pos.x - p.pos.x) * f,
                        y: p.pos.y + (pos.y - p.pos.y) * f,
                    },
                )
            }),
            "movement across a blocked tile"
        );
        Ok(credit - charged)
    }

    pub fn native_move_received(
        &mut self,
        id: i32,
        pos: Pos,
        (time, received): (i32, Instant),
    ) -> Result<NativeInput> {
        let queued_ms = received.elapsed().as_millis();
        let received = received
            .checked_duration_since(self.attack_epoch)
            .context("native MOVE predates world epoch")?
            .as_millis() as u64;
        tracing::debug!(
            player = id,
            client_time = time,
            received,
            queued_ms,
            x = pos.x,
            y = pos.y,
            "native MOVE ingress"
        );
        if self.discard_correcting_move(id, pos, time)? {
            return Ok(NativeInput::DiscardedUntilGotoAck);
        }
        let sentinel = pos.x == -1. && pos.y == -1.;
        if !sentinel {
            self.validate_native_position(id, pos, time)?;
        }
        let p = self.players.get_mut(&id).context("native player missing")?;
        ensure!(
            time >= p.last_move,
            controller("native movement clock regressed")
        );
        ensure!(received >= p.last_move_at, "native ingress clock regressed");
        let speed = p.speed(&self.content, self.now)? * 1000.;
        p.record_motion(if sentinel { p.pos } else { pos }, time, self.now, speed);
        if !sentinel {
            if p.pos.distance(pos) > 0.001 {
                p.last_motion = Some(((pos.y - p.pos.y).atan2(pos.x - p.pos.x), self.now));
            }
            p.pos = pos;
        }
        p.last_move = time;
        p.last_move_at = received;
        Ok(NativeInput::Applied)
    }
    pub fn move_to(&mut self, id: i32, pos: Pos, time: i32) -> Result<()> {
        self.move_timed(id, pos, time, self.now)
    }
    fn move_timed(&mut self, id: i32, pos: Pos, time: i32, received: u64) -> Result<()> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.last_move == 0 || i64::from(time) - i64::from(p.last_move) <= 1000,
            "movement time jump"
        );
        // MOVE precedes condition updates; accept either form across a toggle.
        let sentinel = pos.x == -1. && pos.y == -1.;
        let immobile = p.paused || p.conditions(self.now) & PARALYZED != 0;
        let correct = immobile && !sentinel && p.pos.distance(pos) > 0.001;
        let stationary = immobile || sentinel;
        ensure!(pos.x.is_finite() && pos.y.is_finite(), "invalid position");
        let pos = if stationary { p.pos } else { pos };
        let remaining_credit = self.validate_move_at(id, pos, time, received)?;
        let p = self.players.get_mut(&id).unwrap();
        if !stationary {
            p.movement_credit = remaining_credit;
        }
        let speed = p.speed(&self.content, self.now)? * 1000.;
        p.record_motion(pos, time, self.now, speed);
        if p.pos.distance(pos) > 0.001 {
            p.last_motion = Some(((pos.y - p.pos.y).atan2(pos.x - p.pos.x), self.now));
        }
        p.pos = pos;
        p.last_move = time;
        p.last_move_at = received;
        if correct {
            self.correct_position(id, time, "immobilized movement")?;
        }
        Ok(())
    }
    fn player_step_valid(&self, from: Pos, to: Pos) -> bool {
        let crosses =
            |a: f32, b: f32| (a % 0.5 == 0. && a != b) || (a * 2.).floor() != (b * 2.).floor();
        // The client only revalidates when departing a half-cell boundary or crossing one.
        (!crosses(from.x, to.x) && !crosses(from.y, to.y))
            || self.player_position_valid(Some(from), to)
    }
    fn player_position_valid(&self, from: Option<Pos>, pos: Pos) -> bool {
        if !pos.x.is_finite() || !pos.y.is_finite() {
            return false;
        }
        let Some(cell) = self.map.cell(pos.x, pos.y) else {
            return false;
        };
        let own = (pos.x.floor() as i32, pos.y.floor() as i32);
        let check_own = from.is_none_or(|p| (p.x.floor() as i32, p.y.floor() as i32) != own);
        if check_own && self.content.grounds[&cell.ground] <= 0. {
            return false;
        }
        let side = |n: f32| {
            if n.fract() < 0.5 {
                -1
            } else if n.fract() > 0.5 {
                1
            } else {
                0
            }
        };
        let (dx, dy) = (side(pos.x), side(pos.y));
        let neighbors = [
            (own.0 + dx, own.1),
            (own.0, own.1 + dy),
            (own.0 + dx, own.1 + dy),
        ];
        if neighbors.iter().any(|&(x, y)| {
            (x, y) != own
                && self
                    .map
                    .cell(x as f32, y as f32)
                    .is_none_or(|c| c.ground == 255)
        }) {
            return false;
        }
        self.near_entities(pos, pos).all(|e| {
            let cell = (e.pos.x.floor() as i32, e.pos.y.floor() as i32);
            if check_own && cell == own {
                !self.content.objects[&e.kind].blocks
            } else if cell != own && neighbors.contains(&cell) {
                !self.content.objects[&e.kind].full_occupy
            } else {
                true
            }
        })
    }
    pub fn walkable(&self, pos: Pos) -> bool {
        let Some(c) = self.map.cell(pos.x, pos.y) else {
            return false;
        };
        if self.content.grounds[&c.ground] <= 0. {
            return false;
        }
        self.entities_at(pos)
            .all(|e| !self.content.objects[&e.kind].blocks)
    }
    // Immutable map scenery participates in collision/visibility, not per-tick AI scans.
    fn actors(&self) -> impl Iterator<Item = &Entity> {
        self.actor_ids.iter().map(|id| {
            self.entities
                .get(id)
                .expect("simulated entity missing from storage")
        })
    }
    fn has_world_effect(kind: u16) -> bool {
        matches!(
            kind,
            crate::dungeon::beach::BUM
                | crate::dungeon::lab::HEXXER
                | crate::dungeon::lab::DEHEXXER
        )
    }
    fn world_effect_actors(&self) -> impl Iterator<Item = &Entity> {
        self.world_effect_ids.iter().map(|id| &self.entities[id])
    }
    fn non_population_actors(&self) -> impl Iterator<Item = &Entity> {
        self.non_population_ids
            .iter()
            .map(|id| self.entities.get(id).expect("scripted actor missing"))
    }
    fn entities_near(&self, pos: Pos, radius: f32) -> impl Iterator<Item = &Entity> {
        self.near_entities(
            Pos {
                x: pos.x - radius,
                y: pos.y - radius,
            },
            Pos {
                x: pos.x + radius,
                y: pos.y + radius,
            },
        )
        .filter(move |e| e.pos.distance(pos) < radius)
    }
    fn actors_near(&self, pos: Pos, radius: f32) -> impl Iterator<Item = &Entity> {
        let clamp = |v: f32, max: i32| (v as i32).clamp(0, max - 1) as usize;
        let (x0, x1) = (
            clamp(pos.x - radius - 1., self.map.width),
            clamp(pos.x + radius + 1., self.map.width),
        );
        let (y0, y1) = (
            clamp(pos.y - radius - 1., self.map.height),
            clamp(pos.y + radius + 1., self.map.height),
        );
        self.cells
            .actors_in(x0, y0, x1, y1)
            .map(|id| &self.entities[&id])
            .filter(move |e| e.pos.distance(pos) < radius)
    }
    fn entities_at(&self, pos: Pos) -> impl Iterator<Item = &Entity> {
        self.map.cell(pos.x, pos.y).into_iter().flat_map(move |_| {
            self.cells
                .ids(pos.y as usize * self.map.width as usize + pos.x as usize)
                .iter()
                .map(|id| self.entities.get(id).expect("occupied cell missing entity"))
        })
    }
    fn near_entities(&self, a: Pos, b: Pos) -> impl Iterator<Item = &Entity> {
        let x0 = (a.x.min(b.x) as i32 - 1).max(0);
        let x1 = (a.x.max(b.x) as i32 + 1).min(self.map.width - 1);
        let y0 = (a.y.min(b.y) as i32 - 1).max(0);
        let y1 = (a.y.max(b.y) as i32 + 1).min(self.map.height - 1);
        (y0..=y1).flat_map(move |y| {
            (x0..=x1).flat_map(move |x| {
                self.cells
                    .ids((y * self.map.width + x) as usize)
                    .iter()
                    .map(|id| self.entities.get(id).expect("occupied cell missing entity"))
            })
        })
    }

    pub fn native_shoot_received(
        &mut self,
        id: i32,
        (time, received): (i32, Instant),
        bullet: u8,
        kind: u16,
        start: Pos,
        angle: f32,
    ) -> Result<NativeInput> {
        let queued_ms = received.elapsed().as_millis();
        let received = received
            .checked_duration_since(self.attack_epoch)
            .context("native SHOOT predates world epoch")?
            .as_millis() as u64;
        tracing::debug!(
            player = id,
            client_time = time,
            received,
            queued_ms,
            bullet,
            kind,
            x = start.x,
            y = start.y,
            angle,
            "native SHOOT ingress"
        );
        let ability = self.players.get(&id).is_some_and(
            |p| matches!(p.ability_shot, Some((k, t, _, _)) if k == kind && t == time),
        );
        if !ability && self.discard_correcting_shot(id, time, bullet, kind, start, angle)? {
            return Ok(NativeInput::DiscardedUntilGotoAck);
        }
        let outcome = self.shoot_timed(id, (time, received, true), bullet, kind, start, angle)?;
        self.players.get_mut(&id).unwrap().discarded_attack = None;
        Ok(outcome)
    }
    // Fixture tests drive fixed ticks without sleeping, so script shots use simulation time.
    pub fn shoot(
        &mut self,
        id: i32,
        time: i32,
        bullet: u8,
        kind: u16,
        start: Pos,
        angle: f32,
    ) -> Result<()> {
        self.shoot_timed(id, (time, self.now, false), bullet, kind, start, angle)
            .map(|_| ())
    }
    fn shoot_timed(
        &mut self,
        id: i32,
        (time, received, native): (i32, u64, bool),
        bullet: u8,
        kind: u16,
        start: Pos,
        angle: f32,
    ) -> Result<NativeInput> {
        ensure!(angle.is_finite(), protocol("invalid shot angle"));
        ensure!(
            start.x.is_finite() && start.y.is_finite(),
            protocol("non-finite shot origin")
        );
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.persistence_failure.is_none(),
            "dead, departing or quarantined player cannot shoot"
        );
        ensure!(
            p.active(),
            protocol("dead, departing or quarantined player cannot shoot")
        );

        let ability = matches!(p.ability_shot, Some((k, t, _, _)) if k == kind && t == time);
        let denied_ability = ability && matches!(p.ability_shot, Some((_, _, false, _)));
        ensure!(
            p.character.items[0] == kind as i32
                || native && p.advertised_weapon(kind)
                || ability && p.character.items[1] == kind as i32,
            protocol("weapon does not match equipped item")
        );
        let item = self.content.get(kind)?;
        ensure!(item.shots > 0, "weapon projectile count is zero");
        let shot_index = if ability {
            p.ability_shot.unwrap().3
        } else if time == p.last_attack {
            p.attack_count
        } else {
            0
        };
        // The client displaces every muzzle along central aim, not each projectile's spread angle.
        let aim =
            angle + item.arc.to_radians() * ((item.shots - 1) as f32 / 2. - shot_index as f32);
        // Prediction cannot advance movement credit or authoritative position.
        let center = Pos {
            x: start.x - 0.3 * aim.cos(),
            y: start.y - 0.3 * aim.sin(),
        };
        let condition = p.conditions(self.now);
        let origin = if native {
            self.validate_native_position(id, center, time).map(|_| 0.)
        } else if !p.paused && p.pos.distance(center) > 0.001 {
            self.validate_move_at(id, center, time, received)
        } else {
            Ok(0.)
        };
        // Drop stale predicted shots for one MOVE time-jump window around movement changes.
        let displaced = ability && p.correcting()?
            || origin.is_err()
                && (condition & PARALYZED != 0
                    || p.statuses.iter().any(|(effect, (since, until))| {
                        crate::condition::bit(*effect) & (SLOWED | SPEEDY | PARALYZED) != 0
                            && (self.now - since < 1000
                                || self.now >= *until && self.now - until < 1000)
                    }));
        if !displaced {
            origin.with_context(|| RejectedNativeShot(format!("invalid shot origin: player=({}, {}) origin=({}, {}) angle={} shot_time={} move_time={} server_time={} last_move_at={}", p.pos.x, p.pos.y, start.x, start.y, angle, time, p.last_move, self.now, p.last_move_at)))?;
        }
        let spec = item
            .projectiles
            .get(&0)
            .cloned()
            .context("weapon has no projectile zero")?;
        // The selected client's integer attack period cannot fund a fractional charge indefinitely.
        let dex = p.effective(&self.content, self.now)?[5];
        // Expired buffs remain for the existing one-second native transition window.
        let late_berserk = native
            && p.statuses
                .get(&crate::condition::BERSERK)
                .is_some_and(|(_, until)| self.now >= *until && self.now - until < 1000);
        let berserk =
            condition & crate::condition::bit(crate::condition::BERSERK) != 0 || late_berserk;
        let normal_period = |dex| {
            let frequency = 0.0015 + (0.008 - 0.0015) * f64::from(dex) / 75.;
            (1. / frequency / f64::from(item.rate) / if berserk { 1.5 } else { 1. }).floor() as f32
        };
        let normal_cooldown = normal_period(dex);
        let effective_cooldown =
            reconciliation::attack_cooldown(p, &self.content, self.now, item.rate)?;
        let cooldown =
            if late_berserk && condition & crate::condition::bit(crate::condition::DAZED) == 0 {
                normal_cooldown
            } else {
                effective_cooldown
            };
        let admission_dex = if native { p.advertised_dex(dex) } else { dex };
        let admission = if native {
            normal_period(admission_dex)
        } else {
            cooldown
        };
        let p = self.players.get_mut(&id).unwrap();
        ensure!(time >= 0, protocol("negative shot time"));
        ensure!(
            shot_index < item.shots,
            protocol("too many projectiles in attack")
        );
        ensure!(
            bullet == p.next_bullet,
            protocol("unexpected player bullet counter")
        );
        if !ability && time != p.last_attack {
            let attack_elapsed = i64::from(time) - i64::from(p.last_attack);
            if attack_elapsed < admission as i64 - 2 {
                tracing::warn!(
                    player = id,
                    kind,
                    dex,
                    advertised_dex = admission_dex,
                    native,
                    elapsed_ms = attack_elapsed,
                    required_ms = admission,
                    authoritative_ms = cooldown,
                    client_time = time,
                    last_client_time = p.last_attack,
                    "attack cadence rejected"
                );
                return Err(protocol("attack too fast"));
            }
            let elapsed = received
                .checked_sub(p.last_attack_at)
                .context("attack receive clock regressed")?;
            // Delivery can batch correctly spaced native shots; their clock, not ingress spacing, funds cadence.
            let funding = if native {
                attack_elapsed as f32
            } else {
                elapsed as f32
            };
            let credit = (p.attack_credit + funding).min(cooldown * 2.);
            ensure!(
                native || credit >= admission - 50.,
                RejectedNativeShot(format!(
                    "attack exceeds server time budget: credit={credit} cooldown={cooldown} received_time={received} simulation_time={} last_received_time={} shot_time={time} last_shot_time={}",
                    self.now, p.last_attack_at, p.last_attack
                ))
            );
            p.attack_suppressed =
                if attack_elapsed < cooldown as i64 - 2 || (!native && credit < cooldown - 50.) {
                    Some(AttackSuppression::EffectiveCooldown)
                } else {
                    None
                };
            p.attack_credit = credit
                - match p.attack_suppressed {
                    Some(AttackSuppression::EffectiveCooldown) => normal_cooldown,
                    None => cooldown,
                };
            p.last_attack_at = received;
            p.last_attack = time;
            p.attack_count = 0;
        }
        p.next_bullet = (bullet + 1) % 128;
        if ability {
            if shot_index + 1 == item.shots {
                p.ability_shot = None;
            } else {
                p.ability_shot.as_mut().unwrap().3 += 1;
            }
        } else {
            p.attack_count += 1;
        }
        let draw = p.roll_damage(&spec);
        // In-flight native SHOOT packets must still consume their counter/RNG draw.
        if p.paused
            || displaced
            || denied_ability
            || !ability && (condition & STUNNED != 0 || p.attack_suppressed.is_some())
        {
            tracing::debug!(
                player = id,
                condition,
                displaced,
                denied_ability,
                suppressed = ?p.attack_suppressed,
                "condition suppressed shot"
            );
            return Ok(NativeInput::Applied);
        }
        p.character.shots += 1;
        p.character.lifetime.shots = p
            .character
            .lifetime
            .shots
            .checked_add(1)
            .context("lifetime admitted shots overflow")?;
        let multiplier = if ability {
            1.
        } else if condition & crate::condition::bit(crate::condition::WEAK) != 0 {
            0.5
        } else {
            (0.5 + (f64::from(p.effective(&self.content, self.now)?[2]) / 75.) * (2. - 0.5))
                * if condition & crate::condition::bit(crate::condition::DAMAGING) != 0 {
                    1.5
                } else {
                    1.
                }
        };
        let damage = (f64::from(draw) * multiplier) as i32;
        self.bullets.push(Bullet {
            owner: id,
            source_kind: p.character.class,
            id: bullet,
            friendly: true,
            start,
            pos: start,
            angle,
            born: self.now,
            damage,
            spec,
            hit: BTreeSet::new(),
            hit_enemy: false,
        });
        let mut o = Writer::new();
        o.u8(bullet).i32(id).i16(kind as i16).f32(angle);
        spectator::publish(&self.players[&id], 74, &o.0);
        for (other, p) in &self.players {
            if *other != id && p.known.contains(&id) && p.pos.distance(start) <= 20. {
                send(p, 74, o.0.clone());
            }
        }
        tracing::debug!(player = id, bullet, damage, "player shot");
        Ok(NativeInput::Applied)
    }
    pub fn native_use_item(
        &mut self,
        id: i32,
        time: i32,
        slot: Slot,
        pos: Pos,
    ) -> Result<Option<InventoryPlan>> {
        let ability = slot.kind >= 0
            && slot.slot == 1
            && slot.object == id
            && self
                .players
                .get(&id)
                .is_some_and(|p| p.character.items[1] == slot.kind)
            && self.content.get(slot.kind as u16)?.activate == "Shoot";
        ensure!(!ability || time >= 0, controller("negative ability time"));
        ensure!(
            !ability || (1..=5).contains(&self.content.get(slot.kind as u16)?.shots),
            "unsupported native ability projectile count"
        );
        let plan = self.use_item(id, slot, pos);
        if ability {
            // Even rejected local predictions consume a native bullet ID and damage RNG draw.
            self.players.get_mut(&id).unwrap().ability_shot =
                Some((slot.kind as u16, time, plan.is_ok(), 0));
        }
        plan
    }
    pub fn use_item(&mut self, id: i32, slot: Slot, pos: Pos) -> Result<Option<InventoryPlan>> {
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
        let p = self.players.get(&id).context("player missing")?;
        ensure!(!p.paused, controller("Cannot use items while paused."));
        ensure!(
            p.persistence_failure.is_none(),
            "player is busy, departing, quarantined or dead"
        );
        let source = if slot.object == id {
            &p.character.items
        } else {
            let bag = self
                .entities
                .get(&slot.object)
                .ok_or_else(|| controller("item ownership mismatch"))?;
            ensure!(
                !bag.reserved
                    && bag.accessible(p.character.account)
                    && p.pos.distance(bag.pos) < 1.5
                    && self.content.get(bag.kind)?.class == "Container",
                controller("item container inaccessible")
            );
            &bag.items
        };
        ensure!(
            source.get(slot.slot) == Some(&slot.kind),
            controller("item ownership mismatch")
        );
        ensure!(slot.kind >= 0, controller("empty item slot"));
        let item = self.content.get(slot.kind as u16)?.clone();
        // Saving earned XP must not freeze abilities; inventory mutations remain serialized.
        ensure!(
            p.active()
                && p.character.hp > 0
                && (!p.pending || p.progress_started && !item.abilities.is_empty()),
            controller("player is busy, departing, quarantined or dead")
        );
        if item.xp_boost {
            ensure!(
                item.kind == 0xc42 && item.consumable && item.activations == 0,
                "unsupported XP booster"
            );
            ensure!(
                p.character.level < 20,
                controller("XP boost requires a character below level 20")
            );
            ensure!(
                !p.character.xp_boost.active,
                controller("XP boost is already active")
            );
            ensure!(
                !p.rewards_queued(),
                controller("character progress is awaiting commit")
            );
            let mut plan = self.consumable_plan(id, slot, -1)?;
            plan.xp_boost = Some(slot);
            return Ok(Some(plan));
        }
        if !item.abilities.is_empty() {
            ensure!(slot.object == id, controller("ability must be equipped"));
            self.activate_ability(id, slot, &item, pos)?;
            return Ok(None);
        }
        if matches!(
            item.activate.as_str(),
            "Pet" | "PermaPet" | "Create" | "UnlockPortal"
        ) {
            ensure!(
                slot.object == id,
                controller("item activation requires owned inventory")
            );
        }
        match item.activate.as_str() {
            "Dye" => {
                ensure!(
                    item.class == "Dye"
                        && item.consumable
                        && item.activations == 1
                        && item.textures.iter().any(Option::is_some),
                    "invalid appearance consumable"
                );
                let (mut tex1, mut tex2) = (p.character.tex1, p.character.tex2);
                if let Some(value) = item.textures[0] {
                    tex1 = value;
                }
                if let Some(value) = item.textures[1] {
                    tex2 = value;
                }
                let source = if slot.object == id {
                    None
                } else {
                    Some(self.reserve_ground_dye(id, slot)?)
                };
                let mut plan = self.consumable_plan(id, slot, -1)?;
                plan.ground_dye = source;
                plan.appearance = Some((slot, tex1, tex2));
                Ok(Some(plan))
            }
            "IncrementStat" => {
                ensure!(
                    matches!(
                        item.kind,
                        0xa1f | 0xa20 | 0xa21 | 0xa34 | 0xa35 | 0xa4c | 0xae9 | 0xaea
                    ) && item.consumable
                        && item.activations == 1,
                    "unsupported permanent stat consumable"
                );
                let (stat, amount) = item
                    .stat_increase
                    .context("permanent potion activation missing")?;
                let index = crate::content::STATS
                    .iter()
                    .position(|(id, _)| *id == stat)
                    .context("unsupported permanent potion stat")?;
                let remaining =
                    self.content.classes[&p.character.class].caps[index] - p.character.base[index];
                ensure!(amount > 0, "stat is already at its class cap");
                ensure!(
                    remaining > 0,
                    controller("stat is already at its class cap")
                );
                let amount = amount.min(remaining);
                let snapshot = p.character.clone();
                let mut plan = self.consumable_plan(id, slot, -1)?;
                plan.stat_growth = Some((snapshot, slot, index, amount));
                Ok(Some(plan))
            }
            "Pet" | "PermaPet" => self.begin_companion(id, slot, &item).map(Some),
            "Create" => self.begin_create(id, slot, &item).map(Some),
            "UnlockPortal" => self.begin_unlock(id, slot, &item).map(Some),
            _ => self.begin_consumable(id, slot, &item).map(Some),
        }
    }
    fn consumable_plan(&mut self, id: i32, slot: Slot, next: i32) -> Result<InventoryPlan> {
        let mut plan = self.plan(id, self.players[&id].character.items.clone(), None);
        if slot.object == id {
            plan.items[slot.slot] = next;
        } else {
            let bag = self.entities.get_mut(&slot.object).unwrap();
            let mut items = bag.items.clone();
            items[slot.slot] = next;
            plan.bags.push((slot.object, items));
            plan.loot_slot = Some((slot.object, slot.slot));
        }
        self.bind_vaults(&mut plan)?;
        for (bag, _) in &plan.bags {
            self.entities.get_mut(bag).unwrap().reserved = true;
        }
        self.players.get_mut(&id).unwrap().pending = true;
        Ok(plan)
    }
    fn plan(&self, id: i32, items: Vec<i32>, bag: Option<(i32, Vec<i32>)>) -> InventoryPlan {
        let p = &self.players[&id];
        InventoryPlan {
            ground_dye: None,
            player: id,
            items,
            revision: p.character.revision,
            account: p.character.account,
            character: p.character.id,
            bags: bag.into_iter().collect(),
            effects: Vec::new(),
            stat_growth: None,
            potion: None,
            appearance: None,
            xp_boost: None,
            companion: None,
            vaults: Vec::new(),
            vault_purchase: None,
            dropped: None,
            unlock: None,
            collected: None,
            purchase: None,
            loot_slot: None,
        }
    }
    pub fn drop_item(&mut self, id: i32, slot: Slot) -> Result<InventoryPlan> {
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
        let p = self.players.get(&id).context("drop owner missing")?;
        ensure!(
            p.persistence_failure.is_none(),
            "player busy, departing or dead"
        );
        ensure!(
            !p.pending && !p.death_pending && !p.departing && p.character.alive,
            controller("player busy, departing or dead")
        );
        let pos = p.pos;
        let account = p.character.account;
        let mut items = p.character.items.clone();
        let mut source = if slot.object == id {
            None
        } else {
            let e = self
                .entities
                .get(&slot.object)
                .ok_or_else(|| controller("drop container missing"))?;
            ensure!(
                !e.reserved
                    && e.accessible(account)
                    && pos.distance(e.pos) < 1.5
                    && self.content.get(e.kind)?.class == "Container",
                controller("drop container inaccessible")
            );
            Some((slot.object, e.items.clone()))
        };
        let inventory = match &mut source {
            Some((_, items)) => items,
            None => &mut items,
        };
        ensure!(
            slot.kind >= 0 && inventory.get(slot.slot) == Some(&slot.kind),
            controller("empty or stale drop slot")
        );
        inventory[slot.slot] = -1;
        let mut plan = self.plan(id, items, source);
        self.bind_vaults(&mut plan)?;
        let dropped = self.reserve_drop(pos, account, slot.kind, false)?;
        for (source, _) in &plan.bags {
            self.entities.get_mut(source).unwrap().reserved = true;
        }
        self.players.get_mut(&id).unwrap().pending = true;
        if slot.object != id {
            plan.loot_slot = Some((slot.object, slot.slot));
        }
        plan.dropped = Some(dropped);
        Ok(plan)
    }
    fn reserve_drop(&mut self, pos: Pos, account: i32, kind: i32, private: bool) -> Result<i32> {
        let soulbound = self.content.get(u16::try_from(kind)?)?.soulbound;
        let dropped = self.loot_bag(pos, (soulbound || private).then_some(account), vec![kind])?;
        let bag = self.entities.get_mut(&dropped).unwrap();
        bag.kind = if soulbound { 0x503 } else { 0x500 };
        bag.published = false;
        bag.reserved = true;
        Ok(dropped)
    }
    pub fn swap(&mut self, id: i32, from: Slot, to: Slot) -> Result<InventoryPlan> {
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.persistence_failure.is_none(),
            "player busy, departing or dead"
        );
        ensure!(
            !p.pending && !p.death_pending && !p.departing && p.character.alive,
            controller("player busy, departing or dead")
        );
        ensure!(
            from.object == id
                || to.object == id
                || (self.vaults.contains_key(&from.object) && self.vaults.contains_key(&to.object)),
            controller("swap must involve own inventory or owned vault chests")
        );
        let mut items = p.character.items.clone();
        let mut bags = BTreeMap::new();
        for object in [from.object, to.object]
            .into_iter()
            .filter(|object| *object != id)
        {
            let e = self
                .entities
                .get(&object)
                .ok_or_else(|| controller("container missing"))?;
            ensure!(
                !e.reserved
                    && e.accessible(p.character.account)
                    && p.pos.distance(e.pos) < 1.5
                    && self.content.get(e.kind)?.class == "Container",
                controller("container inaccessible")
            );
            bags.insert(object, e.items.clone());
        }
        let get = |s: &Slot| -> Result<i32> {
            let inventory = if s.object == id {
                &items
            } else {
                bags.get(&s.object).context("unbound swap source")?
            };
            Ok(*inventory
                .get(s.slot)
                .ok_or_else(|| controller("invalid inventory slot"))?)
        };
        let a = get(&from)?;
        let b = get(&to)?;
        ensure!(
            a == from.kind && b == to.kind,
            controller("stale inventory offer")
        );
        let mut replacement = None;
        for (s, k) in [(&from, b), (&to, a)] {
            if s.object != id && k >= 0 {
                let item = self.content.get(u16::try_from(k)?)?;
                let container = &self.entities[&s.object];
                let definition = self.content.get(container.kind)?;
                let accepts = if item.soulbound {
                    definition.put_soulbound
                        && container.extra_stats.contains(&(54, p.character.account))
                } else {
                    definition.put_normal
                };
                if !accepts {
                    ensure!(
                        a >= 0 && b >= 0 && self.bag_expiry.contains_key(&s.object),
                        controller("container rejects this normal or Soulbound item")
                    );
                    replacement = Some(k);
                }
            }
            if s.object == id && s.slot < 4 && k >= 0 {
                let obj = self.content.get(k as u16)?;
                ensure!(
                    !obj.unsupported_equipment,
                    controller("equipment effect not implemented")
                );
                ensure!(
                    obj.slot_type == self.content.get(p.character.class)?.slots[s.slot],
                    controller("incompatible equipment slot")
                );
            }
        }
        if from.object == id {
            items[from.slot] = b
        } else {
            bags.get_mut(&from.object).unwrap()[from.slot] = b
        }
        if to.object == id {
            items[to.slot] = a
        } else {
            bags.get_mut(&to.object).unwrap()[to.slot] = a
        }
        let dropped = if let Some(kind) = replacement {
            // A swap must not put Soulbound gear in public loot or violate the source bag's put flags.
            let source = if from.object == id { to } else { from };
            bags.get_mut(&source.object).unwrap()[source.slot] = -1;
            let private = self.entities[&source.object]
                .extra_stats
                .contains(&(54, p.character.account));
            Some(self.reserve_drop(p.pos, p.character.account, kind, private)?)
        } else {
            None
        };
        let mut plan = self.plan(id, items, None);
        plan.bags = bags.into_iter().collect();
        self.bind_vaults(&mut plan)?;
        plan.dropped = dropped;
        for (container, _) in &plan.bags {
            let source = if from.object == *container {
                &from
            } else {
                &to
            };
            if plan.bags.len() == 1 {
                plan.loot_slot = Some((*container, source.slot));
                if source.kind >= 0
                    && self.entities[container].drop_slots.contains(&source.slot)
                    && self.players[&id].controller == Controller::Human
                    && self.content.get(source.kind as u16)?.class == "Equipment"
                {
                    plan.collected = Some(source.kind as u16);
                }
            }
        }
        for (container, _) in &plan.bags {
            self.entities.get_mut(container).unwrap().reserved = true;
        }
        self.players.get_mut(&id).unwrap().pending = true;
        Ok(plan)
    }
    pub fn committed<T>(&mut self, id: i32, result: Result<T>) -> Result<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.poison(id, format!("{error:#}"))?;
                Err(error)
            }
        }
    }
    pub fn poison(&mut self, id: i32, error: String) -> Result<()> {
        self.players
            .get_mut(&id)
            .context("quarantine owner missing")
            .map(|p| p.quarantine(id, error))
    }
    #[tracing::instrument(skip_all, fields(player=plan.player, account=plan.account, character=plan.character, expected_revision=plan.revision))]
    pub fn finish_inventory(&mut self, plan: InventoryPlan, result: Result<i32>) -> Result<()> {
        ensure!(
            plan.unlock.is_none(),
            "portal unlock requires its directory commit"
        );
        let player = plan.player;
        let source = (plan.character, plan.revision);
        let companion = plan.companion.clone();
        let ground_dye = plan.ground_dye.as_ref().map(|source| source.bag_id());
        let result = self.publish_inventory(plan, result);
        if let Err(error) = &result {
            if let Some(id) = ground_dye
                && let Some(bag) = self.entities.get_mut(&id)
            {
                // An error may conceal COMMIT; retain even a damaged reservation, never refund the source.
                bag.reserved = true;
            }
            self.discard_consumable(source);
            if let Err(cleanup) = self.discard_companion(&companion) {
                tracing::error!(player,%cleanup,"companion cleanup failed after rejected publication");
            }
            self.poison(player, format!("{error:#}"))?;
        }
        result
    }
    fn publish_inventory(&mut self, plan: InventoryPlan, result: Result<i32>) -> Result<()> {
        let revision = result?;
        if plan.ground_dye.is_some() {
            self.validate_ground_dye(&plan)?;
        }
        let p = self
            .players
            .get_mut(&plan.player)
            .context("player disappeared during save")?;
        ensure!(
            p.pending
                && p.persistence_failure.is_none()
                && p.character.id == plan.character
                && p.character.account == plan.account
                && p.character.revision == plan.revision
                && plan.revision.checked_add(1) == Some(revision),
            "inventory completion owner, reservation or revision changed"
        );
        p.pending = false;
        for (id, _) in &plan.bags {
            ensure!(
                self.entities
                    .get(id)
                    .context("reserved bag disappeared")?
                    .reserved,
                "inventory source reservation lost"
            );
        }
        if let Some(dropped) = plan.dropped {
            ensure!(
                self.entities
                    .get(&dropped)
                    .context("reserved ground drop missing")?
                    .reserved,
                "ground drop reservation lost"
            );
        }
        if let Some((bag, slot)) = plan.loot_slot {
            self.entities
                .get_mut(&bag)
                .context("reserved drop source disappeared")?
                .drop_slots
                .remove(&slot);
        }
        let mut o = Writer::new();
        p.character.items = plan.items;
        p.character.revision = revision;
        // Stat growth is silent in the 2013 comparator: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/realm/entities/player/Player.UseItem.cs#L659-L679
        if let Some((snapshot, source, index, amount)) = plan.stat_growth {
            ensure!(
                p.character.base == snapshot.base,
                "reserved potion base stats changed"
            );
            let cap = self.content.classes[&p.character.class].caps[index];
            ensure!(
                snapshot.base[index] + amount <= cap,
                "reserved potion exceeds current class cap"
            );
            let mut committed = snapshot.lifetime.clone();
            committed.inventory_potion_used |= source.object == plan.player;
            p.acknowledge_lifetime(committed)?;
            p.character.base[index] = snapshot.base[index] + amount;
            tracing::info!(
                player = plan.player,
                stat = crate::content::STATS[index].0,
                base = p.character.base[index],
                revision,
                "permanent stat potion committed"
            );
        }
        if plan.potion.is_some() {
            let mut committed = p.committed_lifetime.clone();
            committed.inventory_potion_used = true;
            p.acknowledge_lifetime(committed)?;
        }
        if let Some((slot, tex1, tex2)) = plan.appearance {
            p.character.tex1 = tex1;
            p.character.tex2 = tex2;
            tracing::info!(
                player = plan.player,
                tex1,
                tex2,
                revision,
                source = slot.object,
                slot = slot.slot,
                item = slot.kind,
                "appearance consumable committed"
            );
        }
        if plan.xp_boost.is_some() {
            p.character.xp_boost.active = true;
            tracing::info!(player = plan.player, revision, "XP boost committed");
        }
        let values = p.effective(&self.content, self.now)?;
        p.character.hp = p.character.hp.min(values[0]);
        p.character.mp = p.character.mp.min(values[1]);
        if self.prepared_seed.is_some()
            && let Some((bag, slot)) = plan.loot_slot
        {
            let item = self
                .entities
                .get(&bag)
                .and_then(|bag| bag.items.get(slot))
                .context("committed loot identity missing")?;
            tracing::info!(player=plan.player,character=p.character.id,bag,slot,item=*item,world=%self.name,"loot slot committed");
        }
        for mut vault in plan.vaults {
            let (id, loaded) = self
                .vaults
                .iter_mut()
                .find(|(_, v)| v.chest == vault.chest && v.account == vault.account)
                .context("committed chest missing")?;
            ensure!(
                loaded.revision == vault.revision
                    && self.entities[id].kind == 0x504
                    && self.entities[id].items == loaded.items
                    && self.entities[id].extra_stats.contains(&(54, plan.account))
                    && plan
                        .bags
                        .iter()
                        .any(|(bag, items)| bag == id && items == &vault.items),
                "committed chest identity, ownership or revision changed"
            );
            vault.revision += 1;
            *loaded = vault;
        }
        self.finish_consumable(plan.player, (plan.character, plan.revision), plan.effects)?;
        self.finish_companion(plan.player, plan.companion)?;
        for (id, items) in plan.bags {
            let bag = self.entities.get_mut(&id).unwrap();
            bag.items = items;
            bag.reserved = false;
        }
        if let Some(dropped) = plan.dropped {
            let e = self.entities.get_mut(&dropped).unwrap();
            e.reserved = false;
            e.published = true;
            self.bag_expiry.insert(dropped, self.now + 60_000);
        }
        tracing::info!(player = plan.player, revision, "inventory committed");
        let p = &self.players[&plan.player];
        if plan.purchase.is_some() {
            let packet = shop::reply(true, "Item purchased!");
            send(p, packet.id, packet.body);
        } else {
            o.i32(0);
            send(p, 4, o.0);
        }
        Ok(())
    }
    pub fn action(&mut self, id: i32, a: Action) -> Result<Option<InventoryPlan>> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.controller == Controller::Qa,
            controller("script actions require a QA-owned player")
        );
        self.controlled_action(id, a)
    }
    pub fn autonomous_action(&mut self, id: i32, a: Action) -> Result<Option<InventoryPlan>> {
        ensure!(
            self.players.get(&id).context("player missing")?.controller == Controller::Autonomous,
            "autonomous actions require an autonomous controller"
        );
        self.controlled_action(id, a)
    }
    fn controlled_action(&mut self, id: i32, a: Action) -> Result<Option<InventoryPlan>> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.persistence_failure.is_none(),
            "dead, departing or quarantined player cannot act"
        );
        ensure!(
            p.active(),
            controller("dead, departing or quarantined player cannot act")
        );
        match a {
            Action::Move { x, y } => {
                ensure!(
                    self.player_position_valid(Some(p.pos), Pos { x, y }),
                    controller("goal outside map or blocked")
                );
                self.players
                    .get_mut(&id)
                    .context("player missing")?
                    .bot_goal = Some(Pos { x, y });
                Ok(None)
            }
            Action::Shoot { angle } => {
                ensure!(angle.is_finite(), controller("invalid aim"));
                self.players.get_mut(&id).context("player missing")?.bot_aim = Some(angle);
                Ok(None)
            }
            Action::Stop => {
                let p = self.players.get_mut(&id).context("player missing")?;
                p.bot_goal = None;
                p.bot_aim = None;
                Ok(None)
            }
            Action::Use { slot, x, y } => {
                let kind = *self
                    .players
                    .get(&id)
                    .context("player missing")?
                    .character
                    .items
                    .get(slot)
                    .ok_or_else(|| controller("invalid slot"))?;
                self.use_item(
                    id,
                    Slot {
                        object: id,
                        slot,
                        kind,
                    },
                    Pos { x, y },
                )
            }
            Action::Swap { from, to } => self.swap(id, from, to).map(Some),
            Action::Drop { slot } => self.drop_item(id, slot).map(Some),
        }
    }
    pub fn correct_position(&mut self, id: i32, time: i32, reason: &str) -> Result<()> {
        let p = self.players.get_mut(&id).context("player missing")?;
        let previous_time = p.last_move;
        ensure!(!p.correcting()?, "position correction is already pending");
        let mut out = Writer::new();
        out.i32(id).f32(p.pos.x).f32(p.pos.y);
        reconciliation::goto(p, true, out.0);
        tracing::warn!(
            player = id,
            previous_time,
            client_time = time,
            reason,
            "movement rejected; sent authoritative GOTO"
        );
        Ok(())
    }

    pub(crate) fn pending_progress_where(
        &mut self,
        mut accept: impl FnMut(i32, i32) -> bool,
    ) -> Vec<(i32, Character)> {
        let mut plans = Vec::new();
        for (id, p) in &mut self.players {
            if !p.progress_queued()
                || p.pending
                || p.death_started
                || ((p.death_pending || p.departing) && !p.rewards_queued())
            {
                continue;
            }
            if !accept(*id, p.character.account) {
                continue;
            }
            let mut snapshot = p.character.clone();
            let mut assists = Vec::new();
            let result = (|| -> Result<()> {
                ensure!(
                    p.level_assists_prepared.is_empty(),
                    "unacknowledged level assists"
                );
                match &p.experience_queue {
                    Some(queue) => {
                        ensure!(
                            queue.amount == p.experience_queued
                                && queue.anchor == ProgressAnchor::of(&p.character),
                            "experience queue mismatch: player={} character={} scalar={} sources={} queued_anchor={:?} current_anchor={:?}",
                            id,
                            p.character.id,
                            p.experience_queued,
                            queue.amount,
                            queue.anchor,
                            ProgressAnchor::of(&p.character)
                        );
                        assists = queue.assists[usize::from(p.character.xp_boost.active)].clone();
                    }
                    None => ensure!(
                        p.experience_queued == 0,
                        "missing experience sources: player={} character={} scalar={} sources=0",
                        id,
                        p.character.id,
                        p.experience_queued
                    ),
                }
                ensure!(
                    assists.len() <= 19,
                    "level assist preparation exceeds lifetime levels"
                );
                if p.rewards_queued() {
                    snapshot.gain_experience(&self.content, p.experience_queued, &mut self.rng)?;
                }
                for (count, queued) in std::iter::once(&mut snapshot.pirate_caves)
                    .chain(snapshot.dungeons.iter_mut())
                    .chain(std::iter::once(&mut snapshot.abysses))
                    .chain(std::iter::once(&mut snapshot.trenches))
                    .chain(std::iter::once(&mut snapshot.spider_dens))
                    .chain(std::iter::once(&mut snapshot.mad_labs))
                    .chain(std::iter::once(&mut snapshot.jungles))
                    .chain(std::iter::once(&mut snapshot.manors))
                    .chain(std::iter::once(&mut snapshot.beaches))
                    .chain(std::iter::once(&mut snapshot.davy_lockers))
                    .chain(std::iter::once(&mut snapshot.tombs))
                    .zip(p.clears_queued)
                {
                    *count = count
                        .checked_add(queued)
                        .context("dungeon clear counter overflow")?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                p.quarantine(*id, format!("progress preparation failed: {error:#}"));
                continue;
            }
            snapshot.alive = true;
            p.experience_queued = 0;
            p.experience_queue = None;
            p.progress_snapshot = Some(snapshot.clone());
            p.level_assists_prepared = assists;
            p.clears_queued = [0; crate::dungeon::DUNGEONS.len()];
            p.pending = true;
            p.progress_started = true;
            plans.push((*id, snapshot));
        }
        plans
    }
    pub(crate) fn progress_assists(
        &self,
        id: i32,
        snapshot: &Character,
    ) -> Result<Vec<LevelAssist>> {
        let p = self
            .players
            .get(&id)
            .context("progress assist owner missing")?;
        p.validate_progress_reservation(snapshot)?;
        Ok(p.level_assists_prepared.clone())
    }
    pub(crate) fn class_quest_rank(&self, id: i32) -> Result<i32> {
        let player = self
            .players
            .get(&id)
            .context("class-quest rank player missing")?;
        Ok(player.presentation.stars() / self.content.classes.len() as i32)
    }

    pub fn finish_progress(&mut self, id: i32, snapshot: Character) -> Result<()> {
        let p = self
            .players
            .get_mut(&id)
            .context("progress owner missing")?;
        p.validate_progress_reservation(&snapshot)?;
        p.acknowledge_lifetime(snapshot.lifetime.clone())?;
        let cleared_dungeon = snapshot.pirate_caves > p.character.pirate_caves
            || snapshot.dungeons != p.character.dungeons
            || snapshot.jungles > p.character.jungles
            || snapshot.abysses > p.character.abysses
            || snapshot.trenches > p.character.trenches
            || snapshot.spider_dens > p.character.spider_dens
            || snapshot.mad_labs > p.character.mad_labs
            || snapshot.manors > p.character.manors
            || snapshot.beaches > p.character.beaches
            || snapshot.davy_lockers > p.character.davy_lockers
            || snapshot.tombs > p.character.tombs;
        let leveled = snapshot.level > p.character.level;
        p.character.exp = snapshot.exp;
        p.character.xp_boost = snapshot.xp_boost;
        p.character.fame = snapshot.fame;
        let best = p
            .presentation
            .best_fame
            .entry(p.character.class)
            .or_default();
        *best = (*best).max(snapshot.fame);
        p.character.level = snapshot.level;
        p.character.base = snapshot.base;
        p.character.pirate_caves = snapshot.pirate_caves;
        p.character.dungeons = snapshot.dungeons;
        p.character.abysses = snapshot.abysses;
        p.character.jungles = snapshot.jungles;
        p.character.trenches = snapshot.trenches;
        p.character.spider_dens = snapshot.spider_dens;
        p.character.mad_labs = snapshot.mad_labs;
        p.character.manors = snapshot.manors;
        p.character.beaches = snapshot.beaches;
        p.character.davy_lockers = snapshot.davy_lockers;
        p.character.tombs = snapshot.tombs;
        if leveled && !p.death_pending {
            let values = p.effective(&self.content, self.now)?;
            p.character.hp = values[0];
            p.character.mp = values[1];
            p.hp_fraction = 0.;
            p.mp_fraction = 0.;
        }
        p.pending = false;
        p.progress_started = false;
        p.progress_snapshot = None;
        p.level_assists_prepared.clear();
        tracing::info!(
            player = id,
            level = p.character.level,
            experience = p.character.exp,
            fame = p.character.fame,
            stars = p.presentation.stars(),
            bonus_experience = p.character.xp_boost.bonus_exp,
            xp_boost = p.character.xp_boost.active,
            "progress committed"
        );
        if cleared_dungeon && let Some(exit) = self.clear_exit.take() {
            if self.name == "Tomb of the Ancients" {
                let old_exits: Vec<_> = self
                    .entities
                    .values()
                    .filter(|e| e.kind == crate::dungeon::tomb::EXIT)
                    .map(|e| e.id)
                    .collect();
                for id in old_exits {
                    self.remove_entity(id);
                }
            }
            self.entities
                .get_mut(&exit)
                .context("reserved clear exit missing")?
                .published = true;
            tracing::info!(
                entity = exit,
                dungeon = self.name,
                "durable dungeon clear published exit"
            );
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn pending_deaths(&mut self) -> Vec<(i32, Character, String)> {
        self.pending_deaths_where(|_, _| true)
    }
    pub(crate) fn pending_deaths_where(
        &mut self,
        mut accept: impl FnMut(i32, i32) -> bool,
    ) -> Vec<(i32, Character, String)> {
        let dying: Vec<_> = self
            .players
            .iter()
            .filter(|(_, p)| p.death_pending)
            .map(|(id, _)| *id)
            .collect();
        dying
            .into_iter()
            .filter_map(|id| {
                self.cancel_trade(id, "Trade cancelled by permanent death.");
                let p = self.players.get_mut(&id).unwrap();
                if p.pending || p.rewards_queued() || p.death_started {
                    return None;
                }
                if !accept(id, p.character.account) {
                    return None;
                }
                p.death_started = true;
                p.death_lifetime = Some(p.character.lifetime.clone());
                let mut c = p.character.clone();
                c.alive = false;
                Some((id, c, p.killer.clone()))
            })
            .collect()
    }
    pub fn finish_death(&mut self, id: i32) -> Result<()> {
        let p = self.players.get(&id).context("dying player missing")?;
        ensure!(
            p.death_started
                && p.death_pending
                && p.persistence_failure.is_none()
                && p.death_lifetime.as_ref() == Some(&p.character.lifetime),
            "death not pending or persistence failed"
        );
        let maxed = p
            .character
            .base
            .iter()
            .zip(self.content.classes[&p.character.class].caps)
            .filter(|(value, cap)| **value >= *cap)
            .count();
        let (kind, lifetime) = match maxed {
            0 if p.character.level == 1 => (0x723, Some(30_000)),
            0 if p.character.level < 20 => (0x724, Some(60_000)),
            0 => (0x725, Some(300_000)),
            7 => (0x734, None),
            8 => (0x735, None),
            n => (0x725 + n as u16, None),
        };
        let (pos, name) = (p.pos, p.character.name.clone());
        // Gates or terrain can become blocked after the dying player entered the tile.
        let grave = self.insert_entity(kind, pos, vec![])?;
        let entity = self.entities.get_mut(&grave).unwrap();
        entity.name = name;
        entity
            .expires
            .set(lifetime.map(|ms| self.now + ms), grave, &mut self.timed_ids);
        tracing::info!(player=id, grave, kind, maxed, expires=?entity.expires, "gravestone created");
        let p = self.players.get_mut(&id).unwrap();
        p.acknowledge_lifetime(
            p.death_lifetime
                .as_ref()
                .context("fatal lifetime reservation missing")?
                .clone(),
        )?;
        p.death_lifetime = None;
        p.character.alive = false;
        p.character.xp_boost.active = false;
        p.character.items.fill(-1);
        p.death_pending = false;
        let mut out = Writer::new();
        out.i32(p.character.account)
            .i32(p.character.id)
            .utf(&p.killer);
        tracing::info!(player=id,character=p.character.id,killed_by=%p.killer,"permanent death committed");
        // Low-level deaths were system notices too: https://media.gdcvault.com/gdceurope2012/Presentations/Social%20%26%20Online%20Games%20Summit/Ethan_Levy_Social%26Online_GameDesignIs.pdf#page=54
        let message = format!(
            "{} died at level {}, killed by {}",
            p.character.name, p.character.level, p.killer
        );
        let recipients = self.broadcast_text("", &message);
        tracing::info!(player = id, recipients, "death notice delivered");
        send(&self.players[&id], 41, out.0);
        self.clear_abilities(id);
        self.clear_companions(id);
        Ok(())
    }
    pub(crate) fn stop_controller(&mut self, id: i32) -> Result<()> {
        let p = self
            .players
            .get_mut(&id)
            .context("stopping controller missing")?;
        p.departing = true;
        p.tx = None;
        p.bot_goal = None;
        p.bot_aim = None;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn credit_reservation_for_test(&self, id: i32) -> (Option<Character>, bool) {
        let p = &self.players[&id];
        (p.progress_snapshot.clone(), p.departure_snapshot.is_some())
    }
    pub(crate) fn credit_recipients(&self, giver: i32) -> Vec<(i32, i32)> {
        self.players
            .iter()
            .filter(|(_, p)| p.owes_credit(giver))
            .map(|(id, p)| (*id, p.character.account))
            .collect()
    }
    pub(crate) fn refresh_giver_credits(&mut self, id: i32, count: u64) -> Result<()> {
        let p = self.players.get_mut(&id).context("credit giver missing")?;
        ensure!(
            p.departing || p.death_pending,
            "outgoing assist authority is not sealed"
        );
        ensure!(p.persistence_failure.is_none(), "credit giver quarantined");
        p.character.lifetime.level_assists = p.character.lifetime.level_assists.max(count);
        Ok(())
    }
    pub fn begin_departure(&mut self, id: i32) -> Result<Option<Character>> {
        self.cancel_trade(id, "Trade cancelled by departure.");
        let p = self
            .players
            .get_mut(&id)
            .context("departing player missing")?;
        ensure!(
            p.persistence_failure.is_none(),
            "quarantined persistence failure: {:?}",
            p.persistence_failure
        );
        self.stop_controller(id)?;
        let p = self
            .players
            .get_mut(&id)
            .context("departing player missing")?;
        if p.pending || p.death_pending || p.rewards_queued() {
            return Ok(None);
        };
        p.pending = true;
        p.departure_snapshot = Some(p.character.clone());
        Ok(Some(p.character.clone()))
    }
    pub fn finish_departure(&mut self, id: i32) -> Result<()> {
        let p = self.players.get(&id).context("departing player missing")?;
        let snapshot = p
            .departure_snapshot
            .as_ref()
            .context("departure snapshot missing")?;
        ensure!(
            p.pending
                && p.departing
                && snapshot.id == p.character.id
                && snapshot.account == p.character.account
                && snapshot.revision == p.character.revision
                && snapshot.items == p.character.items,
            "departure reservation changed"
        );
        snapshot.lifetime.follows(&p.character.lifetime)?;
        self.players
            .get_mut(&id)
            .context("departing player missing")?
            .pending = false;
        self.leave(id)?;
        Ok(())
    }
    pub fn inspect_map(&self, player: Option<i32>) -> Result<serde_json::Value> {
        let account = match player {
            Some(id) => {
                self.players
                    .get(&id)
                    .context("map observer missing")?
                    .character
                    .account
            }
            None => -1,
        };
        let blocked: Vec<_> = (0..self.map.height)
            .flat_map(|y| (0..self.map.width).map(move |x| (x, y)))
            .filter(|(x, y)| {
                !self.walkable(Pos {
                    x: *x as f32 + 0.5,
                    y: *y as f32 + 0.5,
                })
            })
            .collect();
        let entities: Vec<_> = self
            .entities
            .values()
            .filter(|e| e.accessible(account))
            .collect();
        Ok(
            serde_json::json!({"width":self.map.width,"height":self.map.height,"blocked":blocked,"entities":entities}),
        )
    }
    pub fn observe(&self, id: i32) -> Result<serde_json::Value> {
        let p = self.players.get(&id).context("player missing")?;
        let entities: Vec<_> = self
            .entities_near(p.pos, 20.)
            .filter(|e| e.accessible(p.character.account))
            .collect();
        let others: Vec<_> = self
            .players
            .iter()
            .filter(|(i, q)| **i != id && p.pos.distance(q.pos) < 20.)
            .map(|(i, q)| serde_json::json!({"id":i,"pos":q.pos,"name":q.character.name}))
            .collect();
        Ok(
            serde_json::json!({"tick":self.tick,"time_ms":self.now,"player":id,"pos":p.pos,"character":p.character,"entities":entities,"players":others,"bullets":self.bullets.len(),"own_projectiles":self.bullets.iter().filter(|b| b.friendly && b.owner == id).map(|b| serde_json::json!({"id":b.id,"start":b.start,"pos":b.pos,"angle":b.angle,"born":b.born})).collect::<Vec<_>>(),"pending":p.pending,"oxygen":p.oxygen}),
        )
    }
    fn loot_bag(&mut self, pos: Pos, owner: Option<i32>, mut items: Vec<i32>) -> Result<i32> {
        ensure!(!items.is_empty(), "loot bag requires at least one item");
        ensure!(
            self.bag_expiry.len() < 128,
            "transient loot bag limit reached"
        );
        let mut appearance = 0;
        for item in &items {
            let object = self.content.get(*item as u16)?;
            appearance = appearance.max(object.bag_type);
            ensure!(
                owner.is_some() || !object.soulbound,
                "public loot cannot contain soulbound equipment"
            );
        }
        let kind = match appearance {
            0 => 0x500,
            1 => 0x503,
            2 => 0x507,
            3 => 0x508,
            4 => 0x509,
            _ => bail!("unsupported loot bag appearance"),
        };
        ensure!(
            items.len() <= 8,
            "loot exceeds bag capacity; split rewards before packaging"
        );
        items.resize(8, -1);
        let id = self.spawn_entity(kind, pos, items)?;
        let bag = self.entities.get_mut(&id).unwrap();
        bag.size = 80;
        if let Some(owner) = owner {
            bag.extra_stats.push((54, owner));
        }
        self.bag_expiry.insert(id, self.now + 60_000);
        tracing::info!(entity = id, owner, kind, "loot bag allocated");
        Ok(id)
    }
    fn loot_bags(&mut self, pos: Pos, owner: Option<i32>, items: &[i32]) -> Result<()> {
        for chunk in items.chunks(8) {
            let bag = self.loot_bag(pos, owner, chunk.to_vec())?;
            self.entities.get_mut(&bag).unwrap().drop_slots = (0..chunk.len()).collect();
        }
        Ok(())
    }
    fn god_loot_bags(&mut self, pos: Pos, loot: crate::loot::GodLoot) -> Result<()> {
        for (owner, items) in loot.private {
            let items: Vec<_> = items.into_iter().map(i32::from).collect();
            self.loot_bags(pos, Some(owner), &items)?;
        }
        if !loot.shared_owners.is_empty() {
            for chunk in loot.shared.chunks(8) {
                let bag =
                    self.loot_bag(pos, None, chunk.iter().copied().map(i32::from).collect())?;
                let entity = self.entities.get_mut(&bag).unwrap();
                // Native stat 54 is scalar; group visibility and collection are enforced server-side.
                entity.bag_owners = loot.shared_owners.clone();
                entity.drop_slots = (0..chunk.len()).collect();
                tracing::info!(entity = bag, owners = ?loot.shared_owners, "contributor-group loot allocated");
            }
        }
        Ok(())
    }
    fn defeat_entity(&mut self, id: i32) -> Result<()> {
        self.defeat_entity_with_killer(id, None)
    }
    fn defeat_entity_with_killer(&mut self, id: i32, killer: Option<i32>) -> Result<()> {
        let e = self.remove_entity(id);
        let definition = &self.content.objects[&e.kind];
        // Reference Reproduce suppresses follower XP; Pentaract Spawn explicitly allows eye XP.
        let reward = if e.kind == crate::dungeon::tomb::ANCHOR
            || e.script.as_ref().is_some_and(|script| script.no_experience)
            || (e.quest.is_some() && matches!(e.kind, 0xd57 | 0xd58 | 0xd5a..=0xd5c))
        {
            0
        } else {
            (definition.hp as f32 * definition.xp_multiplier / 10.).ceil() as i32
        };
        for (id, p) in &mut self.players {
            if p.character.alive && !p.death_pending && !p.departing && p.pos.distance(e.pos) < 25.
            {
                let cap = (crate::store::experience_goal(p.character.level)
                    .expect("validated player level") as f32
                    * if definition.quest { 0.5 } else { 0.1 }) as i32;
                let amount = reward.min(cap);
                if amount != 0 {
                    p.queue_experience(
                        amount,
                        killer,
                        e.damage
                            .get(&p.character.id)
                            .is_some_and(|damage| *damage > 0),
                    )?;
                    tracing::debug!(player = id, enemy = e.kind, amount, "experience queued");
                }
                let assigned = p.quest == Some(e.id)
                    || (crate::quest::final_root(e.kind)
                        && e.quest.is_some()
                        && p.qualified_quest == e.quest);
                if assigned
                    && p.character.hp > 0
                    && e.damage
                        .get(&p.character.id)
                        .is_some_and(|damage| *damage > 0)
                {
                    p.character.lifetime.qualified_quests = p
                        .character
                        .lifetime
                        .qualified_quests
                        .checked_add(1)
                        .context("qualified quest counter overflow")?;
                    p.qualified_quest = None;
                }
                if p.quest == Some(e.id) {
                    // This exact notification also starts the native client's quest-arrow completion delay.
                    let mut out = Writer::new();
                    out.i32(*id).utf("Quest Complete!").u32(0xff00ff00);
                    send(p, wire::NOTIFICATION, out.0);
                    tracing::debug!(player = id, enemy = e.id, "native quest completed");
                }
            }
        }
        if self.name == "Realm"
            && let Some((portal, chance)) = crate::dungeon::portal_drop(e.kind)
            && self.rng.gen_bool(chance)
        {
            self.portal_requests
                .push((portal, self.ocean_reward_position(&e)?));
            tracing::info!(
                enemy = e.kind,
                entity = id,
                portal,
                "defeat earned a dungeon portal"
            );
        }
        if let Err(error) = self.encounter_death(&e) {
            tracing::error!(enemy=e.kind, %error, "defeat mechanic failed");
        }
        if self.encounter
            && let Err(error) = self.drop_loot(&e)
        {
            tracing::error!(enemy=e.kind,%error,"earned encounter loot creation failed");
        }
        Ok(())
    }
    pub(super) fn finish_timed_defeat(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let kind = e.kind;
        let owner = self
            .players
            .iter()
            .filter(|(_, p)| p.character.alive && !p.death_pending && !p.departing)
            .filter_map(|(id, p)| {
                e.damage
                    .get(&p.character.id)
                    .filter(|n| **n > 0)
                    .map(|n| (*id, *n))
            })
            .max_by_key(|(_, n)| *n)
            .map(|(id, _)| id);
        // Self-destruction has no final player bullet, so the largest eligible contributor owns its kill statistic.
        if let Some(owner) = owner {
            let p = self.players.get_mut(&owner).unwrap();
            p.character.kills += 1;
            self.record_lifetime_kill(owner, kind, false)?;
            let p = self.players.get_mut(&owner).unwrap();
            if self.content.objects[&kind].quest {
                p.character.quests += 1;
            }
        }
        self.defeat_entity(id)?;
        tracing::info!(entity = id, kind, ?owner, "timed enemy defeat");
        Ok(())
    }
    fn drop_loot(&mut self, defeated: &Entity) -> Result<()> {
        if crate::endgame::HEROES.contains(&defeated.kind) {
            self.critical_deaths.push(defeated.id);
        } else if (crate::quest::final_root(defeated.kind)
            || matches!(defeated.kind, 0x678 | 0xd50 | 0xd54))
            && let Some(original) = defeated.quest
        {
            self.critical_deaths.push(original);
        }
        if let Some(index) = crate::dungeon::DUNGEONS
            .iter()
            .position(|(_, _, name, boss)| *name == self.name && *boss == defeated.kind)
        {
            let completion_pos = if defeated.kind == crate::dungeon::tomb::ANCHOR {
                defeated
                    .tomb
                    .as_ref()
                    .context("Tomb completion state missing")?
                    .completion_pos
                    .context("Tomb completion lacks a final boss death")?
            } else {
                defeated.pos
            };
            ensure!(!self.cleared, "dungeon clear already recorded");
            self.cleared = true;
            for (id, p) in self.players.iter_mut().filter(|(_, p)| {
                p.character.alive
                    && !p.death_pending
                    && !p.departing
                    && p.pos.distance(completion_pos) < 25.
                    && (index == 0 || defeated.damage.get(&p.character.id).is_some_and(|n| *n > 0))
            }) {
                p.clears_queued[index] = p.clears_queued[index]
                    .checked_add(1)
                    .context("queued dungeon clear overflow")?;
                tracing::info!(
                    player = id,
                    boss = defeated.id,
                    "dungeon participation clear queued"
                );
            }
            if self.clear_exit.is_none() {
                // Dreadstump leaves a Realm Portal: https://www.youtube.com/watch?v=JMZXtUNZ96s (Nov 2012, portal panel at his death taunt)
                let kind = if matches!(
                    defeated.kind,
                    0x927
                        | crate::dungeon::abyss::MALPHAS
                        | crate::dungeon::snake::BOSS
                        | crate::dungeon::jungle::BOSS
                        | crate::dungeon::beach::BOSS
                        | crate::dungeon::davy::BOSS
                        | crate::dungeon::tomb::ANCHOR
                ) {
                    0x704
                } else {
                    0x71d
                };
                let pos = if defeated.kind == crate::dungeon::manor::BOSS {
                    let sites: Vec<_> = self
                        .map
                        .cells
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.object == Some(crate::dungeon::manor::BOSS))
                        .map(|(i, _)| Pos {
                            x: (i % self.map.width as usize) as f32 + 0.5,
                            y: (i / self.map.width as usize) as f32 + 0.5,
                        })
                        .collect();
                    ensure!(
                        sites.len() == 1,
                        "Manor completion requires one authored arena marker"
                    );
                    sites[0]
                } else {
                    defeated.pos
                };
                let exit = self.spawn_exit(kind, pos)?;
                self.entities.get_mut(&exit).unwrap().published = false;
                self.clear_exit = Some(exit);
            }
            tracing::info!(
                boss = defeated.id,
                dungeon = self.name,
                "dungeon clear and exit staged for durable commit"
            );
        }
        if defeated.kind == crate::dungeon::spider::BOSS {
            return self.spider_loot(defeated);
        }
        if defeated.kind == crate::dungeon::lab::BOSS {
            return self.lab_loot(defeated);
        }
        if crate::dungeon::abyss::ROSTER.contains(&defeated.kind) {
            return self.abyss_loot(defeated);
        }
        // Rum is outside the comparator's private threshold; roll it once, not once per raider.
        if defeated.kind == 0xe37
            && self.players.values().any(|p| {
                p.character.alive
                    && !p.death_pending
                    && !p.departing
                    && defeated
                        .damage
                        .get(&p.character.id)
                        .is_some_and(|damage| *damage > 0)
            })
            && self.rng.gen_bool(0.2)
        {
            self.loot_bags(defeated.pos, None, &[0xc25])?;
        }
        let private = crate::loot::private(defeated.kind);
        // A flat 100-damage officer threshold exceeds the selected officers' entire HP.
        let threshold = if defeated.kind == crate::dungeon::trench::HERMIT {
            0
        } else if crate::dungeon::tomb::BOSSES.contains(&defeated.kind) {
            100
        } else if crate::loot::potion_damage(defeated.kind).is_some() {
            0
        } else if matches!(defeated.kind, 0xd56 | 0xd59) {
            self.content.objects[&defeated.kind].hp / 20
        } else if matches!(
            defeated.kind,
            0xd50 | 0xe37 | 0xd5a | 0xd60 | crate::dungeon::tomb::SPHINX | 0x1705
        ) || matches!(
            defeated.kind,
            crate::dungeon::tomb::TREASURE | crate::dungeon::tomb::SARCOPHAGUS
        ) {
            self.content.objects[&defeated.kind].hp / 100
        } else if defeated.kind == crate::dungeon::manor::BOSS {
            100
        } else if defeated.kind == crate::dungeon::manor::COFFIN {
            800
        } else if matches!(defeated.kind, 0x927 | crate::dungeon::davy::BOSS) {
            100
        } else {
            (self.content.objects[&defeated.kind].hp / 10).max(1)
        };
        let mut eligible: Vec<_> = self
            .players
            .values()
            .filter(|p| p.character.alive && !p.death_pending && !p.departing)
            .filter_map(|p| {
                defeated
                    .damage
                    .get(&p.character.id)
                    .filter(|damage| {
                        if defeated.kind == crate::dungeon::tomb::SPHINX {
                            **damage >= threshold
                        } else {
                            **damage > if private { threshold } else { 0 }
                        }
                    })
                    .map(|damage| (p.character.account, *damage))
            })
            .collect();
        eligible.sort_by_key(|(_, damage)| std::cmp::Reverse(*damage));
        if eligible.is_empty() {
            return Ok(());
        }
        if defeated.kind == crate::dungeon::beach::BUM {
            // The selected key is soulbound: preserve one winner without putting bound items in a public bag.
            return self.loot_bags(defeated.pos, Some(eligible[0].0), &[0xc2e]);
        }
        if crate::loot::potion_damage(defeated.kind).is_some() {
            let loot =
                crate::loot::god_loot(&self.content, defeated.kind, &eligible, &mut self.rng);
            return self.god_loot_bags(defeated.pos, loot);
        }
        let mut drops = BTreeMap::<i32, Vec<i32>>::new();
        // The older Tomb loot engine shares two rolls instead of guaranteeing Life to every raider.
        let attempts = if crate::dungeon::tomb::BOSSES.contains(&defeated.kind) {
            2
        } else if private {
            eligible.len()
        } else if crate::loot::period_dungeon(defeated.kind)
            || crate::loot::period_surface(defeated.kind)
            // realm-src Loots.cs Handle rolls public skeleton loot once per death, not per contributor.
            || matches!(defeated.kind, 0x65a..=0x65d)
        {
            1
        } else {
            // Reconstructed leader pools need their own attempt counts: https://github.com/trapped/rotmg_svr/tree/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db
            let multiplier = match defeated.kind {
                0x609 | 0x60d | 0x614 | 0x617 | 0x61a | 0x6c4 | 0x7f00 => 4,
                0x62c => 3,
                _ => 2,
            };
            (eligible.len() * multiplier).min(8)
        };
        for i in 0..attempts {
            let mut rewards = crate::loot::rewards(&self.content, defeated.kind, &mut self.rng);
            if defeated.kind == crate::dungeon::trench::THESSAL
                && i64::from(eligible[i % eligible.len()].1) * 100
                    >= i64::from(self.content.objects[&defeated.kind].hp) * 32
            {
                rewards.push(0xaea);
            }
            for item in rewards {
                if !crate::loot::item_qualified(
                    defeated.kind,
                    item,
                    eligible[i % eligible.len()].1,
                    self.content.objects[&defeated.kind].hp,
                ) {
                    continue;
                }
                let owner = if private {
                    eligible[i % eligible.len()].0
                } else {
                    -1
                };
                let items = drops.entry(owner).or_default();
                if !items.contains(&(item as i32))
                    || matches!(item, 0xa22 | 0xa23)
                    || crate::loot::period_boss(defeated.kind)
                {
                    items.push(item as i32);
                }
            }
        }
        let required = match defeated.kind {
            crate::dungeon::trench::HERMIT => Some([0xa34, 0xa4c]),
            // Sphinx's 1%-threshold potion rolls also have required counts: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db/BehaviorDb.Sphinx.cs
            crate::dungeon::tomb::SPHINX => Some([0xa34, 0xa35]),
            _ => None,
        };
        if let Some(required) = required {
            // NumRequired is an encounter-wide private floor, not a pair per raider: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/loot/Loots.cs
            for item in required {
                if !drops.values().any(|items| items.contains(&item)) {
                    let owner = eligible
                        .choose(&mut self.rng)
                        .expect("nonempty event contributors")
                        .0;
                    drops.entry(owner).or_default().push(item);
                }
            }
        }
        if !crate::loot::fauna_private_table(defeated.kind).is_empty() {
            for (owner, item) in crate::loot::fauna_private_rewards(
                &self.content,
                defeated.kind,
                &eligible,
                &mut self.rng,
            ) {
                drops.entry(owner).or_default().push(i32::from(item));
            }
        }
        for (owner, items) in drops {
            let pos = self.ocean_bag_position(defeated)?;
            self.loot_bags(pos, (owner >= 0).then_some(owner), &items)?;
        }
        Ok(())
    }
    fn move_entity_colliding(
        &mut self,
        id: i32,
        goal: Pos,
        distance: f32,
        slide: bool,
    ) -> Result<()> {
        let old = self.entities[&id].pos;
        let length = old.distance(goal);
        if length <= 0.001 {
            self.entities.get_mut(&id).unwrap().last_step = Pos { x: 0., y: 0. };
            return Ok(());
        }
        let distance = distance.min(length);
        let pos = Pos {
            x: old.x + (goal.x - old.x) / length * distance,
            y: old.y + (goal.y - old.y) / length * distance,
        };
        let samples = (distance / 0.1).ceil().max(1.) as usize;
        let step = Pos {
            x: (pos.x - old.x) / samples as f32,
            y: (pos.y - old.y) / samples as f32,
        };
        let kind = self.entities[&id].kind;
        let mut resolved = old;
        for i in 1..=samples {
            let next = if slide {
                Pos {
                    x: resolved.x + step.x,
                    y: resolved.y + step.y,
                }
            } else {
                Pos {
                    x: old.x + step.x * i as f32,
                    y: old.y + step.y * i as f32,
                }
            };
            let horizontal = Pos {
                x: next.x,
                y: resolved.y,
            };
            let vertical = Pos {
                x: resolved.x,
                y: next.y,
            };
            let clear = |pos| {
                (kind != crate::dungeon::trench::HERMIT && self.jungle_walkable(kind, pos))
                    || self.ocean_walkable(kind, pos)
            };
            let x_clear = clear(horizontal);
            let y_clear = clear(vertical);
            if clear(next) && (!slide || x_clear || y_clear) {
                resolved = next;
            } else if slide {
                // Wall contact removes the blocked component, not the entire movement step.
                resolved = if x_clear && (!y_clear || step.x.abs() >= step.y.abs()) {
                    horizontal
                } else if y_clear {
                    vertical
                } else {
                    break;
                };
            } else {
                bail!("NPC movement crosses a blocked tile");
            }
        }
        ensure!(
            distance == 0. || resolved.distance(old) > 0.,
            "NPC movement blocked on both axes"
        );
        if slide && resolved.distance(pos) > 0.001 {
            tracing::debug!(
                entity = id,
                kind,
                ?old,
                ?pos,
                ?resolved,
                "NPC movement projected along obstacle"
            );
        }
        self.position_entity(id, resolved)
    }
    fn position_entity(&mut self, id: i32, pos: Pos) -> Result<()> {
        ensure!(
            pos.x.is_finite() && pos.y.is_finite() && self.map.cell(pos.x, pos.y).is_some(),
            "entity position outside world"
        );
        let old = self.entities[&id].pos;
        let cell = |p: Pos| p.y as usize * self.map.width as usize + p.x as usize;
        assert!(
            self.cells.remove(cell(old), id),
            "moving NPC missing from spatial index"
        );
        self.cells
            .insert(cell(pos), id, self.actor_ids.contains(&id));
        let e = self.entities.get_mut(&id).unwrap();
        e.pos = pos;
        e.last_step = Pos {
            x: pos.x - old.x,
            y: pos.y - old.y,
        };
        Ok(())
    }
    // Follow owns its acquisition clock; encounter age must not restart a pursuit.
    // https://github.com/moistosaurus/realm-src/tree/1855c017eab870d13f0b61b1cf0b9e66a01922ff
    fn follow_goal(
        &mut self,
        id: i32,
        target: Option<Pos>,
        motion: crate::encounter::Motion,
        delta: u64,
    ) -> Option<Pos> {
        let crate::encounter::Motion::Follow {
            sight,
            stop,
            duration,
            cooldown,
            ..
        } = motion
        else {
            unreachable!("Follow goal requires Follow motion");
        };
        let pos = self.entities[&id].pos;
        let e = self.entities.get_mut(&id).unwrap();
        let old = e.following;
        let target = target.filter(|p| pos.distance(*p) < sight);
        match e.following {
            Following::Unknown => {
                if target.is_some() && e.follow_remaining == 0 {
                    e.following = Following::Acquired;
                    e.follow_remaining = duration;
                } else {
                    e.follow_remaining = e.follow_remaining.saturating_sub(delta);
                }
            }
            Following::Resting => match target {
                None => {
                    e.following = Following::Unknown;
                    e.follow_remaining = duration;
                }
                Some(p) if pos.distance(p) > stop + 1. => {
                    e.following = Following::Acquired;
                    e.follow_remaining = duration;
                }
                _ => {}
            },
            Following::Acquired => {}
        }
        if e.following == Following::Acquired {
            match target {
                None => {
                    e.following = Following::Unknown;
                    e.follow_remaining = 0;
                }
                Some(_) if duration > 0 && e.follow_remaining == 0 => {
                    e.following = Following::Unknown;
                    e.follow_remaining = cooldown;
                }
                Some(target) => {
                    e.follow_remaining = e.follow_remaining.saturating_sub(delta);
                    if pos.distance(target) <= stop {
                        e.following = Following::Resting;
                        e.follow_remaining = 0;
                    }
                }
            }
        }
        if old != e.following {
            e.goal = None;
        }
        (e.following == Following::Acquired)
            .then_some(target)
            .flatten()
    }
    fn npc_step(&mut self) -> Vec<(i32, Vec<u8>)> {
        use crate::encounter::{Motion, profile};
        let mut packets = Vec::new();
        if !self.encounter || self.safe {
            return packets;
        }
        self.beach_notice();
        let delta = self.now.saturating_sub(self.last_npc_step);
        self.last_npc_step = self.now;
        let audience: Vec<_> = self
            .players
            .values()
            .filter(|p| p.character.alive && !p.death_pending && !p.departing && !p.paused)
            .map(|p| p.pos)
            .chain(self.decoy_positions())
            .collect();
        let ids = self.npc_candidates(&audience);
        'entities: for id in ids {
            let Some(entity) = self.entities.get(&id) else {
                continue;
            };
            let kind = entity.kind;
            let _context = crate::supervision::npc(id, kind);
            let pos = entity.pos;
            if entity.hp == self.content.objects[&kind].hp
                && (kind == crate::dungeon::abyss::IDOL || kind == 0xd7e && entity.stage.is_none())
            {
                continue;
            }
            let profile = profile(kind).expect("validated encounter profile");
            // Self-destruction is a deadline, not target acquisition or a weapon cooldown.
            let shield_burst = self.shield_burst(id);
            let armed_abyss_missile = kind == 0x909 && entity.stage == Some(1);
            let armed_abyss_flamer = kind == 0x90b && Self::abyss_flamer_armed(entity);
            let burst = shield_burst
                || (matches!(kind, 0x91a | crate::dungeon::manor::MINE)
                    && entity.expires == Some(self.now + 50)
                    && self.now >= entity.next_shot);
            if !burst
                && !armed_abyss_flamer
                && entity.conditions(self.now) & crate::condition::bit(crate::condition::STASIS)
                    != 0
            {
                self.suspend_npc(id, delta);
                continue;
            }
            let nearest = self
                .players
                .values()
                .filter(|p| {
                    p.character.alive
                        && !p.death_pending
                        && !p.departing
                        && !p.paused
                        && (matches!(kind, 0x6d8 | crate::dungeon::manor::MINE)
                            || p.conditions(self.now)
                                & crate::condition::bit(crate::condition::INVISIBLE)
                                == 0)
                })
                .map(|p| {
                    (
                        kind == crate::dungeon::davy::BOSS
                            && entity.reaction_until > self.now
                            && pos.distance(p.pos) < 12.
                            && p.conditions(self.now)
                                & crate::condition::bit(crate::condition::PARALYZED)
                                != 0,
                        p.pos,
                    )
                })
                .chain(self.decoy_positions().map(|p| (false, p)))
                .min_by(|(ap, a), (bp, b)| {
                    bp.cmp(ap)
                        .then_with(|| pos.distance(*a).total_cmp(&pos.distance(*b)))
                })
                .map(|(_, p)| p);
            let wake = match kind {
                crate::dungeon::manor::KAGE
                | crate::dungeon::manor::BAT
                | crate::dungeon::manor::CHASER
                | crate::dungeon::manor::DECOY
                | 0x1727 => 20.,
                crate::dungeon::snake::GUARD | crate::dungeon::snake::DART => 25.,
                kind if crate::dungeon::tomb::contains(kind) => 30.,
                0x953 | 0x954 => 6.,
                0x619 => 8.,
                0x609 | 0xd06 | crate::dungeon::abyss::MALPHAS => 11.,
                0x617 => 8.,
                0x61a => 10.5,
                0x6d8 => 12.,
                0x91a => 7.,
                crate::dungeon::spider::BOSS | crate::dungeon::lab::BOSS => 12.,
                0xd90 | 0xd7e => 8.,
                0xd82 | 0xd83 => 10.,
                _ => 12.,
            };
            // Activated enemies keep the same sight radius; they do not keep
            // shooting once the player leaves that radius.
            let abyss = crate::dungeon::abyss::ROSTER.contains(&kind)
                || kind == 0x651 && self.name == "Abyss of Demons";
            let radius = if abyss
                && (entity.activated.is_some() || kind != crate::dungeon::abyss::MALPHAS)
            {
                20.
            } else {
                wake
            };
            let target = if burst && kind == crate::dungeon::manor::MINE {
                nearest.filter(|p| pos.distance(*p) < 60.)
            } else if burst {
                Some(pos)
            } else {
                nearest.filter(|p| pos.distance(*p) < radius)
            };
            // Visible mobs need simulation, not longer-range targeting: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/Collision.cs#L187-L215
            let simulation_radius =
                if self.content.objects[&kind].quest && entity.activated.is_none() {
                    radius
                } else {
                    radius.max(20.)
                };
            let nearby = self.players.values().any(|p| {
                p.character.alive
                    && !p.death_pending
                    && !p.departing
                    && !p.paused
                    && pos.distance(p.pos) < simulation_radius
            });
            let manor_clock =
                crate::dungeon::manor::ROSTER.contains(&kind) && entity.activated.is_some();
            if !burst
                && !armed_abyss_missile
                && !armed_abyss_flamer
                && target.is_none()
                && !nearby
                && !manor_clock
            {
                self.suspend_npc(id, delta);
                continue;
            }
            if let Err(error) = self.reproduce_god(id, delta) {
                tracing::error!(entity = id, kind, %error, "ordinary god reproduction refused");
                continue;
            }
            let activated = *self
                .entities
                .get_mut(&id)
                .unwrap()
                .activated
                .get_or_insert(self.now);
            let elapsed = self.now - activated;
            if let Err(error) = self.encounter_mechanics(id, target, elapsed) {
                tracing::error!(entity=id, kind, %error, "encounter mechanic refused");
                continue;
            }
            let Some(current) = self.entities.get(&id) else {
                continue;
            };
            let elapsed = self.now - current.activated.expect("activated encounter");
            let flags = current.conditions(self.now);
            let lair_escort = matches!(kind, 0xdaa | 0xdac);
            let queued_hero = kind == 0x668 || matches!(kind, 0x67e..=0x682);
            let mut hero_continue = false;
            let mut target = target;
            let (mut motion, mut speed) = if lair_escort {
                if let Err(error) = self.lair_escort_step(id, target, delta) {
                    self.entities.get_mut(&id).unwrap().last_step = Pos { x: 0., y: 0. };
                    tracing::error!(entity=id, %error, "Lair escort movement refused");
                    continue;
                }
                (Motion::Stationary, 0.)
            } else if crate::dungeon::lab::ROSTER.contains(&kind) {
                self.lab_movement(id)
            } else if crate::dungeon::abyss::ROSTER.contains(&kind) {
                match self.abyss_movement(id) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"Abyss movement refused");
                        continue;
                    }
                }
            } else if queued_hero {
                match self.hero_movement(id, target, delta) {
                    Ok((motion, speed, completed)) => {
                        hero_continue = completed;
                        (motion, speed)
                    }
                    Err(error) => {
                        tracing::error!(entity=id, %error, "Realm hero movement refused");
                        continue;
                    }
                }
            } else if crate::dungeon::davy::profile(kind).is_some() {
                self.locker_movement(id, elapsed)
            } else if crate::dungeon::jungle::ROSTER.contains(&kind) {
                crate::encounter::jungle::movement(
                    kind,
                    current.stage.expect("Jungle movement phase"),
                    current.parent.is_some(),
                )
            } else if crate::dungeon::manor::ROSTER.contains(&kind) {
                self.manor_movement(id, target, elapsed)
            } else if crate::dungeon::tomb::contains(kind) {
                match self.tomb_movement(id, target) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"Tomb movement refused");
                        continue;
                    }
                }
            } else if crate::dungeon::trench::ROSTER.contains(&kind) {
                self.ocean_movement(id, target, elapsed)
            } else if crate::encounter::landmarks::ROSTER.contains(&kind) {
                match self.landmark_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"landmark movement refused");
                        continue;
                    }
                }
            } else if current.quest.is_some() {
                self.quest_movement(id, target, elapsed)
            } else if crate::encounter::fauna::ROSTER.contains(&kind) {
                match self.fauna_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"fauna movement refused");
                        continue;
                    }
                }
            } else if kind == 0x6c6 || (self.entities[&id].camp.is_some() && midlands::member(kind))
            {
                match self.midland_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"inland family movement refused");
                        continue;
                    }
                }
            } else if gods::GODS.contains(&kind)
                || gods::CONSTRUCTS.contains(&kind)
                || (kind == 0x653 && self.entities[&id].camp.is_some())
            {
                match self.god_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"Godlands movement refused");
                        continue;
                    }
                }
            } else if golems::ROOTS.contains(&kind) || matches!(kind, 0x6d2..=0x6d7) {
                match self.golem_movement(id, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"golem movement refused");
                        continue;
                    }
                }
            } else if self.entities[&id].camp.is_some() {
                match self.family_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"family movement refused");
                        continue;
                    }
                }
            } else if crate::endgame::ROSTER.contains(&kind) {
                match self.finale_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"endgame movement refused");
                        continue;
                    }
                }
            } else {
                crate::encounter::movement(kind, self.entities[&id].hp, elapsed)
            };
            // Completion invokes at most one successor, after publishing the first motor step.
            // https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/Behavior.cs
            for invocation in 0..2 {
                if lair_escort {
                    break;
                }
                let pos = self.entities[&id].pos;
                if target.is_none()
                    && matches!(
                        motion,
                        Motion::Chase { .. }
                            | Motion::Maintain { .. }
                            | Motion::Retreat { .. }
                            | Motion::Orbit {
                                anchor: crate::encounter::Anchor::Player,
                                ..
                            }
                            | Motion::Protect {
                                anchor: crate::encounter::Anchor::Player,
                                ..
                            }
                    )
                {
                    motion = Motion::Wander;
                    speed = 4.;
                }
                let e = self.entities.get_mut(&id).unwrap();
                if e.motion != Some(motion) {
                    e.motion = Some(motion);
                    e.following = Following::Unknown;
                    e.follow_remaining = 0;
                    e.strict_initialized = false;
                    e.charge = None;
                    e.goal = None;
                    if matches!(motion, Motion::Pace { .. }) {
                        e.phase = 1.;
                    }
                    tracing::debug!(entity = id, kind, ?motion, speed, "NPC movement entered");
                }
                let planned_motion =
                    if flags & crate::condition::bit(crate::condition::PARALYZED) != 0 {
                        if let Some(charge) = &mut e.charge {
                            charge.suspend(delta);
                        }
                        Motion::Stationary
                    } else {
                        motion
                    };
                let goal = match planned_motion {
                    Motion::Wander if kind == crate::dungeon::abyss::MALPHAS => {
                        match self.abyss_malphas_goal(id, target) {
                            Ok(goal) => Some(goal),
                            Err(error) => {
                                tracing::error!(entity=id,%error,"Abyss movement goal refused");
                                continue;
                            }
                        }
                    }

                    Motion::Stationary => {
                        speed = 0.;
                        Some(pos)
                    }
                    Motion::Charge { .. } => self.charge_goal(id, target, motion, speed),
                    Motion::WanderHome { radius } => {
                        let e = self.entities.get_mut(&id).unwrap();
                        if e.goal.is_none_or(|p| {
                            p.distance(e.pos) < 0.1 || p.distance(e.home) > radius + 0.01
                        }) {
                            let angle = self.rng.gen_range(0. ..std::f32::consts::TAU);
                            let distance = self.rng.gen_range(0. ..radius);
                            e.goal = Some(Pos {
                                x: e.home.x + distance * angle.cos(),
                                y: e.home.y + distance * angle.sin(),
                            });
                        }
                        e.goal
                    }
                    Motion::Follow { idle, .. } => {
                        if let Some(target) = self.follow_goal(id, target, motion, delta) {
                            Some(target)
                        } else if idle == 0. {
                            speed = 0.;
                            Some(pos)
                        } else {
                            None
                        }
                    }
                    Motion::Pace { distance } => {
                        let e = self.entities.get_mut(&id).unwrap();
                        Some(match e.goal {
                            Some(goal) if pos.distance(goal) > 0.001 => goal,
                            _ => {
                                if e.goal.is_some() {
                                    e.phase = -e.phase;
                                }
                                Pos {
                                    x: pos.x + distance * e.phase,
                                    y: pos.y,
                                }
                            }
                        })
                    }
                    Motion::Tangential => {
                        let step = self.entities[&id].last_step;
                        let length = step.x.hypot(step.y);
                        if length == 0. {
                            speed = 0.;
                            Some(pos)
                        } else {
                            Some(Pos {
                                x: pos.x + step.x / length,
                                y: pos.y + step.y / length,
                            })
                        }
                    }
                    Motion::Maintain { sight, distance }
                        if target.is_some_and(|p| pos.distance(p) < sight) =>
                    {
                        let target = target.expect("acquired maintain-distance target");
                        let angle = (pos.y - target.y).atan2(pos.x - target.x);
                        Some(Pos {
                            x: target.x + distance * angle.cos(),
                            y: target.y + distance * angle.sin(),
                        })
                    }
                    Motion::Retreat { distance } => {
                        let target = target.expect("target-dependent retreat guarded");
                        if pos.distance(target) >= distance
                            || matches!(kind, 0x67e..=0x682)
                                && pos.x == target.x
                                && pos.y == target.y
                        {
                            speed = 0.;
                        }
                        let angle = (pos.y - target.y).atan2(pos.x - target.x);
                        // Retracting gates at radius 8 without clamping the final outward step.
                        let distance = if matches!(kind, 0x67e..=0x682) {
                            pos.distance(target) + 1.
                        } else {
                            distance
                        };
                        Some(Pos {
                            x: target.x + distance * angle.cos(),
                            y: target.y + distance * angle.sin(),
                        })
                    }
                    Motion::Protect { anchor, distance } => {
                        let leader = match self.orbit_anchor(id, anchor, target) {
                            Ok(leader) => leader,
                            Err(error) => {
                                tracing::error!(entity=id, %error, "protection anchor refused");
                                continue 'entities;
                            }
                        };
                        if pos.distance(leader) <= distance {
                            speed = 0.;
                        }
                        Some(leader)
                    }
                    Motion::Chase { sight, stop }
                        if target.is_some_and(|p| pos.distance(p) < sight) =>
                    {
                        let target = target.expect("acquired chase target");
                        if pos.distance(target) <= stop {
                            speed = 0.;
                        }
                        // Per-tick aim jitter keeps chasers from pinning on one wall point: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/movement/Chasing.cs
                        Some(Pos {
                            x: target.x + self.rng.gen_range(-2..2) as f32 / 2.,
                            y: target.y + self.rng.gen_range(-2..2) as f32 / 2.,
                        })
                    }
                    Motion::Orbit { anchor, radius } | Motion::StrictOrbit { anchor, radius } => {
                        let target = match self.orbit_anchor(id, anchor, target) {
                            Ok(target) => target,
                            Err(error) => {
                                tracing::error!(entity=id, %error, "orbit anchor refused");
                                continue 'entities;
                            }
                        };
                        let e = self.entities.get_mut(&id).unwrap();
                        let shield_formation = matches!(kind, 0x900 | 0x902)
                            && matches!(motion, Motion::StrictOrbit { .. });
                        if shield_formation && !e.strict_initialized {
                            e.phase = self.rng.gen_range(0. ..std::f32::consts::TAU);
                            e.strict_initialized = true;
                            speed = 0.;
                            Some(pos)
                        } else {
                            let direction = if let Some(orbit) = e.orbit {
                                if pos.distance(target) > 0.001 {
                                    e.phase = (pos.y - target.y).atan2(pos.x - target.x);
                                }
                                orbit.direction
                            } else {
                                if !shield_formation && e.goal.is_none() {
                                    e.phase = if matches!(motion, Motion::StrictOrbit { .. }) {
                                        self.rng.gen_range(0. ..std::f32::consts::TAU)
                                    } else {
                                        (pos.y - target.y).atan2(pos.x - target.x)
                                    };
                                }
                                1.
                            };
                            // This older shield formation clock ignores the slowed motor.
                            let angular_speed = if shield_formation {
                                speed
                            } else if e.quest.is_some()
                                || e.orbit.is_some()
                                || matches!(motion, Motion::StrictOrbit { .. })
                            {
                                (if e.quest.is_some() || e.orbit.is_some() {
                                    speed / 1.5
                                } else {
                                    speed
                                }) * if flags & crate::condition::bit(crate::condition::SLOWED) != 0
                                {
                                    0.5
                                } else {
                                    1.
                                }
                            } else {
                                speed
                            };
                            if flags & crate::condition::bit(crate::condition::PARALYZED) == 0 {
                                e.phase = (e.phase + direction * angular_speed / radius * 0.05)
                                    % std::f32::consts::TAU;
                            }
                            Some(Pos {
                                x: target.x + e.phase.cos() * radius,
                                y: target.y + e.phase.sin() * radius,
                            })
                        }
                    }
                    Motion::Circle => {
                        let boss = self
                            .actors()
                            .find(|e| e.kind == 0x927 && pos.distance(e.pos) < 10.)
                            .map(|e| e.pos);
                        boss.map(|boss| {
                            let e = self.entities.get_mut(&id).unwrap();
                            if e.goal.is_none() {
                                e.phase = self.rng.gen_range(0. ..std::f32::consts::TAU);
                            }
                            e.phase = (e.phase + profile.speed / 3. * 0.05) % std::f32::consts::TAU;
                            Pos {
                                x: boss.x + e.phase.cos() * 3.,
                                y: boss.y + e.phase.sin() * 3.,
                            }
                        })
                    }
                    _ => None,
                };
                let goal = if kind == 0xd06 && pos.distance(self.entities[&id].home) > 7. {
                    speed = 7.8;
                    Some(
                        self.orbit_anchor(id, crate::encounter::Anchor::Home, target)
                            .expect("live boss home"),
                    )
                } else {
                    goal
                };
                let goal = if let Some(goal) = goal {
                    goal
                } else {
                    let e = self.entities.get_mut(&id).unwrap();
                    speed = match motion {
                        Motion::Wander | Motion::Maintain { .. } => speed,
                        Motion::Follow { idle, .. } | Motion::Charge { idle, .. } => idle,
                        Motion::Chase { .. } if kind == 0x600 => 4.,
                        Motion::Chase { .. } if !(0x687..=0x689).contains(&kind) => 4.,
                        Motion::Protect { .. } => profile.speed,
                        _ => 2.,
                    };
                    let realm = self.name == "Realm";
                    if e.goal.is_none_or(|goal| {
                        goal.distance(e.pos) < 0.1
                            || if realm {
                                goal.distance(e.pos) > 1.01
                            } else {
                                goal.distance(e.home) > 2.01
                            }
                    }) {
                        let angle = self.rng.gen_range(0. ..std::f32::consts::TAU);
                        let (center, radius) = if realm {
                            (e.pos, 1.)
                        } else {
                            (e.home, self.rng.gen_range(0. ..2.))
                        };
                        e.goal = Some(if realm && e.pos.distance(e.home) > 24. {
                            e.home
                        } else {
                            Pos {
                                x: center.x + angle.cos() * radius,
                                y: center.y + angle.sin() * radius,
                            }
                        });
                    }
                    e.goal.expect("wander destination assigned")
                };
                if flags & crate::condition::bit(crate::condition::PARALYZED) != 0 {
                    speed = 0.;
                } else if flags & crate::condition::bit(crate::condition::SLOWED) != 0 {
                    speed *= 0.5;
                }
                if flags & crate::condition::bit(crate::condition::PARALYZED) == 0 {
                    self.entities.get_mut(&id).unwrap().goal = Some(goal);
                }
                let distance = if matches!(motion, Motion::StrictOrbit { .. }) && speed > 0. {
                    pos.distance(goal)
                } else {
                    speed / 1.5 * 0.05
                };
                if let Err(error) = self.move_entity_colliding(
                    id,
                    goal,
                    distance,
                    !matches!(motion, Motion::Charge { .. } | Motion::StrictOrbit { .. }),
                ) {
                    let e = self.entities.get_mut(&id).unwrap();
                    e.goal = None;
                    e.last_step = Pos { x: 0., y: 0. };
                    tracing::debug!(entity=id,%error,"NPC movement blocked; retargeting");
                }
                if !hero_continue || invocation == 1 {
                    break;
                }
                let pos = self.entities[&id].pos;
                target = self
                    .players
                    .values()
                    .filter(|p| {
                        p.character.alive
                            && !p.death_pending
                            && !p.departing
                            && !p.paused
                            && p.conditions(self.now)
                                & crate::condition::bit(crate::condition::INVISIBLE)
                                == 0
                    })
                    .map(|p| p.pos)
                    .chain(self.decoy_positions())
                    .filter(|p| pos.distance(*p) < 20.)
                    .min_by(|a, b| pos.distance(*a).total_cmp(&pos.distance(*b)));
                match self.hero_movement(id, target, delta) {
                    Ok((next_motion, next_speed, completed)) => {
                        motion = next_motion;
                        speed = next_speed;
                        hero_continue = completed;
                    }
                    Err(error) => {
                        tracing::error!(entity=id, %error, "Realm hero continuation refused");
                        break;
                    }
                }
            }
            if queued_hero {
                let e = self.entities.get_mut(&id).unwrap();
                e.last_step = Pos {
                    x: e.pos.x - pos.x,
                    y: e.pos.y - pos.y,
                };
            }
            let e = &self.entities[&id];
            let schedule = if burst && kind != crate::dungeon::manor::MINE {
                None
            } else {
                match self.npc_attack_schedule(id, elapsed) {
                    Ok(schedule) => schedule,
                    Err(error) => {
                        tracing::error!(entity=id,kind,%error,"NPC attack schedule refused");
                        continue;
                    }
                }
            };
            if profile.interval == 0
                || e.activated.is_none()
                || (!burst
                    && ((schedule.is_none()
                        && (self.now < e.next_shot
                            || target.is_none_or(|p| e.pos.distance(p) > profile.range)))
                        || flags & crate::condition::bit(crate::condition::STUNNED) != 0))
            {
                continue;
            }
            let result = if let Some(schedule) = &schedule {
                self.scheduled_attacks(id, target, schedule)
            } else {
                let Some(target) = target else { continue };
                self.legacy_enemy_shots(id, target, elapsed)
            };
            let shots = match result {
                Ok(shots) => shots,
                Err(error) => {
                    tracing::error!(entity=id, kind, %error, "NPC projectiles refused");
                    continue;
                }
            };
            let shots = if schedule.is_none()
                && flags & crate::condition::bit(crate::condition::DAZED) != 0
            {
                enemy_shots::dazed(shots)
            } else {
                shots
            };
            let interval = match kind {
                0x633 if elapsed % 10000 >= 5000 => 500,
                0xd06 if elapsed % 43300 >= 32300 => 1300,
                0xd06 if elapsed % 43300 >= 12300 => 500,
                0xd90 if (elapsed / 13000) % 4 == 1 => 2000,
                0xd90 if (elapsed / 13000) % 4 >= 2 => 1000,
                _ => profile.interval,
            };
            if schedule.is_none() {
                self.entities.get_mut(&id).unwrap().next_shot = self.now + interval;
            }
            if shield_burst {
                self.entities.get_mut(&id).unwrap().stage = Some(3);
                tracing::debug!(entity = id, kind, "Shield Orc final command selected");
            }
            if self.bullets.len() + shots.len() > 2048 {
                tracing::warn!(
                    entity = id,
                    active = self.bullets.len(),
                    "NPC shot refused at global projectile limit"
                );
                continue;
            }
            packets.extend(
                self.emit_enemy_shots(id, shots)
                    .into_iter()
                    .map(|body| (id, body)),
            );
        }
        packets
    }
    pub(super) fn emit_enemy_shots(&mut self, id: i32, shots: Vec<(u8, f32)>) -> Vec<Vec<u8>> {
        let kind = self.entities[&id].kind;
        let mut notices = Vec::with_capacity(shots.len());
        for (index, angle) in shots {
            let spec = self.content.objects[&kind]
                .projectiles
                .get(&index)
                .expect("selected encounter projectile missing")
                .clone();
            let e = self.entities.get_mut(&id).unwrap();
            e.bullet = e.bullet.wrapping_add(1);
            let bullet = Bullet {
                owner: id,
                source_kind: kind,
                id: e.bullet,
                friendly: false,
                start: e.pos,
                pos: e.pos,
                angle,
                born: self.now,
                damage: if spec.min == spec.max {
                    spec.min
                } else {
                    self.rng.gen_range(spec.min..spec.max)
                },
                spec,
                hit: BTreeSet::new(),
                hit_enemy: false,
            };
            notices.push(enemy_shots::EnemyShot {
                bullet: bullet.id,
                projectile: index,
                start: bullet.start,
                angle,
                damage: bullet.damage,
            });
            if kind == 0x927 {
                tracing::info!(
                    boss = id,
                    projectile = index,
                    time_ms = self.now,
                    "Dreadstump projectile emitted"
                );
            }
            self.bullets.push(bullet);
        }
        // Shoot then retire, independent of the proximity poll; keep the owner until publication: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db/BehaviorDb.UndeadLair.cs#L462-L480
        if matches!(kind, 0xda2 | 0xda3) && !notices.is_empty() {
            self.entities.get_mut(&id).unwrap().expires.set(
                Some(self.now + 50),
                id,
                &mut self.timed_ids,
            );
            tracing::debug!(
                entity = id,
                kind,
                shots = notices.len(),
                "Lair trap detonated"
            );
        }
        enemy_shots::packets(id, &notices)
    }
    pub fn step(&mut self) {
        let started = crate::performance::Measure::start();
        let players_started = crate::performance::Measure::start();
        self.now += 50;
        self.admissions.retain(|_, a| a.expires > self.now);
        self.tick += 1;
        self.lab_step();
        self.expire_consumable_boosts();
        self.companion_step()
            .expect("validated companion simulation");
        self.trade_step();
        self.fountain_heal();
        self.expire_goto_acks();
        self.expire_native_projectiles();
        let ids: Vec<_> = self.players.keys().copied().collect();
        for id in &ids {
            let p = &self.players[id];
            if !p.active() {
                continue;
            }
            if p.paused {
                // Script controllers still own their MOVE clock while gameplay is paused.
                if p.controller != Controller::Human {
                    let pos = p.pos;
                    let time = self.now as i32;
                    if let Err(e) = self.move_to(*id, pos, time) {
                        tracing::error!(player=id,error=%e,"script idle MOVE rejected");
                    }
                }
                continue;
            }
            let p = self.players.get_mut(id).unwrap();
            let values = p
                .effective(&self.content, self.now)
                .expect("validated player equipment and base stats");
            let flags = p.conditions(self.now);
            p.statuses
                .retain(|_, (_, deadline)| deadline.saturating_add(1000) > self.now);
            let regeneration = if flags & crate::condition::bit(crate::condition::BLEEDING) != 0 {
                0.
            } else {
                1. + values[6] as f32 / 8.
            } + if flags & crate::condition::bit(crate::condition::HEALING) != 0
            {
                20.
            } else {
                0.
            };
            if p.character.hp < values[0]
                && regeneration > 0.
                && flags & crate::condition::bit(crate::condition::SICK) == 0
            {
                p.hp_fraction += regeneration * 0.05;
                let n = p.hp_fraction as i32;
                p.character.hp = (p.character.hp + n).min(values[0]);
                p.hp_fraction -= n as f32;
            } else {
                p.hp_fraction = 0.;
            }
            if flags & crate::condition::bit(crate::condition::BLEEDING) != 0 {
                p.character.hp = (p.character.hp - 1).max(1);
            }
            if flags & crate::condition::bit(crate::condition::QUIET) != 0 {
                p.character.mp = 0;
            }
            if p.character.mp < values[1]
                && flags & crate::condition::bit(crate::condition::QUIET) == 0
            {
                p.mp_fraction += (0.6 + values[7] as f32 / 16.7) * 0.05;
                let n = p.mp_fraction as i32;
                p.character.mp = (p.character.mp + n).min(values[1]);
                p.mp_fraction -= n as f32;
            } else {
                p.mp_fraction = 0.;
            }
            let goal = p.bot_goal;
            let aim = p.bot_aim;
            if let Some(goal) = goal {
                let d = p.pos.distance(goal);
                if d > 0.02 {
                    let ground = self
                        .map
                        .cell(p.pos.x, p.pos.y)
                        .expect("player inside map")
                        .ground;
                    let step = p
                        .speed(&self.content, self.now)
                        .expect("valid player equipment")
                        * self.content.grounds[&ground]
                        * 50.;
                    let n = d.min(step);
                    let pos = Pos {
                        x: p.pos.x + (goal.x - p.pos.x) / d * n,
                        y: p.pos.y + (goal.y - p.pos.y) / d * n,
                    };
                    let time = self.now as i32;
                    if let Err(e) = self.move_to(*id, pos, time) {
                        tracing::debug!(player=id,error=%e,"bot movement blocked");
                    }
                }
            }
            if self.players[id].controller != Controller::Human {
                let pos = self.players[id].pos;
                let time = self.now as i32;
                if let Err(e) = self.move_to(*id, pos, time) {
                    tracing::error!(player=id,error=%e,"script idle MOVE rejected");
                }
            }
            if let Some(angle) = aim {
                let p = &self.players[id];
                let time = self.now as i32;
                let kind = p.character.items[0] as u16;
                let weapon = &self.content.objects[&kind];
                let shots = weapon.shots;
                let arc = weapon.arc.to_radians();
                let cooldown =
                    reconciliation::attack_cooldown(p, &self.content, self.now, weapon.rate)
                        .expect("valid player equipment");
                // A held trigger fires when the weapon is ready instead of retrying every tick.
                if time != p.last_attack
                    && i64::from(time) - i64::from(p.last_attack) < cooldown as i64 - 2
                {
                    continue;
                }
                let first = p.next_bullet;
                let pos = p.pos;
                let start = Pos {
                    x: pos.x + 0.3 * angle.cos(),
                    y: pos.y + 0.3 * angle.sin(),
                };
                for i in 0..shots {
                    let angle = angle + arc * (i as f32 - (shots - 1) as f32 / 2.);
                    if self
                        .shoot(*id, time, (first + i as u8) % 128, kind, start, angle)
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        self.performance
            .borrow_mut()
            .work("players", players_started);
        let population_started = crate::performance::Measure::start();
        if let Err(error) = self.repopulate() {
            tracing::error!(%error,"Realm population replenishment failed");
        }
        self.performance
            .borrow_mut()
            .work("population", population_started);
        let transient_started = crate::performance::Measure::start();
        let expired: Vec<_> = self
            .bag_expiry
            .iter()
            .filter(|(id, deadline)| {
                let bag = &self.entities[id];
                !bag.reserved
                    && (**deadline <= self.now
                        || (self.content.objects[&bag.kind].class == "Container"
                            && bag.items.iter().all(|item| *item == -1)))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.remove_entity(id);
            tracing::debug!(entity = id, "expired transient or empty container removed");
        }
        self.performance
            .borrow_mut()
            .work("transients", transient_started);
        let mechanics_started = crate::performance::Measure::start();
        self.mechanic_expiry();
        self.abyss_step();
        if let Err(error) = self.locker_step() {
            tracing::error!(%error, "Davy Locker progression refused");
        }
        self.ground_step();
        self.ocean_step();
        self.event_step();
        self.ability_step();
        self.performance
            .borrow_mut()
            .work("mechanics", mechanics_started);
        let npc_started = crate::performance::Measure::start();
        let shots = self.npc_step();
        self.performance.borrow_mut().work("npc", npc_started);
        let combat_started = crate::performance::Measure::start();
        self.grenade_step();
        for id in &ids {
            if let Err(e) = self.updates(*id, false) {
                tracing::error!(player=id,error=%e,"world update failed");
            }
        }
        for (owner, shot) in shots {
            self.send_enemy_shot(owner, shot);
        }
        let guns: Vec<_> = self
            .gun_ids
            .iter()
            .filter(|id| self.now >= self.entities[id].next_shot)
            .copied()
            .collect();
        for id in guns {
            let e = self.entities.get_mut(&id).unwrap();
            e.next_shot = self.now + self.rng.gen_range(2000..3500);
            e.bullet = e.bullet.wrapping_add(1);
            let angle = match e.kind {
                0x0802 => std::f32::consts::FRAC_PI_2,
                0x0804 => 0.,
                0x0805 => std::f32::consts::PI,
                _ => 0.,
            };
            let spec = self.content.objects[&e.kind]
                .projectiles
                .get(&0)
                .cloned()
                .expect("validated tutorial gun projectile zero");
            let b = Bullet {
                owner: id,
                source_kind: e.kind,
                id: e.bullet,
                friendly: false,
                start: e.pos,
                pos: e.pos,
                angle,
                born: self.now,
                damage: spec.min,
                spec,
                hit: BTreeSet::new(),
                hit_enemy: false,
            };
            let mut o = Writer::new();
            o.u8(b.id)
                .i32(id)
                .u8(0)
                .f32(b.start.x)
                .f32(b.start.y)
                .f32(angle)
                .i16(b.damage as i16);
            self.bullets.push(b);
            self.send_enemy_shot(id, o.0);
        }
        let mut bullets = std::mem::take(&mut self.bullets);
        self.detached_bullets = bullets.len();
        for b in &mut bullets {
            for sample in 1..=5 {
                if b.hit.contains(&-1) || (!b.spec.multi_hit && !b.hit.is_empty()) {
                    break;
                }
                let old = b.pos;
                b.pos = b.position(
                    (self.now - 50 + sample * 10)
                        .max(b.born)
                        .min(b.born + b.spec.lifetime as u64),
                );
                if self
                    .map
                    .cell(b.pos.x, b.pos.y)
                    .is_none_or(|c| c.ground == 255)
                    || self.entities_at(b.pos).any(|e| {
                        let o = &self.content.objects[&e.kind];
                        o.static_object
                            && (!o.enemy || !b.friendly)
                            && (o.enemy_occupy_square || o.occupy_square && !b.spec.passes_cover)
                    })
                {
                    b.hit.insert(-1);
                    break;
                }
                if !b.friendly
                    && !b.spec.multi_hit
                    && self.decoy_positions().any(|pos| inside_box(pos, b.pos))
                {
                    b.hit.insert(-1);
                    break;
                }
                if b.friendly {
                    let targets: Vec<_> = self
                        .near_entities(old, b.pos)
                        .filter(|e| {
                            e.hp > 0
                                && e.conditions(self.now)
                                    & (crate::condition::bit(crate::condition::STASIS)
                                        | crate::condition::bit(crate::condition::INVINCIBLE))
                                    == 0
                                && self.content.objects[&e.kind].destructible
                                && !b.hit.contains(&e.id)
                                && inside_box(e.pos, b.pos)
                        })
                        .map(|e| e.id)
                        .collect();
                    if let Some(target) = targets.iter().min_by(|a, b_id| {
                        self.entities[a]
                            .pos
                            .distance(b.pos)
                            .total_cmp(&self.entities[b_id].pos.distance(b.pos))
                    }) {
                        if self.players.get(&b.owner).is_some_and(|p| p.paused) {
                            b.hit.insert(*target);
                            break;
                        }
                        if !b.hit_enemy && self.content.objects[&self.entities[target].kind].enemy {
                            b.hit_enemy = true;
                            if let Some(p) = self
                                .players
                                .get_mut(&b.owner)
                                .filter(|p| p.character.alive && !p.death_started && !p.departing)
                            {
                                p.character.lifetime.projectile_hits = p
                                    .character
                                    .lifetime
                                    .projectile_hits
                                    .checked_add(1)
                                    .expect("lifetime projectile hit overflow");
                            }
                        }
                        self.damage_enemy(
                            b.owner,
                            *target,
                            b.damage,
                            b.spec.armor_piercing,
                            &b.spec.effects,
                            Some(b.id),
                        )
                        .expect("validated enemy damage");
                        b.hit.insert(*target);
                    }
                } else if !self.safe {
                    for id in &ids {
                        let p = &self.players[id];
                        if p.character.alive
                            && !p.paused
                            && !p.departing
                            && !p.death_pending
                            && p.controller != Controller::Human
                            && p.conditions(self.now)
                                & (crate::condition::bit(crate::condition::STASIS)
                                    | crate::condition::bit(crate::condition::INVINCIBLE))
                                == 0
                            && !b.hit.contains(id)
                            && inside_box(p.pos, b.pos)
                        {
                            self.damage_player(*id, b, true)
                                .expect("validated projectile damage");
                            b.hit.insert(*id);
                            if !b.spec.multi_hit {
                                break;
                            }
                        }
                    }
                }
            }
        }
        bullets.retain(|b| {
            self.now - b.born < b.spec.lifetime as u64
                && !b.hit.contains(&-1)
                && (b.spec.multi_hit || b.hit.is_empty())
        });
        bullets.append(&mut self.bullets);
        self.bullets = bullets;
        self.detached_bullets = 0;
        self.reconcile_enemy_deaths();
        self.performance
            .borrow_mut()
            .work("combat_and_updates", combat_started);
        let packets_started = crate::performance::Measure::start();
        if self.tick.is_multiple_of(2) {
            // The client moves each object (target - shown) / tickTime with no clamp: a late tick advertised as 100ms
            // overshoots, and a catch-up tick shorter than the 100ms cadence would fling objects across the screen.
            let now = self.clock.now();
            let tick_ms = i32::try_from(now.duration_since(self.new_tick_at).as_millis())
                .expect("NEW_TICK interval exceeds i32 milliseconds")
                .max(100);
            self.new_tick_at = now;
            for id in ids {
                if let Err(e) = self.updates(id, false) {
                    tracing::error!(player=id,error=%e,"visibility update failed");
                    continue;
                }
                let p = &self.players[&id];
                let dynamic: Vec<_> = p
                    .known
                    .iter()
                    .filter(|entity| {
                        self.players.contains_key(entity)
                            || self.companions.contains(entity)
                            || self.entities.get(entity).is_some_and(|e| {
                                e.hp > 0
                                    || !e.items.is_empty()
                                    || self.content.objects[&e.kind].class == "Portal"
                            })
                    })
                    .copied()
                    .collect();
                let mut o = Writer::new();
                o.i32(p.tick).i32(tick_ms).i16(dynamic.len() as i16);
                for entity in dynamic {
                    let status = self.stats(entity, id).expect("visible entity must exist");
                    o.0.extend(
                        self.players
                            .get_mut(&id)
                            .unwrap()
                            .tick_stats
                            .encode(entity, status),
                    );
                }
                send(&self.players[&id], wire::NEW_TICK, o.0);
                let p = self.players.get_mut(&id).unwrap();
                let dex = p
                    .effective(&self.content, self.now)
                    .expect("visible player stats must be valid")[5];
                p.remember_weapon_tick(dex);
                p.tick += 1;
            }
        }
        self.performance
            .borrow_mut()
            .work("tick_packets", packets_started);
        self.performance.borrow_mut().work("step", started);
    }
    fn report_performance(&self, world_id: i32) {
        if let Some(timings) = self.performance.borrow_mut().report() {
            tracing::info!(world_id,world=%self.name, entities=self.entities.len(), actors=self.actor_ids.len(),
                bullets=self.bullets.len(),compact_entities=self.entities.shared(),scenery_promotions=self.entities.promotions,spatial_cells=self.cells.len(),known_entities=self.players.values().map(|p|p.known.len()).sum::<usize>(),
                pending_shoot_acks=self.players.values().map(|p|p.shoot_acks.len()).sum::<usize>(),
                weapon_tick_records=self.players.values().map(|p|p.weapon_ticks.len()).sum::<usize>(),
                humans=self.players.values().filter(|p| p.controller==Controller::Human).count(),
                bots=self.players.values().filter(|p| p.controller==Controller::Autonomous).count(),
                qa_actors=self.players.values().filter(|p| p.controller==Controller::Qa).count(),
                discovered_tiles=self.players.values().map(|p|p.tiles.len()).sum::<usize>(),
                cached_view_tiles=self.players.values().map(|p|p.view_tiles.borrow().tiles.len()).sum::<usize>(),
                cached_view_payload_bytes=self.players.values().map(|p|p.view_tiles.borrow().tiles.allocated_bytes()).sum::<usize>(),
                tick_stats_elided=self.players.values().map(|p|p.tick_stats.elided).sum::<u64>(),
                terrain_rebuilds=self.players.values().map(|p|p.view_tiles.borrow().rebuilds).sum::<u64>(),
                incremental_terrain_tiles=self.players.values().map(|p|p.view_tiles.borrow().incremental_tiles).sum::<u64>(),
                cloned_snapshot_tiles=self.players.values().map(|p|p.view_tiles.borrow().cloned_snapshot_tiles).sum::<u64>(),
                timings=%timings,"world performance");
        }
    }
}
fn inside_box(a: Pos, b: Pos) -> bool {
    (a.x - b.x).abs() <= 0.5 && (a.y - b.y).abs() <= 0.5
}

#[cfg(test)]
mod chase_tests;
#[cfg(test)]
mod death_chat_tests;
#[cfg(test)]
mod dungeon_return_tests;
#[cfg(test)]
mod hermit_leash_tests;
#[cfg(test)]
mod hermit_loot_tests;
#[cfg(test)]
mod instance_lifetime_tests;
#[cfg(test)]
mod period_loot_tests;
#[cfg(test)]
mod pirate_generation_tests;
#[cfg(test)]
mod reward_accuracy_tests;
#[cfg(test)]
mod spectator_tests;
#[cfg(test)]
mod sphinx_loot_tests;
#[cfg(test)]
mod sphinx_site_tests;
#[cfg(test)]
mod weapon_tests;
#[cfg(test)]
mod world_effect_tests;

#[cfg(test)]
impl World {
    pub(crate) fn test_paused_script_heartbeats(&mut self, id: i32) -> Result<()> {
        let p = self.players.get_mut(&id).unwrap();
        assert!(p.active() && p.paused && p.controller != Controller::Human);
        let position = p.pos;
        let credit = p.movement_credit;
        let character = serde_json::to_value(&p.character)?;
        let fractions = (p.hp_fraction, p.mp_fraction);
        // Nonzero residues detect accidentally entering even the full-HP/MP regeneration branch.
        p.hp_fraction = 0.25;
        p.mp_fraction = 0.5;
        p.bot_goal = Some(Pos {
            x: position.x + 1.,
            y: position.y,
        });
        p.bot_aim = Some(0.);
        let goal = p.bot_goal;
        let next_bullet = p.next_bullet;
        let mut moves = Vec::new();
        for _ in 0..40 {
            self.step();
            let p = &self.players[&id];
            moves.push((p.last_move, self.now as i32));
            assert_eq!((p.pos.x, p.pos.y), (position.x, position.y));
            assert_eq!(p.movement_credit, credit);
            assert_eq!(serde_json::to_value(&p.character)?, character);
            assert_eq!((p.hp_fraction, p.mp_fraction), (0.25, 0.5));
            assert_eq!(p.bot_goal.map(|p| (p.x, p.y)), goal.map(|p| (p.x, p.y)));
            assert_eq!(p.bot_aim, Some(0.));
            assert_eq!(p.next_bullet, next_bullet);
            assert!(!self.bullets.iter().any(|b| b.owner == id));
        }
        // Exercise the real guard before checking the recorded heartbeat times.
        self.move_to(id, position, self.now as i32)?;
        assert!(moves.iter().all(|(actual, expected)| actual == expected));
        let p = self.players.get_mut(&id).unwrap();
        p.hp_fraction = fractions.0;
        p.mp_fraction = fractions.1;
        p.bot_goal = None;
        p.bot_aim = None;
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn native_map_effects_use_decimal_or_prefixed_hex_numbers() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    assert!(!content.objects[&0x105].stasis_immune);
    let map = |literal: &str| {
        Arc::new(Map {
            width: 1,
            height: 1,
            cells: vec![crate::content::Cell {
                ground: 0x46,
                object: Some(0x105),
                config: format!("name:native;eff:{literal};size:137;conn:16"),
                terrain: 0,
                region: 0,
            }],
            spawns: vec![(0.5, 0.5)],
        })
    };
    for (literal, expected) in [
        ("16", 16),
        ("0x10", 16),
        ("+16", 16),
        ("-16", -16),
        ("0x80000000", i32::MIN),
        ("0xffffffff", -1),
    ] {
        let world = World::new(content.clone(), map(literal)).unwrap();
        let entity = world.entities.values().next().unwrap();
        assert_eq!(entity.condition, expected, "eff:{literal}");
        assert_eq!(entity.name, "native");
        assert_eq!(entity.size, 137);
        assert!(entity.extra_stats.contains(&(41, 16)));
    }
    for literal in ["0x0x10", "0x100000000", "2147483648", "garbage"] {
        assert!(
            World::new(content.clone(), map(literal)).is_err(),
            "eff:{literal}"
        );
    }
}

#[cfg(test)]
#[test]
fn fixture_population_seed_is_reproducible_without_prepared_authority() {
    let data = std::path::Path::new("data");
    let content = Arc::new(Content::load(data).unwrap());
    for seed in [12351, 423] {
        let mut left = Directory::test_with_seed(data, content.clone(), 16, seed).unwrap();
        let right = Directory::test_with_seed(data, content.clone(), 16, seed).unwrap();
        let mut expected_layout_rng = rand::rngs::StdRng::seed_from_u64(seed);
        for _ in 0..2 {
            let _: usize = expected_layout_rng.gen_range(0..1);
        }
        assert_eq!(
            left.realm_rng.clone().r#gen::<u64>(),
            expected_layout_rng.r#gen::<u64>()
        );
        assert!(left.prepared_seed.is_none() && right.prepared_seed.is_none());
        let mut streams = BTreeSet::new();
        for (id, a) in left.worlds.iter().filter(|(_, w)| w.name == "Realm") {
            let b = &right.worlds[id];
            assert!(a.prepared_seed.is_none() && b.prepared_seed.is_none());
            let next = (
                a.rng.clone().r#gen::<u64>(),
                a.event_rng.clone().r#gen::<u64>(),
            );
            assert_eq!(
                next,
                (
                    b.rng.clone().r#gen::<u64>(),
                    b.event_rng.clone().r#gen::<u64>()
                )
            );
            assert!(streams.insert(next));
            assert_eq!(
                a.entities
                    .values()
                    .map(|e| (e.kind, e.pos.x, e.pos.y))
                    .collect::<Vec<_>>(),
                b.entities
                    .values()
                    .map(|e| (e.kind, e.pos.x, e.pos.y))
                    .collect::<Vec<_>>()
            );
            assert!(a.map.cells.iter().zip(&b.map.cells).all(|(a, b)| (
                a.ground, a.object, &a.config, a.terrain, a.region
            ) == (
                b.ground, b.object, &b.config, b.terrain, b.region
            )));
            assert_eq!(
                a.entities
                    .values()
                    .filter(|e| e.kind == crate::dungeon::manor::KAGE)
                    .count(),
                2
            );
        }
        assert_eq!(streams.len(), 2);
        let worlds = left.worlds.len();
        assert!(left.prepared_instance("realm-shore").is_err());
        assert!(
            left.prepared_instance_with_seed("realm-shore", 423)
                .is_err()
        );
        assert_eq!(left.worlds.len(), worlds);
    }
}

#[cfg(test)]
#[test]
fn populated_realm_fixture_retains_graveyard_space() {
    let data = std::path::Path::new("data");
    let content = Arc::new(Content::load(data).unwrap());
    for seed in [730, 1093] {
        let directory = Directory::fixture(data, content.clone(), 16, 12351, Some(seed))
            .unwrap_or_else(|error| panic!("fixture population seed {seed}: {error:#}"));
        for world in directory.worlds.values().filter(|w| w.name == "Realm") {
            let count = |kind| world.entities.values().filter(|e| e.kind == kind).count();
            assert_eq!(count(crate::dungeon::manor::KAGE), 2);
            assert!((17..25).contains(&count(0x91f)));
        }
    }
}
