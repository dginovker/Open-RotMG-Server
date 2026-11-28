use super::*;
use crate::{
    behavior::{
        self, ActionOutcome, Announcement, AnnouncementKey, Deadline, Effect, Frame, Identity,
        MotionProgress, Offer, Scope, Service, Step, TerminalContext, TerminalPlan, TerminalReason,
        Token,
    },
    content::game::{Behavior, BehaviorKind as K, Condition, Disposition, Field as F, Value},
};
use std::sync::atomic::AtomicU64;

pub(super) mod admission;
mod collision;
mod combat;
mod environment;
mod interest;
mod maps;
mod movement;
mod portals;
mod spawn;
mod terminal;
#[cfg(test)]
pub(super) mod tests;

/// Each faulted executor was logged and its entity retired without rewards; the world stayed consistent.
#[derive(Debug)]
pub(super) struct ActorsQuarantined(Vec<String>);
impl std::fmt::Display for ActorsQuarantined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "XML actors quarantined after executor faults: {:?}",
            self.0
        )
    }
}
impl std::error::Error for ActorsQuarantined {}

static NEXT_WORLD: AtomicU64 = AtomicU64::new(1);
const CHILD_LIMIT: usize = 512;
// Preserve owner density: max 75,035 family slots (FSoD3, all 16 layouts) +25%, rounded to 8192.
pub(super) const ACTOR_LIMIT: usize = 98_304;
// The selected 1024-square Realm may retain every cell's object plus the unchanged dynamic allowance.
const PHYSICAL_LIMIT: usize = 1024 * 1024 + ACTOR_LIMIT;

#[derive(Clone)]
struct Motion {
    goal: Pos,
    origin: Pos,
    angle: f64,
    radius: f64,
    wander_remaining: f64,
    direction: f64,
    anchor: Option<i32>,
    closing: bool,
    born: u64,
    speed: f64,
    until: Option<u64>,
}
struct Published {
    scope: Scope,
    definition: Arc<crate::content::game::Definition>,
    state: u32,
    generation: u64,
    targets: Option<Vec<u16>>,
}
pub(super) struct Runtime {
    game: Arc<crate::content::game::GameData>,
    domain: Option<WorldDomain>,
    elevation_blocks: Option<environment::ElevationBlocks>,
    pub(super) world: u64,
    pub(super) instance: String,
    pub(super) template: Option<String>,
    pub(super) terminals: BTreeMap<i32, terminal::Pending>,
    pub(super) terminal_capacity: Arc<std::sync::atomic::AtomicUsize>,
    /// Server-wide planned allocations retained by every World's staged terminals.
    pub(super) terminal_allocations: Arc<std::sync::atomic::AtomicUsize>,
    pub(super) terminal_leases: BTreeMap<i32, (terminal::Permit, Option<String>)>,
    pub(super) deliveries: BTreeMap<String, crate::store::world_events::Event>,
    pub(super) blocked_deliveries: BTreeSet<String>,
    pub(super) bag_entitlements:
        BTreeMap<(i32, usize), (String, usize, crate::loot::evaluator::Owner, u64)>,
    pub(super) terminal_publications: BTreeSet<String>,
    pub(super) runnable: BTreeSet<i32>,
    /// Runnable actors that must step without a nearby player (new, waiting, queued or near-player last tick).
    /// Idle dormant actors leave it, so Realm ticks visit only actors near players instead of every runnable.
    pub(super) awake: BTreeSet<i32>,
    // Boxed: an inline executor would make every map node carry ~11 executors' worth of slots.
    actors: BTreeMap<i32, Box<behavior::Actor>>,
    definitions: BTreeMap<u16, Arc<crate::content::game::Definition>>,
    identities: BTreeMap<i32, Identity>,
    conditions: BTreeMap<Token, Condition>,
    /// Own executor states; listener (order) states live in `listening` so revocation never scans every actor.
    active: BTreeMap<Scope, BTreeSet<u32>>,
    listening: BTreeMap<Scope, BTreeSet<u32>>,
    pub(super) visible: BTreeSet<i32>,
    unstarted: BTreeSet<i32>,
    condition_base: BTreeMap<(i32, u8), bool>,
    sizes: BTreeMap<i32, f64>,
    breath: BTreeMap<i32, f64>,
    admissions: BTreeMap<AnnouncementKey, Announcement>,
    retry_admissions: BTreeSet<i32>,
    retry_terminals: BTreeSet<i32>,
    chat: Vec<(u64, i32, Pos, String)>,
    auras: BTreeMap<(Token, i32, u8), u64>,
    protectors: BTreeMap<Token, spawn::Protectors>,
    pub(super) blasts: Vec<combat::Blast>,
    poisons: Vec<combat::Poison>,
    births: Vec<spawn::Birth>,
    motions: BTreeMap<Token, Motion>,
    deadlines: BTreeMap<Token, (u64, Deadline)>,
    orders: BTreeMap<(i32, u32), Published>,
    tags: BTreeSet<u32>,
    claims: BTreeSet<i32>,
    clear_participants: BTreeSet<i32>,
    /// Kinds whose committed defeat can end a Realm quest line; followers that merely carry the tag never do.
    pub(super) quest_carriers: BTreeSet<u16>,
    stamps: BTreeMap<i32, maps::PendingStamp>,
    facts: BTreeMap<i32, terminal::Facts>,
    retry_since: BTreeMap<i32, u64>,
    abandon_physical: BTreeSet<i32>,
    delivery_since: BTreeMap<String, u64>,
    portals: BTreeMap<i32, portals::Published>,
    #[cfg(test)]
    fail_next_terminal_birth_plan: bool,
}
impl Runtime {
    pub(super) fn new(content: &Content, domain: Option<WorldDomain>, map: &Map) -> Result<Self> {
        World::xml_validate_content(content)?;
        let world = NEXT_WORLD
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| anyhow::anyhow!("XML world identity exhausted"))?;
        Ok(Self {
            game: content.game.as_ref().expect("validated GameData").clone(),
            domain,
            elevation_blocks: environment::ElevationBlocks::new(map)?,
            world,
            instance: crate::store::world_events::new_id(),
            template: None,
            terminals: BTreeMap::new(),
            terminal_capacity: Arc::default(),
            terminal_allocations: Arc::default(),
            terminal_leases: BTreeMap::new(),
            deliveries: BTreeMap::new(),
            blocked_deliveries: BTreeSet::new(),
            bag_entitlements: BTreeMap::new(),
            terminal_publications: BTreeSet::new(),
            runnable: BTreeSet::new(),
            awake: BTreeSet::new(),
            actors: BTreeMap::new(),
            definitions: BTreeMap::new(),
            identities: BTreeMap::new(),
            conditions: BTreeMap::new(),
            active: BTreeMap::new(),
            listening: BTreeMap::new(),
            visible: BTreeSet::new(),
            unstarted: BTreeSet::new(),
            condition_base: BTreeMap::new(),
            sizes: BTreeMap::new(),
            breath: BTreeMap::new(),
            admissions: BTreeMap::new(),
            retry_admissions: BTreeSet::new(),
            retry_terminals: BTreeSet::new(),
            chat: vec![],
            auras: BTreeMap::new(),
            protectors: BTreeMap::new(),
            blasts: vec![],
            poisons: vec![],
            births: vec![],
            motions: BTreeMap::new(),
            deadlines: BTreeMap::new(),
            orders: BTreeMap::new(),
            tags: BTreeSet::new(),
            claims: BTreeSet::new(),
            clear_participants: BTreeSet::new(),
            quest_carriers: quest_carriers(
                content.game.as_ref().expect("validated GameData"),
                content,
            ),
            stamps: BTreeMap::new(),
            facts: BTreeMap::new(),
            retry_since: BTreeMap::new(),
            abandon_physical: BTreeSet::new(),
            delivery_since: BTreeMap::new(),
            portals: BTreeMap::new(),
            #[cfg(test)]
            fail_next_terminal_birth_plan: false,
        })
    }
}
fn value(b: &Behavior, f: F) -> Option<&Value> {
    b.fields.get(&f)
}
// Executor reads are compiled values: GameData load refuses a behavior missing a declared executor read.
#[track_caller]
fn declared(b: &Behavior, f: F) {
    debug_assert!(
        crate::content::game::executor_reads(b.kind).contains(&f),
        "undeclared executor read {:?} {f:?} at {}; add it to compile.rs executor_reads or read it with opt_n",
        b.kind,
        std::panic::Location::caller()
    );
}
fn opt_n(b: &Behavior, f: F) -> Option<f64> {
    match value(b, f) {
        Some(Value::Number(v) | Value::Radians(v)) => Some(*v),
        Some(Value::Integer(v)) => Some(f64::from(*v)),
        None => None,
        other => panic!("invalid compiled numeric {:?} {f:?}: {other:?}", b.kind),
    }
}
#[track_caller]
fn n(b: &Behavior, f: F) -> f64 {
    declared(b, f);
    opt_n(b, f).unwrap_or_else(|| panic!("{:?} {f:?} has no compiled executor value", b.kind))
}
#[track_caller]
fn ms(b: &Behavior, f: F) -> u64 {
    declared(b, f);
    match value(b, f) {
        Some(Value::Millis(v)) => *v,
        other => panic!("invalid compiled time {:?} {f:?}: {other:?}", b.kind),
    }
}
#[track_caller]
fn flag(b: &Behavior, f: F) -> bool {
    declared(b, f);
    match value(b, f) {
        Some(Value::Bool(v)) => *v,
        other => panic!("invalid compiled bool {:?} {f:?}: {other:?}", b.kind),
    }
}
/// Heroes, quest roots and final forms, plus every enemy their compiled state Metamorphose/MetamorphoseAtDeath
/// chains replace them with (Phoenix Lord -> Phoenix Egg); Order behaviors target other actors and are excluded.
fn quest_carriers(game: &crate::content::game::GameData, content: &Content) -> BTreeSet<u16> {
    let mut carriers: BTreeSet<u16> = (game.definitions.keys().copied())
        .filter(|k| {
            matches!(
                game.site_role(*k),
                Some(
                    crate::content::game::SiteRole::Hero
                        | crate::content::game::SiteRole::Closure
                        | crate::content::game::SiteRole::Event
                )
            ) || crate::quest::final_root(*k)
        })
        .collect();
    let mut pending: Vec<u16> = carriers.iter().copied().collect();
    while let Some(kind) = pending.pop() {
        let states = &game.definitions[&kind].states;
        let issued: BTreeSet<_> = states
            .iter()
            .flat_map(|s| &s.orders)
            .map(|o| o.state)
            .collect();
        let own = |s: &&crate::content::game::State| {
            std::iter::successors(Some(s.id), |id| states[*id as usize].parent)
                .all(|id| !issued.contains(&id))
        };
        for b in states.iter().filter(own).flat_map(|s| &s.behaviors) {
            if matches!(b.kind, K::Metamorphose | K::MetamorphoseAtDeath) {
                for child in objects(b, F::ChildId) {
                    if content.objects[child].enemy && carriers.insert(*child) {
                        pending.push(*child);
                    }
                }
            }
        }
    }
    carriers
}
fn objects(b: &Behavior, f: F) -> &[u16] {
    match value(b, f) {
        Some(Value::Objects(v)) => v,
        None => &[],
        other => panic!("invalid compiled objects {f:?}: {other:?}"),
    }
}
fn point(pos: Pos, angle: f64, distance: f64) -> Pos {
    Pos {
        x: pos.x + (angle.cos() * distance) as f32,
        y: pos.y + (angle.sin() * distance) as f32,
    }
}
fn heading(a: Pos, b: Pos) -> f64 {
    (f64::from(b.y) - f64::from(a.y)).atan2(f64::from(b.x) - f64::from(a.x))
}

fn bubble_seconds(b: &Behavior) -> Result<u8> {
    let Some(Value::Millis(time)) = value(b, F::BubbleTime) else {
        bail!("Say missing compiled bubbleTime")
    };
    // The selected TEXT byte cannot represent fractions; authored ceiling avoids shortening source presentation.
    u8::try_from(time.div_ceil(1000)).context("bubbleTime exceeds native whole-second byte")
}
fn native_validate(b: &Behavior) -> Result<()> {
    native_validate_fields(b).with_context(|| format!("{} behavior={}", b.source, b.local))
}
fn native_validate_fields(b: &Behavior) -> Result<()> {
    // Movement IR carries no other runtime type guard, so admission rechecks its declared reads.
    let canonical = match behavior::service(b.kind) {
        Some(Service::Movement) => crate::content::game::executor_reads(b.kind),
        _ => &[],
    };
    for field in canonical {
        let value = b.fields.get(field).with_context(|| {
            format!(
                "{:?} missing compiled {field:?}; compiler must materialize qualified defaults",
                b.kind
            )
        })?;
        let valid = match field {
            F::Duration | F::RadiusWobblePeriod => matches!(value, Value::Millis(_)),
            F::Predictive | F::Clockwise | F::AvoidWater => matches!(value, Value::Bool(_)),
            F::Angle => matches!(value, Value::Radians(_)),
            F::Terrain => {
                matches!(value, Value::NativeTerrain)
                    || matches!(value, Value::Grounds(gs) if !gs.is_empty())
            }
            _ => matches!(value, Value::Number(_) | Value::Integer(_)),
        };
        ensure!(
            valid,
            "{:?} invalid compiled {field:?} type: {value:?}",
            b.kind
        );
    }
    if b.kind == K::Wander {
        ensure!(
            matches!(value(b, F::Duration), None | Some(Value::Millis(_))),
            "Wander invalid compiled Duration type"
        );
    }
    let unresolved = match b.kind {
        K::Follow => Some(F::Distance),
        K::Circle | K::KeepDistance => Some(F::Range),
        _ => None,
    };
    if let Some(field) = unresolved {
        ensure!(
            !b.fields.contains_key(&field),
            "unresolved {:?} legacy {field:?}: compiler must normalize aliases before defaults",
            b.kind
        );
    }
    if matches!(b.kind, K::Protect | K::Orbit) {
        ensure!(
            !objects(b, F::ProtecteeId).is_empty(),
            "{:?} requires explicit linked protecteeId capability",
            b.kind
        );
    }
    if b.kind == K::Walk {
        ensure!(
            matches!(value(b, F::GroundTypes), Some(Value::Grounds(gs)) if !gs.is_empty()),
            "Walk requires explicit linked groundTypes capability"
        );
    }
    if b.kind == K::ChangeGround {
        ensure!(
            matches!(value(b, F::GroundId), Some(Value::Grounds(gs)) if gs.len() == 1),
            "ChangeGround requires one linked ground, not an ignored choice list"
        );
    }
    let unsupported: &[F] = match b.kind {
        K::ObjectToss => &[F::Damage, F::Radius],
        K::Protect => &[F::Radius],
        K::Shoot => &[F::AcquireRange],
        _ => &[],
    };
    for field in unsupported {
        ensure!(
            !b.fields.contains_key(field),
            "native capability for {:?} {field:?} unavailable; field cannot be ignored",
            b.kind
        );
    }
    if b.kind == K::Say {
        bubble_seconds(b)?;
    }
    if let Some(repeats) = opt_n(b, F::FlashRepeats) {
        ensure!(
            repeats <= 16_777_216.,
            "FlashRepeats exceeds exact native float count"
        );
    }
    for field in [
        F::Damage,
        F::MinDamage,
        F::MaxDamage,
        F::Healing,
        F::MaxHeal,
        F::PoisonTotalDamage,
    ] {
        if let Some(number) = opt_n(b, field) {
            ensure!(
                number >= 0. && number.fract() == 0.,
                "{field:?} cannot be represented by native nonnegative integer damage/healing"
            );
        }
    }
    for field in [F::TextureSourceId, F::ResignPet] {
        ensure!(
            !b.fields.contains_key(&field),
            "native capability for {field:?} unavailable in selected client/lifecycle"
        );
    }
    if let Some(sound) = opt_n(b, F::SoundId) {
        ensure!(sound <= 255., "soundId exceeds native byte");
    }
    if let Some(shots) = opt_n(b, F::NumShots) {
        ensure!(shots <= 255., "numShots exceeds native volley/child limit");
    }
    for field in [F::Damage, F::MinDamage, F::MaxDamage] {
        if b.fields.contains_key(&field) {
            ensure!(
                n(b, field) <= i16::MAX as f64,
                "{field:?} exceeds native damage wire"
            );
        }
    }
    for (field, value) in &b.fields {
        if let Value::Number(number) | Value::Radians(number) = value {
            ensure!(
                number.is_finite() && number.abs() <= f32::MAX as f64,
                "{field:?} cannot be represented by native scalar coordinates"
            );
        }
    }
    for field in [
        F::AltTextureId,
        F::Healing,
        F::MaxHeal,
        F::PoisonTotalDamage,
        F::MinSize,
        F::MaxSize,
    ] {
        if let Some(number) = opt_n(b, field) {
            ensure!(
                number <= i32::MAX as f64,
                "{field:?} exceeds native integer stat"
            );
        }
    }
    if b.kind == K::Buzz && b.fields.contains_key(&F::Range) {
        ensure!(
            opt_n(b, F::Range) == opt_n(b, F::AcquireRange),
            "unresolved Buzz legacy range/acquireRange: compiler must normalize aliases before defaults"
        );
    }
    if b.kind == K::Protect {
        if let Some(range) = opt_n(b, F::Range) {
            ensure!(
                range == n(b, F::ReprotectRange),
                "unresolved Protect legacy range/reprotectRange: compiler must normalize aliases before defaults"
            );
        }
        ensure!(
            n(b, F::ProtectionRange) >= n(b, F::ReprotectRange) && n(b, F::ReprotectRange) >= 0.,
            "invalid Protect hysteresis"
        );
    }
    if b.kind == K::Orbit {
        for field in [F::RadiusVariability, F::SpeedVariability] {
            ensure!(n(b, field) >= 0., "invalid {field:?} variability");
        }
        let speed_upper = n(b, F::Speed) * (1. + n(b, F::SpeedVariability));
        ensure!(
            n(b, F::SpeedVariability) <= 1.
                && speed_upper.is_finite()
                && speed_upper <= f32::MAX as f64,
            "unsupported Orbit nonnegative speed envelope before sampling"
        );
        let amplitude = n(b, F::RadiusWobbleDistance);
        ensure!(amplitude >= 0., "negative Orbit wobble amplitude");
        if amplitude > 0. {
            ensure!(
                b.fields.contains_key(&F::RadiusWobblePeriod) && ms(b, F::RadiusWobblePeriod) > 0,
                "positive Orbit wobble requires compiled positive radiusWobblePeriod; compiler owns the qualified 15-second default"
            );
        }
        let upper = n(b, F::Radius) * (1. + n(b, F::RadiusVariability)) + amplitude;
        ensure!(
            upper.is_finite() && upper <= f32::MAX as f64,
            "unrepresentable Orbit radial/wobble envelope before sampling"
        );
        let protection = b.fields.contains_key(&F::ProtectionRange);
        let reprotect = b.fields.contains_key(&F::ReprotectRange);
        ensure!(
            protection == reprotect,
            "Orbit needs both protectionRange and reprotectRange or neither"
        );
        if let (Some(outer), Some(inner)) =
            (opt_n(b, F::ProtectionRange), opt_n(b, F::ReprotectRange))
        {
            ensure!(
                outer.is_finite()
                    && inner.is_finite()
                    && outer > 0.
                    && inner >= 0.
                    && inner <= outer,
                "invalid Orbit protection/reprotect hysteresis"
            );
        }
    }
    if b.kind == K::Shadow {
        let (outer, inner, speed) = (n(b, F::Range), n(b, F::ReshadowRange), n(b, F::Speed));
        ensure!(
            outer.is_finite()
                && outer > 0.
                && inner.is_finite()
                && inner >= 0.
                && inner <= outer
                && speed.is_finite()
                && speed >= 0.,
            "invalid Shadow range/reshadow/speed"
        );
    }
    Ok(())
}

impl World {
    pub(super) fn xml_portal_expiry(&self, id: i32) -> Option<u64> {
        self.xml
            .deadlines
            .iter()
            .filter(|(token, _)| token.scope.identity.actor == id)
            .map(|(_, (at, _))| *at)
            .chain(self.xml.portals.get(&id).and_then(|p| p.expires))
            .min()
    }
    pub(super) fn xml_terminal_pending(&self, id: i32) -> bool {
        self.xml.retry_terminals.contains(&id)
    }
    pub(super) fn xml_diagnostics(&self, id: i32) -> Option<serde_json::Value> {
        let actor = self.xml.actors.get(&id)?;
        Some(
            serde_json::json!({"input_fingerprint":self.xml_game().fingerprint,"active_ms":actor.active_ms(),"states":actor.path().iter().map(|state|{let state=&actor.definition().states[*state as usize];serde_json::json!({"id":state.id,"name":state.name,"source":state.source})}).collect::<Vec<_>>(),"terminal_pending":self.xml.retry_terminals.contains(&id),"admission_pending":self.xml.retry_admissions.contains(&id)}),
        )
    }
    pub(super) fn xml_prepare_portal_actor(&mut self, id: i32) -> Result<()> {
        if self
            .xml_declared_admission(self.entities[&id].kind, "reserved portal program")?
            .disposition
            == Disposition::Active
        {
            let actor = self.xml_actor(id)?;
            self.xml.actors.insert(id, actor);
        }
        Ok(())
    }
    fn xml_preflight_portal_program(&self, kind: u16) -> Result<()> {
        let d = self.xml_portal_definition(kind, "portal program preflight")?;
        behavior::validate(d)?;
        maps::preflight_program(self.xml_game(), d)?;
        for state in interest::initial_states(d) {
            for b in &d.states[state as usize].behaviors {
                native_validate(b)?;
                if matches!(b.kind, K::Decay | K::Metamorphose) {
                    let Some(Value::Millis(delay)) = value(b, F::Time) else {
                        bail!("{} missing compiled lifecycle time", b.source)
                    };
                    self.now
                        .checked_add(*delay)
                        .context("reserved portal lifecycle clock overflow")?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn xml_preflight_portal_activation(&self, id: i32) -> Result<()> {
        let kind = self.entities[&id].kind;
        self.xml_preflight_portal_program(kind)?;
        if self.xml_game().definitions[&kind].disposition == Disposition::Active {
            let actor = self
                .xml
                .actors
                .get(&id)
                .context("reserved portal program absent")?;
            ensure!(
                actor.path().is_empty() && actor.active_ms() == 0,
                "reserved portal program already activated"
            );
        }
        Ok(())
    }
    pub(super) fn xml_activate_portal(&mut self, id: i32) -> Result<()> {
        if self
            .xml_declared_admission(self.entities[&id].kind, "published portal program")?
            .disposition
            == Disposition::Active
        {
            let mut actor = self.xml_actor(id)?;
            let result = actor.tick(
                Frame {
                    delta_ms: 0,
                    dormant: true,
                    stasis: false,
                    stunned: false,
                },
                self,
            );
            self.xml.actors.insert(id, actor);
            result?;
        }
        Ok(())
    }
    pub(super) fn xml_world_id(&self) -> u64 {
        self.xml.world
    }
    pub(super) fn xml_check_catalog(&self) -> Result<()> {
        ensure!(
            Arc::ptr_eq(
                &self.xml.game,
                self.content
                    .game
                    .as_ref()
                    .context("runtime GameData absent")?
            ),
            "world={} owning catalog changed; cached programs/reservations cannot change catalogs",
            self.name
        );
        Ok(())
    }
    pub(super) fn xml_game(&self) -> &crate::content::game::GameData {
        self.content
            .game
            .as_ref()
            .expect("validated runtime GameData")
    }
    /// Recovered scripted containers (Tutorial Loot Bag, tutorial_objects.xml:67 RemoveOnEmpty) are inventory sources too.
    pub(super) fn xml_container_admission(&self, kind: u16, context: &str) -> Result<()> {
        use crate::content::game::{AdmissionRole, NativeService};
        let definition = self.xml_game().definitions.get(&kind);
        let role = match definition.map(|d| &d.disposition) {
            Some(Disposition::Active | Disposition::Passive) => AdmissionRole::Scripted,
            _ => AdmissionRole::Native(NativeService::Container),
        };
        self.xml_game().admission(kind, role, context)?;
        Ok(())
    }
    pub(super) fn xml_admit_all(&mut self) -> Result<()> {
        for id in self.entities.keys().copied().collect::<Vec<_>>() {
            let e = &self.entities[&id];
            if self
                .xml_game()
                .definitions
                .get(&e.kind)
                .is_some_and(|d| d.disposition == Disposition::Active)
            {
                ensure!(
                    self.xml_placeable(e.kind, e.pos, &[id], None)?,
                    "initial map NPC geometry blocked actor={id} map={}",
                    self.name
                );
            }
            self.xml_admit(id)?;
        }
        Ok(())
    }
    pub(super) fn xml_admit(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let definition = self.xml_declared_admission(e.kind, "actor admission")?;
        self.xml_preflight_position(e.kind, e.pos)?;
        let active = definition.disposition == Disposition::Active;
        let terminal_only = admission::terminal_only(definition);
        if active {
            if !terminal_only {
                self.xml.unstarted.insert(id);
            }
            self.xml_refresh_interest(id)?;
        }
        Ok(())
    }
    fn xml_identity(&self, id: i32) -> Result<Identity> {
        let e = self.entities.get(&id).context("XML actor absent")?;
        if let Some(identity) = self.xml.identities.get(&id) {
            return Ok(*identity);
        }
        let encounter = if let Some(quest) = *e.quest {
            quest as u64
        } else if let Some(parent) = *e.parent {
            ensure!(parent != id, "self-parent XML ownership");
            self.xml_identity(parent)?
                .encounter
                .context("parent encounter absent")?
        } else if let Some(camp) = e.camp {
            self.camps[camp]
                .leader
                .context("camp leader ownership absent")? as u64
        } else {
            id as u64
        };
        Ok(Identity {
            world: self.xml.world,
            actor: id,
            parent: *e.parent,
            encounter: Some(encounter),
        })
    }
    fn xml_actor(&mut self, id: i32) -> Result<Box<behavior::Actor>> {
        if let Some(actor) = self.xml.actors.remove(&id) {
            return Ok(actor);
        }
        let identity = self.xml_identity(id)?;
        let kind = self.entities[&id].kind;
        if !self.xml.definitions.contains_key(&kind) {
            let definition = self
                .xml_game()
                .definitions
                .get(&kind)
                .context("XML actor definition absent")?
                .clone();
            self.xml.definitions.insert(kind, Arc::new(definition));
        }
        self.xml.identities.insert(id, identity);
        behavior::Actor::new(
            self.xml.definitions[&kind].clone(),
            identity,
            self.prepared_seed.unwrap_or(self.xml.world) ^ id as u64,
        )
        .map(Box::new)
    }

    pub(super) fn xml_forget(&mut self, id: i32) {
        self.xml.portals.remove(&id);
        let own: Vec<_> = (self.xml.active.range(self.xml_actor_scopes(id)))
            .map(|(scope, _)| *scope)
            .collect();
        for scope in own {
            self.xml.active.remove(&scope);
        }
        let issued = self.xml_revoke_scopes(|s| s.identity.actor == id || s.issuer == id);
        self.xml_withdraw_orders(issued);
        self.xml.actors.remove(&id);
        self.xml.identities.remove(&id);
        self.xml.retry_admissions.remove(&id);
        self.xml.condition_base.retain(|(actor, _), _| *actor != id);
        self.xml.sizes.remove(&id);
        self.xml.visible.remove(&id);
        self.xml.unstarted.remove(&id);
        self.xml
            .admissions
            .retain(|k, _| k.owner.scope.identity.actor != id);
    }
    fn xml_scope(&self, scope: Scope) -> Result<&Entity> {
        self.xml_check_catalog()?;
        ensure!(
            scope.identity.world == self.xml.world,
            "cross-world XML scope"
        );
        let id = scope.identity.actor;
        let actor = self.entities.get(&id).context("XML actor absent")?;
        let identity = match self.xml.identities.get(&id) {
            Some(identity) => *identity,
            None => self.xml_identity(id)?,
        };
        ensure!(scope.identity == identity, "XML full identity mismatch");
        Ok(actor)
    }
    pub(super) fn xml_step(&mut self) -> Result<()> {
        self.xml_check_catalog()?;
        let expired = self
            .xml
            .portals
            .iter()
            .filter(|(id, p)| {
                p.expires.is_some_and(|at| at <= self.now)
                    && self.entities.get(id).is_some_and(|e| !e.reserved)
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in expired {
            self.remove_entity(id);
        }
        let delta = self
            .now
            .checked_sub(self.last_npc_step)
            .context("XML world clock regressed")?;
        self.last_npc_step = self.now;
        self.xml.auras.retain(|_, until| *until > self.now);
        self.xml_refresh_auras();
        let near: BTreeSet<i32> = self
            .players
            .values()
            .filter(|p| eligible(p))
            .flat_map(|p| self.actors_near(p.pos, 20.5).map(|e| e.id))
            .filter(|id| self.xml.runnable.contains(id))
            .collect();
        let ids: BTreeSet<_> = self
            .xml
            .awake
            .union(&near)
            .copied()
            .filter(|id| {
                self.entities.get(id).is_some_and(|e| {
                    e.published
                        && self
                            .xml_game()
                            .definitions
                            .get(&e.kind)
                            .is_some_and(|d| d.disposition == Disposition::Active)
                })
            })
            .chain(self.xml.retry_terminals.iter().copied())
            .chain(self.xml.retry_admissions.iter().copied())
            .collect();
        let mut quarantined = Vec::new();
        for id in ids {
            if self
                .xml
                .stamps
                .get(&id)
                .is_some_and(|pending| !pending.published)
            {
                continue;
            }
            if !self.entities.contains_key(&id) && !self.xml.retry_terminals.contains(&id) {
                continue;
            }
            if self.xml.retry_terminals.contains(&id) || self.xml.retry_admissions.contains(&id) {
                self.xml_retry_held(id)?;
                continue;
            }
            let mut actor = self.xml_actor(id)?;
            let e = &self.entities[&id];
            let flags = e.conditions(self.now);
            let dormant = self
                .players
                .values()
                .all(|p| !eligible(p) || p.pos.distance(e.pos) > 20.)
                && !self.xml_owned_leash_is_outside(e);
            if !dormant {
                // Population reinforcement clocks, Oryx family counts and diagnostics read first non-dormant tick as activation.
                let now = self.now;
                let e = self.entities.get_mut(&id).unwrap();
                e.activated.get_or_insert(now);
            }
            let result = actor.tick(
                Frame {
                    delta_ms: delta,
                    dormant,
                    stasis: flags & crate::condition::bit(crate::condition::STASIS) != 0,
                    stunned: flags & crate::condition::bit(crate::condition::STUNNED) != 0,
                },
                self,
            );
            if dormant && actor.idle_when_dormant() {
                self.xml.awake.remove(&id);
            } else {
                self.xml.awake.insert(id);
            }
            if !actor.retired() {
                self.xml.actors.insert(id, actor);
            }
            if let Err(error) = result {
                if self.xml.retry_terminals.contains(&id) {
                    self.xml_bound_terminal_retry(id, &error);
                    continue;
                }
                if self.xml.retry_admissions.contains(&id) {
                    terminal::log_pending(id, &error, "XML lifecycle publication pending");
                    continue;
                }
                self.xml_quarantine(id, error, &mut quarantined)?;
            }
        }
        let due: Vec<_> = self
            .xml
            .deadlines
            .iter()
            .filter(|(token, (at, _))| {
                *at <= self.now
                    && self
                        .xml
                        .stamps
                        .get(&token.scope.identity.actor)
                        .is_none_or(|p| p.published)
                    && !self
                        .xml
                        .retry_terminals
                        .contains(&token.scope.identity.actor)
            })
            .map(|(t, (_, d))| (*t, *d))
            .collect();
        for (token, deadline) in due {
            if !self.entities.contains_key(&token.scope.identity.actor) {
                self.xml.deadlines.remove(&token);
                continue;
            }
            let id = token.scope.identity.actor;
            let mut actor = self.xml_actor(id)?;
            let result = actor.expire_lifecycle(deadline, self);
            if result.is_ok() || !self.xml.retry_terminals.contains(&id) {
                self.xml.deadlines.remove(&token);
            }
            if !actor.retired() {
                self.xml.actors.insert(id, actor);
            }
            if let Err(error) = result {
                if !self.xml.retry_terminals.contains(&id) {
                    self.xml_quarantine(id, error, &mut quarantined)?;
                    continue;
                }
                self.xml_bound_terminal_retry(id, &error);
            }
        }
        self.xml_birth_deadlines()?;
        self.xml_protector_deadlines()?;
        self.xml_combat_deadlines()?;
        if quarantined.is_empty() {
            Ok(())
        } else {
            Err(ActorsQuarantined(quarantined).into())
        }
    }
    /// A poisoned executor would refuse every later tick; retiring its entity contains the fault to it.
    fn xml_quarantine(
        &mut self,
        id: i32,
        error: anyhow::Error,
        out: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(e) = self.entities.get(&id) {
            ensure!(
                !e.reserved,
                "{error:#}: faulted XML actor holds a reservation; cannot quarantine"
            );
        }
        let kind = self.entities.get(&id).map(|e| e.kind);
        let detail = format!(
            "actor={id} kind={kind:#x?} world={}: {error:#}",
            self.xml.world
        );
        tracing::error!(actor = id, ?kind, error = %detail, "XML actor executor fault; entity removed without rewards, world continues");
        if kind.is_some() {
            self.remove_entity(id);
        }
        self.xml.actors.remove(&id);
        out.push(detail);
        Ok(())
    }
    /// Retries one held terminal or lifecycle publication without advancing World time.
    fn xml_retry_held(&mut self, id: i32) -> Result<()> {
        let mut actor = self.xml_actor(id)?;
        let retry = if self.xml.retry_terminals.contains(&id) {
            actor.retry_terminal(self)
        } else {
            actor.retry_admissions(self)
        };
        if !actor.retired() {
            self.xml.actors.insert(id, actor);
        }
        if let Err(error) = retry {
            if self.xml.retry_terminals.contains(&id) {
                self.xml_bound_terminal_retry(id, &error);
            } else {
                tracing::error!(actor=id,error=%format!("{error:#}"),"XML lifecycle publication remains pending");
            }
        } else {
            self.xml.retry_since.remove(&id);
        }
        Ok(())
    }
    /// A paused clock stops ticks, so native pause settlement retries terminals whose captured facts await recipients.
    pub(crate) fn xml_settle_captured_terminals(&mut self) -> Result<()> {
        let held: Vec<_> = (self.xml.retry_terminals.iter().copied())
            .filter(|id| self.xml.facts.contains_key(id))
            .filter(|id| self.xml.stamps.get(id).is_none_or(|p| p.published))
            .collect();
        for id in held {
            self.xml_retry_held(id)?;
        }
        Ok(())
    }
    pub(super) fn xml_defeated(&mut self, id: i32, context: TerminalContext) -> Result<()> {
        if self.xml.claims.contains(&id) {
            return Ok(());
        }
        let mut actor = self.xml_actor(id)?;
        let result = if self.xml.retry_terminals.contains(&id) {
            actor.retry_terminal(self)
        } else {
            actor.defeated_with_context(context, self)
        };
        if !actor.retired() {
            self.xml.actors.insert(id, actor);
        }
        if self.xml.terminals.contains_key(&id) || self.xml.facts.contains_key(&id) {
            Ok(())
        } else {
            result
        }
    }
    fn xml_targets(&self, scope: Scope, range: f64, invisible: bool) -> Result<Vec<(i32, Pos)>> {
        let e = self.xml_scope(scope)?;
        let mut targets: Vec<_> = self
            .players
            .iter()
            .filter(|(_, p)| {
                eligible(p)
                    && (invisible
                        || p.newbie_until <= self.now
                            && p.conditions(self.now)
                                & crate::condition::bit(crate::condition::INVISIBLE)
                                == 0)
                    && f64::from(p.pos.distance(e.pos)) <= range
            })
            .map(|(id, p)| (*id, p.pos))
            .collect();
        targets.sort_by(|(a, ap), (b, bp)| {
            ap.distance(e.pos)
                .total_cmp(&bp.distance(e.pos))
                .then(a.cmp(b))
        });
        Ok(targets)
    }
    fn xml_owned_leash_is_outside(&self, actor: &Entity) -> bool {
        if actor.parent.is_none() {
            return false;
        }
        self.xml_activations(actor.id).any(|(scope, states)| {
            self.xml_program(*scope).is_some_and(|d| {
                states.iter().any(|state| {
                    d.states[*state as usize].behaviors.iter().any(|b| {
                        b.kind == K::Protect
                            && self
                                .xml_owned_anchor(actor, objects(b, F::ProtecteeId))
                                .is_some_and(|anchor| {
                                    f64::from(actor.pos.distance(anchor.pos))
                                        > n(b, F::ProtectionRange)
                                })
                    })
                })
            })
        })
    }

    /// Decoys draw attacks and pursuit like the nearest visible player.
    pub(super) fn xml_nearest_hostile_target(
        &self,
        origin: Pos,
        min: f64,
        max: f64,
    ) -> Option<(i32, Pos)> {
        let ranked = |id, pos: Pos| {
            let distance = f64::from(pos.distance(origin));
            (min <= distance && distance <= max).then_some((distance, id, pos))
        };
        self.players
            .iter()
            .filter_map(|(&id, p)| {
                let target = ranked(id, p.pos)?;
                (eligible(p)
                    && p.newbie_until <= self.now
                    && p.conditions(self.now) & crate::condition::bit(crate::condition::INVISIBLE)
                        == 0)
                    .then_some(target)
            })
            .chain(self.decoy_targets().filter_map(|(id, pos)| ranked(id, pos)))
            .min_by(|(a, ai, _), (b, bi, _)| a.total_cmp(b).then(ai.cmp(bi)))
            .map(|(_, id, pos)| (id, pos))
    }
    fn xml_members(&self, scope: Scope, kinds: &[u16], range: f64) -> Result<Vec<i32>> {
        let e = self.xml_scope(scope)?;
        // Actor-only selectors need not walk the Realm's roughly 10^5 scenery entities.
        let candidates: Box<dyn Iterator<Item = &Entity>> = if !kinds.is_empty()
            && kinds
                .iter()
                .all(|&kind| World::actor_kind(&self.content, kind))
        {
            Box::new(self.actors_in_sections(e.pos, range as f32))
        } else if (2. * range + 3.).powi(2) < self.entities.len() as f64 {
            let r = range as f32;
            Box::new(self.near_entities(
                Pos {
                    x: e.pos.x - r,
                    y: e.pos.y - r,
                },
                Pos {
                    x: e.pos.x + r,
                    y: e.pos.y + r,
                },
            ))
        } else {
            Box::new(self.entities.values())
        };
        let mut ids: Vec<_> = candidates
            .filter(|other| {
                (other.hp > 0 || self.content.objects[&other.kind].hp == 0)
                    && other.id != e.id
                    && (kinds.is_empty() || kinds.contains(&other.kind))
                    && f64::from(e.pos.distance(other.pos)) <= range
                    && match scope
                        .identity
                        .encounter
                        .filter(|_| scope.identity.parent.is_some())
                    {
                        Some(encounter) => {
                            self.xml
                                .identities
                                .get(&other.id)
                                .map(|i| i.encounter)
                                .unwrap_or_else(|| other.quest.map(|n| n as u64))
                                == Some(encounter)
                        }
                        None => true,
                    }
            })
            .map(|e| e.id)
            .collect();
        ids.sort_by(|a, b| {
            self.entities[a]
                .pos
                .distance(e.pos)
                .total_cmp(&self.entities[b].pos.distance(e.pos))
                .then(a.cmp(b))
        });
        Ok(ids)
    }
    fn xml_condition(&mut self, owner: Token, condition: &Condition, enabled: bool) -> Result<()> {
        let id = owner.scope.identity.actor;
        let index = match condition {
            Condition::Selected(i) => *i,
            Condition::StunImmune => 12,
        };
        let bit = crate::condition::bit(index);
        if enabled {
            let e = self.entities.get(&id).context("condition actor absent")?;
            self.xml
                .condition_base
                .entry((id, index))
                .or_insert(e.condition & bit != 0);
            self.xml.conditions.insert(owner, condition.clone());
        } else {
            self.xml.conditions.remove(&owner);
        }
        let owned = self.xml.conditions.iter().any(|(t, c)| {
            t.scope.identity.actor == id
                && match c {
                    Condition::Selected(i) => *i == index,
                    Condition::StunImmune => index == 12,
                }
        });
        let base = self
            .xml
            .condition_base
            .get(&(id, index))
            .copied()
            .unwrap_or(false);
        if let Some(e) = self.entities.get_mut(&id) {
            if owned || base {
                e.condition |= bit;
            } else {
                e.condition &= !bit;
            }
        }
        if !owned {
            self.xml.condition_base.remove(&(id, index));
        }
        Ok(())
    }
}
fn eligible(p: &Player) -> bool {
    p.character.alive && p.character.hp > 0 && !p.death_pending && !p.departing && !p.paused
}

impl behavior::World for World {
    fn player_distances(&self, scope: Scope) -> Result<impl Iterator<Item = f64>> {
        let origin = self.xml_scope(scope)?.pos;
        Ok(self
            .players
            .values()
            .filter(|p| eligible(p))
            .map(move |p| f64::from(p.pos.distance(origin))))
    }
    fn hp_fraction(&self, scope: Scope) -> Result<f64> {
        let e = self.xml_scope(scope)?;
        let max = self.content.objects[&e.kind].hp;
        ensure!(max > 0, "HP predicate on actor without HP");
        Ok(f64::from(e.hp) / f64::from(max))
    }
    fn exists(&self, scope: Scope, kinds: &[u16]) -> Result<bool> {
        let actor = self.xml_scope(scope)?;
        // World-wide transitions must not scan the Realm’s unrelated scenery each tick.
        Ok(kinds.iter().copied().any(|kind| {
            self.entities
                .of_kind(kind)
                .any(|e| e.id != actor.id && (e.hp > 0 || self.content.objects[&kind].hp == 0))
        }))
    }
    fn ground(&self, scope: Scope) -> Result<u8> {
        let p = self.xml_scope(scope)?.pos;
        Ok(self
            .map
            .cell(p.x, p.y)
            .context("actor ground absent")?
            .ground)
    }
    fn global_tag(&self, scope: Scope, symbol: u32) -> Result<bool> {
        self.xml_scope(scope)?;
        Ok(self.xml.tags.contains(&symbol))
    }
    fn player_says(&self, scope: Scope, text: &str) -> Result<bool> {
        let pos = self.xml_scope(scope)?.pos;
        // BehaviorsAndTransitions.txt:272 specifies a 30-square, case/whitespace-insensitive phrase.
        Ok(self.xml.chat.iter().any(|(at, speaker, p, t)| {
            let mut phrase = text.split_whitespace();
            self.now.saturating_sub(*at) <= 50
                && p.distance(pos) <= 30.
                && self.players.get(speaker).is_some_and(eligible)
                && t.split_whitespace().all(|word| {
                    phrase
                        .next()
                        .is_some_and(|part| part.eq_ignore_ascii_case(word))
                })
                && phrase.next().is_none()
        }))
    }
    fn orders(&self, scope: Scope) -> Result<Vec<Offer>> {
        let pos = self.xml_scope(scope)?.pos;
        self.xml
            .orders
            .iter()
            .filter(|((id, _), _)| self.entities.contains_key(id))
            .map(|((id, local), o)| {
                Ok(Offer {
                    issuer: o.scope.identity,
                    definition: o.definition.clone(),
                    local: *local,
                    state: o.state,
                    generation: o.generation,
                    targets: o.targets.clone(),
                    distance: f64::from(pos.distance(self.entities[id].pos)),
                })
            })
            .collect()
    }
    fn size(&self, scope: Scope) -> Result<f64> {
        Ok(*self
            .xml
            .sizes
            .get(&scope.identity.actor)
            .unwrap_or(&f64::from(self.xml_scope(scope)?.size)))
    }
    fn ready(&self, service: Service, b: &Behavior, step: Step) -> Result<bool> {
        if service == Service::Map {
            let e = self.xml_scope(step.token.scope)?;
            if self.xml.stamps.contains_key(&e.id) {
                return Ok(true);
            }
            let Some(Value::Map(id)) = value(b, F::Map) else {
                bail!("missing linked stamp")
            };
            let stamp = match self.xml_prepare_stamp(*id, e.pos) {
                Err(error) if error.is::<maps::StampBlocked>() => return Ok(false),
                stamp => stamp,
            }
            .map_err(|error| {
                let context = format!("{error:#}; {} actor={} PlaceMap readiness", b.source, e.id);
                error.context(context)
            })?;
            return Ok(self.xml_stamp_capacity(&stamp, step.token.scope.identity));
        }
        if service == Service::Movement {
            return self.xml_motion_ready(b, step);
        }
        if service == Service::Attack {
            if matches!(b.kind, K::Shoot | K::Explode)
                && objects(b, F::ChildId).is_empty()
                && self.bullets.len() + n(b, F::NumShots) as usize > 2048
            {
                return Ok(false);
            }
            if b.kind == K::GrenadeToss && self.xml.blasts.len() >= 128 {
                return Ok(false);
            }
            if b.kind == K::Explode && n(b, F::Range) > 0. {
                return Ok(!self
                    .xml_targets(step.token.scope, n(b, F::Range), false)?
                    .is_empty());
            }
            return Ok(!targeted(b) || self.xml_attack_target(b, step)?.is_some());
        }
        if service == Service::Spawn {
            return self.xml_spawn_ready(b, step);
        }
        Ok(match service {
            Service::Heal => !self.xml_heal_targets(b, step)?.is_empty(),
            Service::DamageRecord => !self
                .xml_members(step.token.scope, objects(b, F::TargetId), 40.)?
                .is_empty(),
            Service::RemoveOnEmpty => self.players.values().all(|p| !eligible(p)),
            _ => true,
        })
    }
    fn advance(&mut self, b: &Behavior, step: Step) -> Result<MotionProgress> {
        if !self.xml.motions.contains_key(&step.token) {
            return Ok(MotionProgress::Idle);
        }
        let moved = self
            .xml_move(b, step)
            .with_context(|| format!("{} behavior={}", b.source, b.local))?;
        Ok(if moved {
            MotionProgress::Moving
        } else if b.kind != K::Follow {
            MotionProgress::Idle
        } else if self.xml.motions.contains_key(&step.token) {
            MotionProgress::Tracking
        } else {
            MotionProgress::ChaseEnded
        })
    }
    fn apply(
        &mut self,
        service: Service,
        b: &Behavior,
        step: Step,
        rng: &mut rand::rngs::StdRng,
    ) -> Result<ActionOutcome> {
        self.xml_scope(step.token.scope)?;
        if let Some(sound) = opt_n(b, F::SoundId) {
            self.xml_validate_sound(self.xml_scope(step.token.scope)?.kind, sound as u32)?;
        }
        match service {
            Service::Movement => {
                let before = rng.clone();
                let prior = self.xml.motions.get(&step.token).cloned();
                let attempt = (|| -> Result<bool> {
                    if !self.xml_start_motion(b, step, rng)? {
                        return Ok(false);
                    }
                    let ready = self.xml_move(b, step)?;
                    if ready && step.delta_ms == 0 && b.kind == K::Shadow {
                        self.xml_shadow_teleport(b, step)?;
                    }
                    Ok(ready)
                })()
                .with_context(|| {
                    format!(
                        "{} behavior={} speed={} delta_ms={}",
                        b.source,
                        b.local,
                        n(b, F::Speed),
                        step.delta_ms
                    )
                });
                if b.kind == K::Follow
                    && matches!(attempt, Ok(false))
                    && self.xml.motions.contains_key(&step.token)
                {
                    return Ok(ActionOutcome::Tracking);
                }
                if !matches!(attempt, Ok(true)) {
                    match prior {
                        Some(m) => {
                            self.xml.motions.insert(step.token, m);
                        }
                        None => {
                            self.xml.motions.remove(&step.token);
                        }
                    }
                    match attempt {
                        Ok(false) => {
                            *rng = before;
                            return Ok(ActionOutcome::Blocked);
                        }
                        Err(error) => return Err(error),
                        _ => unreachable!(),
                    }
                }
            }
            Service::Attack => return self.xml_attack(b, step, rng),
            Service::Spawn => return self.xml_spawn(b, step, rng),
            Service::Aura => self.xml_aura(b, step)?,
            Service::Heal => self.xml_heal(b, step)?,
            Service::Map => return self.xml_place_map(b, step, rng),
            Service::Ground => {
                let Some(Value::Grounds(grounds)) = value(b, F::GroundId) else {
                    bail!("missing ChangeGround linked ground")
                };
                let pos = self.xml_scope(step.token.scope)?.pos;
                let i = pos.y as usize * self.map.width as usize + pos.x as usize;
                let ground = *grounds.first().context("empty ground list")?;
                Arc::make_mut(&mut self.map).cells[i].ground = ground;
                let tile = (pos.x.floor() as i16, pos.y.floor() as i16);
                for p in self
                    .players
                    .values_mut()
                    .filter(|p| p.tiles.contains(&tile))
                {
                    p.tile_updates.push((tile.0, tile.1, ground));
                }
                self.invalidate_sight();
            }
            Service::Delete => {
                let pos = self.xml_scope(step.token.scope)?.pos;
                let ids: Vec<_> = self
                    .entities_near(pos, n(b, F::Radius) as f32)
                    .filter(|e| objects(b, F::ObjectId).contains(&e.kind))
                    .map(|e| e.id)
                    .collect();
                if ids.iter().any(|id| self.entities[id].reserved) {
                    return Ok(ActionOutcome::Blocked);
                }
                for id in ids {
                    self.remove_entity(id);
                }
            }
            Service::DamageRecord => {
                let Some(id) = self
                    .xml_members(step.token.scope, objects(b, F::TargetId), 40.)?
                    .first()
                    .copied()
                else {
                    return Ok(ActionOutcome::Blocked);
                };
                let record = self.entities[&id].damage.clone();
                self.entities
                    .get_mut(&step.token.scope.identity.actor)
                    .context("copy receiver absent")?
                    .damage = record;
            }
            Service::Visual => self.xml_visual(b, step)?,
            Service::Speech => return self.xml_speech(b, step, rng),
            Service::Sound => {
                self.xml_sound(step.token.scope.identity.actor, n(b, F::SoundId) as u32)?
            }
            Service::Visibility => self.xml_refresh_interest(step.token.scope.identity.actor)?,
            Service::Breath => {
                let pos = self.xml_scope(step.token.scope)?.pos;
                let increment = n(b, F::Speed) * step.delta_ms as f64 / 1000.;
                let mut grants = Vec::new();
                for (id, p) in self.players.iter().filter(|(_, p)| {
                    // The recovered vent uses a strict radius (behavior.cc:2672–2683).
                    eligible(p) && f64::from(p.pos.distance(pos)) < n(b, F::Radius)
                }) {
                    let credit =
                        self.xml.breath.get(&p.character.id).copied().unwrap_or(0.) + increment;
                    ensure!(
                        credit.is_finite() && credit >= 0. && credit.floor() <= i32::MAX as f64,
                        "native breath credit exceeds integer status capacity"
                    );
                    grants.push((*id, p.character.id, credit.floor() as i32, credit.fract()));
                }
                for (id, character, amount, remainder) in grants {
                    self.xml.breath.insert(character, remainder);
                    let p = self
                        .players
                        .get_mut(&id)
                        .expect("preflighted breath recipient");
                    if p.oxygen >= 0 {
                        p.oxygen = (i64::from(p.oxygen) + i64::from(amount)).min(100) as i32;
                    }
                }
            }
            Service::RemoveOnEmpty => {
                self.remove_entity(step.token.scope.identity.actor);
                return Ok(ActionOutcome::Removed);
            }
        }
        Ok(ActionOutcome::Applied)
    }
    fn announce(&mut self, scope: Scope, a: &Announcement) -> Result<()> {
        self.xml.retry_admissions.insert(scope.identity.actor);
        self.xml_prepare_announcement(scope, a)?;
        if let Some(prior) = self.xml.admissions.get(&a.key) {
            ensure!(prior == a, "announcement activation payload changed");
        } else {
            self.xml_publish_announcement(a);
            self.xml.admissions.insert(a.key, a.clone());
        }
        self.xml.retry_admissions.remove(&scope.identity.actor);
        Ok(())
    }
    fn terminal(&mut self, scope: Scope, plan: &TerminalPlan) -> Result<()> {
        // An Order listener's plan keeps its order scope as origin (behavior.rs merge_terminal); the retiring actor must match.
        ensure!(
            scope.identity == plan.origin.identity,
            "terminal origin mismatch"
        );
        self.xml_terminal(plan)
    }
    fn effect(&mut self, scope: Scope, effect: Effect<'_>) -> Result<()> {
        ensure!(
            scope.identity.world == self.xml.world,
            "cross-world XML effect"
        );
        match effect {
            Effect::Enter(state) => {
                self.xml_activations_mut(scope)
                    .entry(scope)
                    .or_default()
                    .insert(state);
                if scope.order.is_none() {
                    self.xml.unstarted.remove(&scope.identity.actor);
                }
                self.xml_refresh_interest(scope.identity.actor)?;
                tracing::debug!(actor=scope.identity.actor,state,?scope.order,"XML state entered");
            }
            Effect::Exit(state) => {
                let activations = self.xml_activations_mut(scope);
                if let Some(states) = activations.get_mut(&scope) {
                    states.remove(&state);
                    if states.is_empty() {
                        activations.remove(&scope);
                    }
                }
                self.xml_refresh_interest(scope.identity.actor)?;
                tracing::debug!(actor=scope.identity.actor,state,?scope.order,"XML state exited");
            }
            Effect::ForgetLocal(token) => {
                self.xml.protectors.remove(&token);
                self.xml.motions.remove(&token);
                self.xml.deadlines.remove(&token);
                self.xml.auras.retain(|(t, _, _), _| *t != token);
                self.xml_refresh_auras();
            }
            Effect::ScheduleLifecycle {
                deadline, delay_ms, ..
            } => {
                self.xml.deadlines.insert(
                    deadline.owner,
                    (
                        self.now
                            .checked_add(delay_ms)
                            .context("lifecycle clock overflow")?,
                        deadline,
                    ),
                );
            }
            Effect::Condition {
                owner,
                condition,
                enabled,
            } => self.xml_condition(owner, condition, enabled)?,
            Effect::GlobalTag {
                symbol,
                present,
                notify,
            } => {
                if present {
                    self.xml.tags.insert(symbol);
                } else {
                    self.xml.tags.remove(&symbol);
                }
                if notify {
                    let text = self
                        .xml_game()
                        .symbols
                        .get(symbol as usize)
                        .context("global tag symbol absent")?;
                    let mut packet = Writer::new();
                    packet.i32(scope.identity.actor).utf(text).u32(0xffffff00);
                    for p in self.players.values() {
                        send(p, wire::NOTIFICATION, packet.0.clone());
                    }
                }
            }
            Effect::PublishOrder {
                state,
                local,
                generation,
                targets,
            } => {
                let definition = self
                    .xml_program(scope)
                    .context("stable order definition absent")?
                    .clone();
                self.xml.orders.insert(
                    (scope.identity.actor, local),
                    Published {
                        scope,
                        definition,
                        state,
                        generation,
                        targets: targets.map(<[u16]>::to_vec),
                    },
                );
            }
            Effect::WithdrawOrder(local) => {
                self.xml_withdraw_orders(vec![(scope.identity.actor, local)])
            }
            Effect::ClearAltTexture => {
                if let Some(entity) = self.entities.get_mut(&scope.identity.actor) {
                    Self::xml_alt_texture(entity, 0);
                }
            }
            Effect::Size(size) => {
                ensure!(
                    size.is_finite() && size >= 0. && size <= i32::MAX as f64,
                    "invalid native size"
                );
                self.xml.sizes.insert(scope.identity.actor, size);
                self.entities
                    .get_mut(&scope.identity.actor)
                    .context("size actor absent")?
                    .size = size.round() as i32;
            }
        }
        Ok(())
    }
}
fn targeted(b: &Behavior) -> bool {
    !matches!(
        value(b, F::Type),
        Some(Value::Mode(crate::content::game::Mode::Auto))
    ) && !matches!(value(b, F::Type), Some(Value::Nova))
        && b.kind != K::Explode
}

impl World {
    fn xml_heal_targets(&self, b: &Behavior, step: Step) -> Result<Vec<i32>> {
        if b.kind == K::HealZap {
            // Recovered HealZapBehavior::Update (behavior.cc:1995-2027) heals only the closest damaged player.
            return Ok(self
                .xml_targets(step.token.scope, n(b, F::Range), true)?
                .into_iter()
                .filter(|(id, _)| {
                    cooperative::ready(&self.players[id])
                        && self.players[id].character.hp
                            < self.players[id]
                                .effective(&self.content, self.now)
                                .expect("validated player stats")[0]
                })
                .map(|(id, _)| id)
                .take(1)
                .collect());
        }
        let mut kinds = objects(b, F::TargetIds).to_vec();
        kinds.extend_from_slice(objects(b, F::TargetGroups));
        ensure!(
            !kinds.is_empty(),
            "Heal needs linked targetIds/targetGroups"
        );
        let mut ids = self.xml_members(step.token.scope, &kinds, n(b, F::Range))?;
        // Recovered FindHealTargets scans the census around the host, so a host in its own target group heals itself (behavior.cc:1306-1317).
        let host = self.xml_scope(step.token.scope)?;
        if kinds.contains(&host.kind) {
            ids.insert(0, host.id);
        }
        ids.retain(|id| self.entities[id].hp < self.content.objects[&self.entities[id].kind].hp);
        // DECA heal.md: an absent maxTargets (-1) heals every target.
        if let Some(max) = opt_n(b, F::MaxTargets) {
            ids.truncate(max as usize);
        }
        Ok(ids)
    }
    fn xml_heal(&mut self, b: &Behavior, step: Step) -> Result<()> {
        let targets = self.xml_heal_targets(b, step)?;
        if !targets.is_empty() && (b.kind == K::HealZap || !flag(b, F::HideVisual)) {
            self.xml_publish_actor(step.token.scope.identity.actor)?;
        }
        for id in targets {
            if b.kind == K::HealZap {
                let max = self.players[&id].effective(&self.content, self.now)?[0];
                let p = self.players.get_mut(&id).expect("heal preflight target");
                let before = p.character.hp;
                if p.conditions(self.now) & crate::condition::bit(crate::condition::SICK) == 0 {
                    p.character.hp = (i64::from(p.character.hp) + n(b, F::Healing) as i64)
                        .min(i64::from(max)) as i32;
                }
                p.audit_hp(id, &self.name, self.now, "xml_heal_zap", before);
                let (gained, pos) = (p.character.hp - before, p.pos);
                let healer = step.token.scope.identity.actor;
                if gained > 0 {
                    // Base fountain feedback (cooperative.rs:217-226): "+N" text and the heal effect.
                    let mut out = Writer::new();
                    out.i32(id).utf(&format!("+{gained}")).u32(0xff00ff00);
                    for p in self
                        .players
                        .values()
                        .filter(|p| p.known.contains(&healer) && p.known.contains(&id))
                    {
                        send(p, wire::NOTIFICATION, out.0.clone());
                        send(p, wire::SHOW_EFFECT, cooperative::effect(1, id, pos));
                    }
                }
                self.xml_lightning(
                    step.token.scope.identity.actor,
                    self.players[&id].pos,
                    color(b, 0xffffff),
                )?;
            } else if flag(b, F::HideVisual) {
                let max = self.content.objects[&self.entities[&id].kind].hp;
                let e = self.entities.get_mut(&id).expect("heal preflight target");
                if e.conditions(self.now) & crate::condition::bit(crate::condition::SICK) == 0 {
                    // DECA heal.md: an absent maxHeal (-1) heals to full.
                    e.hp = opt_n(b, F::MaxHeal).map_or(max, |heal| {
                        (i64::from(e.hp) + heal as i64).min(i64::from(max)) as i32
                    });
                }
            } else {
                self.heal_enemy(
                    step.token.scope.identity.actor,
                    id,
                    opt_n(b, F::MaxHeal).map_or(
                        f64::from(self.content.objects[&self.entities[&id].kind].hp),
                        |heal| {
                            heal.min(f64::from(self.content.objects[&self.entities[&id].kind].hp))
                        },
                    ) as i32,
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn xml_clear_player_effects(&mut self, player: i32, character: i32) {
        self.xml.auras.retain(|(_, target, _), _| *target != player);
        self.xml.breath.remove(&character);
        self.xml
            .chat
            .retain(|(_, speaker, _, _)| *speaker != player);
        if let Some(p) = self.players.get_mut(&player) {
            p.xml_conditions = 0;
        }
    }
    fn xml_refresh_auras(&mut self) {
        for p in self.players.values_mut() {
            p.xml_conditions = 0;
        }
        for ((_, id, index), until) in &self.xml.auras {
            if *until > self.now
                && let Some(p) = self.players.get_mut(id)
            {
                p.xml_conditions |= crate::condition::bit(*index);
            }
        }
    }
    fn xml_aura(&mut self, b: &Behavior, step: Step) -> Result<()> {
        let conditions = match value(b, F::Effect) {
            Some(Value::Conditions(cs)) => cs,
            other => bail!("invalid linked aura effects {other:?}"),
        };
        let until = self
            .now
            .checked_add(ms(b, F::Cooldown))
            .and_then(|n| n.checked_add(50))
            .context("aura expiry overflow")?;
        let ids = self.xml_targets(
            step.token.scope,
            n(b, F::Range),
            // ConditionEffectAura has no ignoreInvisible attribute and always skips invisible players.
            !(b.kind == K::ConditionEffectAura || flag(b, F::IgnoreInvisible)),
        )?;
        for (id, _) in ids {
            for c in conditions {
                let index = match c {
                    Condition::Selected(i) => *i,
                    Condition::StunImmune => 12,
                };
                if b.kind == K::ClearConditionEffectAura {
                    self.players
                        .get_mut(&id)
                        .expect("aura preflight target")
                        .statuses
                        .remove(&index);
                    self.xml
                        .auras
                        .retain(|(_, target, effect), _| *target != id || *effect != index);
                } else {
                    self.xml.auras.insert((step.token, id, index), until);
                }
            }
        }
        self.xml_refresh_auras();
        Ok(())
    }
    fn xml_publish_actor(&mut self, id: i32) -> Result<()> {
        let actor = self
            .entities
            .get(&id)
            .context("XML publication actor absent")?;
        let always_visible = self.xml.visible.contains(&id);
        let missing: Vec<_> = self
            .players
            .iter()
            .filter(|(_, p)| {
                !p.known.contains(&id)
                    && (always_visible || actor.pos.distance(p.pos) < PLAYER_VIEW_RADIUS)
                    && Self::entity_visible_to(actor, p)
            })
            .map(|(&player, _)| player)
            .collect();
        for p in missing {
            self.updates(p, false)?;
        }
        Ok(())
    }
    fn xml_effect(&mut self, effect: u8, owner: i32, a: Pos, b: Pos, color: u32) -> Result<()> {
        self.xml_publish_actor(owner)?;
        self.xml_effect_known(effect, owner, a, b, color);
        Ok(())
    }
    fn xml_effect_known(&self, effect: u8, owner: i32, a: Pos, b: Pos, color: u32) {
        let mut out = Writer::new();
        out.u8(effect)
            .i32(owner)
            .f32(a.x)
            .f32(a.y)
            .f32(b.x)
            .f32(b.y)
            .u32(color);
        for p in self.players.values().filter(|p| {
            p.known.contains(&owner)
                && self
                    .entities
                    .get(&owner)
                    .is_some_and(|e| e.accessible(p.character.account))
        }) {
            send(p, wire::SHOW_EFFECT, out.0.clone());
        }
    }
    fn xml_diffuse(&mut self, owner: i32, center: Pos, radius: f32, color: u32) -> Result<()> {
        self.xml_effect(
            8,
            owner,
            center,
            Pos {
                x: center.x + radius,
                y: center.y,
            },
            color,
        )
    }
    fn xml_lightning(&mut self, owner: i32, endpoint: Pos, color: u32) -> Result<()> {
        self.xml_effect(11, owner, endpoint, Pos { x: 100., y: 0. }, color)
    }
    fn xml_shoot_blast(&mut self, b: &Behavior, owner: i32) -> Result<()> {
        if b.kind == K::Shoot && flag(b, F::BlastEffect) {
            // The selected client has no ConeBlast renderer; this authored binding is a cosmetic one-tile AreaBlast.
            self.xml_effect(
                5,
                owner,
                Pos { x: 1., y: 0. },
                Pos { x: 0., y: 0. },
                0xffffffff,
            )?;
        }
        Ok(())
    }
}
fn color(b: &Behavior, default: u32) -> u32 {
    match value(b, F::Color) {
        Some(Value::Colors(cs)) => cs[0],
        None => default,
        other => panic!("invalid compiled color {other:?}"),
    }
}

impl World {
    pub(super) fn xml_live_count(&self) -> usize {
        self.xml.runnable.len() + self.xml.births.len() + self.pending_deployments()
    }
    pub(super) fn xml_reserve_ids(&self, count: usize) -> Result<Vec<i32>> {
        ensure!(
            self.entities.len() + self.xml.births.len() + self.pending_deployments() + count
                <= PHYSICAL_LIMIT,
            "physical entity batch bound exceeded before IDs: entities={} births={} deployments={} count={count} limit={PHYSICAL_LIMIT}",
            self.entities.len(),
            self.xml.births.len(),
            self.pending_deployments()
        );
        let count = i32::try_from(count)?;
        if count == 0 {
            return Ok(vec![]);
        }
        let start = self
            .ids
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                n.checked_add(count).filter(|end| *end < i32::MAX)
            })
            .map_err(|_| anyhow::anyhow!("XML entity IDs exhausted"))?;
        Ok((start..start + count).collect())
    }
    fn xml_own(
        &mut self,
        id: i32,
        parent: Option<i32>,
        encounter: Option<u64>,
        population_terrain: Option<u8>,
    ) -> Result<()> {
        ensure!(self.entities.contains_key(&id), "new child absent");
        self.mark_population_child(id, population_terrain);
        self.set_parent(id, parent);
        self.xml.identities.insert(
            id,
            Identity {
                world: self.xml.world,
                actor: id,
                parent,
                encounter,
            },
        );
        Ok(())
    }
    /// Only a defeatable replacement inherits the quest; a portal birth (Sphinx -> Tomb) or an abandoned replacement
    /// ends the line.
    pub(super) fn xml_quest_heir(&self, births: &[(u16, Pos, bool)]) -> Option<usize> {
        births
            .iter()
            .position(|(kind, _, replacement)| *replacement && self.content.objects[kind].enemy)
    }
    /// Release7 behavior.cc:614-622 and 748-764 create death births and victory portals at host+offset without
    /// collision checks; a blocked offset takes the host's own position, which it occupied. Only a host on
    /// unplaceable ground reaches the bounded abandon.
    fn xml_terminal_birth_position(&self, kind: u16, host: &Entity, offset: Pos) -> Pos {
        match self.xml_preflight_publication(kind, offset, &[host.id], None) {
            Ok(()) => offset,
            Err(error) => {
                tracing::debug!(actor = host.id, kind, x = offset.x, y = offset.y, error = %format!("{error:#}"), "terminal birth offset blocked; placed at host position");
                host.pos
            }
        }
    }
    pub(crate) fn template_id(&self) -> Option<&str> {
        self.xml.template.as_deref()
    }

    pub(super) fn xml_world_template(&self) -> Option<&crate::content::game::World> {
        self.xml_game()
            .world_template(self.xml.template.as_deref()?)
    }
    fn xml_victory_birth(&self, host: &Entity) -> Result<Option<(u16, Pos, bool)>> {
        use crate::content::game::ClearRoute;
        let rule = self
            .xml_world_template()
            .and_then(|t| t.clear.as_ref())
            .filter(|r| r.boss == host.kind)
            .with_context(|| {
                format!(
                    "VictoryAtDeath actor {:#x} has no selected Clear intent in {:?}",
                    host.kind, self.xml.template
                )
            })?;
        let kind = match rule.route {
            ClearRoute::Victory(kind) => kind,
            ClearRoute::None => return Ok(None),
            _ => bail!(
                "VictoryAtDeath actor {:#x} has incompatible Clear intent in {:?}",
                host.kind,
                self.xml.template
            ),
        };
        self.xml_portal_definition(kind, "VictoryAtDeath terminal factory")?;
        // The +0.2 offset only keeps the portal off the corpse's loot bag.
        let offset = Pos {
            x: host.pos.x + 0.2,
            y: host.pos.y + 0.2,
        };
        Ok(Some((
            kind,
            self.xml_terminal_birth_position(kind, host, offset),
            false,
        )))
    }
    fn xml_plan_terminal_births(
        &mut self,
        plan: &TerminalPlan,
        e: &Entity,
    ) -> Result<Vec<(u16, Pos, bool)>> {
        #[cfg(test)]
        if std::mem::take(&mut self.xml.fail_next_terminal_birth_plan) {
            bail!("injected terminal birth preflight failure");
        }
        let mut births = Vec::new();
        if plan.reason == TerminalReason::Metamorphose {
            let b = plan
                .behavior
                .as_ref()
                .context("metamorphose initiator absent")?;
            let kind = *objects(b, F::ChildId)
                .first()
                .context("replacement kind absent")?;
            births.push((kind, e.pos, true));
        }
        for hook in &plan.hooks {
            let b = &hook.behavior;
            let mut rng = rand::rngs::StdRng::seed_from_u64(hook.seed);
            match b.kind {
                K::MetamorphoseAtDeath => {
                    let kind = *objects(b, F::ChildId)
                        .first()
                        .context("death replacement kind absent")?;
                    let pos = point(
                        e.pos,
                        rng.gen_range(0.0..std::f64::consts::TAU),
                        n(b, F::PosJitter) * rng.r#gen::<f64>(),
                    );
                    births.push((kind, self.xml_terminal_birth_position(kind, e, pos), true));
                }
                K::ReleaseSpawnAtDeath => {
                    let min = n(b, F::Min) as usize;
                    // An absent max spawns exactly min.
                    let max = opt_n(b, F::Max).map_or(min, |max| max as usize);
                    ensure!(
                        min <= max && max <= CHILD_LIMIT,
                        "death spawn bounds invalid"
                    );
                    let kind = *objects(b, F::ChildId)
                        .first()
                        .context("death child kind absent")?;
                    for _ in 0..rng.gen_range(min..=max) {
                        let pos = point(
                            e.pos,
                            rng.gen_range(0.0..std::f64::consts::TAU),
                            rng.r#gen::<f64>() * n(b, F::Range),
                        );
                        births.push((kind, self.xml_terminal_birth_position(kind, e, pos), false));
                    }
                }
                K::VictoryAtDeath => births.extend(self.xml_victory_birth(e)?),
                _ => bail!("unsupported terminal hook {:?}", b.kind),
            }
        }
        ensure!(
            self.xml_live_count().saturating_sub(1) + births.len() <= ACTOR_LIMIT
                && births.len() <= CHILD_LIMIT,
            "terminal births exceed live bounds: live={} births={} limit={ACTOR_LIMIT} child_limit={CHILD_LIMIT}",
            self.xml_live_count(),
            births.len()
        );
        let released = births
            .iter()
            .filter(|(_, _, replacement)| !*replacement)
            .count();
        ensure!(
            self.xml_owned_count(e.id) + released <= CHILD_LIMIT,
            "terminal births exceed parent child budget: owned={} births={released} limit={CHILD_LIMIT}",
            self.xml_owned_count(e.id)
        );

        Ok(births)
    }
    fn xml_terminal(&mut self, plan: &TerminalPlan) -> Result<()> {
        let identity = plan.origin.identity;
        ensure!(
            identity.world == self.xml.world,
            "cross-world terminal claim"
        );
        if self.xml.claims.contains(&identity.actor) {
            self.xml.retry_terminals.remove(&identity.actor);
            return Ok(());
        }
        self.xml.retry_terminals.insert(identity.actor);
        for a in &plan.announcements {
            self.xml_prepare_announcement(a.key.owner.scope, a)?;
        }
        if let Some(pending) = self.xml.terminals.get(&identity.actor)
            && !pending.receipt
        {
            return Err(terminal::TerminalWait(format!(
                "terminal event {} awaiting durable receipt; original plan retained",
                pending.event.id
            ))
            .into());
        }
        let e = self
            .entities
            .get(&identity.actor)
            .context("terminal actor absent")?
            .clone();
        let stamp = self
            .xml
            .stamps
            .get(&e.id)
            .is_some_and(|stamp| !stamp.published);
        ensure!(
            !e.reserved
                || stamp
                || self.xml.facts.contains_key(&e.id)
                || self
                    .xml
                    .terminals
                    .get(&e.id)
                    .is_some_and(|pending| pending.receipt),
            "{}:{} object={} terminal actor={} kind={:#x} world={} reservation conflicts with lifecycle; requires phase4 atomic inventory/expiry reconciliation",
            self.xml_game().definitions[&e.kind].source.file,
            self.xml_game().definitions[&e.kind].source.line,
            self.xml_game().definitions[&e.kind].source.object,
            e.id,
            e.kind,
            self.xml.world
        );
        // Defeat-time facts and seed precede every retryable stamp/capacity/geometry preflight.
        self.capture_terminal_facts(plan, &e)?;
        ensure!(
            !stamp,
            "terminal actor={} awaits its unpublished PlaceMap stamp receipt; defeat facts/seed retained",
            e.id
        );
        let mut abandoned = Vec::new();
        let local = !self.xml.terminals.contains_key(&e.id);
        let mut births = if local {
            let planned = self.xml_plan_terminal_births(plan, &e);
            self.terminal_physical(
                e.id,
                || serde_json::json!("scripted_birth_plan"),
                planned,
                &mut abandoned,
            )?
            .unwrap_or_default()
        } else {
            serde_json::from_value(
                self.xml.terminals[&e.id].event.terminal_payload()?.actions["publication"]["births"].clone(),
            )?
        };
        let mut checked = Vec::with_capacity(births.len());
        for (kind, pos, replacement) in births {
            let placement = self.xml_preflight_publication(kind, pos, &[e.id], None);
            if self
                .terminal_physical(
                    e.id,
                    || serde_json::json!({"kind":kind,"position":pos,"replacement":replacement}),
                    placement,
                    &mut abandoned,
                )?
                .is_some()
            {
                checked.push((kind, pos, replacement));
            }
        }
        births = checked;
        if local {
            self.stage_terminal(plan, &e, &births, abandoned)?;
        } else if !abandoned.is_empty() {
            tracing::error!(actor=e.id,event=%self.xml.terminals[&e.id].event.id, ?abandoned, "journaled terminal births abandoned at publication");
        }
        let clear_portal: Option<(u16, Pos)> = if local {
            None
        } else {
            serde_json::from_value(self.xml.terminals[&e.id].event.terminal_payload()?.actions["publication"]["clear_portal"].clone())?
        };
        if let Some((kind, pos)) = clear_portal {
            match self.xml_preflight_publication(kind, pos, &[e.id], None) {
                Ok(()) => births.push((kind, pos, false)),
                Err(error) if self.xml.abandon_physical.contains(&e.id) => {
                    tracing::error!(actor = e.id, event = %self.xml.terminals[&e.id].event.id, kind, x = pos.x, y = pos.y, error = %format!("{error:#}"), "journaled clear portal physical_effect_abandoned at publication");
                }
                Err(error) => return Err(error),
            }
        }
        ensure!(
            self.xml_live_count().saturating_sub(1) + births.len() <= ACTOR_LIMIT
                && self.xml_owned_count(e.id)
                    + births
                        .iter()
                        .filter(|(_, _, replacement)| !*replacement)
                        .count()
                    <= CHILD_LIMIT,
            "combined terminal portal/birth budget changed: live={} births={} limit={ACTOR_LIMIT} child_limit={CHILD_LIMIT}",
            self.xml_live_count(),
            births.len()
        );
        if matches!(
            plan.behavior
                .as_ref()
                .and_then(|b| value(b, F::ParticleEffect)),
            Some(Value::Nova)
        ) {
            self.xml_publish_actor(e.id)?;
        }
        let ids = self.xml_reserve_ids(births.len())?;
        if births.iter().any(|(kind, _, _)| self.xml_is_portal(*kind))
            && let Some(exit) = self.clear_exit.take()
        {
            self.remove_entity(exit);
        }
        let heir = self.xml_quest_heir(&births);
        let successor = e.quest.is_some() && heir.is_some();
        let population_terrain = self.population_origin(&e);
        for (index, ((kind, p, replacement), id)) in births.into_iter().zip(ids).enumerate() {
            let event = self.xml.terminals[&e.id].event.id.clone();
            self.xml_insert_publication(id, kind, p, &event)
                .expect("preflighted journaled terminal publication");
            if clear_portal.is_some_and(|(clear, _)| clear == kind) {
                self.clear_exit = Some(id);
            }
            self.xml_own(
                id,
                if replacement {
                    identity.parent
                } else {
                    Some(e.id)
                },
                identity.encounter,
                population_terrain,
            )
            .expect("preflighted terminal ownership");
            self.xml_activate_publication(id)
                .expect("preflighted terminal portal activation");
            if replacement {
                self.entities
                    .get_mut(&id)
                    .expect("preflighted replacement")
                    .damage = e.damage.clone();
            }
            if heir == Some(index) {
                // A transformed quest root stays the same quest: arrow, encounter and Realm-cycle credit follow it.
                self.set_quest(id, *e.quest);
            }
        }
        if self.xml.claims.insert(e.id)
            && self.encounter
            && let Some(credit) = crate::endgame::critical_credit(
                self.xml_game().site_role(e.kind) == Some(crate::content::game::SiteRole::Hero),
                e.id,
                *e.quest,
                successor,
                self.xml.quest_carriers.contains(&e.kind),
            )
        {
            self.critical_deaths.push(credit);
            // Base enemy_damage.rs:164-178: the living killer names the Realm's "killed my <boss>" notice.
            if self.template_id() == Some("realm")
                && let Some(TerminalContext::PlayerKill { player, .. }) = &plan.context
                && let Some(p) = self.players.get(player).filter(|p| {
                    p.character.alive
                        && !p.death_pending
                        && !p.departing
                        && !p.character.name.is_empty()
                })
            {
                self.critical_killers
                    .insert(credit, (e.kind, p.character.name.clone()));
            }
        }
        self.xml.abandon_physical.remove(&e.id);
        self.xml.retry_since.remove(&e.id);
        if let Some(b) = plan
            .behavior
            .as_ref()
            .filter(|b| matches!(value(b, F::ParticleEffect), Some(Value::Nova)))
        {
            self.xml_effect_known(
                8,
                e.id,
                e.pos,
                Pos {
                    x: e.pos.x + 1.,
                    y: e.pos.y,
                },
                color(b, 0xffffffff),
            );
        }
        for a in &plan.announcements {
            self.xml_publish_announcement(a);
        }
        self.entities.get_mut(&e.id).unwrap().reserved = false;
        self.remove_entity(e.id);
        let actor = e.id;
        if plan.rewards.loot
            || plan.rewards.xp
            || plan
                .hooks
                .iter()
                .any(|h| h.behavior.kind == K::VictoryAtDeath)
        {
            // A rewarded local terminal had no recipient or loot; stage_terminal logged it at DEBUG.
            if !local {
                tracing::info!(
                    event=%self.xml.terminals[&e.id].event.id,
                    actor = e.id,
                    kind = e.kind,
                    loot = plan.rewards.loot,
                    xp = plan.rewards.xp,
                    "XML terminal effects published from exact durable journal receipt"
                );
            }
        }
        self.xml.retry_terminals.remove(&actor);
        if local {
            self.finish_local_terminal(actor)?;
        }
        Ok(())
    }
    fn xml_prepare_announcement(&self, scope: Scope, a: &Announcement) -> Result<()> {
        self.xml_scope(scope)?;
        ensure!(
            a.key.owner.scope == scope,
            "announcement owner scope mismatch"
        );
        ensure!(
            self.xml_game().string_lists.get(a.list as usize).is_some(),
            "announcement numeric list absent"
        );
        ensure!(
            a.text.len() <= u16::MAX as usize,
            "announcement text exceeds native UTF length"
        );
        Ok(())
    }
    fn xml_publish_announcement(&self, a: &Announcement) {
        let packet = chat::text_packet("Oryx the Mad God", -1, 0, &a.text);
        for p in self.players.values() {
            send(p, packet.id, packet.body.clone());
        }
        tracing::info!(world=self.xml.world,actor=a.key.owner.scope.identity.actor,?a.key, list=a.list, choice=a.choice,source=%a.source.file,"XML lifecycle announcement published");
    }
    fn xml_alt_texture(entity: &mut Entity, texture: i32) {
        entity.extra_stats.retain(|(stat, _)| *stat != 61);
        entity.extra_stats.push((61, texture));
    }
    fn xml_visual(&mut self, b: &Behavior, step: Step) -> Result<()> {
        let id = step.token.scope.identity.actor;
        if b.kind == K::SetAltTexture {
            let e = self.entities.get_mut(&id).context("texture actor absent")?;
            Self::xml_alt_texture(e, n(b, F::AltTextureId) as i32);
        } else {
            let color = match value(b, F::Color) {
                Some(Value::Colors(cs)) => *cs.first().context("empty effect colors")?,
                _ => 0xffffffff,
            };
            let period = ms(b, F::FlashPeriod) as f32 / 1000.;
            ensure!(period > 0., "flash period must be positive");
            self.xml_publish_actor(id)?;
            self.npc_flash(id, color, period, period * n(b, F::FlashRepeats) as f32);
        }
        Ok(())
    }
    fn xml_speech(
        &mut self,
        b: &Behavior,
        step: Step,
        rng: &mut rand::rngs::StdRng,
    ) -> Result<ActionOutcome> {
        use crate::content::game::TextPart;
        ensure!(
            b.kind != K::OryxTaunt,
            "OryxTaunt is an executor lifecycle operation, never native Speech"
        );
        let e = self.xml_scope(step.token.scope)?.clone();
        let broadcast = flag(b, F::Broadcast);
        let target = self
            .xml_targets(step.token.scope, 20., true)?
            .first()
            .map(|(id, _)| self.players[id].character.name.as_str());
        let parts = if b.kind == K::Say {
            let Some(Value::TextChoices(cs)) = value(b, F::Text) else {
                bail!("Say choices absent")
            };
            cs.get(step.text_choice.context("Say choice absent")?)
                .context("Say choice invalid")?
        } else {
            let Some(Value::StringList(id)) = value(b, F::ListId) else {
                bail!("Taunt requires linked listId")
            };
            let list = self
                .xml_game()
                .string_lists
                .get(*id as usize)
                .context("linked taunt list absent")?;
            // Release 7.0 behavior.cc:787-796 suppresses local Taunt without a player or line.
            if !broadcast && target.is_none() {
                return Ok(ActionOutcome::Blocked);
            }
            let Some(choices) = list
                .categories
                .get("everySoOften")
                .filter(|cs| !cs.is_empty())
            else {
                return Ok(ActionOutcome::Blocked);
            };
            &choices[rng.gen_range(0..choices.len())]
        };
        let mut text = String::new();
        for part in parts {
            text.push_str(&match part {
                TextPart::Literal(s) => s.clone(),
                TextPart::Hitpoints => e.hp.to_string(),
                TextPart::Count => self
                    .players
                    .values()
                    .filter(|p| eligible(p))
                    .count()
                    .to_string(),
                TextPart::Player => match target {
                    Some(name) => name.to_owned(),
                    // The recovered broadcast-Taunt substitution is literal, not a real recipient.
                    None if b.kind == K::Taunt && broadcast => "Rob".into(),
                    None => return Ok(ActionOutcome::Blocked),
                },
                TextPart::Killer => {
                    bail!("killer placeholder needs terminal reward dialogue context")
                }
            });
        }
        let packet = chat::text_packet(
            &self.content.objects[&e.kind].name,
            e.id,
            if b.kind == K::Say {
                bubble_seconds(b)?
            } else {
                10
            },
            &text,
        );
        self.xml_publish_actor(e.id)?;
        for p in self.players.values().filter(|p| {
            e.accessible(p.character.account)
                && (broadcast || (p.known.contains(&e.id) && p.pos.distance(e.pos) <= 20.))
        }) {
            send(p, packet.id, packet.body.clone());
        }
        Ok(ActionOutcome::Applied)
    }
    fn xml_place_map(
        &mut self,
        b: &Behavior,
        step: Step,
        _rng: &mut rand::rngs::StdRng,
    ) -> Result<ActionOutcome> {
        let Some(Value::Map(id)) = value(b, F::Map) else {
            bail!("missing linked stamp")
        };
        if let Some(pending) = self.xml.stamps.get(&step.token.scope.identity.actor) {
            ensure!(
                pending.source == maps::source(step),
                "PlaceMap original source opportunity changed"
            );
            if !pending.published {
                return Ok(ActionOutcome::Pending);
            }
            let event = pending.event.id.clone();
            self.xml.stamps.remove(&step.token.scope.identity.actor);
            self.release_terminal_event(&event);
            return Ok(ActionOutcome::Applied);
        }
        let stamp = match self.xml_prepare_stamp(*id, self.xml_scope(step.token.scope)?.pos) {
            Err(error) if error.is::<maps::StampBlocked>() => return Ok(ActionOutcome::Blocked),
            stamp => stamp,
        }
        .map_err(|error| {
            let context = format!(
                "{error:#}; {} actor={} PlaceMap execution",
                b.source, step.token.scope.identity.actor
            );
            error.context(context)
        })?;
        if !self.xml_stamp_capacity(&stamp, step.token.scope.identity) {
            return Ok(ActionOutcome::Blocked);
        }
        if stamp.has_portals(self) {
            return self.xml_stage_stamp(stamp, step, b);
        }
        self.xml_apply_stamp(stamp, step.token.scope.identity)?;
        Ok(ActionOutcome::Applied)
    }
    pub(super) fn xml_chat(&mut self, player: i32, text: &str) -> Result<()> {
        let p = self
            .players
            .get(&player)
            .filter(|p| eligible(p))
            .context("XML chat requires an eligible owning-world speaker")?;
        self.xml
            .chat
            .retain(|(at, _, _, _)| self.now.saturating_sub(*at) <= 50);
        ensure!(
            self.xml.chat.len() < 128,
            "XML dialogue event budget exhausted"
        );
        self.xml.chat.push((self.now, player, p.pos, text.into()));
        Ok(())
    }
}
