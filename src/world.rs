use crate::server::lifecycle::{CloseReason, controller, protocol};
use crate::{
    content::{
        Content, Map, PARALYZED, Projectile, SLOWED, SPEEDY, STUNNED,
        game::{AdmissionRole, NativeService, ProgressKey, geometry::Geometry},
    },
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
        atomic::{AtomicI32, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};
mod abilities;
pub(crate) mod bot_inputs;
pub(crate) mod spectator;

mod cell_index;
pub(crate) mod chat;
#[cfg(test)]
mod community_postclear_audit;

mod companions;
mod consumables;
mod davy;
mod dungeon_entry;
mod endgame;
mod enemy_damage;
mod enemy_shots;
mod entity_store;
mod execution;
#[cfg(test)]
mod ghost_ship;
mod ground_dye;

mod gold;

mod ground;

mod guilds;

mod landmark_ai;

mod manor;

mod names;
mod ocean_setpiece;

mod population;
mod population_density;

mod realm_events;
mod realm_voice;

pub(crate) mod realm_population;
mod realm_portal;
mod realm_quests;

mod realm_setpieces;
mod sight;
mod skull_erosion;

pub(crate) mod native_lag;
mod tick_stats;
mod tracked;

pub(crate) use chat::{chat_error, chat_info, chat_private, validate as validate_chat};
pub use endgame::ForcedTransfer;

mod cooperative;
mod evidence;

mod mechanics;

mod native_hits;

mod npc_attacks;
mod npc_queries;
mod xml;
pub(crate) use native_hits::MAX_PENDING_SHOOT_UNITS;
#[cfg(test)]
pub(crate) use xml::tests::drain_events;
mod reconciliation;

mod resurrection;

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
    #[serde(skip)]
    drop_source: Option<String>,
    pub extra_stats: Vec<(u8, i32)>,
    bag_owners: Vec<i32>,
    pub reserved: bool,
    published: bool,
    damage: BTreeMap<i32, i32>,
    goal: Option<Pos>,
    last_step: Pos,
    phase: f32,
    orbit: Option<realm_events::Orbit>,
    next_shot: u64,
    activated: Option<u64>,
    parent: tracked::Tracked<i32>,
    camp: Option<usize>,
    #[serde(skip)]
    population_slot: Option<usize>,
    quest: tracked::Tracked<i32>,
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
        out.field("drop_source", &self.drop_source);
        out.field("extra_stats", &self.extra_stats);
        out.field("bag_owners", &self.bag_owners);
        out.field("reserved", &self.reserved);
        out.field("published", &self.published);
        out.field("damage", &self.damage);
        out.field("home", &self.home);
        out.field("goal", &self.goal);
        out.field("last_step", &self.last_step);
        out.field("phase", &self.phase);
        out.field("orbit", &self.orbit);
        out.field("next_shot", &self.next_shot);
        out.field("activated", &self.activated);
        out.field("parent", &self.parent);
        out.field("camp", &self.camp);
        out.field("population_slot", &self.population_slot);
        out.field("quest", &*self.quest);
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
        let mut out = serializer.serialize_struct("Entity", 30)?;
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
    legendary: bool,
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
    xml_conditions: i32,
    known_since: BTreeMap<i32, u64>,
    shoot_acks: VecDeque<(Instant, Vec<Bullet>)>,
    weapon_ticks: VecDeque<(i32, i32, i32)>,
    native_lag: native_lag::Window,
    audit_motion: crate::audit::MotionWindow,
    audit_view: Option<evidence::View>,
    audit_revision: u64,
    visible_projectiles: BTreeMap<(i32, u8), native_hits::VisibleProjectile>,
    overlaps: VecDeque<(u64, Bullet)>,
    predicted_deaths: BTreeMap<i32, (u8, Option<u64>)>,
    ground_hits: BTreeMap<usize, i32>,
    // Kithio hides a player from enemies for 3000 ms after entry or teleport.
    newbie_until: u64,
    bot_damage: VecDeque<(u64, i32)>,
    volley_margin_ms: Arc<AtomicU64>,
    oxygen: i32,
    tiles: BTreeSet<(i16, i16)>,
    tile_updates: Vec<(i16, i16, u8)>,
    tile_scan: Option<(i32, i32)>,
    sight_tiles: Option<BTreeSet<(i16, i16)>>,
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
    clears_queued: [u64; ProgressKey::COUNT],
    pub(crate) progress_started: bool,
    terminal_captures: usize,
    pub death_pending: bool,
    persistence_failure: Option<String>,
    pub(crate) inputs: Option<bot_inputs::Inputs>,
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
    fn record_bot_damage(&mut self, now: u64, hp_before: i32) {
        let damage = hp_before - self.character.hp;
        if self.controller != Controller::Autonomous || damage <= 0 {
            return;
        }
        self.bot_damage.retain(|(at, _)| now - *at < 3000);
        if let Some((_, total)) = self.bot_damage.back_mut().filter(|(at, _)| *at == now) {
            *total += damage;
        } else {
            self.bot_damage.push_back((now, damage));
        }
    }

    #[cfg(test)]
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
    fn audit_fact(
        &self,
        id: i32,
        world: &str,
        now: u64,
        event: &str,
        values: impl FnOnce() -> serde_json::Value,
    ) {
        if self.controller == Controller::Human {
            crate::audit::combat(
                serde_json::json!({"event":event,"world":world,"world_ms":now,
                "identity":[self.character.account,self.character.id,id],"values":values()} ),
            );
        }
    }
    fn acknowledge_lifetime(&mut self, committed: crate::store::FameLifetime) -> Result<()> {
        committed.follows(&self.committed_lifetime)?;
        self.character.lifetime.follows(&self.committed_lifetime)?;
        self.character.lifetime.merge(&committed)?;
        // Later live facts remain different from this acknowledged lower bound and must still save.
        self.committed_lifetime = committed;
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
        self.terminal_captures > 0 || self.ordinary_rewards_queued()
    }
    fn ordinary_rewards_queued(&self) -> bool {
        self.experience_queued != 0
            || self.experience_queue.is_some()
            || self.clears_queued != [0; ProgressKey::COUNT]
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
    damage_origin: enemy_damage::DamageOrigin,
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
    #[cfg(test)]
    pub(crate) fn ability_ready_ms(&self) -> u64 {
        self.ability_ready_ms
    }

    pub(crate) fn conditions(&self, now: u64) -> i32 {
        self.xml_conditions | self.lab_conditions | crate::condition::mask(&self.statuses, now)
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
// Ordinary entrances last a minute in the June 2012 guide; keys share that window: https://web.archive.org/web/20120609023836/http://forums.wildshadow.com/wiki/realm-mad-god/dungeons
const DUNGEON_PORTAL_TICKS: u64 = 60_000 / 50;
/// Loot bag kinds by item bag_type appearance.
pub(crate) const LOOT_BAGS: [u16; 5] = [0x500, 0x503, 0x507, 0x508, 0x509];
pub(crate) const PLAYER_VIEW_RADIUS: f32 = 20.;

#[derive(Clone, Copy, Serialize)]
pub struct PortalPlan {
    locked: Option<i32>,
    opened: i32,
    source: Slot,
    world: u64,
    native_lifetime: Option<u64>,
}
impl PortalPlan {
    fn lifetime(self) -> Option<u64> {
        self.native_lifetime
    }
}
impl InventoryPlan {
    pub(crate) fn event_id(&self) -> &str {
        &self.permit.event
    }
}
struct InventoryPermit {
    count: Arc<std::sync::atomic::AtomicUsize>,
    event: String,
}
impl Drop for InventoryPermit {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}
#[derive(Clone, Serialize)]
pub struct InventoryPlan {
    accepted_at: u64,
    #[serde(skip)]
    permit: Arc<InventoryPermit>,
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
    pub collected: Option<crate::store::CollectedItem>,
    pub purchase: Option<(i32, crate::store::Purchase)>,
    loot_slot: Option<(i32, usize)>,
    entitlement_move: bool,
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
#[cfg(test)]
pub(crate) fn test_content() -> Content {
    static CONTENT: std::sync::LazyLock<Content> = std::sync::LazyLock::new(|| {
        let data = std::path::Path::new("data");
        let mut content = Content::load(data).unwrap();
        content.game = Some(Arc::new(
            crate::content::game::GameData::load(data, &content).unwrap(),
        ));
        content
    });
    CONTENT.clone()
}
/// Production worlds draw fresh entropy; unit tests share one fixed seed so long simulations reproduce.
#[cfg(not(test))]
fn world_rng() -> rand::rngs::StdRng {
    rand::rngs::StdRng::from_entropy()
}
#[cfg(test)]
fn world_rng() -> rand::rngs::StdRng {
    rand::SeedableRng::seed_from_u64(0x5eed)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorldDomain {
    Realm,
    NonRealm,
}

type InventoryEvent = (
    InventoryPlan,
    bool,
    crate::store::world_events::Event,
    std::time::Instant,
    Option<(u64, u64)>,
);
pub struct World {
    inventory_capacity: Arc<std::sync::atomic::AtomicUsize>,
    xml: xml::Runtime,
    pub content: Arc<Content>,
    pub map: Arc<Map>,
    navigation_geometry: Box<std::cell::RefCell<view::GeometryCache>>,
    pub(crate) realm_layout: Option<String>,
    /// Where this Realm map's Godlands lie, as a veteran who played it before remembers.
    pub(crate) godlands: Arc<Vec<Pos>>,
    /// One spot in every sizeable Godlands square, for travelling the whole of it.
    pub(crate) godland_route: Arc<Vec<Pos>>,
    /// The terrain tag alone does not make a tile safe for a Godlands landing.
    pub(crate) godland_tiles: Arc<BTreeSet<(i16, i16)>>,
    performance: std::cell::RefCell<crate::performance::Window>,
    realm_tick_mean: Option<(Instant, u64)>,
    pub entities: entity_store::Entities,
    actor_ids: BTreeSet<i32>,
    // Pool effects and Beach announcements must not scan every actor each tick.
    // Snapshots publish after every job; scanning ~10^5 Realm actors for portals cost ~10ms each time.
    portal_ids: BTreeSet<i32>,
    // Superset of actors that ever received an expiry or parent; mechanic_expiry rechecks the exact predicate.
    timed_ids: BTreeSet<i32>,
    // Live: spawners counted their children by scanning every parented actor, which overran the Realm tick budget.
    children: BTreeMap<i32, BTreeSet<i32>>,
    // Quest roots can have neither a parent nor an expiry, so timed_ids cannot cover membership.
    quest_ids: BTreeSet<i32>,
    gun_ids: BTreeSet<i32>,
    // Terminal deadlines run in every world, including Realms without any Flamers.
    // Only Malphas carries Abyss landings; scanning every Realm actor for them cost 7-8 ms per spawn check.
    // Only fully dormant ordinary population can leave this set; clocks and unclassified births stay global.
    non_population_ids: BTreeSet<i32>,
    quests_by_level: BTreeMap<Option<i32>, BTreeSet<i32>>,
    pub players: BTreeMap<i32, Player>,
    pub guild: Option<(i32, i32)>,
    pub bullets: Vec<Bullet>,
    // Collision callbacks can emit while the original batch is outside this vector.
    detached_bullets: usize,
    // Client-reported hits refused by terminal capacity outlive their projectile until they land.
    owed_shots: Vec<(Bullet, i32)>,
    ability_state: abilities::State,
    companions: companions::State,
    cells: cell_index::CellIndex,
    merchants: BTreeMap<i32, shop::Offer>,
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
    uses_breath: Option<f64>,
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
    population_children: BTreeMap<i32, u8>,
    population_next: u64,
    reproduction: BTreeMap<(i32, usize), u64>,
    summon_waits: BTreeSet<i32>,
    reproduction_at: [u64; 20],
    reproduction_bucket: usize,
    npc_paused_since: BTreeMap<i32, u64>,
    setpieces: Vec<realm_setpieces::Area>,
    ghost_towers: Vec<realm_setpieces::Area>,
    phoenix_pyres: Vec<realm_setpieces::Area>,
    event_sites: Vec<realm_setpieces::EventSite>,
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
    recent_market_pairs: BTreeMap<[i32; 2], u64>,
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
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DepartureOutcome {
    Retired,
    TransferReady,
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
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PreparedNativeTransferPhase {
    SourceRetiring,
    SourceDeparted,
    HelloConsumedAdmissionPending,
}
#[derive(Serialize)]
pub(crate) struct PreparedNativeTransition {
    pub account: i32,
    pub character: i32,
    pub destination: i32,
    pub phase: PreparedNativeTransferPhase,
}
#[derive(Clone, Copy)]
struct PreparedNativeSource {
    player: i32,
    departed: bool,
}
struct Grant {
    prepared_source: Option<PreparedNativeSource>,
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
    expires: Option<u64>,
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
    realms: Vec<Arc<crate::terrain::realms::Source>>,
    realm_rng: rand::rngs::StdRng,
    portal_rng: rand::rngs::StdRng,
    portal_positions: Vec<Pos>,
    content: Arc<Content>,
    ids: Arc<AtomicI32>,
    grants: BTreeMap<Vec<u8>, Grant>,
    portals: BTreeMap<i32, Portal>,
    terminal_allocations: Arc<std::sync::atomic::AtomicUsize>,
    inventory_events: BTreeMap<String, InventoryEvent>,
    inventory_capacity: Arc<std::sync::atomic::AtomicUsize>,
    chat_budgets: BTreeMap<i32, chat::ChatBudget>,
    guild_invites: BTreeMap<i32, guilds::Invitation>,
    endgame: endgame::RealmDirectory,
    realm_population: realm_population::Population,
    teleports: BTreeMap<i32, Instant>,
    pub realm_lockouts: BTreeMap<i32, Instant>,
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
            "native_lag": p.native_lag.snapshot(Instant::now(), true),
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
    pub fn human_player_count(&self) -> usize {
        self.worlds
            .summaries()
            .map(|(_, w)| {
                w.players
                    .values()
                    .filter(|p| p.controller == Controller::Human)
                    .count()
            })
            .sum()
    }
    pub fn health(&self, started: Instant) -> serde_json::Value {
        let worlds: Vec<_> = self.worlds.summaries().collect();
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
        let busiest_realm_tick_mean_us = worlds
            .iter()
            .filter_map(|(_, w)| w.realm_tick_mean)
            .filter(|(at, _)| at.elapsed() <= Duration::from_secs(90))
            .map(|(_, mean_us)| mean_us)
            .max();
        serde_json::json!({
            "status":"development","uptime_seconds":started.elapsed().as_secs(),
            "busiest_realm_tick_mean_us":busiest_realm_tick_mean_us,
            "worlds":worlds.len(),"players":self.human_player_count(),"bots":bots,"qa_actors":qa_actors,
            "entities":worlds.iter().map(|(_,w)|w.entities).sum::<usize>(),
            "projectiles":worlds.iter().map(|(_,w)|w.bullets).sum::<usize>(),
            "tick":self.tick,"max_tick_us":self.max_tick_us,"tick_scope":"coordinator, not world simulation",
            "world_progress":worlds.iter().map(|(_,w)|serde_json::json!({"tick":w.tick,"simulation_ms":w.now,"published_age_ms":w.published.elapsed().as_millis()})).collect::<Vec<_>>(),
            "paused":self.clock.paused(),"process":std::process::id(),"revision":option_env!("GITHUB_SHA"),
        })
    }

    #[cfg(test)]
    pub(crate) fn test_with_seed(
        _path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
    ) -> Result<Self> {
        Self::fixture(content, max_players, seed, None, 2)
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        mut content: Arc<Content>,
        max_players: usize,
        seed: u64,
        prepared_seed: Option<u64>,
        realms: usize,
    ) -> Result<Self> {
        // Admission and lifecycle tests need a real closure roster, not a simulated ecosystem.
        let game = Arc::make_mut(Arc::make_mut(&mut content).game.as_mut().unwrap());
        for site in &mut game.realm.as_mut().unwrap().sites {
            site.min = 0;
            site.max = 0;
        }
        let mut map = Map {
            elevation: None,
            width: 96,
            height: 96,
            cells: vec![
                crate::content::Cell {
                    ground: 0x18,
                    object: None,
                    config: String::new(),
                    terrain: 0,
                    region: 0,
                };
                96 * 96
            ],
            spawns: vec![(12.5, 12.5), (12.5, 24.5)],
        };
        for x in [70, 80] {
            map.cells[70 * 96 + x].object = Some(0x67d);
        }
        Self::configured(
            content,
            max_players,
            seed,
            prepared_seed,
            vec![crate::terrain::realms::Source::Fixture(Arc::new(map))],
            realms,
        )
    }

    #[cfg(test)]
    fn populated_fixture(content: Arc<Content>, max_players: usize, seed: u64) -> Result<Self> {
        let mut map = crate::terrain::generate(&content, seed, 1024)?;
        // Contiguous pads give the small procedural test island the sourced Tower and Grove habitats.
        for (radius, terrain, ground) in [(230., 4, 0x47), (315., 7, 0x56)] {
            for i in 0..24 {
                let angle = i as f64 * std::f64::consts::TAU / 24.;
                let (cx, cy) = (
                    512 + (radius * angle.cos()) as i32,
                    512 + (radius * angle.sin()) as i32,
                );
                for y in cy - 16..=cy + 16 {
                    for x in cx - 16..=cx + 16 {
                        let cell = &mut map.cells[(y * 1024 + x) as usize];
                        cell.ground = ground;
                        cell.terrain = terrain;
                        cell.object = None;
                        cell.config.clear();
                    }
                }
            }
        }
        let map = Arc::new(map);
        Self::configured(
            content,
            max_players,
            seed,
            None,
            vec![crate::terrain::realms::Source::Fixture(map)],
            2,
        )
        .with_context(|| format!("test Realm fixture map/layout seed {seed}"))
    }
    pub fn with_seed(
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        realm_count: usize,
    ) -> Result<Self> {
        Self::configured(
            content.clone(),
            max_players,
            seed,
            None,
            crate::terrain::realms::linked_sources(&content)?,
            realm_count,
        )
    }
    pub(crate) fn prepared(
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        realm_count: usize,
    ) -> Result<Self> {
        Self::configured(
            content.clone(),
            max_players,
            seed,
            Some(seed),
            crate::terrain::realms::linked_sources(&content)?,
            realm_count,
        )
    }
    fn configured(
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        prepared_seed: Option<u64>,
        realms: Vec<crate::terrain::realms::Source>,
        realm_count: usize,
    ) -> Result<Self> {
        ensure!(
            (1..=realm_population::MAX_CURRENT).contains(&realm_count),
            "initial realm_count must be between 1 and 8"
        );
        ensure!(!realms.is_empty(), "Realm layout pool is empty");
        crate::endgame::validate(&content)?;
        let templates = Self::linked_static_maps(&content)?;
        let mut d = Self {
            clock: crate::clock::Clock::default(),
            worlds: execution::Worlds::new(),
            templates,
            realms: realms.into_iter().map(Arc::new).collect(),
            realm_rng: rand::rngs::StdRng::seed_from_u64(seed),
            portal_rng: rand::rngs::StdRng::seed_from_u64(seed ^ u64::from_le_bytes(*b"entrance")),
            portal_positions: Vec::new(),
            content,
            ids: Arc::new(AtomicI32::new(1000)),
            grants: BTreeMap::new(),
            portals: BTreeMap::new(),
            terminal_allocations: Arc::default(),
            inventory_events: BTreeMap::new(),
            inventory_capacity: Arc::default(),
            chat_budgets: BTreeMap::new(),
            guild_invites: BTreeMap::new(),
            endgame: endgame::RealmDirectory::default(),
            realm_population: realm_population::Population::default(),
            teleports: BTreeMap::new(),
            realm_lockouts: BTreeMap::new(),
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
            (-3, "realm", "Realm", false),
            (-4, "nexus-limbo", "Nexus Limbo", true),
        ] {
            let world = d.make(template, name, safe, None)?;
            if template == "nexus" {
                let game = world.xml_game();
                let map = &game.maps[game.world_template(template).unwrap().map as usize];
                d.portal_positions = world
                    .realm_portal_positions()
                    .with_context(|| format!("World nexus map={}", map.name))?;
            }
            d.worlds.insert(id, world);
        }
        for slot in 0..realm_count {
            let origin = if slot == 0 {
                -3
            } else {
                let id = d.next_world;
                let world = d.make("realm", "Realm", false, None)?;
                d.worlds.insert(id, world);
                d.next_world += 1;
                id
            };
            d.bind_realm_heroes(origin, d.worlds[&origin].hero_identities())?;
            let name =
                (seed as usize % realm_portal::NAMES.len() + slot) % realm_portal::NAMES.len();
            let portal = d.install_realm_portal(slot)?;
            let position = d.worlds[&-2].entities[&portal].pos;
            tracing::info!(
                realm = origin,
                portal,
                x = position.x,
                y = position.y,
                "Realm entrance bound"
            );
            let record = d.endgame.realms.get_mut(&origin).unwrap();
            record.name = name;
            record.portal = Some(portal);
            record.portal_slot = slot;
        }
        d.sync_realm_portal();
        Ok(d)
    }
    fn linked_static_maps(content: &Content) -> Result<BTreeMap<String, Arc<Map>>> {
        let game = content.game.as_ref().context("Directory GameData absent")?;
        game.worlds
            .iter()
            .filter_map(|world| {
                let binding = &game.maps[world.map as usize];
                if matches!(binding.geometry, Geometry::Generator(_)) && world.id != "realm"
                    || matches!(binding.geometry, Geometry::Pool(_)) && world.id == "realm"
                {
                    return None;
                }
                Some(
                    (|| {
                        let Geometry::Encoded(asset) = &binding.geometry else {
                            bail!("incompatible playable world geometry")
                        };
                        ensure!(
                            !asset.realm && world.id != "realm",
                            "incompatible encoded world geometry"
                        );
                        let map = asset.decode(content)?;
                        ensure!(!map.spawns.is_empty(), "map has no spawn cells");
                        Ok((world.id.clone(), Arc::new(map)))
                    })()
                    .with_context(|| format!("World {} map={}", world.id, binding.name)),
                )
            })
            .collect()
    }
    fn make(
        &mut self,
        template: &str,
        name: &str,
        safe: bool,
        owner: Option<i32>,
    ) -> Result<World> {
        self.construction(template, name, safe, owner)()
    }
    fn construction(
        &mut self,
        template: &str,
        name: &str,
        safe: bool,
        owner: Option<i32>,
    ) -> impl FnOnce() -> Result<World> + Send + use<> {
        let source = (template == "realm").then(|| {
            let mut draw = self
                .realm_rng
                .gen_range(0..self.realms.iter().map(|s| s.weight()).sum());
            self.realms
                .iter()
                .find(|s| {
                    if draw < s.weight() {
                        true
                    } else {
                        draw -= s.weight();
                        false
                    }
                })
                .expect("nonempty linked Realm pool with positive weights")
                .clone()
        });
        #[cfg(test)]
        let fixture_seed = source.as_ref().and_then(|source| {
            matches!(&**source, crate::terrain::realms::Source::Fixture(_))
                .then(|| self.realm_rng.clone().r#gen::<u64>())
        });
        let terminal_allocations = self.terminal_allocations.clone();
        let inventory_capacity = self.inventory_capacity.clone();
        let (content, ids, clock) = (self.content.clone(), self.ids.clone(), self.clock.clone());
        let (prepared_seed, next_world, max_players) =
            (self.prepared_seed, self.next_world, self.max_players);
        let map = self.templates.get(template).cloned();
        let template = template.to_owned();
        let name = name.to_owned();
        move || {
            let game = content
                .game
                .as_ref()
                .context("world construction GameData absent")?;
            let spec = game
                .world_template(&template)
                .with_context(|| format!("unknown world template={template}"))?;
            let binding = &game.maps[spec.map as usize];
            let domain = if source.is_some() {
                WorldDomain::Realm
            } else {
                WorldDomain::NonRealm
            };
            let map = (|| match &binding.geometry {
                Geometry::Encoded(_) => map.context("linked static map not loaded"),
                Geometry::Generator(generator) => {
                    let seed =
                        || prepared_seed.map_or_else(rand::random, |s| s ^ next_world as u64);
                    crate::dungeon::generate(&content, spec, generator, seed).map(Arc::new)
                }
                Geometry::Pool(_) => source
                    .as_ref()
                    .context("linked Realm layout not selected")?
                    .open(&content),
                _ => bail!("map is not playable world geometry"),
            })()
            .with_context(|| format!("World {template} map={}", binding.name))?;
            let mut world = World::with_ids_context(content.clone(), map, ids, Some(domain))?;
            world.inventory_capacity = inventory_capacity;
            world.xml.terminal_allocations = terminal_allocations;
            world.realm_layout = source.as_ref().map(|source| source.name().to_owned());
            world.clock = clock.clone();
            world.attack_epoch = clock.now();
            world.new_tick_at = clock.now();
            #[cfg(test)]
            if let Some(seed) = fixture_seed {
                world.rng = rand::rngs::StdRng::seed_from_u64(seed);
                world.event_rng = rand::rngs::StdRng::seed_from_u64(seed ^ 0x7265616c6d);
                tracing::debug!(seed, "test Realm population seeded");
            }
            if let Some(seed) = prepared_seed {
                let seed = seed ^ next_world as u64;
                world.rng = rand::rngs::StdRng::seed_from_u64(seed);
                world.event_rng = rand::rngs::StdRng::seed_from_u64(seed ^ 0x7265616c6d);
                world.prepared_seed = Some(seed);
            }
            world.xml.template = Some(template.clone());
            world.uses_breath = spec
                .services
                .uses_breath
                .as_ref()
                .map(|service| service.value);
            world.name = name;
            // Select the client's existing starfield, not dungeon void: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Rotmg123.5.1/src/_H_Z_/Background.as
            world.background = match template.as_str() {
                "sprite-world" => 1,
                "nexus" | "vault" => 2,
                _ => 0,
            };
            world.safe = safe;
            world.owner = owner;
            world.max_players = max_players;
            world.encounter = matches!(template.as_str(), "realm" | "tutorial" | "kitchen")
                || world
                    .xml_world_template()
                    .is_some_and(|t| t.clear.is_some());
            if template == "realm" {
                let started = Instant::now();
                let sites = source.as_ref().context("Realm source missing")?.sites()?;
                world.populate_authored_sites(&sites)?;
                use crate::content::game::SitePhase;
                world.populate_named_sites(&sites, SitePhase::Landmarks)?;
                world.populate_realm_population()?;
                world.populate_named_sites(&sites, SitePhase::AfterPopulation)?;
                world.reserve_realm_events()?;
                let godland_cells = godland_cells(&world.map, &world.content);
                let spots = godland_spots(&godland_cells);
                world.godlands = Arc::new(godlands(&spots, &world.map.spawns));
                world.godland_route = Arc::new(spots.into_iter().map(|(_, p)| p).collect());
                world.godland_tiles = Arc::new(
                    godland_cells
                        .into_values()
                        .flatten()
                        .map(|p| (p.x as i16, p.y as i16))
                        .collect(),
                );
                tracing::info!(
                    layout = world.realm_layout,
                    entities = world.entities.len(),
                    actors = world.actor_ids.len(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "Realm simulation populated"
                );
            }
            world.prepare_endgame()?;
            if world.encounter {
                let kinds: BTreeSet<_> = world
                    .entities
                    .values()
                    .filter(|e| {
                        let object = &world.content.objects[&e.kind];
                        object.enemy
                            && !matches!(e.kind, 0x802..=0x805)
                            && (object.class != "Wall" || object.passive_destructible())
                    })
                    .map(|e| e.kind)
                    .collect();
                for kind in kinds {
                    if !world.content.objects[&kind].passive_destructible() {
                        world.xml_declared_admission(kind, "construction roster")?;
                    }
                }
            }
            for kind in [
                0x173, 0x653, 0x918, 0x919, 0xd07, 0xd08, 0xd09, 0xd0a, 0xdaa, 0xdab, 0xdac, 0xdad,
                0xdae, 0xdaf, 0xdb0, 0xdb1, 0xdb2,
            ] {
                world.xml_declared_admission(kind, "construction roster")?;
            }
            // Admission is t0: entering every root state here keeps a ~150ms Realm start off the first tick.
            world.xml_step()?;
            Ok(world)
        }
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
    #[cfg(test)]
    pub(crate) fn character_transferring(&self, character: i32) -> bool {
        self.grants.values().any(|g| {
            g.character == character
                && g.authority.state() & (TRANSFER_REVOKED | TRANSFER_CONSUMED) == 0
        })
    }
    pub(crate) fn prepared_native_transition(
        &self,
        character: i32,
    ) -> Result<Option<PreparedNativeTransition>> {
        ensure!(
            self.prepared_seed.is_some(),
            "native transition reads require prepared state"
        );
        ensure!(
            self.worlds
                .values()
                .flat_map(|w| w.players.values())
                .filter(|p| p.character.id == character)
                .count()
                <= 1,
            "multiple native source sessions"
        );
        let mut transition = None;
        for (key, grant) in &self.grants {
            if grant.character != character || grant.expires <= self.tick {
                continue;
            }
            let state = grant.authority.state();
            if state & (TRANSFER_REVOKED | TRANSFER_ADMITTED | TRANSFER_RETIRED) != 0
                || state & TRANSFER_WRITE != 0 && state & TRANSFER_RESERVED == 0
                || state & TRANSFER_DEPARTED != 0 && state & TRANSFER_WRITE == 0
            {
                continue;
            }
            let Some(origin) = grant.origin.filter(|origin| *origin != grant.destination) else {
                continue;
            };
            let Some(custody) = grant.prepared_source.as_ref() else {
                continue;
            };
            if !self.worlds.contains_key(&origin) {
                continue;
            }
            let Some(destination) = self.worlds.get(&grant.destination) else {
                continue;
            };
            let Some(admission) = destination.admissions.get(key) else {
                continue;
            };
            let consumed = state & TRANSFER_CONSUMED != 0;
            if !destination.admission_open
                || admission.expires <= destination.now
                || admission.account != grant.account
                || admission.character != character
                || admission.consumed != consumed
            {
                continue;
            }
            let connected = self.worlds.iter().find_map(|(world, w)| {
                w.players
                    .iter()
                    .find(|(_, p)| p.character.id == character)
                    .map(|(id, p)| (*world, *id, p))
            });
            let phase = if let Some((world, id, player)) = connected {
                if custody.departed
                    || custody.player != id
                    || state & TRANSFER_DEPARTED != 0
                    || world != origin
                    || player.character.account != grant.account
                    || !player.native_transferring()
                    || !player.character.alive
                    || player.death_pending
                    || player.character.hp <= 0
                    || consumed
                {
                    continue;
                }
                PreparedNativeTransferPhase::SourceRetiring
            } else {
                if !custody.departed
                    || state & TRANSFER_RESERVED != 0 && state & TRANSFER_DEPARTED == 0
                {
                    continue;
                }
                // HELLO consumes authority before LOAD inserts the original avatar.
                if consumed {
                    PreparedNativeTransferPhase::HelloConsumedAdmissionPending
                } else {
                    PreparedNativeTransferPhase::SourceDeparted
                }
            };
            ensure!(transition.is_none(), "multiple live native transfer owners");
            transition = Some(PreparedNativeTransition {
                account: grant.account,
                character,
                destination: grant.destination,
                phase,
            });
        }
        Ok(transition)
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
    /// Marked cheaters share one Realm: the open Realm with the fewest occupants.
    fn quietest_realm(&self) -> Result<i32> {
        self.available_realms()
            .into_iter()
            .min_by_key(|(id, occupancy)| (*occupancy, *id))
            .map(|(id, _)| id)
            .ok_or_else(|| controller("no open Realm has capacity"))
    }
    pub fn initial(&self, game: i32, account: i32) -> Result<i32> {
        let destination = match game {
            0 | -2 => -2,
            -1 => -1,
            -3 if self.content.reviewed(account) => self.quietest_realm()?,
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
        let registration = self
            .content
            .game
            .as_ref()
            .and_then(|game| game.world_template(template));
        let instance = registration.and_then(|t| t.instance.as_ref());
        ensure!(
            return_route.is_some() || registration.is_none_or(|t| t.clear_key.is_none()),
            "dungeon {template} requires an explicit return route"
        );
        ensure!(
            matches!((template, account), ("vault", Some(_)) | ("kitchen", None))
                || account.is_none() && instance.is_some(),
            "invalid private/dungeon instance policy for {template}"
        );
        if let Some(account) = account
            && let Some((id, _)) = self.worlds.iter().find(|(_, w)| w.owner == Some(account))
        {
            return Ok(*id);
        }
        let name = match template {
            "vault" => "Vault".to_owned(),
            "kitchen" => "Kitchen".to_owned(),
            _ => instance
                .context("dungeon instance registration missing")?
                .name
                .clone(),
        };
        let mut world = self.make(template, &name, template == "vault", account)?;
        // Entrances expire before their dungeons; preserve RECONNECT's exact world identity: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/kabam/rotmg/messaging/impl/incoming/Reconnect.as
        world.return_route = return_route;
        if let Some(DungeonReturn::Realm(origin)) = return_route
            && let Some(record) = self.endgame.realms.get_mut(&origin)
        {
            record.empty_since = None;
        }
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
        if self.endgame.realms.contains_key(&origin) {
            Ok(DungeonReturn::Realm(origin))
        } else if world
            .xml_world_template()
            .is_some_and(|t| t.clear_key.is_some())
        {
            world
                .return_route
                .context("parent dungeon has no return route")
        } else {
            Ok(DungeonReturn::Random)
        }
    }
    fn dungeon_realm(&self, world: &World, account: i32) -> Result<i32> {
        match world.return_route.context("dungeon has no return route")? {
            DungeonReturn::Random => self.initial(-3, account),
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
        let Some(portal) = portal else {
            // Escape is a protocol command, not use of a physical portal.
            ensure!(origin != -2, protocol("already in destination world"));
            return Ok((account, character, -2));
        };
        let route = {
            let e = world
                .entities
                .get(&portal)
                .ok_or_else(|| protocol("portal missing"))?;
            world.xml_portal_definition(e.kind, "actual Directory portal transfer")?;
            ensure!(e.published && !e.reserved, protocol("portal not published"));
            ensure!(
                !world.xml_terminal_pending(portal)
                    && world
                        .xml_portal_expiry(portal)
                        .is_none_or(|at| at > world.now),
                protocol("portal XML lifecycle expired or terminal publication pending")
            );
            // The client sends only a portal ID, so a reach refusal is not a malformed packet: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Rotmg123.5.1/src/com/company/assembleegameclient/net/_1f.as#L509-L513
            let distance = p.pos.distance(e.pos);
            ensure!(distance <= 2., {
                tracing::info!(
                    player,
                    portal,
                    player_x = p.pos.x,
                    player_y = p.pos.y,
                    portal_x = e.pos.x,
                    portal_y = e.pos.y,
                    distance,
                    "portal transfer refused: out of reach"
                );
                controller("portal out of reach")
            });
            world
                .xml_portal_route(e.kind)
                .map_err(|error| anyhow::Error::new(CloseReason::Protocol).context(error))?
                .destination
                .clone()
        };
        use crate::content::game::PortalDestination::*;
        let destination = match route {
            NamedRealm => {
                let (id, record) = self
                    .endgame
                    .realms
                    .iter()
                    .find(|(_, record)| record.portal == Some(portal))
                    .ok_or_else(|| protocol("stale named Realm portal"))?;
                ensure!(
                    record.enabled && self.worlds[id].admission_open,
                    controller("Realm is closed")
                );
                ensure!(
                    !self.content.reviewed(account) || *id == self.quietest_realm()?,
                    controller("Your account may only enter the least populated Realm.")
                );
                *id
            }
            OriginRealm => self.dungeon_realm(world, account)?,
            Nexus => -2,
            RandomRealm => self.initial(-3, account)?,
            FreshKitchen => self.instance("kitchen", None, None)?,
            OwnedVault => self.instance("vault", Some(account), None)?,
            GuildHall => {
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
            Instance(template) => {
                if let Err(error) = self.bind_journaled_portal(origin, portal) {
                    tracing::error!(portal, origin, %error, "journaled portal construction refused; same portal/template retained");
                    return Err(error.context(controller(
                        "Dungeon construction failed; the portal remains. Try again or use Nexus.",
                    )));
                }
                let binding = self.portals.get(&portal).with_context(|| {
                    anyhow::Error::new(CloseReason::Protocol).context(format!(
                        "portal {portal} has no live {template} instance binding"
                    ))
                })?;
                ensure!(
                    binding.origin == origin
                        && binding.expires.is_none_or(|expires| expires > self.tick),
                    protocol("dungeon portal binding expired or belongs to another world")
                );
                binding
                    .destination
                    .context("linked instance has no destination")?
            }
            Locked => return Err(protocol("portal is locked")),
        };
        ensure!(
            destination != origin,
            protocol("already in destination world")
        );
        let locked = self.realm_lockout(account, destination);
        ensure!(
            locked == 0,
            anyhow::Error::new(CloseReason::Controller).context(format!(
                "You cannot enter the Realm for another {locked} seconds."
            ))
        );
        Ok((account, character, destination))
    }
    /// Easy Client `/vault` and `/realm`: the account's Vault, or the most populated open Realm.
    pub fn shortcut(&mut self, player: i32, vault: bool) -> Result<(i32, i32, i32)> {
        let origin = self.location(player)?;
        let p = &self.worlds[&origin].players[&player];
        ensure!(
            p.character.alive && p.character.hp > 0 && !p.death_pending && !p.departing,
            controller("character cannot transfer")
        );
        let (account, character) = (p.character.account, p.character.id);
        let destination = if vault {
            self.instance("vault", Some(account), None)?
        } else if self.content.reviewed(account) {
            self.quietest_realm()?
        } else {
            self.available_realms()
                .into_iter()
                .max_by_key(|(id, occupancy)| (*occupancy, -id))
                .map(|(id, _)| id)
                .ok_or_else(|| controller("no open Realm has capacity"))?
        };
        ensure!(
            destination != origin,
            controller(if vault {
                "You are already in your Vault."
            } else {
                "You are already in the most populated Realm."
            })
        );
        let locked = self.realm_lockout(account, destination);
        ensure!(
            locked == 0,
            anyhow::Error::new(CloseReason::Controller).context(format!(
                "You cannot enter the Realm for another {locked} seconds."
            ))
        );
        tracing::info!(
            player,
            account,
            origin,
            destination,
            vault,
            "Easy Client shortcut routed"
        );
        Ok((account, character, destination))
    }
    /// Seconds left before `account` may enter `world`; zero unless `world` is a Realm.
    pub(crate) fn realm_lockout(&self, account: i32, world: i32) -> u64 {
        if !self.endgame.realms.contains_key(&world) {
            return 0;
        }
        self.realm_lockouts.get(&account).map_or(0, |until| {
            until.saturating_duration_since(Instant::now()).as_secs()
        })
    }
    pub fn issue(
        &mut self,
        account: i32,
        character: i32,
        destination: i32,
    ) -> Result<(i32, Vec<u8>, String)> {
        self.retire_transfer_grants();
        // Live Nov 27: a transfer interrupted before departure kept its admission for 120 s, so every portal kicked
        // the player. While the character is still live here, its never-departed grants are dead and replaceable.
        if self.worlds.values().any(|w| {
            w.players
                .values()
                .any(|p| p.character.account == account && p.character.id == character)
        }) {
            let stale: Vec<_> = self
                .grants
                .iter()
                .filter(|(_, g)| g.account == account && g.authority.state() == 0)
                .map(|(key, _)| key.clone())
                .collect();
            for key in stale {
                tracing::info!(account, character, "stale transfer admission replaced");
                self.cancel_admission(account, key)?;
            }
        }
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
        let prepared_source = self.prepared_seed.and(origin).and_then(|origin| {
            self.worlds[&origin].players.iter().find_map(|(id, p)| {
                (p.character.account == account && p.character.id == character).then_some(
                    PreparedNativeSource {
                        player: *id,
                        departed: false,
                    },
                )
            })
        });
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
                prepared_source,
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
        for id in origin.into_iter().chain(std::iter::once(destination)) {
            if let Some(record) = self.endgame.realms.get_mut(&id) {
                record.empty_since = None;
            }
        }
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
        if !self.endgame.realms.contains_key(&grant.destination) {
            return Ok(());
        }
        let Some(dungeon) = world.xml_world_template().and_then(|t| t.exit_key) else {
            return Ok(());
        };
        let entity = world
            .entities
            .get(&portal)
            .context("earned exit portal missing")?;
        if !world.xml_portal_route(entity.kind)?.earned_exit {
            return Ok(());
        }
        ensure!(
            player.character.alive
                && player.character.hp > 0
                && !player.death_pending
                && !player.departing
                && entity.published
                && player.pos.distance(entity.pos) <= 2.,
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
                self.endgame.realms.contains_key(&grant.destination)
                    && destination.players.len() < destination.max_players,
                "earned exit destination is full or unavailable"
            );
        }
        Ok(())
    }
    fn accept_source_write(
        &self,
        source: i32,
        id: i32,
        key: &[u8],
    ) -> Result<Option<crate::dungeon::HistoricalDungeon>> {
        let grant = self
            .grants
            .get(key)
            .context("source transfer grant missing")?;
        ensure!(grant.expires > self.tick, "source transfer grant expired");
        ensure!(
            grant.authority.state() & TRANSFER_REVOKED == 0,
            "source transfer grant revoked"
        );
        self.validate_exit_destination(grant)?;
        if let Some(exit) = &grant.exit {
            let world = &self.worlds[&source];
            let p = &world.players[&id];
            ensure!(
                exit.source == source
                    && exit.player == id
                    && p.character.alive
                    && !p.death_pending
                    && world
                        .entities
                        .get(&exit.portal)
                        .is_some_and(|e| e.published && e.pos.distance(p.pos) <= 2.),
                "earned exit source changed"
            );
        }
        // Cancellation after acceptance cannot undo the earned exit in the owned SQL write.
        grant.authority.transition(
            TRANSFER_WRITE,
            TRANSFER_RESERVED,
            TRANSFER_WRITE | TRANSFER_DEPARTED | TRANSFER_CONSUMED,
        )?;
        Ok(grant.exit.as_ref().map(|exit| exit.dungeon))
    }
    pub(crate) fn begin_source_departure(
        &mut self,
        id: i32,
        key: Option<Vec<u8>>,
    ) -> Result<Option<(Character, Option<Vec<u8>>)>> {
        let source = self.location(id)?;
        if let Some(grant) = key.as_ref().and_then(|key| self.grants.get_mut(key)) {
            let p = &self.worlds[&source].players[&id];
            let state = grant.authority.state();
            ensure!(
                grant.account == p.character.account
                    && grant.character == p.character.id
                    && state & (TRANSFER_WRITE | TRANSFER_DEPARTED | TRANSFER_CONSUMED) == 0
                    && state & TRANSFER_RESERVED != 0,
                "source transfer grant is foreign or already accepted"
            );
            if p.death_pending || !p.character.alive {
                grant.exit = None;
            }
        }
        let Some(mut snapshot) = self.worlds.get_mut(&source).unwrap().begin_departure(id)? else {
            return Ok(None);
        };
        let key = key.filter(|key| match self.accept_source_write(source, id, key) {
            Ok(exit) => {
                if let Some(exit) = exit {
                    snapshot.lifetime.realm_exits.insert(exit);
                    self.worlds.get_mut(&source).unwrap().players.get_mut(&id).unwrap().departure_snapshot =
                        Some(snapshot.clone());
                }
                true
            }
            Err(error) => {
                tracing::warn!(player=id, account=snapshot.account, character=snapshot.id, origin=source, %error,
                    "source transfer cancelled before save; retiring in origin");
                false
            }
        });
        Ok(Some((snapshot, key)))
    }
    pub(crate) fn finish_source_departure(
        &mut self,
        id: i32,
        snapshot: Character,
        key: Option<Vec<u8>>,
    ) -> Result<DepartureOutcome> {
        let source = self.location(id)?;
        ensure!(
            self.worlds[&source].players[&id]
                .departure_snapshot
                .as_ref()
                == Some(&snapshot),
            "source departure snapshot changed"
        );
        let witness = key
            .as_deref()
            .and_then(|key| self.capture_dungeon_entry(id, key));
        let prepared_departure = self.prepared_seed.is_some()
            && snapshot.alive
            && snapshot.hp > 0
            && self.worlds[&source].players[&id]
                .persistence_failure
                .is_none()
            && !self.worlds[&source].players[&id].death_pending;
        self.worlds.get_mut(&source).unwrap().finish_departure(id)?;
        // Forced departures use no reservation bits; absence alone cannot prove source custody retired.
        if prepared_departure {
            for grant in self.grants.values_mut().filter(|grant| {
                grant.origin == Some(source)
                    && grant.account == snapshot.account
                    && grant.character == snapshot.id
            }) {
                if let Some(custody) = grant.prepared_source.as_mut().filter(|s| s.player == id) {
                    custody.departed = true;
                }
            }
        }
        let Some(key) = key else {
            return Ok(DepartureOutcome::Retired);
        };
        let publish: Result<()> = (|| {
            let grant = self
                .grants
                .get(&key)
                .context("committed source transfer grant missing")?;
            ensure!(
                grant.account == snapshot.account && grant.character == snapshot.id,
                "committed source transfer binding changed"
            );
            ensure!(
                grant.expires > self.tick,
                "committed source transfer grant expired"
            );
            self.validate_exit_destination(grant)?;
            let grant = self.grants.get_mut(&key).unwrap();
            grant.authority.transition(
                TRANSFER_DEPARTED,
                TRANSFER_WRITE,
                TRANSFER_DEPARTED | TRANSFER_CONSUMED,
            )?;
            grant.entry_witness = witness;
            Ok(())
        })();
        if let Err(error) = publish {
            tracing::warn!(player=id, account=snapshot.account, character=snapshot.id, origin=source, %error,
                "source transfer cancelled after save; retired in origin");
            return Ok(DepartureOutcome::Retired);
        }
        Ok(DepartureOutcome::TransferReady)
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

    fn retire_expired_portals(&mut self) {
        let expired_portals: Vec<_> = self
            .portals
            .iter()
            .filter(|(id, binding)| {
                binding.expires.is_some_and(|expires| expires <= self.tick)
                    || !self
                        .worlds
                        .summary(&binding.origin)
                        .expect("portal origin exists")
                        .portal_ids
                        .contains(id)
            })
            .map(|(id, binding)| (*id, binding.origin))
            .collect();
        for (id, origin) in expired_portals {
            let world = self.worlds.get_mut(&origin).expect("portal origin exists");
            if world.entities.get(&id).is_none_or(|e| !e.reserved) {
                if world.entities.contains_key(&id) {
                    world.remove_entity(id);
                }
                self.portals.remove(&id);
                tracing::info!(portal = id, origin, "dungeon portal expired");
            }
            self.worlds.release_world(&origin);
        }
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
        self.realm_population_step(self.clock.now());
        self.sync_realm_portal();
        self.worlds.release_world(&-2);
        self.retire_expired_portals();
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
        // Preserve the incident component consumed by shutdown.
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
    if let Some(tx) = &p.tx {
        let result = tx.packets.try_send(Packet { id, body });
        if let Err(error) = result {
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
    #[cfg(test)]
    fn in_domain(content: Arc<Content>, map: Arc<Map>, domain: WorldDomain) -> Result<Self> {
        Self::with_ids_context(content, map, Arc::new(AtomicI32::new(1000)), Some(domain))
    }
    pub(crate) fn with_ids(
        content: Arc<Content>,
        map: Arc<Map>,
        ids: Arc<AtomicI32>,
    ) -> Result<Self> {
        Self::with_ids_context(content, map, ids, None)
    }
    fn with_ids_context(
        content: Arc<Content>,
        map: Arc<Map>,
        ids: Arc<AtomicI32>,
        domain: Option<WorldDomain>,
    ) -> Result<Self> {
        Self::xml_map_admission(&content, &map, domain)?;
        let mut xml = xml::Runtime::new(&content, domain, &map)?;
        let mut entities = entity_store::Entities::default();
        let mut scenery_templates = BTreeMap::new();
        let mut actor_ids = BTreeSet::new();
        let mut quests_by_level: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let mut merchants = BTreeMap::new();
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
                            last_step: Pos { x: 0., y: 0. },
                            phase: 0.,
                            orbit: None,
                            hp: o.hp,
                            size: o.size,
                            condition: if o.stasis_immune {
                                crate::condition::bit(crate::condition::STASIS_IMMUNE)
                            } else {
                                0
                            },
                            statuses: BTreeMap::new(),
                            items: o.equipment.clone(),
                            drop_slots: BTreeSet::new(),
                            drop_source: None,
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
                            activated: None,
                            parent: Default::default(),
                            camp: None,
                            population_slot: None,
                            quest: Default::default(),
                            expires: Default::default(),
                            next_aux: 0,
                            blocked_since: None,
                            reaction_until: 0,
                            bullet: 0,
                        })),
                    };
                    let settings = Self::xml_map_settings(&content, kind, &c.config)?;
                    if let Some(name) = settings.name {
                        e.name = name;
                    }
                    if let Some(size) = settings.size {
                        e.size = size;
                    }
                    e.condition |= settings.condition;
                    e.extra_stats.extend(settings.stats);
                    if o.class == "Merchant" {
                        merchants.insert(id, shop::configure(&mut e, &content)?);
                    }
                    let actor = Self::actor_kind(&content, kind);
                    cells.insert(y * map.width as usize + x, id, actor);
                    if actor {
                        actor_ids.insert(id);
                    }
                    if o.quest {
                        quests_by_level.entry(o.quest_level).or_default().insert(id);
                    }
                    if !actor && !o.destructible && o.class == "GameObject" {
                        e.share(&mut scenery_templates, &c.config);
                    }
                    entities.insert(id, e);
                }
            }
        }
        xml.runnable = actor_ids
            .iter()
            .copied()
            .filter(|id| {
                !xml::admission::terminal_only(
                    &content.game.as_ref().unwrap().definitions[&entities[id].kind],
                )
            })
            .collect();
        xml.awake = xml.runnable.clone();
        let portal_ids = actor_ids
            .iter()
            .copied()
            .filter(|id| Self::snapshot_portal(&content, entities[id].kind))
            .collect();
        let gun_ids = actor_ids
            .iter()
            .copied()
            .filter(|id| (0x802..=0x805).contains(&entities[id].kind))
            .collect();
        let mut world = Self {
            inventory_capacity: Arc::default(),
            xml,
            content,
            map,
            navigation_geometry: Box::default(),
            realm_layout: None,
            godlands: Arc::default(),
            godland_route: Arc::default(),
            godland_tiles: Arc::default(),
            timed_ids: BTreeSet::new(),
            children: BTreeMap::new(),
            quest_ids: BTreeSet::new(),
            gun_ids,
            portal_ids,
            entities,
            non_population_ids: actor_ids.clone(),
            actor_ids,
            quests_by_level,
            players: BTreeMap::new(),
            guild: None,
            bullets: Vec::new(),
            detached_bullets: 0,
            owed_shots: Vec::new(),
            ability_state: abilities::State::default(),
            companions: companions::State::default(),
            cells,
            merchants,
            rng: world_rng(),
            now: 0,
            last_npc_step: 0,
            performance: std::cell::RefCell::default(),
            realm_tick_mean: None,
            attack_epoch: Instant::now(),
            clock: crate::clock::Clock::default(),
            new_tick_at: Instant::now(),
            prepared_seed: None,
            tick: 0,
            ids,
            name: "Development tutorial".into(),
            background: 0,
            uses_breath: None,
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
            population_children: BTreeMap::new(),
            population_next: 10000,
            reproduction: BTreeMap::new(),
            summon_waits: BTreeSet::new(),
            reproduction_at: [0; 20],
            reproduction_bucket: 0,
            npc_paused_since: BTreeMap::new(),
            setpieces: vec![],
            ghost_towers: vec![],
            phoenix_pyres: vec![],
            event_sites: vec![],
            critical_deaths: vec![],
            critical_killers: BTreeMap::new(),
            event_reserves: vec![],
            event_waiting: None,
            event_choice: None,
            event_rng: world_rng(),
            admission_open: true,
            bag_expiry: BTreeMap::new(),
            cleared: false,
            clear_exit: None,
            vaults: BTreeMap::new(),
            closed_vaults: BTreeMap::new(),
            trades: BTreeMap::new(),
            trade_requests: BTreeMap::new(),
            recent_market_pairs: BTreeMap::new(),
        };
        world.xml_admit_all()?;
        Ok(world)
    }
    /// Players whose persistence is still in flight, named with the outstanding flags.
    pub fn unsettled(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for (id, p) in &self.players {
            ensure!(
                p.persistence_failure.is_none(),
                "character {} is quarantined",
                p.character.id
            );
            let flags = [
                ("pending", p.pending),
                (
                    "lifetime_uncommitted",
                    p.character.lifetime != p.committed_lifetime,
                ),
                ("terminal_captures", p.terminal_captures > 0),
                ("rewards_queued", p.ordinary_rewards_queued()),
                ("progress_started", p.progress_started),
                ("death_pending", p.death_pending),
                ("departing", p.departing),
            ];
            if flags.iter().any(|(_, set)| *set) {
                out.push(format!(
                    "world={} player={id} character={} {:?}",
                    self.name,
                    p.character.id,
                    flags
                        .iter()
                        .filter(|(_, set)| *set)
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                ));
            }
        }
        Ok(out)
    }
    pub(crate) fn restored_vault(content: Arc<Content>, owner: i32) -> Result<Self> {
        let mut maps = Directory::linked_static_maps(&content)?;
        let map = maps
            .remove("vault")
            .context("restored Vault lacks linked map")?;
        let mut world = Self::with_ids(content, map, Arc::new(AtomicI32::new(1000)))?;
        world.xml.template = Some("vault".into());
        world.name = "Vault".into();
        world.owner = Some(owner);
        Ok(world)
    }
    #[cfg(test)]
    pub(crate) fn set_test_template(&mut self, id: &str) {
        self.xml.template = Some(id.into());
    }
    pub fn load_vaults(&mut self, vaults: Vec<Vault>) -> Result<()> {
        self.xml_check_catalog()?;
        let account = self.owner.context("Vault world has no owner")?;
        ensure!(
            self.template_id() == Some("vault")
                && !vaults.is_empty()
                && vaults.len() <= crate::store::VAULT_CHESTS as usize
                && vaults.iter().all(|v| v.account == account
                    && (0..crate::store::VAULT_CHESTS).contains(&v.chest)
                    && v.items.len() == 8
                    && v.items.iter().all(|i| (-1..=65535).contains(i))
                    && v.revision >= 0),
            "invalid private vault state"
        );
        for kind in [0x504, 0x505] {
            self.xml_game().admission(
                kind,
                AdmissionRole::Native(NativeService::Container),
                "private vault load",
            )?;
        }
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
        ensure!(
            self.xml_live_count() + cells.len() <= xml::ACTOR_LIMIT,
            "private vault load exceeds native entity capacity: live={} requested={} limit={}",
            self.xml_live_count(),
            cells.len(),
            xml::ACTOR_LIMIT
        );
        for (chest, pos) in cells.iter().enumerate() {
            self.xml_preflight_native_entity(
                if by_chest.contains_key(&(chest as i32)) {
                    0x504
                } else {
                    0x505
                },
                NativeService::Container,
                *pos,
                "private vault load preflight",
            )?;
        }
        for (chest, pos) in cells.into_iter().enumerate() {
            let chest = chest as i32;
            if let Some(vault) = by_chest.get(&chest) {
                let id = self.insert_native_entity(
                    0x504,
                    NativeService::Container,
                    pos,
                    vault.items.clone(),
                    "loaded private vault chest",
                )?;
                self.entities
                    .get_mut(&id)
                    .unwrap()
                    .extra_stats
                    .push((54, account));
                self.vaults.insert(id, vault.clone());
            } else {
                let id = self.insert_native_entity(
                    0x505,
                    NativeService::Container,
                    pos,
                    Vec::new(),
                    "loaded private vault sale",
                )?;
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
        self.insert_entity(kind, pos, items)
    }
    fn insert_entity(&mut self, kind: u16, pos: Pos, items: Vec<i32>) -> Result<i32> {
        self.xml_raw_admission(kind)?;
        self.xml_preflight_admission(kind, pos)?;
        ensure!(
            self.xml_live_count() < xml::ACTOR_LIMIT,
            "raw entity capacity exhausted: live={} limit={} kind={kind:#x} world={}",
            self.xml_live_count(),
            xml::ACTOR_LIMIT,
            self.name
        );
        ensure!(
            self.xml_placeable(kind, pos, &[], None)?,
            "native entity admission geometry blocked"
        );
        let id = allocate_id(&self.ids)?;
        self.insert_entity_with_id(id, kind, pos, items)
    }
    fn insert_entity_with_id(
        &mut self,
        id: i32,
        kind: u16,
        pos: Pos,
        items: Vec<i32>,
    ) -> Result<i32> {
        self.xml_preflight_admission(kind, pos)?;
        self.insert_entity_data(id, kind, pos, items)
    }
    fn insert_entity_data(&mut self, id: i32, kind: u16, pos: Pos, items: Vec<i32>) -> Result<i32> {
        ensure!(
            pos.x.is_finite() && pos.y.is_finite() && self.map.cell(pos.x, pos.y).is_some(),
            "entity spawn outside map"
        );
        let definition = self.content.get(kind)?;
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
                    last_step: Pos { x: 0., y: 0. },
                    phase: 0.,
                    orbit: None,
                    hp: definition.hp,
                    size: definition.size,
                    condition: if definition.stasis_immune {
                        crate::condition::bit(crate::condition::STASIS_IMMUNE)
                    } else {
                        0
                    },
                    statuses: BTreeMap::new(),
                    items,
                    drop_slots: BTreeSet::new(),
                    drop_source: None,
                    extra_stats,
                    bag_owners: Vec::new(),
                    reserved: false,
                    published: true,
                    damage: BTreeMap::new(),
                    next_shot: 0,
                    activated: None,
                    parent: Default::default(),
                    camp: None,
                    population_slot: None,
                    quest: Default::default(),
                    expires: Default::default(),
                    next_aux: 0,
                    blocked_since: None,
                    reaction_until: 0,
                    bullet: 0,
                })),
            },
        );
        self.index_actor(id);
        self.xml_admit(id)?;
        if self.content.objects[&kind].blocks_sight {
            self.invalidate_sight();
        }
        Ok(id)
    }
    pub(super) fn set_parent(&mut self, id: i32, parent: Option<i32>) {
        let e = self.entities.get_mut(&id).expect("parented entity missing");
        let previous = *e.parent;
        e.parent.set(parent, id, &mut self.timed_ids);
        if let Some(previous) = previous {
            self.unlink_child(previous, id);
        }
        if let Some(parent) = parent {
            self.children.entry(parent).or_default().insert(id);
        }
    }
    fn unlink_child(&mut self, parent: i32, id: i32) {
        let children = self.children.get_mut(&parent).expect("child index missing");
        assert!(children.remove(&id), "indexed child missing");
        if children.is_empty() {
            self.children.remove(&parent);
        }
    }
    pub(super) fn child_entities(&self, parent: i32) -> impl Iterator<Item = &Entity> {
        self.children
            .get(&parent)
            .into_iter()
            .flatten()
            .map(|id| &self.entities[id])
    }
    fn index_actor(&mut self, id: i32) {
        let e = &self.entities[&id];
        let kind = e.kind;
        let cell = e.pos.y.floor() as usize * self.map.width as usize + e.pos.x.floor() as usize;
        let actor = Self::actor_kind(&self.content, kind);
        self.cells.insert(cell, id, actor);
        if e.parent.is_some() || e.expires.is_some() {
            self.timed_ids.insert(id);
        }
        if let Some(parent) = *e.parent {
            self.children.entry(parent).or_default().insert(id);
        }
        if e.quest.is_some() {
            self.quest_ids.insert(id);
        }
        if !actor {
            return;
        }
        self.actor_ids.insert(id);
        if !xml::admission::terminal_only(&self.xml_game().definitions[&e.kind]) {
            self.xml.runnable.insert(id);
            self.xml.awake.insert(id);
        }
        if Self::snapshot_portal(&self.content, kind) {
            self.portal_ids.insert(id);
        }
        if (0x802..=0x805).contains(&kind) {
            self.gun_ids.insert(id);
        }
        self.non_population_ids.insert(id);
        if let Some(level) = self.quest_target_level(e) {
            self.quests_by_level.entry(level).or_default().insert(id);
        }
    }
    #[cfg(test)]
    fn spawn_exit(&mut self, kind: u16, loot: Pos) -> Result<i32> {
        self.spawn_clear_portal(kind, loot, false)
    }
    #[cfg(test)]
    fn spawn_clear_portal(&mut self, kind: u16, loot: Pos, keep_origin: bool) -> Result<i32> {
        let pos = self.clear_portal_position(loot, keep_origin)?;
        self.insert_portal_entity(kind, pos, "validated clear/unlock portal factory")
    }
    /// Quest ownership changes an entity's quest-arrow level (Phoenix Egg), so the level index must follow it.
    pub(super) fn set_quest(&mut self, id: i32, quest: Option<i32>) {
        if let Some(level) = self.quest_target_level(&self.entities[&id])
            && let Some(ids) = self.quests_by_level.get_mut(&level)
        {
            ids.remove(&id);
            if ids.is_empty() {
                self.quests_by_level.remove(&level);
            }
        }
        let e = self.entities.get_mut(&id).expect("quest owner exists");
        e.quest.set(quest, id, &mut self.quest_ids);
        if let Some(level) = self.quest_target_level(&self.entities[&id]) {
            self.quests_by_level.entry(level).or_default().insert(id);
        }
    }
    fn clear_portal_position(&self, loot: Pos, keep_origin: bool) -> Result<Pos> {
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
        Ok(pos)
    }
    pub(crate) fn remove_entity(&mut self, id: i32) -> Entity {
        assert!(
            !self.entities[&id].reserved,
            "cannot remove a reserved container"
        );
        self.actor_ids.remove(&id);
        self.xml.runnable.remove(&id);
        self.xml.awake.remove(&id);
        self.portal_ids.remove(&id);
        self.timed_ids.remove(&id);
        self.quest_ids.remove(&id);
        self.gun_ids.remove(&id);
        self.non_population_ids.remove(&id);
        self.summon_waits.remove(&id);
        self.npc_paused_since.remove(&id);
        self.ability_state.objects.remove(&id);
        self.merchants.remove(&id);
        let entity = self.entities.remove(&id).expect("indexed entity missing");
        if let Some(parent) = *entity.parent {
            self.unlink_child(parent, id);
        }
        if entity.population_slot.is_some() {
            let entries = self
                .xml_game()
                .realm
                .as_ref()
                .and_then(|r| r.reproduction.get(&entity.kind))
                .map_or(0, Vec::len);
            for entry in 0..entries {
                self.reproduction.remove(&(id, entry));
            }
        }
        if let Some(level) = self.quest_target_level(&entity) {
            let ids = self
                .quests_by_level
                .get_mut(&level)
                .expect("quest index missing");
            assert!(ids.remove(&id), "indexed quest missing");
            if ids.is_empty() {
                self.quests_by_level.remove(&level);
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
        self.population_children.remove(&id);
        self.xml_forget(id);
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
        if self.content.objects[&entity.kind].blocks_sight {
            self.invalidate_sight();
        }
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
        let id = self.enter_at(
            ch,
            tx,
            Some(Pos { x, y }),
            AccountPresentation {
                guild: None,
                credits: 100,
                fame: 0,
                best_fame: BTreeMap::new(),
            },
            false,
        )?;
        self.players.get_mut(&id).unwrap().newbie_until = 0;
        Ok(id)
    }

    pub fn enter_at(
        &mut self,
        ch: Character,
        tx: Option<Output>,
        position: Option<Pos>,
        presentation: AccountPresentation,
        legendary: bool,
    ) -> Result<i32> {
        let controller = if tx.is_some() {
            Controller::Human
        } else {
            Controller::Qa
        };
        self.enter_admitted(ch, tx, position, presentation, legendary, None, controller)
    }
    pub(crate) fn reserved_slots(&self) -> usize {
        self.admissions
            .values()
            .filter(|a| a.expires > self.now)
            .count()
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn enter_admitted(
        &mut self,
        ch: Character,
        tx: Option<Output>,
        position: Option<Pos>,
        presentation: AccountPresentation,
        legendary: bool,
        admission: Option<Vec<u8>>,
        controller: Controller,
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
        self.xml_game().admission(
            ch.class,
            AdmissionRole::Native(NativeService::Player),
            "authenticated character entry",
        )?;
        if let Some(kind) = ch.pet_kind {
            self.content.permanent_companion(kind)?;
            self.xml_game().entity_admission(
                kind,
                AdmissionRole::Native(NativeService::Companion),
                "durable character companion entry",
            )?;
        }
        let mut draw = self.rng.clone();
        let pos = if let Some(pos) = position {
            pos
        } else {
            let &(x, y) = self
                .map
                .spawns
                .choose(&mut draw)
                .context("world has no spawn cells")?;
            Pos { x, y }
        };
        ensure!(
            self.walkable(pos),
            "character entry position is blocked or outside the world"
        );
        self.xml_preflight_position(ch.class, pos)?;
        if let Some(kind) = ch.pet_kind {
            self.preflight_companion(kind, pos)?;
        }
        let last_move_at = if controller == Controller::Human {
            self.clock
                .now()
                .checked_duration_since(self.attack_epoch)
                .context("native entry predates world epoch")?
                .as_millis() as u64
        } else {
            self.now
        };
        let char_id = ch.id;
        let id = allocate_id(&self.ids)?;
        if let Some(kind) = ch.pet_kind {
            self.attach_companion(id, &ch, kind, pos)?;
        }
        self.players.insert(
            id,
            Player {
                spectators: None,
                dungeon_entries: VecDeque::new(),
                weapon_ticks: VecDeque::from([(-1, ch.items[0], dex)]),
                native_lag: native_lag::Window::default(),
                audit_motion: crate::audit::MotionWindow::default(),
                audit_view: None,
                audit_revision: 0,
                committed_lifetime: ch.lifetime.clone(),
                character: ch,
                pos,
                departing: false,
                account_lists: AccountLists::default(),
                presentation,
                legendary,
                paused: false,
                native_update_acked: false,
                controller,
                tx,
                known: BTreeSet::new(),
                tick_stats: tick_stats::Cache::default(),
                quest: None,
                qualified_quest: None,
                statuses: BTreeMap::new(),
                lab_conditions: 0,
                xml_conditions: 0,
                known_since: BTreeMap::new(),
                shoot_acks: VecDeque::new(),
                visible_projectiles: BTreeMap::new(),
                overlaps: VecDeque::new(),
                predicted_deaths: BTreeMap::new(),
                ground_hits: BTreeMap::new(),
                newbie_until: self.now + 3000,
                bot_damage: VecDeque::new(),
                volley_margin_ms: Arc::default(),
                oxygen: if self.uses_breath.is_some() { 100 } else { -1 },
                tiles: BTreeSet::new(),
                tile_updates: vec![],
                tile_scan: None,
                sight_tiles: None,
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
                clears_queued: [0; ProgressKey::COUNT],
                progress_started: false,
                terminal_captures: 0,
                death_pending: false,
                persistence_failure: None,
                inputs: None,
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
                last_move_at,
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
        self.rng = draw;
        if let Some(key) = admission {
            self.admissions.remove(&key);
        }
        let mut o = Writer::new();
        o.i32(id).i32(char_id);
        send(&self.players[&id], wire::CREATE_SUCCESS, o.0);
        self.updates(id, true)?;
        if self.template_id() == Some("realm") {
            let welcome =
                chat::text_packet("#Oryx the Mad God", -1, 0, "You are food for my minions!");
            send(&self.players[&id], welcome.id, welcome.body);
        }
        tracing::info!(
            player = id,
            character = char_id,
            world = %self.name,
            x = pos.x,
            y = pos.y,
            "player entered development world"
        );
        let p = &self.players[&id];
        if p.controller == Controller::Human {
            crate::audit::record(
                "player_enter",
                serde_json::json!({"account":p.character.account,"character":char_id,"player":id,"world":self.name}),
            );
        }
        Ok(id)
    }

    pub fn leave(&mut self, id: i32) -> Result<Character> {
        self.clear_abilities(id);
        self.cancel_trade(id, "Trade cancelled by departure.");
        let p = self.players.get(&id).context("player not found")?;
        ensure!(
            !p.pending && !p.death_pending && p.terminal_captures == 0,
            "inventory, captured terminal or death operation pending"
        );
        self.clear_companions(id);
        let mut p = self.players.remove(&id).unwrap();
        self.xml_clear_player_effects(id, p.character.id);
        if p.controller == Controller::Human {
            crate::audit::flush_combat(true);
            p.audit_motion
                .flush(p.character.account, p.character.id, id, &self.name, true);
        }
        self.bullets.retain(|b| b.owner != id);
        tracing::info!(player = id, character = p.character.id, "player left");
        if p.controller == Controller::Human {
            crate::audit::record(
                "player_leave",
                serde_json::json!({"account":p.character.account,"character":p.character.id,"player":id,"world":self.name}),
            );
        }
        Ok(p.character)
    }
    pub(crate) fn audit_native<T>(
        &mut self,
        id: i32,
        operation: &str,
        request: serde_json::Value,
        work: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let before = self.players.get(&id).filter(|p| p.controller == Controller::Human).map(|p| {
            let slots = [("object", "slot"), ("to_object", "to_slot")].into_iter().filter_map(|(object_key, slot_key)| {
                let object = request[object_key].as_i64()? as i32;
                let slot = request[slot_key].as_u64()? as usize;
                let entity = self.entities.get(&object);
                let own = object == id;
                Some(serde_json::json!({"object":object,"slot":slot,"own_inventory":own,
                    "actual_item":if own {p.character.items.get(slot)} else {entity.and_then(|e|e.items.get(slot))},
                    "entity_exists":entity.is_some(),"container_kind":entity.map(|e|e.kind),
                    "reserved":entity.map(|e|e.reserved),"accessible":entity.map(|e|e.accessible(p.character.account)),
                    "distance":entity.map(|e|p.pos.distance(e.pos))}))
            }).collect::<Vec<_>>();
            serde_json::json!({"account":p.character.account,"character":p.character.id,"player":id,
                "world":self.name,"x":p.pos.x,"y":p.pos.y,"class":p.character.class,
                "hp":p.character.hp,"mp":p.character.mp,"equipment":&p.character.items[..4],
                "next_bullet":p.next_bullet,"last_attack":p.last_attack,"last_move":p.last_move,
                "advertised_weapons":p.weapon_ticks.iter().map(|(tick,weapon,dex)|serde_json::json!([tick,weapon,dex])).collect::<Vec<_>>(),
                "paused":p.paused,"pending":p.pending,"death_pending":p.death_pending,"slots":slots})
        });
        let result = work(self);
        if let Some(before) = before {
            let denominator = if matches!(operation, "move" | "shot") {
                32
            } else {
                1
            };
            let sampled = rand::random::<u8>().is_multiple_of(denominator);
            if result.is_err() || sampled {
                crate::audit::record(
                    "native_action",
                    serde_json::json!({"operation":operation,"request":request,"before":before,
                    "returned_ok":result.is_ok(),"error":result.as_ref().err().map(|e|format!("{e:#}").chars().take(512).collect::<String>()),
                    "healthy_sample_denominator":denominator}),
                );
            }
        }
        result
    }
    pub fn stats(&self, id: i32, recipient: i32) -> Result<Vec<u8>> {
        let mut o = Writer::new();
        if let Some(p) = self.players.get(&id) {
            let values = p.effective(&self.content, self.now)?;
            o.i32(id)
                .f32(p.pos.x)
                .f32(p.pos.y)
                .i16(46 + 4 * i16::from(id == recipient));
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
            // Reviewed cheaters look like unnamed guests to others; their own client keeps the truth so its name panel still quotes the paid rename price.
            stat(
                &mut o,
                56,
                i32::from(
                    crate::store::name_chosen(p.character.account, &p.character.name)
                        && (id == recipient || !self.content.reviewed(p.character.account)),
                ),
            );
            stat(&mut o, 57, p.character.fame);
            stat(
                &mut o,
                58,
                p.presentation
                    .next_quest(p.character.class, p.character.fame),
            );
            // The selected client treats every value except -1 as red: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Rotmg123.5.1/src/com/company/assembleegameclient/objects/Player.as#L785
            stat(&mut o, 59, if p.legendary { 1 } else { -1 });
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
        if let Some(p) = self.players.get(&id) {
            p.audit_fact(id,&self.name,self.now,"condition_applied",||serde_json::json!({"index":index,"since":self.now,"until":p.statuses[&index].1,"conditions":p.conditions(self.now)}));
        }
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
    fn quest_target_level(&self, entity: &Entity) -> Option<Option<i32>> {
        let kind = if entity.kind == 0x6c2 && entity.quest.is_some() {
            0x6c1
        } else {
            entity.kind
        };
        let definition = &self.content.objects[&kind];
        definition.quest.then_some(definition.quest_level)
    }
    fn quest_target(&self, id: i32) -> Option<i32> {
        if self.safe {
            return None;
        }
        let player = &self.players[&id];
        let level = player.character.level;
        let inputs = (player.controller == Controller::Autonomous)
            .then_some(player.inputs.as_ref())
            .flatten();
        let eligible = |e: &Entity| {
            let (min, max) = self.content.objects[&e.kind].quest_window;
            e.hp > 0
                && e.accessible(player.character.account)
                && (i32::from(min)..=i32::from(max)).contains(&level)
                && inputs
                    .and_then(|inputs| inputs.unreachable_targets.get(&e.id))
                    .is_none_or(|until| self.now >= *until)
        };
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
                if !eligible(e) {
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
        let nearest = best?;
        // A detour across the distance tie should not reassign a bot's chosen journey.
        if let Some(preferred) = inputs
            .and_then(|inputs| inputs.preferred_quest)
            .and_then(|id| self.entities.get(&id))
            && eligible(preferred)
            && self.quest_target_level(preferred).is_some()
            && preferred.pos.distance(player.pos) <= nearest.pos.distance(player.pos) + 24.
        {
            return Some(preferred.id);
        }
        Some(nearest.id)
    }
    /// Delivers to every ready player, bots included, but counts only humans.
    fn broadcast_text(&self, name: &str, text: &str) -> usize {
        let packet = chat::text_packet(name, -1, 0, text);
        let mut recipients = 0;
        for p in self
            .players
            .values()
            .filter(|p| !p.departing && p.persistence_failure.is_none())
        {
            send(p, packet.id, packet.body.clone());
            recipients += usize::from(p.controller == Controller::Human);
        }
        recipients
    }
    fn visible(&self, id: i32) -> BTreeSet<i32> {
        self.visible_with(id, self.quest_target(id))
    }
    fn entity_visible_to(e: &Entity, player: &Player) -> bool {
        e.accessible(player.character.account)
            && player
                .sight_tiles
                .as_ref()
                .is_none_or(|sight| sight.contains(&(e.pos.x as i16, e.pos.y as i16)))
    }
    fn visible_with(&self, id: i32, quest: Option<i32>) -> BTreeSet<i32> {
        let player = &self.players[&id];
        let pos = player.pos;
        self.entities_near(pos, PLAYER_VIEW_RADIUS)
            .chain(
                self.xml
                    .visible
                    .iter()
                    .filter_map(|id| self.entities.get(id)),
            )
            .filter(|e| Self::entity_visible_to(e, player))
            .map(|e| e.id)
            .chain(quest)
            .chain(
                self.players
                    .iter()
                    .filter(|(_, p)| p.pos.distance(pos) < PLAYER_VIEW_RADIUS)
                    .map(|(i, _)| *i),
            )
            .collect()
    }
    fn player_markers(&self, id: i32) -> impl Iterator<Item = (i32, &Player)> {
        self.players
            .iter()
            .filter(move |(peer, player)| {
                **peer != id
                    && self.players[&id].known.contains(peer)
                    && player.active()
                    && player.character.hp > 0
            })
            .map(|(peer, player)| (*peer, player))
    }
    pub fn updates(&mut self, id: i32, initial: bool) -> Result<()> {
        self.players
            .get_mut(&id)
            .context("audit state player missing")?
            .audit_state(id, &self.name, self.now, &self.content, "updates")?;
        let started = crate::performance::Measure::start();
        let pos = self.players[&id].pos;
        let center = (pos.x as i32, pos.y as i32);
        let rescan = initial || self.players[&id].tile_scan != Some(center);
        if rescan {
            // Selected bookshelves carry BlocksSight; keep unopened rooms undiscovered: https://github.com/OpenRotMG/Open-RotMG-Client/blob/4d171652c410fe4da26065b79441d1c0c2da9f94/client.swf
            let sight = self.room_sight_world().then(|| self.room_sight(center));
            if let Some(sight) = &sight {
                tracing::debug!(
                    player = id,
                    tiles = sight.len(),
                    world = %self.name,
                    "room sight recalculated"
                );
            }
            self.players.get_mut(&id).unwrap().sight_tiles = sight;
        }
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
        let mut tiles = Vec::new();
        for y in ((pos.y as i32 - 20).max(0))..=(pos.y as i32 + 20).min(self.map.height - 1) {
            for x in ((pos.x as i32 - 20).max(0))..=(pos.x as i32 + 20).min(self.map.width - 1) {
                let dx = x - pos.x as i32;
                let dy = y - pos.y as i32;
                if rescan
                    && dx * dx + dy * dy <= 20 * 20
                    && !self.players[&id].tiles.contains(&(x as i16, y as i16))
                    && self.players[&id]
                        .sight_tiles
                        .as_ref()
                        .is_none_or(|sight| sight.contains(&(x as i16, y as i16)))
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
        // The base notified before retargeting (79a4c92b:src/world.rs:6343). Hold even before
        // journal staging: progress backpressure can leave defeat captured but not yet staged.
        let quest = match self.players[&id].quest {
            Some(held) if self.xml_terminal_pending(held) && self.entities.contains_key(&held) => {
                Some(held)
            }
            _ => quest,
        };
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
            controller("non-finite movement")
        );
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.active(),
            controller("dead, departing or quarantined player cannot move")
        );
        ensure!(time >= 0, controller("negative movement time"));
        let elapsed = if p.last_move == 0 {
            100.
        } else {
            (i64::from(time) - i64::from(p.last_move)) as f32
        };
        ensure!(
            elapsed >= 0.,
            controller("movement time reversed").context(format!(
                "movement time reversed: client_time={time} previous_client_time={}",
                p.last_move
            ))
        );
        let cell = self
            .map
            .cell(p.pos.x, p.pos.y)
            .context("player outside map")?;
        let speed = p.speed(&self.content, self.now)? * self.content.grounds[&cell.ground];
        let push = if p.paused || p.conditions(self.now) & PARALYZED != 0 {
            (0., 0.)
        } else {
            self.content.pushes[&cell.ground]
        };
        let distance = p.pos.distance(pos);
        let input_distance = |delta| {
            p.pos.distance(Pos {
                x: pos.x - push.0 * delta,
                y: pos.y - push.1 * delta,
            })
        };
        ensure!(
            p.conditions(self.now) & PARALYZED == 0 || distance <= 0.001,
            controller("paralyzed movement")
        );
        ensure!(
            input_distance(elapsed) <= speed * elapsed + 0.3,
            controller("movement exceeds speed").context(format!(
                "movement exceeds speed: from=({}, {}) to=({}, {}) distance={distance} allowed={} speed={speed} client_elapsed_ms={elapsed}",
                p.pos.x, p.pos.y, pos.x, pos.y, speed * elapsed + 0.3
            ))
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
            controller("movement exceeds server time budget").context(format!(
                "movement exceeds server time budget: distance={distance} credit={credit} server_elapsed_ms={server_elapsed} client_elapsed_ms={elapsed} previous_credit={}",
                p.movement_credit
            ))
        );
        let traveled = self
            .player_travel_distance(p.pos, pos)
            .ok_or_else(|| controller("movement across a blocked tile"))?;
        let extra = (traveled - distance).max(0.);
        ensure!(
            input_distance(elapsed) + extra <= speed * elapsed + 0.3
                && charged + extra <= credit + 0.001,
            controller("collision slide exceeds movement budget")
        );
        Ok(credit - charged - extra)
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
        let pos = if sentinel { self.players[&id].pos } else { pos };
        self.validate_native_coordinates(id, pos, time)?;
        let p = self.players.get_mut(&id).unwrap();
        ensure!(received >= p.last_move_at, "native ingress clock regressed");
        // Rechecking client collision or speed here turns delayed packet delivery into snap-backs.
        let speed = p.speed(&self.content, self.now)? * 1000.;
        if p.controller == Controller::Human {
            let ground = self
                .map
                .cell(pos.x, pos.y)
                .expect("validated native position")
                .ground;
            let from_ground = self
                .map
                .cell(p.pos.x, p.pos.y)
                .expect("existing player position")
                .ground;
            let push = self.content.pushes[&ground];
            let from_push = self.content.pushes[&from_ground];
            let conditions = p.conditions(self.now);
            let epoch = p.audit_motion.epoch;
            p.audit_motion.observe(|| serde_json::json!({"client_ms":time,"received_ms":received,"world_ms":self.now,
                "from":[p.pos.x,p.pos.y],"position":[pos.x,pos.y],"speed":speed,"conditions":conditions,"ground":ground,
                "ground_speed":self.content.grounds[&ground],"ground_push":[push.0*1000.,push.1*1000.],
                "from_ground":from_ground,"from_ground_speed":self.content.grounds[&from_ground],"from_ground_push":[from_push.0*1000.,from_push.1*1000.],
                "epoch":epoch,"hp":p.character.hp}));
        }
        p.record_motion(pos, time, self.now, speed);
        if p.pos.distance(pos) > 0.001 {
            p.last_motion = Some(((pos.y - p.pos.y).atan2(pos.x - p.pos.x), self.now));
        }
        p.pos = pos;
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
            controller("movement time jump")
        );
        // MOVE precedes condition updates; accept either form across a toggle: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/kabam/rotmg/messaging/impl/GameServerConnection.as#L1182-L1194
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
    fn player_segment_valid(&self, from: Pos, to: Pos) -> bool {
        let samples = (from.distance(to) / 0.1).ceil().max(1.) as usize;
        (1..=samples).all(|i| {
            let f = i as f32 / samples as f32;
            self.player_step_valid(
                from,
                Pos {
                    x: from.x + (to.x - from.x) * f,
                    y: from.y + (to.y - from.y) * f,
                },
            )
        })
    }
    fn player_travel_distance(&self, from: Pos, to: Pos) -> Option<f32> {
        if self.player_segment_valid(from, to) {
            return Some(from.distance(to));
        }
        // MOVE endpoints can straddle a slide around a corner: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Rotmg123.5.1/src/com/company/assembleegameclient/objects/Player.as#L353-L423
        let (min_x, max_x) = (from.x.min(to.x), from.x.max(to.x));
        let (min_y, max_y) = (from.y.min(to.y), from.y.max(to.y));
        let mut best = None;
        for y in min_y.floor() as i32 - 1..=max_y.floor() as i32 + 1 {
            for x in min_x.floor() as i32 - 1..=max_x.floor() as i32 + 1 {
                let Some(cell) = self.map.cell(x as f32, y as f32) else {
                    continue;
                };
                let (blocked, full) = self
                    .entities_at(Pos {
                        x: x as f32 + 0.5,
                        y: y as f32 + 0.5,
                    })
                    .fold(
                        (self.content.grounds[&cell.ground] <= 0., cell.ground == 255),
                        |(blocked, full), e| {
                            let object = &self.content.objects[&e.kind];
                            (blocked || object.blocks, full || object.full_occupy)
                        },
                    );
                if !blocked && !full {
                    continue;
                }
                let (low, high) = if full { (-0.5, 1.5) } else { (-0.01, 1.) };
                for (dx, dy) in [(low, low), (low, high), (high, low), (high, high)] {
                    let corner = Pos {
                        x: x as f32 + dx,
                        y: y as f32 + dy,
                    };
                    if corner.x < min_x || corner.x > max_x || corner.y < min_y || corner.y > max_y
                    {
                        continue;
                    }
                    let distance = from.distance(corner) + corner.distance(to);
                    if best.is_none_or(|best| distance < best)
                        && self.player_segment_valid(from, corner)
                        && self.player_segment_valid(corner, to)
                    {
                        best = Some(distance);
                    }
                }
            }
        }
        best
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
    #[cfg(test)]
    pub(crate) fn regress_native_ingress_for_test(&mut self, id: i32) -> Pos {
        self.players.get_mut(&id).unwrap().last_move_at = u64::MAX;
        self.accept_native_move_for_test(id)
    }
    #[cfg(test)]
    pub(crate) fn set_admission_open_for_test(&mut self, open: bool) {
        self.admission_open = open;
    }
    #[cfg(test)]
    pub(crate) fn accept_native_move_for_test(&mut self, id: i32) -> Pos {
        let p = self.players.get_mut(&id).unwrap();
        p.tick = p.tick.max(1);
        p.pos
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
    fn snapshot_portal(content: &Content, kind: u16) -> bool {
        content.objects[&kind].is_portal()
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
        self.actors_in_sections(pos, radius)
            .filter(move |e| e.pos.distance(pos) < radius)
    }
    fn actors_in_sections(&self, pos: Pos, radius: f32) -> impl Iterator<Item = &Entity> {
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
        (y0..=y1)
            .flat_map(move |y| {
                (x0..=x1)
                    .flat_map(move |x| self.cells.ids((y * self.map.width + x) as usize).iter())
            })
            .map(move |id| self.entities.get(id).expect("occupied cell missing entity"))
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
        self.xml_check_catalog()?;
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
        self.xml_game().admission(
            kind,
            AdmissionRole::Native(NativeService::Inventory),
            "actual equipped weapon projectile",
        )?;
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
            // Terrain checks undo client prediction and recreate MOVE snap-backs: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/networking/handlers/PlayerShootPacketHandler.cs
            self.validate_native_coordinates(id, center, time)
                .map(|_| 0.)
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
        let normal_cooldown = p.normal_attack_period(dex, self.now, item.rate, native);
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
            p.normal_attack_period(admission_dex, self.now, item.rate, true)
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
            damage_origin: enemy_damage::DamageOrigin {
                context: crate::behavior::TerminalContext::PlayerKill {
                    world: self.xml.world,
                    player: id,
                    name: p.character.name.clone(),
                },
                character: Some(p.character.id),
            },
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
        if shot_index == 0 {
            // Aim error to the best-aligned enemy separates autoaim from human aim; origin vs self exposes magnet aim.
            let error = |e: &Entity| {
                ((e.pos.y - start.y).atan2(e.pos.x - start.x) - aim + std::f32::consts::PI)
                    .rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI
            };
            let target = self
                .actors_near(start, 15.)
                .filter(|e| self.content.objects[&e.kind].enemy)
                .min_by(|a, b| error(a).abs().total_cmp(&error(b).abs()))
                .map(|e| serde_json::json!([e.id, e.kind, e.pos.x, e.pos.y, error(e)]));
            let p = &self.players[&id];
            p.audit_fact(id, &self.name, self.now, "shot_aim", || {
                serde_json::json!({"client_ms":time,"bullet":bullet,
                "aim":aim,"origin":[start.x,start.y],"self":[p.pos.x,p.pos.y],"target":target})
            });
        }
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
        self.xml_check_catalog()?;
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
            self.xml_container_admission(bag.kind, "owned item source container")?;
            &bag.items
        };
        ensure!(
            source.get(slot.slot) == Some(&slot.kind),
            controller("item ownership mismatch")
        );
        self.check_entitlement_slot(id, slot)?;
        ensure!(slot.kind >= 0, controller("empty item slot"));
        let item = self.content.get(slot.kind as u16)?.clone();
        // Saving earned XP must not freeze abilities; inventory mutations remain serialized.
        ensure!(
            p.active()
                && p.character.hp > 0
                && (!p.pending || p.progress_started && !item.abilities.is_empty()),
            controller("player is busy, departing, quarantined or dead")
        );
        self.xml_game().admission(
            item.kind,
            AdmissionRole::Native(NativeService::Inventory),
            "owned item activation",
        )?;
        let p = &self.players[&id];
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
        let mut plan = self.plan(id, self.players[&id].character.items.clone(), None)?;
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
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
        for (bag, _) in &plan.bags {
            self.entities.get_mut(bag).unwrap().reserved = true;
        }
        self.players.get_mut(&id).unwrap().pending = true;
        Ok(plan)
    }
    fn plan(
        &self,
        id: i32,
        items: Vec<i32>,
        bag: Option<(i32, Vec<i32>)>,
    ) -> Result<InventoryPlan> {
        let p = &self.players[&id];
        self.inventory_capacity.fetch_update(Ordering::AcqRel,Ordering::Acquire,|count|(count<128).then_some(count+1)).map_err(|_|anyhow::anyhow!("inventory journal admission backpressure:128 retained native plans; original events require reconciliation"))?;
        let permit = Arc::new(InventoryPermit {
            count: self.inventory_capacity.clone(),
            event: crate::store::world_events::new_id(),
        });
        Ok(InventoryPlan {
            accepted_at: self.now,
            permit,
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
            entitlement_move: false,
        })
    }
    pub fn drop_item(&mut self, id: i32, slot: Slot) -> Result<InventoryPlan> {
        self.xml_check_catalog()?;
        self.check_entitlement_slot(id, slot)?;
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
            self.xml_container_admission(e.kind, "owned drop source container")?;
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
        let mut plan = self.plan(id, items, source)?;
        self.bind_vaults(&mut plan)?;
        let private = self
            .xml
            .bag_entitlements
            .get(&(slot.object, slot.slot))
            .is_some_and(|(_, _, owner, _)| {
                matches!(owner, crate::loot::evaluator::Owner::Character(_))
            });
        let dropped = self.reserve_drop(pos, account, slot.kind, private)?;
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
        for (source, _) in &plan.bags {
            self.entities.get_mut(source).unwrap().reserved = true;
        }
        self.players.get_mut(&id).unwrap().pending = true;
        if slot.object != id {
            plan.loot_slot = Some((slot.object, slot.slot));
            plan.entitlement_move = true;
        }
        plan.dropped = Some(dropped);
        Ok(plan)
    }
    fn reserve_drop(&mut self, pos: Pos, account: i32, kind: i32, private: bool) -> Result<i32> {
        let soulbound = self.content.get(u16::try_from(kind)?)?.soulbound;
        self.xml_preflight_native_entity(
            if soulbound { 0x503 } else { 0x500 },
            NativeService::Container,
            pos,
            "actual dropped item bag appearance",
        )?;
        let dropped = self.loot_bag(pos, (soulbound || private).then_some(account), vec![kind])?;
        let bag = self
            .entities
            .set_kind(dropped, if soulbound { 0x503 } else { 0x500 });
        bag.published = false;
        bag.reserved = true;
        Ok(dropped)
    }
    pub fn swap(&mut self, id: i32, from: Slot, to: Slot) -> Result<InventoryPlan> {
        self.xml_check_catalog()?;
        self.check_entitlement_slot(id, from)?;
        self.check_entitlement_slot(id, to)?;
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
            self.xml_container_admission(e.kind, "owned swap source container")?;
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
        for kind in [a, b].into_iter().filter(|kind| *kind >= 0) {
            self.xml_game().admission(
                u16::try_from(kind)?,
                AdmissionRole::Native(NativeService::Inventory),
                "actual owned inventory swap",
            )?;
        }
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
        let drop_request = if let Some(kind) = replacement {
            // A swap must not put Soulbound gear in public loot or violate the source bag's put flags.
            let source = if from.object == id { to } else { from };
            bags.get_mut(&source.object).unwrap()[source.slot] = -1;
            let private = self.entities[&source.object]
                .extra_stats
                .contains(&(54, p.character.account));
            Some((p.pos, p.character.account, kind, private))
        } else {
            None
        };
        let mut plan = self.plan(id, items, None)?;
        plan.bags = bags.into_iter().collect();
        self.bind_vaults(&mut plan)?;
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
                    plan.collected = Some(crate::store::CollectedItem {
                        item: source.kind as u16,
                        source: self.entities[container]
                            .drop_source
                            .clone()
                            .context("generated loot lacks its original source")?,
                    });
                }
            }
        }
        if let Some((pos, account, kind, private)) = drop_request {
            plan.dropped = Some(self.reserve_drop(pos, account, kind, private)?);
        }
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
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
        let ground_dye = plan.ground_dye.as_ref().map(|source| source.bag_id());
        let result = self.publish_inventory(plan, result);
        if let Err(error) = &result {
            if let Some(id) = ground_dye
                && let Some(bag) = self.entities.get_mut(&id)
            {
                // An error may conceal COMMIT; retain even a damaged reservation, never refund the source.
                bag.reserved = true;
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
        if plan.entitlement_move {
            let (bag, slot) = plan
                .loot_slot
                .context("ground entitlement move lacks source")?;
            let dropped = plan
                .dropped
                .context("ground entitlement move lacks destination")?;
            ensure!(
                self.entities[&bag].items.get(slot) == self.entities[&dropped].items.first(),
                "ground entitlement move changed allocated item"
            );
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
        p.pending = false;
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
        let hp_before = p.character.hp;
        p.character.hp = p.character.hp.min(values[0]);
        p.audit_hp(
            plan.player,
            &self.name,
            self.now,
            "equipment_cap",
            hp_before,
        );
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
            if plan.entitlement_move
                && let Some(source) = plan.loot_slot
                && let Some((event, ordinal, owner, _)) = self.xml.bag_entitlements.remove(&source)
            {
                self.xml
                    .bag_entitlements
                    .insert((dropped, 0), (event, ordinal, owner, self.now + 60_000));
            }
        }
        if !plan.entitlement_move
            && let Some(source) = plan.loot_slot
        {
            self.xml.bag_entitlements.remove(&source);
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
                p.inputs = None;
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
        p.audit_fact(id,&self.name,self.now,"correction",||serde_json::json!({"reason":reason.chars().take(512).collect::<String>(),"client_ms":time,"destination":[p.pos.x,p.pos.y],"epoch":p.audit_motion.epoch+1}));
        reconciliation::goto(p, true, out.0);
        p.native_lag.correct(reason);
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
                || (p.terminal_captures > 0 && !p.ordinary_rewards_queued())
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
            p.clears_queued = [0; ProgressKey::COUNT];
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
        let goal = p
            .presentation
            .next_quest(p.character.class, p.character.fame);
        let class_quest_completed = goal > 0 && snapshot.fame >= goal;
        let position = p.pos;
        let leveled = snapshot.level > p.character.level;
        let announcement = (leveled && snapshot.level == 20)
            .then(|| format!("{} achieved level 20", p.character.name));
        p.character.kills = p.character.kills.max(snapshot.kills);
        p.character.quests = p.character.quests.max(snapshot.quests);
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
            let hp_before = p.character.hp;
            p.character.hp = values[0];
            p.audit_hp(id, &self.name, self.now, "level_up", hp_before);
            p.character.mp = values[1];
            p.hp_fraction = 0.;
            p.mp_fraction = 0.;
        }
        p.pending = false;
        p.progress_started = false;
        p.progress_snapshot = None;
        p.level_assists_prepared.clear();
        // Bot commits were 40% of journal volume and pushed lag evidence out of retention.
        if p.controller != Controller::Autonomous {
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
        }
        // Compare the prior goal so the terminal quest also completes: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/realm/entities/player/Player.Leveling.cs#L174-L184
        if class_quest_completed {
            let mut out = Writer::new();
            out.i32(id).utf("Class Quest Complete!").u32(0xff00ff00);
            for (observer, p) in &self.players {
                if (*observer == id || p.known.contains(&id)) && p.pos.distance(position) < 25. {
                    send(p, wire::NOTIFICATION, out.0.clone());
                }
            }
            tracing::debug!(
                player = id,
                goal,
                fame = snapshot.fame,
                "class quest completed"
            );
        }
        // Period wording, published only after durable progress: https://media.gdcvault.com/gdceurope2012/Presentations/Social%20%26%20Online%20Games%20Summit/Ethan_Levy_Social%26Online_GameDesignIs.pdf#page=54
        if let Some(text) = announcement {
            let recipients = self.broadcast_text("", &text);
            tracing::info!(player = id, recipients, "level twenty announced");
        }
        if cleared_dungeon && let Some(exit) = self.clear_exit.take() {
            if let Some(retire) = self
                .xml_world_template()
                .and_then(|world| world.clear.as_ref()?.retire)
            {
                let old_exits: Vec<_> = self
                    .entities
                    .values()
                    .filter(|e| e.kind == retire)
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
        // Maxed graves last until reset, unlike later ten-minute rules: https://web.archive.org/web/20120606073802/http://forums.wildshadow.com/wiki/realm-mad-god/gravestones
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
        self.xml_clear_player_effects(id, self.players[&id].character.id);
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
    pub fn inspect_map(&self, player: Option<i32>) -> Result<Vec<u8>> {
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
        // A full Realm's JSON value tree dwarfs the actual map and pins allocator memory.
        #[derive(Serialize)]
        struct Snapshot<'a> {
            width: i32,
            height: i32,
            blocked: Vec<(i32, i32)>,
            entities: Vec<&'a Entity>,
        }
        serde_json::to_vec(&Snapshot {
            width: self.map.width,
            height: self.map.height,
            blocked,
            entities,
        })
        .context("serialize development world map")
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
            serde_json::json!({"tick":self.tick,"time_ms":self.now,"player":id,"pos":p.pos,"character":p.character,"entities":entities,"xml":entities.iter().filter_map(|e|self.xml_diagnostics(e.id).map(|d|(e.id.to_string(),d))).collect::<BTreeMap<_,_>>(),"players":others,"bullets":self.bullets.len(),"own_projectiles":self.bullets.iter().filter(|b| b.friendly && b.owner == id).map(|b| serde_json::json!({"id":b.id,"start":b.start,"pos":b.pos,"angle":b.angle,"born":b.born})).collect::<Vec<_>>(),"pending":p.pending,"oxygen":p.oxygen}),
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
            let kind = u16::try_from(*item).context("loot item outside selected identity range")?;
            self.xml_game().admission(
                kind,
                AdmissionRole::Native(NativeService::Inventory),
                "actual loot container item",
            )?;
            let object = self.content.get(kind)?;
            appearance = appearance.max(object.bag_type);
            ensure!(
                owner.is_some() || !object.soulbound,
                "public loot cannot contain soulbound equipment"
            );
        }
        let kind = *LOOT_BAGS
            .get(usize::from(appearance))
            .context("unsupported loot bag appearance")?;
        ensure!(
            items.len() <= 8,
            "loot exceeds bag capacity; split rewards before packaging"
        );
        items.resize(8, -1);
        let id = self.insert_native_entity(
            kind,
            NativeService::Container,
            pos,
            items,
            "bounded loot bag ownership factory",
        )?;
        let bag = self.entities.get_mut(&id).unwrap();
        bag.size = 80;
        if let Some(owner) = owner {
            bag.extra_stats.push((54, owner));
        }
        self.bag_expiry.insert(id, self.now + 60_000);
        tracing::info!(entity = id, owner, kind, "loot bag allocated");
        Ok(id)
    }
    #[cfg(test)]
    pub(crate) fn defeat_entity(&mut self, id: i32) -> Result<()> {
        let kind = self
            .entities
            .get(&id)
            .context("terminal actor absent")?
            .kind;
        self.xml_declared_admission(kind, "terminal lifecycle")?;
        self.xml_defeated(id, crate::behavior::TerminalContext::NonPlayer)
    }

    fn position_entity(&mut self, id: i32, pos: Pos) -> Result<()> {
        ensure!(
            pos.x.is_finite() && pos.y.is_finite() && self.map.cell(pos.x, pos.y).is_some(),
            "entity position outside world"
        );
        let old = self.entities[&id].pos;
        let cell = |p: Pos| p.y as usize * self.map.width as usize + p.x as usize;
        if cell(old) != cell(pos) {
            assert!(
                self.cells.remove(cell(old), id),
                "moving NPC missing from spatial index"
            );
            self.cells
                .insert(cell(pos), id, self.actor_ids.contains(&id));
        }
        let e = self.entities.get_mut(&id).unwrap();
        e.pos = pos;
        e.last_step = Pos {
            x: pos.x - old.x,
            y: pos.y - old.y,
        };
        Ok(())
    }
    /// Native timed scenery (gravestones) expires here; XML actors own their lifetime through XML Decay.
    fn native_expiry(&mut self) {
        let expired: Vec<_> =
            self.timed_ids
                .iter()
                .copied()
                .filter(|id| {
                    let e = &self.entities[id];
                    e.expires.is_some_and(|at| at <= self.now)
                        && !e.reserved
                        && !self.xml_terminal_custody(*id)
                        && !self.bag_expiry.contains_key(id)
                        && self.xml_game().definitions.get(&e.kind).is_none_or(|d| {
                            d.disposition == crate::content::game::Disposition::Native
                        })
                })
                .collect();
        for id in expired {
            tracing::debug!(
                entity = id,
                kind = self.entities[&id].kind,
                "native timed entity expired"
            );
            self.remove_entity(id);
        }
    }
    fn npc_step(&mut self) {
        if let Err(error) = self.xml_step() {
            // Quarantined actors were already logged and retired; anything else is a world invariant.
            assert!(
                error.is::<xml::ActorsQuarantined>(),
                "validated XML world execution: {error:?}"
            );
        }
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
                damage_origin: enemy_damage::DamageOrigin::nonplayer(),
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
        enemy_shots::packets(id, &notices)
    }
    pub fn step(&mut self) {
        let started = crate::performance::Measure::start();
        let players_started = crate::performance::Measure::start();
        self.now += 50;
        self.admissions.retain(|_, a| a.expires > self.now);
        self.tick += 1;
        crate::audit::flush_combat(false);
        if self.tick.is_multiple_of(100) {
            for (id, p) in &mut self.players {
                if p.controller == Controller::Human {
                    p.audit_motion.flush(
                        p.character.account,
                        p.character.id,
                        *id,
                        &self.name,
                        false,
                    );
                }
            }
        }
        self.merchant_step();
        self.expire_consumable_boosts();
        self.companion_step()
            .expect("validated companion simulation");
        self.trade_step();
        self.expire_goto_acks();
        self.expire_native_projectiles();
        let ids: Vec<_> = self.players.keys().copied().collect();
        for id in &ids {
            let p = self.players.get_mut(id).unwrap();
            if p.controller == Controller::Human {
                for (index, (since, until)) in &p.statuses {
                    if *until > self.now - 50 && *until <= self.now {
                        p.audit_fact(*id,&self.name,self.now,"condition_expired",||serde_json::json!({"index":index,"since":since,"until":until,"conditions":p.conditions(self.now)}));
                    }
                }
                p.audit_state(*id, &self.name, self.now, &self.content, "world_step")
                    .expect("valid audited player state");
            }
            self.step_bot_inputs(*id);
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
                // Period seals add 28 HP/s, not 20: https://web.archive.org/web/20120418103926id_/http://forums.wildshadow.com/wiki/realm-mad-god/class-guide-paladin
                28.
            } else {
                0.
            };
            let hp_before = p.character.hp;
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
                let before_bleed = p.character.hp;
                p.character.hp = (p.character.hp - 1).max(1);
                p.record_bot_damage(self.now, before_bleed);
            }
            p.audit_hp(
                *id,
                &self.name,
                self.now,
                "regeneration_bleeding",
                hp_before,
            );
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
        self.ground_step();
        self.breath_step();
        if let Err(error) = self.ability_step() {
            tracing::error!(error=%format!("{error:#}"), "native ability deadline failed");
        }
        self.owed_shot_step();
        self.performance
            .borrow_mut()
            .work("mechanics", mechanics_started);
        let npc_started = crate::performance::Measure::start();
        self.npc_step();
        self.native_expiry();
        self.performance.borrow_mut().work("npc", npc_started);
        let combat_started = crate::performance::Measure::start();
        for id in &ids {
            if let Err(e) = self.updates(*id, false) {
                tracing::error!(player=id,error=%e,"world update failed");
            }
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
                        .filter(|e| self.hittable(b, e) && inside_box(e.pos, b.pos))
                        .map(|e| e.id)
                        .collect();
                    if let Some(target) = targets.iter().min_by(|a, b_id| {
                        self.entities[a]
                            .pos
                            .distance(b.pos)
                            .total_cmp(&self.entities[b_id].pos.distance(b.pos))
                    }) && self.bullet_hit(b, *target)
                    {
                        break;
                    }
                } else if !self.safe {
                    for id in &ids {
                        let p = &self.players[id];
                        if p.character.alive
                            && !p.paused
                            && !p.departing
                            && !p.death_pending
                            && p.conditions(self.now)
                                & (crate::condition::bit(crate::condition::STASIS)
                                    | crate::condition::bit(crate::condition::INVINCIBLE))
                                == 0
                            && !b.hit.contains(id)
                            && inside_box(p.pos, b.pos)
                        {
                            if p.controller == Controller::Human {
                                self.observe_overlap(*id, b)
                                    .expect("validated projectile overlap");
                                continue;
                            }
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
                let p = &self.players[&id];
                let watched = p.controller != Controller::Autonomous
                    || p.spectators
                        .as_ref()
                        .is_some_and(|s| s.receiver_count() > 0);
                // Unwatched bots refreshed visibility after combat this tick; a second pass was ~1/3 of bot visibility CPU.
                if watched && let Err(e) = self.updates(id, false) {
                    tracing::error!(player=id,error=%e,"visibility update failed");
                    continue;
                }
                let p = &self.players[&id];
                if watched {
                    let dynamic: Vec<_> = p
                        .known
                        .iter()
                        .filter(|entity| {
                            self.players.contains_key(entity)
                                || self.companions.contains(entity)
                                || self.entities.get(entity).is_some_and(|e| {
                                    e.hp > 0
                                        || !e.items.is_empty()
                                        || matches!(
                                            self.content.objects[&e.kind].class.as_str(),
                                            "Portal" | "Merchant"
                                        )
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
                } else {
                    // Bot runtimes ignore NEW_TICK; after skipping, the first spectator tick must carry every stat.
                    self.players.get_mut(&id).unwrap().tick_stats.forget();
                }
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
    fn report_performance(&mut self, world_id: i32) {
        if let Some(timings) = self.performance.get_mut().report() {
            if self.template_id() == Some("realm") {
                let mean_us = timings["timings"]["step"]["mean_us"]
                    .as_u64()
                    .expect("Realm tick report missing step mean");
                self.realm_tick_mean = Some((Instant::now(), mean_us));
            }
            tracing::info!(world_id,world=%self.name, entities=self.entities.len(), actors=self.actor_ids.len(),
                ordinary_children=self.population_children.len(),bullets=self.bullets.len(),compact_entities=self.entities.shared(),scenery_promotions=self.entities.promotions,spatial_cells=self.cells.len(),known_entities=self.players.values().map(|p|p.known.len()).sum::<usize>(),
                native_lag=%serde_json::json!(self.players.iter().filter(|(_,p)|p.controller==Controller::Human).map(|(id,p)|serde_json::json!({"player":id,"metrics":p.native_lag.snapshot(Instant::now(),false)})).collect::<Vec<_>>()),
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
            let (humans, bots) = (
                self.players
                    .values()
                    .filter(|p| p.controller == Controller::Human)
                    .count(),
                self.players
                    .values()
                    .filter(|p| p.controller == Controller::Autonomous)
                    .count(),
            );
            if humans + bots > 0 {
                let pick = |group: &str, phase: &str| {
                    let t = &timings[group][phase];
                    serde_json::json!({"count":t["count"],"p95_us":t["p95_us"],"max_us":t["max_us"]})
                };
                let summary = serde_json::json!({"step_hz":timings["achieved_step_hz"],"lateness_over_50ms":timings["missed_tick_deadlines"],
                    "missed_by_cause":timings["counters"].as_object().map(|c|c.iter().filter(|(k,_)|k.starts_with("missed_")).map(|(k,v)|(k.clone(),v.clone())).collect::<serde_json::Map<_,_>>()),
                    "step_wall":pick("timings","step"),"step_cpu":pick("thread_cpu","step"),"cpu_wait":pick("timings","cpu_wait"),"lateness":pick("timings","scheduling_lateness")});
                tracing::info!(target:"lag",world_id,world=%self.name,humans,bots,entities=self.entities.len(),summary=%summary,"world tick summary");
            }
        }
    }
}
impl World {
    pub(super) fn hittable(&self, b: &Bullet, e: &Entity) -> bool {
        e.hp > 0
            && e.conditions(self.now)
                & (crate::condition::bit(crate::condition::STASIS)
                    | crate::condition::bit(crate::condition::INVINCIBLE))
                == 0
            && self.content.objects[&e.kind].destructible
            && !b.hit.contains(&e.id)
    }
    /// A stop flag keeps a custody-blocked projectile available without claiming a hit.
    pub(super) fn bullet_hit(&mut self, b: &mut Bullet, target: i32) -> bool {
        if self.players.get(&b.owner).is_some_and(|p| p.paused) {
            b.hit.insert(target);
            return true;
        }
        if let Err(error) = self.reserve_terminal_damage(target, b.damage, b.spec.armor_piercing) {
            tracing::warn!(player=b.owner,actor=target,%error,"terminal backpressure retains unspent projectile hit");
            return true;
        }
        b.hit.insert(target);
        if !b.hit_enemy && self.content.objects[&self.entities[&target].kind].enemy {
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
        self.damage_enemy_from(
            &b.damage_origin,
            target,
            b.damage,
            b.spec.armor_piercing,
            &b.spec.effects,
            Some(b.id),
        )
        .expect("validated enemy damage");
        false
    }
}

fn inside_box(a: Pos, b: Pos) -> bool {
    (a.x - b.x).abs() <= 0.5 && (a.y - b.y).abs() <= 0.5
}

#[cfg(test)]
mod legends_glow_tests;
#[cfg(test)]
mod reviewed_name_tests;
#[cfg(test)]
mod shop_tests;

#[cfg(test)]
mod cheat_evidence_tests;
#[cfg(test)]
mod combat_tests;
#[cfg(test)]
mod cooperative_tests;
#[cfg(test)]
mod movement_tests;

#[cfg(test)]
mod death_chat_tests;

#[cfg(test)]
mod reward_accuracy_tests;

#[cfg(test)]
mod paladin_tests;
#[cfg(test)]
mod wizard_tests;

#[cfg(test)]
mod necromancer_tests;
#[cfg(test)]
mod priest_tests;

#[cfg(test)]
mod chamber_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod travel_tests;

#[cfg(test)]
mod knight_tests;

#[cfg(test)]
mod assassin_tests;

#[cfg(test)]
mod mystic_tests;

#[cfg(test)]
mod class_quest_tests;
#[cfg(test)]
mod ghost_ship_tests;
#[cfg(test)]
mod quest_tests;

#[cfg(test)]
mod map_tests;

fn godland_cells(map: &Map, content: &Content) -> BTreeMap<(i32, i32), Vec<Pos>> {
    let mut squares: BTreeMap<(i32, i32), Vec<Pos>> = BTreeMap::new();
    for (i, cell) in map.cells.iter().enumerate() {
        if cell.terrain == 1
            && content.grounds[&cell.ground] > 0.
            && !content.hazards.contains_key(&cell.ground)
        {
            let (x, y) = (i as i32 % map.width, i as i32 / map.width);
            let pos = Pos {
                x: x as f32 + 0.5,
                y: y as f32 + 0.5,
            };
            squares.entry((x / 24, y / 24)).or_default().push(pos);
        }
    }
    squares
}

fn godland_spots(squares: &BTreeMap<(i32, i32), Vec<Pos>>) -> Vec<(usize, Pos)> {
    let mut spots: Vec<_> = squares
        .values()
        .filter(|cells| cells.len() >= 150)
        .map(|cells| {
            let n = cells.len() as f32;
            let middle = Pos {
                x: cells.iter().map(|p| p.x).sum::<f32>() / n,
                y: cells.iter().map(|p| p.y).sum::<f32>() / n,
            };
            let spot = *cells
                .iter()
                .min_by(|a, b| a.distance(middle).total_cmp(&b.distance(middle)))
                .expect("a counted square has cells");
            (cells.len(), spot)
        })
        .collect();
    spots.sort_by(|a, b| b.0.cmp(&a.0));
    spots
}

fn godlands(spots: &[(usize, Pos)], spawns: &[(f32, f32)]) -> Vec<Pos> {
    let mut seen = BTreeSet::new();
    // The largest squares cluster; otherwise a veteran can walk past a closer entrance to the same band.
    spawns
        .iter()
        .filter_map(|&(x, y)| {
            let spawn = Pos { x, y };
            spots
                .iter()
                .min_by(|(_, p), (_, q)| p.distance(spawn).total_cmp(&q.distance(spawn)))
        })
        .chain(spots)
        .filter_map(|(_, p)| seen.insert((p.x as i16, p.y as i16)).then_some(*p))
        .take(crate::bots::REMEMBERED_GODLANDS)
        .collect()
}

#[cfg(test)]
mod prepared_native_transition_tests {
    use super::*;

    struct Fixture {
        d: Directory,
        store: crate::store::Store,
        ch: Character,
        player: i32,
        _inbox: mpsc::Receiver<Packet>,
    }
    impl Fixture {
        async fn new() -> Self {
            let content = Arc::new(test_content());
            let store =
                crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
            let account = store
                .list("prepared-native-custody-unit".into())
                .await
                .unwrap();
            let ch = store
                .create(account.id, content.get(crate::content::WIZARD).unwrap())
                .await
                .unwrap();
            let mut d = Directory::fixture(content, 16, 1344, Some(1344), 2).unwrap();
            let (packets, mut inbox) = mpsc::channel(256);
            let (lost, _) = watch::channel(None);
            let player = d
                .worlds
                .get_mut(&-3)
                .unwrap()
                .enter(ch.clone(), Some(Output { packets, lost }))
                .unwrap();
            while inbox.try_recv().is_ok() {}
            Self {
                d,
                store,
                ch,
                player,
                _inbox: inbox,
            }
        }
        fn phase(&self) -> Option<String> {
            self.d
                .prepared_native_transition(self.ch.id)
                .unwrap()
                .map(|t| {
                    serde_json::to_value(t).unwrap()["phase"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
        }
        async fn depart(&mut self, key: Option<Vec<u8>>) {
            let (snapshot, accepted) = self
                .d
                .begin_source_departure(self.player, key.clone())
                .unwrap()
                .unwrap();
            assert_eq!(accepted, key);
            assert_eq!(self.phase().as_deref(), Some("source_retiring"));
            let persisted = self.store.progress(snapshot.clone()).await;
            self.d
                .worlds
                .get_mut(&-3)
                .unwrap()
                .committed(self.player, persisted)
                .unwrap();
            self.d
                .finish_source_departure(self.player, snapshot, key)
                .unwrap();
            assert_eq!(self.phase().as_deref(), Some("source_departed"));
        }
        async fn load(&mut self, time: i32, key: Vec<u8>) {
            let (character, arrival) = self
                .d
                .consume(self.ch.account, -2, time, key.clone())
                .unwrap();
            assert_eq!(character, self.ch.id);
            assert_eq!(
                self.phase().as_deref(),
                Some("hello_consumed_admission_pending")
            );
            self.d
                .admit_transfer(arrival, self.ch.account, character, -2)
                .unwrap();
            assert_eq!(self.phase(), None);
            let saved = self.store.load(self.ch.account, character).await.unwrap();
            let presentation = self.store.presentation(self.ch.account).await.unwrap();
            let legend = self.store.is_legend(self.ch.account).await.unwrap();
            let player = self
                .d
                .worlds
                .get_mut(&-2)
                .unwrap()
                .enter_admitted(
                    saved,
                    None,
                    None,
                    presentation,
                    legend,
                    Some(key),
                    Controller::Human,
                )
                .unwrap();
            assert_eq!(self.d.worlds[&-2].players[&player].character.id, self.ch.id);
            assert_eq!(self.phase(), None);
        }
    }

    #[tokio::test]
    async fn prepared_manual_and_unreserved_forced_custody_through_hello_and_load() {
        for forced in [false, true] {
            let mut f = Fixture::new().await;
            let (time, key, mut reservation) = if forced {
                // The Nexus route uses the same forced_transfer constructor as collective Castle.
                f.d.return_to_nexus(f.player, -3, false);
                let transfer = f.d.forced_transfer(f.player).unwrap().unwrap();
                assert_eq!(transfer.destination, Some(-2));
                assert_eq!(f.d.grants[&transfer.key].authority.state(), 0);
                assert_eq!(f.phase().as_deref(), Some("source_retiring"));
                (transfer.key_time, transfer.key, None)
            } else {
                let (time, key, _) = f.d.issue(f.ch.account, f.ch.id, -2).unwrap();
                let reservation = f.d.reserve_transfer(key.clone()).unwrap();
                (time, key, Some(reservation))
            };
            assert!(f.d.consume(f.ch.account, -2, time, key.clone()).is_err());
            f.depart((!forced).then(|| key.clone())).await;
            if let Some(r) = &mut reservation {
                r.disarm();
            }
            let bits = f.d.grants[&key].authority.state();
            assert_eq!(
                bits,
                if forced {
                    0
                } else {
                    TRANSFER_RESERVED | TRANSFER_WRITE | TRANSFER_DEPARTED
                }
            );
            f.load(time, key).await;
        }
    }

    #[tokio::test]
    async fn prepared_departed_rejects_stale_cancelled_foreign_and_duplicate_authority() {
        let mut f = Fixture::new().await;
        let (time, key, _) = f.d.issue(f.ch.account, f.ch.id, -2).unwrap();
        let mut reservation = f.d.reserve_transfer(key.clone()).unwrap();
        f.depart(Some(key.clone())).await;
        reservation.disarm();
        let expires = f.d.grants[&key].expires;
        f.d.grants.get_mut(&key).unwrap().expires = f.d.tick;
        assert_eq!(f.phase(), None);
        f.d.grants.get_mut(&key).unwrap().expires = expires;
        let state = f.d.grants[&key].authority.state();
        for flag in [TRANSFER_REVOKED, TRANSFER_ADMITTED, TRANSFER_RETIRED] {
            f.d.grants[&key]
                .authority
                .0
                .store(state | flag, Ordering::SeqCst);
            assert_eq!(f.phase(), None);
            f.d.grants[&key].authority.0.store(state, Ordering::SeqCst);
        }
        let admission =
            f.d.worlds
                .get_mut(&-2)
                .unwrap()
                .admissions
                .remove(&key)
                .unwrap();
        assert_eq!(f.phase(), None);
        f.d.worlds
            .get_mut(&-2)
            .unwrap()
            .admissions
            .insert(key.clone(), admission);
        let expires = f.d.worlds[&-2].admissions[&key].expires;
        let now = f.d.worlds[&-2].now;
        f.d.worlds
            .get_mut(&-2)
            .unwrap()
            .admissions
            .get_mut(&key)
            .unwrap()
            .expires = now;
        assert_eq!(f.phase(), None);
        f.d.worlds
            .get_mut(&-2)
            .unwrap()
            .admissions
            .get_mut(&key)
            .unwrap()
            .expires = expires;
        for account_mismatch in [true, false] {
            let admission =
                f.d.worlds
                    .get_mut(&-2)
                    .unwrap()
                    .admissions
                    .get_mut(&key)
                    .unwrap();
            if account_mismatch {
                admission.account += 1;
            } else {
                admission.character += 1;
            }
            assert_eq!(f.phase(), None);
            let admission =
                f.d.worlds
                    .get_mut(&-2)
                    .unwrap()
                    .admissions
                    .get_mut(&key)
                    .unwrap();
            admission.account = f.ch.account;
            admission.character = f.ch.id;
        }
        f.d.worlds
            .get_mut(&-2)
            .unwrap()
            .admissions
            .get_mut(&key)
            .unwrap()
            .consumed = true;
        assert_eq!(f.phase(), None);
        f.d.worlds
            .get_mut(&-2)
            .unwrap()
            .admissions
            .get_mut(&key)
            .unwrap()
            .consumed = false;
        f.d.worlds.get_mut(&-2).unwrap().admission_open = false;
        assert_eq!(f.phase(), None);
        f.d.worlds.get_mut(&-2).unwrap().admission_open = true;
        let custody = f.d.grants.get_mut(&key).unwrap().prepared_source.take();
        assert_eq!(f.phase(), None);
        f.d.grants.get_mut(&key).unwrap().prepared_source = custody;
        let grant = f.d.grants.remove(&key).unwrap();
        assert_eq!(f.phase(), None);
        let mut duplicate = key.clone();
        duplicate[0] ^= 1;
        f.d.grants.insert(
            duplicate.clone(),
            Grant {
                prepared_source: grant.prepared_source,
                entry_portal: None,
                entry_witness: None,
                exit: None,
                authority: TransferAuthority(Arc::new(AtomicU8::new(state))),
                origin: grant.origin,
                account: grant.account,
                character: grant.character,
                destination: grant.destination,
                key_time: grant.key_time,
                expires: grant.expires,
            },
        );
        f.d.grants.insert(key.clone(), grant);
        f.d.worlds.get_mut(&-2).unwrap().admissions.insert(
            duplicate.clone(),
            Admission {
                account: f.ch.account,
                character: f.ch.id,
                expires,
                consumed: false,
            },
        );
        assert!(f.d.prepared_native_transition(f.ch.id).is_err());
        f.d.cancel_admission(f.ch.account, duplicate).unwrap();
        let (_, arrival) = f.d.consume(f.ch.account, -2, time, key.clone()).unwrap();
        assert_eq!(
            f.phase().as_deref(),
            Some("hello_consumed_admission_pending")
        );
        f.d.admit_transfer(arrival, f.ch.account, f.ch.id, -2)
            .unwrap();
        assert_eq!(f.phase(), None);
        f.d.cancel_admission(f.ch.account, key).unwrap();
        assert_eq!(f.phase(), None);
    }

    #[tokio::test]
    async fn prepared_absence_or_consumption_without_actual_source_retirement_is_not_proof() {
        let mut f = Fixture::new().await;
        f.d.return_to_nexus(f.player, -3, false);
        let transfer = f.d.forced_transfer(f.player).unwrap().unwrap();
        let other =
            f.d.worlds
                .get_mut(&-2)
                .unwrap()
                .enter(f.ch.clone(), None)
                .unwrap();
        assert!(f.d.prepared_native_transition(f.ch.id).is_err());
        f.d.worlds
            .get_mut(&-2)
            .unwrap()
            .players
            .remove(&other)
            .unwrap();
        let old = f.d.grants[&transfer.key].prepared_source.unwrap();
        f.d.grants
            .get_mut(&transfer.key)
            .unwrap()
            .prepared_source
            .as_mut()
            .unwrap()
            .player += 1;
        assert_eq!(f.phase(), None);
        f.d.grants.get_mut(&transfer.key).unwrap().prepared_source = Some(old);
        let hp = f.d.worlds[&-3].players[&f.player].character.hp;
        for (alive, health, dying) in [(false, hp, false), (true, 0, false), (true, hp, true)] {
            let p =
                f.d.worlds
                    .get_mut(&-3)
                    .unwrap()
                    .players
                    .get_mut(&f.player)
                    .unwrap();
            p.character.alive = alive;
            p.character.hp = health;
            p.death_pending = dying;
            assert_eq!(f.phase(), None);
        }
        let p =
            f.d.worlds
                .get_mut(&-3)
                .unwrap()
                .players
                .get_mut(&f.player)
                .unwrap();
        p.character.alive = true;
        p.character.hp = hp;
        p.death_pending = false;
        f.d.worlds
            .get_mut(&-3)
            .unwrap()
            .players
            .get_mut(&f.player)
            .unwrap()
            .persistence_failure = Some("source save failed".into());
        assert_eq!(f.phase(), None);
        f.d.worlds
            .get_mut(&-3)
            .unwrap()
            .players
            .get_mut(&f.player)
            .unwrap()
            .persistence_failure = None;
        // Inject lost custody without the production finish_source_departure boundary.
        f.d.worlds
            .get_mut(&-3)
            .unwrap()
            .players
            .remove(&f.player)
            .unwrap();
        assert_eq!(f.phase(), None);
        let _arrival =
            f.d.consume(f.ch.account, -2, transfer.key_time, transfer.key)
                .unwrap();
        assert_eq!(
            f.phase(),
            None,
            "HELLO consumption is not a durable-retirement witness"
        );
    }
}

#[cfg(test)]
#[test]
fn godlands_landmarks_include_the_route_from_the_shore() {
    let squares = (0..=16)
        .map(|x| {
            let p = Pos {
                x: 100.5 + 24. * x as f32,
                y: 100.5,
            };
            ((x, 0), vec![p; if x == 0 { 150 } else { 576 }])
        })
        .collect();
    let spots = godlands(&godland_spots(&squares), &[(10.5, 100.5)]);
    assert_eq!(spots.len(), crate::bots::REMEMBERED_GODLANDS);
    assert_eq!(spots[0].x, 100.5);
}

#[cfg(test)]
#[test]
fn both_live_realm_maps_have_godlands_spots() {
    let content = Content::load(std::path::Path::new("data")).unwrap();
    for layout in [
        "classic-world-02-reconstruction",
        "classic-world-10-reconstruction",
    ] {
        let bytes = std::fs::read(format!("data/realms/{layout}.wmap")).unwrap();
        let map = Map::decode_realm_cells(&bytes, &content, [2100, 2100]).unwrap();
        let spots = godlands(&godland_spots(&godland_cells(&map, &content)), &map.spawns);
        assert!(spots.len() >= 4, "{layout}: {spots:?}");
        for p in spots {
            assert_eq!(map.cell(p.x, p.y).unwrap().terrain, 1, "{layout} {p:?}");
        }
    }
}
