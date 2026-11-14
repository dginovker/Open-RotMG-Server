use crate::{
    content::Content,
    wire::{self, Packet, Reader},
    world::{
        Action, Pos, Slot,
        view::{OutputState, PlayerView},
    },
};
use anyhow::{Context, Result, ensure};
use rand::{Rng, SeedableRng, seq::IteratorRandom};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

pub(crate) mod adventures;
mod calls;
pub(crate) mod cctv;
mod combat;
mod composer;
mod crowds;
pub(crate) mod dialogue;
mod director;
mod entries;
mod events;
mod intentions;

mod bands;
pub(crate) use bands::REMEMBERED as REMEMBERED_GODLANDS;
mod persona;
mod plan;
mod progression;
pub(crate) mod pursuits;
#[cfg(test)]
mod quest_tests;
mod quests;

mod realms;

mod social;

/// Who a bot is: identity, temperament and skill. Built from `persona::PERSONAS`.
#[derive(Clone, Copy)]
struct Profile {
    // Keep persisted registry keys independent of the names visible to players.
    identity: &'static str,
    // Guest catalog selectors are not account-visible names.
    name: &'static str,
    recovery: f32,
    resources: f32,
    companions: f32,
    reaction: u64,
    attention: usize,
    velocity_step: f32,
    directions: usize,
    pickup_clearance: f32,
    knows_blue: bool,
    knows_minimap: bool,
    stars: i32,
    role: persona::Role,
}
impl Profile {
    fn is_guest(&self) -> bool {
        self.identity.starts_with("guest:v1:")
    }
}

#[cfg(test)]
const ARCHETYPES: [Profile; 6] = [
    Profile {
        identity: "Bayleaf",
        name: "Triceps",
        recovery: 0.62,
        resources: 4.0,
        companions: 3.0,
        reaction: 6,
        attention: 4,
        velocity_step: 1.0,
        directions: 8,
        pickup_clearance: 1.3,
        knows_blue: true,
        knows_minimap: true,
        stars: 20,
        role: persona::Role::Grinder,
    },
    Profile {
        identity: "Mossy",
        name: "Biceps",
        recovery: 0.35,
        resources: 8.0,
        companions: 3.0,
        reaction: 4,
        attention: 8,
        velocity_step: 0.25,
        directions: 16,
        pickup_clearance: 0.8,
        knows_blue: false,
        knows_minimap: true,
        stars: 20,
        role: persona::Role::Grinder,
    },
    Profile {
        identity: "Trellis",
        name: "BattleCrus",
        recovery: 0.48,
        resources: 3.0,
        companions: 8.0,
        reaction: 5,
        attention: 2,
        velocity_step: 2.0,
        directions: 8,
        pickup_clearance: 1.05,
        knows_blue: false,
        knows_minimap: false,
        stars: 20,
        role: persona::Role::Grinder,
    },
    Profile {
        identity: "Juniper",
        name: "Kadabu",
        recovery: 0.55,
        resources: 3.0,
        companions: 7.0,
        reaction: 5,
        attention: 6,
        velocity_step: 0.5,
        directions: 16,
        pickup_clearance: 1.5,
        knows_blue: true,
        knows_minimap: true,
        stars: 20,
        role: persona::Role::Grinder,
    },
    Profile {
        identity: "Reed",
        name: "ExElite",
        recovery: 0.42,
        resources: 4.0,
        companions: 5.0,
        reaction: 6,
        attention: 3,
        velocity_step: 2.0,
        directions: 4,
        pickup_clearance: 0.65,
        knows_blue: false,
        knows_minimap: false,
        stars: 20,
        role: persona::Role::Grinder,
    },
    Profile {
        identity: "Bramble",
        name: "Pixl",
        recovery: 0.38,
        resources: 6.0,
        companions: 6.0,
        reaction: 4,
        attention: 5,
        velocity_step: 1.0,
        directions: 8,
        pickup_clearance: 0.95,
        knows_blue: false,
        knows_minimap: true,
        stars: 20,
        role: persona::Role::Grinder,
    },
];
#[derive(Clone, Debug, PartialEq, Eq)]
enum Goal {
    Recover,
    Remedy(i32),
    Dodge,
    Fight(i32),
    Hunt(i32),
    Venture,
    Bank(i32),
    Break(i32),
    Loot(i32),
    Follow(i32),
    Gather(i32),
    Portal(i32),
    Revisit(i16, i16),
    Survey(i16, i16),
    Explore(i16, i16),
    Linger,
    NexusNoRoom,
    Wait,
}
#[derive(Debug)]
enum Journey {
    Portal(i32),
    Nexus,
    Regroup(i32),
}
struct Decision {
    goal: Goal,
    movement: Option<Pos>,
    aim: Option<Pos>,
    item: Option<Action>,
    journey: Option<Journey>,
    chat: Option<&'static str>,
}
/// What a bot is doing for the next while; reflexes may interrupt any of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Activity {
    Busy,
    Survive,
    Trade,
    Equip,
    Event,
    Consume,
    Social,
    Bank,
    Linger,
    Call,
    Adventure,
}
impl std::fmt::Display for Activity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "busy",
            Self::Survive => "surviving",
            Self::Trade => "trading",
            Self::Equip => "sorting gear",
            Self::Event => "at an event",
            Self::Consume => "using items",
            Self::Social => "socializing",
            Self::Bank => "banking",
            Self::Linger => "hanging out in Nexus",
            Self::Call => "answering a call",
            Self::Adventure => "adventuring",
        })
    }
}
/// One decision's shared perception, computed once and read by reflexes and activities.
struct Scene<'a> {
    enemies: Vec<&'a crate::world::view::VisibleObject>,
    nearest: Option<&'a crate::world::view::VisibleObject>,
    quarry: Option<&'a crate::world::view::VisibleObject>,
    perception: combat::Perception,
    inspecting: bool,
    range: f32,
    passes_cover: bool,
    health: f32,
    threatened: bool,
    /// Hardest hit faced in the last half minute, after own defense.
    hit: i32,
    retreat: bool,
    outmatched: bool,
    grid: Arc<Grid>,
    remedy: Option<&'a crate::world::view::VisibleObject>,
}
struct Brain {
    draft: Option<composer::Draft>,
    clipboard: String,
    call_goal: Option<Goal>,
    aftermath: Option<events::Speech>,
    market_words: Option<(pursuits::market::Message, String)>,
    profile: Profile,
    activity: Activity,
    intent: serde_json::Value,
    mind: pursuits::Mind,
    group: Option<adventures::Adventure>,
    group_lost_since: Option<u64>,
    crowd: crowds::Crowd,
    offered: Option<adventures::Offer>,
    proposing: bool,
    session: (i32, i32, i32),
    last_tick: Option<u64>,
    decision_logged: bool,
    goal: Goal,
    rng: rand::rngs::StdRng,
    heading: f32,
    steering: Option<(i32, f32)>,
    /// The kept journey and its distance-proportional deadline (ms).
    /// The trip being followed: its goal, deadline and the destination chosen when it started.
    travel: Option<(Goal, u64, Pos)>,
    /// Hunt/Fight targets whose approach failed, until a time (ms); moving targets defeat tile exclusion.
    unreachable: BTreeMap<i32, u64>,
    last_aim: u64,
    previous: Pos,
    stationary: u64,
    destination: Option<Pos>,
    blocked: BTreeMap<(i16, i16), u64>,
    /// Stalls per destination tile and the last one, so a tile that keeps failing stays excluded longer.
    stalls: BTreeMap<(i16, i16), (u32, u64)>,
    /// Recent positions and steps, to catch travel that moves back and forth without getting anywhere.
    trail: VecDeque<(u64, Pos, Option<Pos>, Goal)>,
    /// The known edge a trip toward unseen ground is heading for, per target tile.
    edge: Option<(TilePoint, TilePoint)>,
    next_snapshot: u64,
    cover: BTreeMap<TilePoint, bool>,
    visits: BTreeMap<(i16, i16), u64>,
    companion: Option<i32>,
    calls: calls::Calls,
    entries: entries::Entries,
    agreement: Option<dialogue::Agreement>,
    invitation_until: u64,
    next_decision: u64,
    recover_until: u64,
    tracks: BTreeMap<i32, (Pos, u64, Pos)>,
    next_teleport: u64,
    plan: plan::Plan,
    nexus_until: u64,
    nexus_spot: Option<Pos>,
    portal: Option<(i32, u64)>,
    hunting: Option<Pos>,
    /// Hardest hit seen and when; survives a brief loss of sight of the shooter.
    danger: (i32, u64),
    next_train_call: u64,
    /// When this session first wanted to end, so a fight in company can delay logging off only briefly.
    finishing_since: Option<u64>,
    /// Last logged Realm quest and whether it was worth joining.
    quest_interest: Option<(i32, bool)>,
    last_encounter: u64,
    navigation: Option<Arc<Grid>>,
    navigation_version: Option<(u64, u64)>,
    navigation_blockers: BTreeSet<TilePoint>,
    navigation_removed: BTreeSet<TilePoint>,
    navigation_rebuilds: u64,
    navigation_reuses: u64,
    performance: Arc<Mutex<crate::performance::Window>>,
}
impl Brain {
    fn new(profile: Profile, view: &PlayerView) -> Self {
        let seed = view.session.account as u64;
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let heading = rng.gen_range(0.0..std::f32::consts::TAU);
        let mut mind = pursuits::Mind::new(view.session.account);
        mind.cognition = pursuits::Cognition {
            attention: profile.attention,
            velocity_step: profile.velocity_step,
            directions: profile.directions,
            pickup_clearance: profile.pickup_clearance,
        };
        mind.market.name = profile.name.into();
        mind.market.trader = profile.role == persona::Role::Trader;
        mind.events.knows_blue = profile.knows_blue;
        Self {
            draft: None,
            clipboard: String::new(),
            call_goal: None,
            aftermath: None,
            market_words: None,
            profile,
            activity: Activity::Busy,
            intent: serde_json::Value::Null,
            mind,
            group: None,
            group_lost_since: None,
            crowd: crowds::Crowd::new(profile.knows_minimap),
            offered: None,
            proposing: false,
            session: (
                view.session.world,
                view.session.player,
                view.session.character,
            ),
            last_tick: None,
            decision_logged: false,
            goal: Goal::Wait,
            rng,
            heading,
            steering: None,
            travel: None,
            unreachable: BTreeMap::new(),
            last_aim: 0,
            previous: view.position,
            stationary: view.tick,
            destination: None,
            blocked: BTreeMap::new(),
            stalls: BTreeMap::new(),
            trail: VecDeque::new(),
            edge: None,
            next_snapshot: 0,
            cover: BTreeMap::new(),
            visits: BTreeMap::new(),
            companion: None,
            calls: calls::Calls::default(),
            entries: entries::Entries::default(),
            agreement: None,
            invitation_until: 0,
            next_decision: view.tick + seed % profile.reaction,
            recover_until: 0,
            tracks: BTreeMap::new(),
            next_teleport: 0,
            plan: plan::Plan::default(),
            nexus_until: if view.map.name == "Nexus" {
                view.time_ms + 10000
            } else {
                0
            },
            nexus_spot: None,
            portal: None,
            hunting: None,
            danger: (0, 0),
            next_train_call: 0,
            finishing_since: None,
            quest_interest: None,
            last_encounter: view.time_ms,
            navigation: None,
            navigation_version: None,
            navigation_blockers: BTreeSet::new(),
            navigation_removed: BTreeSet::new(),
            navigation_rebuilds: 0,
            navigation_reuses: 0,
            performance: Arc::default(),
        }
    }
    fn receive(
        &mut self,
        packet: Packet,
        view: &PlayerView,
        legacy_invitations: bool,
        content: &Content,
    ) -> Result<bool> {
        if packet.id != wire::TEXT {
            return Ok(false);
        }
        let mut r = Reader::new(&packet.body);
        let name = r.utf()?;
        let actor = r.i32()?;
        r.i32()?;
        r.u8()?;
        let recipient = r.utf()?;
        let text = r.utf()?;
        r.utf()?;
        r.done()?;
        self.receive_call(view, content, actor, &name, &recipient, &text)?;
        if legacy_invitations
            && actor > 0
            && actor != self.session.1
            && recipient.is_empty()
            && view
                .objects
                .iter()
                .any(|o| o.id == actor && o.hp > 0 && !o.equipped.is_empty())
            && text.eq_ignore_ascii_case(&format!("{} follow me", self.mind.market.name.as_str()))
        {
            self.companion = Some(actor);
            self.invitation_until = view.time_ms + 60000;
            return Ok(true);
        }
        Ok(false)
    }

    fn navigation_grid(&mut self, v: &PlayerView, content: &Content, avoid: &[Pos]) -> Arc<Grid> {
        let measuring = crate::performance::Measure::start();
        let observed: BTreeMap<_, _> = v
            .objects
            .iter()
            .filter(|o| o.blocks && !content.objects[&o.kind].enemy)
            .fold(BTreeMap::new(), |mut observed, o| {
                *observed.entry(tile(o.position)).or_insert(false) |=
                    content.objects[&o.kind].full_occupy;
                observed
            });
        // Only nearby observed absence clears remembered cover, not an out-of-view removal.
        self.cover.retain(|point, _| {
            center(*point).distance(v.position) >= 19. || observed.contains_key(point)
        });
        self.cover.extend(observed);
        let full_occupy: BTreeSet<_> = self
            .cover
            .iter()
            .filter_map(|(point, full)| full.then_some(*point))
            .chain(
                v.objects
                    .iter()
                    .filter(|o| content.objects[&o.kind].full_occupy)
                    .map(|o| tile(o.position)),
            )
            .collect();
        let mut blockers: BTreeSet<_> = self
            .cover
            .keys()
            .chain(self.blocked.keys())
            .filter(|point| **point != tile(v.position))
            .copied()
            .collect();
        blockers.extend(
            v.objects
                .iter()
                .filter(|o| o.blocks)
                .map(|o| tile(o.position)),
        );
        for enemy in avoid {
            let (x, y) = tile(*enemy);
            for x in x.saturating_sub(9)..=x.saturating_add(9) {
                for y in y.saturating_sub(9)..=y.saturating_add(9) {
                    if center((x, y)).distance(*enemy) < 8. {
                        blockers.insert((x, y));
                    }
                }
            }
        }
        // Live: lava killed 16 level 20s a day; bots routed through it like ordinary slow ground.
        blockers.extend(hazards_near(v, content, 12).filter(|p| *p != tile(v.position)));
        if self.mind.events.knows_blue && v.map.name == "Mad Lab" {
            for pool in v.objects.iter().filter(|p| {
                p.kind == crate::dungeon::lab::HEXXER
                    && ((v.position.x as i32 - p.position.x as i32).abs() >= 2
                        || (v.position.y as i32 - p.position.y as i32).abs() >= 2)
            }) {
                let (x, y) = tile(pool.position);
                for x in x.saturating_sub(1)..=x.saturating_add(1) {
                    for y in y.saturating_sub(1)..=y.saturating_add(1) {
                        blockers.insert((x, y));
                    }
                }
            }
            blockers.remove(&tile(v.position));
        }
        let version = v.tiles.version();
        let terrain_changed = self.navigation_version != Some(version);
        let obstacles_changed = self.navigation_blockers != blockers;
        let terrain_reusable = self
            .navigation
            .as_ref()
            .is_some_and(|grid| !terrain_changed && grid.dimensions == (v.map.width, v.map.height));
        let equivalent = terrain_reusable
            && self
                .navigation
                .as_ref()
                .expect("reusable navigation missing")
                .full_occupy
                == full_occupy
            && (!obstacles_changed || {
                let grid = self
                    .navigation
                    .as_ref()
                    .expect("reusable navigation missing");
                let removed: BTreeSet<_> = blockers
                    .iter()
                    .filter(|p| grid.walkable.contains(p) || self.navigation_removed.contains(p))
                    .copied()
                    .collect();
                removed == self.navigation_removed
            });
        self.performance
            .lock()
            .expect("bot timings poisoned")
            .work("navigation_input", measuring);
        if equivalent {
            self.navigation_reuses += 1;
            self.navigation_blockers = blockers;
            let mut p = self.performance.lock().expect("bot timings poisoned");
            p.count("navigation_reuses", 1);
            p.count("equivalent_obstacle_reuses", u64::from(obstacles_changed));
            return self
                .navigation
                .as_ref()
                .expect("equivalent navigation missing")
                .clone();
        }
        let measuring = crate::performance::Measure::start();
        let mut grid = if terrain_reusable {
            let mut grid = self.navigation.take().expect("reusable navigation missing");
            let g = Arc::make_mut(&mut grid);
            for point in &self.navigation_removed {
                g.walkable.insert(*point);
            }
            g.invalidate();
            grid
        } else {
            Arc::new(Grid::terrain(v))
        };
        let g = Arc::make_mut(&mut grid);
        g.full_occupy = full_occupy;
        g.performance = self.performance.clone();
        self.navigation_removed.clear();
        for point in &blockers {
            if g.walkable.remove(point) {
                self.navigation_removed.insert(*point);
            }
        }
        {
            let mut p = self.performance.lock().expect("bot timings poisoned");
            p.work("navigation_build", measuring);
            p.count("navigation_tiles_built", 0);
            p.count("navigation_terrain_reuses", u64::from(terrain_reusable));
            p.count("terrain_invalidations", u64::from(terrain_changed));
            p.count("obstacle_invalidations", u64::from(obstacles_changed));
        }
        self.navigation_rebuilds += 1;
        self.navigation_version = Some(version);
        self.navigation_blockers = blockers;
        self.navigation = Some(grid.clone());
        grid
    }
    fn decide(&mut self, v: &PlayerView, content: &Content) -> Result<Decision> {
        ensure!(
            self.session == (v.session.world, v.session.player, v.session.character),
            "controller session changed"
        );
        ensure!(
            self.last_tick.is_none_or(|last| v.tick > last),
            "controller observation clock did not advance"
        );
        ensure!(
            v.lifecycle.alive
                && !v.lifecycle.death_pending
                && !v.lifecycle.departing
                && !v.lifecycle.quarantined
                && matches!(v.lifecycle.output, OutputState::Live),
            "controller is not live"
        );
        ensure!(
            v.tiles.contains(&tile(v.position)),
            "controller own terrain missing"
        );
        ensure!(
            v.projectiles
                .iter()
                .all(|b| b.speed > 0. && b.lifetime_ms > 0.),
            "projectile trajectory cannot be inferred from observation"
        );
        self.last_tick = Some(v.tick);
        if v.time_ms >= self.nexus_until {
            self.nexus_spot = None;
        }
        self.calls.observe(v);
        self.next_decision = v.tick + self.profile.reaction;
        self.blocked.retain(|_, until| *until > v.tick);
        if v.time_ms >= self.invitation_until {
            self.companion = None;
            self.agreement = None;
        }
        if self.previous.distance(v.position) > 0.15 {
            self.stationary = v.tick;
        }
        if v.tick.saturating_sub(self.stationary) > 30
            && let Some(destination) = self.destination
        {
            // Live: one bot re-chose the same unreachable tile 123 times an hour; repeats back off exponentially.
            self.stalls
                .retain(|_, (_, last)| v.tick.saturating_sub(*last) < 12_000);
            let (repeats, last) = self.stalls.entry(tile(destination)).or_insert((0, 0));
            *repeats += 1;
            *last = v.tick;
            let excluded = 100u64 << (*repeats - 1).min(6);
            self.blocked.insert(tile(destination), v.tick + excluded);
            tracing::warn!(player=v.session.player,tick=v.tick,goal=?self.goal,x=destination.x,y=destination.y,repeats=*repeats,excluded,"autonomous route made no progress; temporarily excluded");
            // Login crowds briefly wedge bots on the Nexus spawn; only a stall elsewhere is a loop.
            if v.map.name != "Nexus" {
                self.plan.record(
                    plan::now(),
                    plan::Kind::Aborted,
                    self.plan.owner(),
                    "movement blocked".into(),
                );
            }
            self.heading = self.rng.gen_range(0.0..std::f32::consts::TAU);
            self.stationary = v.tick;
        }
        self.snapshot_stalled_travel(v);
        self.previous = v.position;
        self.visits.insert(tile(v.position), v.tick);
        let decision = Decision {
            goal: Goal::Wait,
            movement: None,
            aim: None,
            item: None,
            journey: None,
            chat: None,
        };
        let now = plan::now();
        self.observe_goal(v, content, now)?;
        self.plan.observe(v, now);
        let (activity, decision) = if v.lifecycle.busy {
            (Activity::Busy, decision)
        } else {
            self.arbitrate(v, content, decision, now)?
        };
        if activity != Activity::Adventure {
            self.steering = None;
            self.travel = None;
        }
        if decision.aim.is_some() {
            self.last_aim = v.time_ms;
        }
        self.set_activity(v, activity);
        // An unreachable hunt is travel, not engagement, so it can still raise no_progress.
        self.plan.engaged = matches!(
            activity,
            Activity::Survive
                | Activity::Trade
                | Activity::Linger
                | Activity::Social
                | Activity::Busy
        ) || matches!(decision.goal, Goal::Fight(_) | Goal::Hunt(_))
            && v.time_ms.saturating_sub(self.last_aim) < 5000;
        self.intent = self
            .mind
            .intentions
            .as_ref()
            .expect("observed intentions")
            .payload(
                v,
                content,
                activity,
                cctv::reason(activity, &decision, v, content),
                self.plan.step.as_ref(),
            );
        Ok(decision)
    }
    /// Keeps one main goal, abandoning it explicitly when it stops making measurable progress.
    fn observe_goal(&mut self, v: &PlayerView, content: &Content, now: u64) -> Result<()> {
        let identity = self.profile.identity;
        let intentions = self
            .mind
            .intentions
            .as_mut()
            .context("unmigrated account intentions")?;
        let before = intentions.label(content);
        intentions.observe(v, content, identity)?;
        if self.plan.stalled(intentions.progress(v), now) {
            let reason = format!(
                "{} made no measurable progress",
                before.clone().unwrap_or_default()
            );
            intentions.abandon("abandoned: no measurable progress within its patience", now);
            self.plan
                .record(now, plan::Kind::Aborted, Some(plan::Owner::Goal), reason);
            intentions.observe(v, content, identity)?;
        }
        let after = intentions.label(content);
        self.plan.goal_label.clone_from(&after);
        if after != before
            && let Some(label) = after
        {
            self.plan
                .record(now, plan::Kind::Started, Some(plan::Owner::Goal), label);
        }
        self.mind.market.observe_goal(
            v,
            content,
            self.mind.future_class,
            self.mind
                .intentions
                .as_ref()
                .expect("observed intentions")
                .item_target(),
        )
    }
    /// Bold personas trust a crowd above their level, the way fame trains drew in low levels.
    fn ambitious(&self) -> bool {
        self.profile.recovery < 0.45
    }
    fn quest_appeal(
        &self,
        v: &PlayerView,
        content: &Content,
        quest: &crate::world::view::VisibleObject,
    ) -> Option<quests::Appeal> {
        (v.map.name == "Realm" && v.character.level == 20 && v.quest == Some(quest.id)).then(|| {
            quests::appeal(
                &self.profile,
                content,
                quest.kind,
                self.crowd.near(v, quest.position),
                self.plan.last_progress,
            )
        })
    }
    fn set_activity(&mut self, v: &PlayerView, activity: Activity) {
        if activity != self.activity {
            tracing::info!(account=v.session.account,player=v.session.player,from=%self.activity,to=%activity,"autonomous activity changed");
            self.activity = activity;
        }
    }
    /// Reflexes first, then the first activity with something to do; adventuring is the default.
    fn arbitrate(
        &mut self,
        v: &PlayerView,
        content: &Content,
        mut decision: Decision,
        now: u64,
    ) -> Result<(Activity, Decision)> {
        self.mind.events.observe(v);
        let party = social::companions(v, self.group.as_ref());
        if self.group.is_some() && party.is_empty() {
            self.group_lost_since.get_or_insert(v.time_ms);
        } else {
            self.group_lost_since = None;
        }
        decision.item = self
            .mind
            .social
            .healing(v, content, self.profile.companions)?;
        if decision.item.is_none()
            && !self.mind.market.reserves_stock()
            && v.character.hp as f32 / v.character.stats[0] as f32 >= self.profile.recovery
        {
            decision.item = progression::mana(v, content)?;
        }
        let reserved = self.mind.market.reserves_stock();
        let inspecting = self.mind.inspecting(v, content);
        let perception = combat::Perception::of(&self.mind.cognition);
        let equipment = if decision.item.is_none() && !reserved {
            self.mind.assess(v, content)?
        } else {
            None
        };
        let equipping = equipment.is_some();
        if equipping {
            decision.item = equipment;
        }
        let enemies: Vec<_> = v
            .objects
            .iter()
            .filter(|o| {
                content.objects[&o.kind].enemy
                    && o.hp > 0
                    && !matches!(
                        o.kind,
                        crate::dungeon::lab::HEXXER
                            | crate::dungeon::lab::DEHEXXER
                            | crate::dungeon::lab::BUBBLE
                            | crate::dungeon::manor::GRAVE
                    )
                    // A Kage Kami nobody has hit is still disguised as a grave; bots do not dig graveyards up, so players find him.
                    && !(o.kind == crate::dungeon::manor::KAGE && o.hp >= o.max_hp)
            })
            .collect();
        let nearest = enemies.iter().min_by(|a, b| {
            a.position
                .distance(v.position)
                .total_cmp(&b.position.distance(v.position))
        });
        let quarry = enemies
            .iter()
            .filter(|enemy| {
                progression::can_fight(v, content, enemy)
                    && enemy.position.distance(v.position) <= 32.
            })
            .min_by(|a, b| {
                a.position
                    .distance(v.position)
                    .total_cmp(&b.position.distance(v.position))
            });
        if let Some(enemy) = quarry {
            self.hunting = Some(enemy.position);
            self.last_encounter = v.time_ms;
        }
        self.tracks
            .retain(|_, (_, seen, _)| v.time_ms.saturating_sub(*seen) < 1000);
        for enemy in enemies.iter().take(self.mind.cognition.attention) {
            let mut velocity = Pos { x: 0., y: 0. };
            if let Some((previous, time, _)) = self.tracks.get(&enemy.id) {
                let elapsed = v.time_ms.saturating_sub(*time) as f32 / 1000.;
                if elapsed > 0. && elapsed < 1. && previous.distance(enemy.position) < elapsed * 8.
                {
                    velocity = Pos {
                        x: (enemy.position.x - previous.x) / elapsed,
                        y: (enemy.position.y - previous.y) / elapsed,
                    };
                }
            }
            self.tracks.insert(
                enemy.id,
                (enemy.position, v.time_ms, perception.velocity(velocity)),
            );
            if self.tracks.len() > 32 {
                let first = *self.tracks.keys().next().expect("bounded tracks");
                self.tracks.remove(&first);
            }
        }
        let weapon = content.get(v.character.items[0] as u16)?;
        let projectile = weapon
            .projectiles
            .get(&0)
            .context("weapon projectile missing")?;
        let range = projectile.speed * projectile.lifetime / 10000.;
        let health = v.character.hp as f32 / v.character.stats[0] as f32;
        let threatened = combat::clearance(v, v.position) < self.mind.cognition.pickup_clearance
            || v.throws.iter().any(|t| combat::bombed(t, v.position));
        // Health is judged in hits of what this player faces, so more max HP buys more courage.
        let seen = combat::biggest_hit(v, content, &enemies);
        if seen > 0 {
            self.danger = (seen, v.time_ms);
        }
        let hit = if v.time_ms.saturating_sub(self.danger.1) < 30_000 {
            self.danger.0.max(seen)
        } else {
            seen
        };
        // A persona's own share of health is the floor; hard hitters raise it by hits in reserve, up to a cap.
        let (hp, max) = (v.character.hp as f32, v.character.stats[0] as f32);
        let recovery = self.profile.recovery;
        let hits = |n: f32| n * hit as f32;
        let wounded =
            (max * recovery).max(hits(1.5 + 5. * recovery).min(max * (0.4 + 0.5 * recovery)));
        let healthy = (wounded + hits(3.))
            .max(max * (recovery + 0.15))
            .min(max * 0.95);
        if hp < wounded {
            self.recover_until = v.tick + 60;
        }
        let outmatched = enemies.iter().any(|enemy| {
            !progression::can_fight(v, content, enemy) && enemy.position.distance(v.position) < 8.
        });
        // Live: bots "backed off" in the Nexus with nothing near; away from enemies a hurt player keeps its plan while it heals.
        let pressed = enemies
            .iter()
            .any(|e| e.position.distance(v.position) < 15.);
        let retreat = outmatched
            || pressed
                && (v.tick < self.recover_until
                    || (matches!(self.goal, Goal::Recover) && hp < healthy));
        let avoid: Vec<_> = enemies
            .iter()
            .filter(|enemy| !retreat && !progression::can_fight(v, content, enemy))
            // A level 20 does not route around the Realm quest it has chosen to take on.
            .filter(|enemy| {
                !self
                    .quest_appeal(v, content, enemy)
                    .is_some_and(|a| a.joins(false))
            })
            .map(|e| e.position)
            .collect();
        let grid = self.navigation_grid(v, content, &avoid);
        let remedy = self
            .mind
            .events
            .remedy(v)
            .filter(|blue| grid.next(v.position, blue.position).is_some());
        if events::shared(&v.map.name) {
            tracing::info!(
                account = v.session.account,
                character = v.session.character,
                tick = v.tick,
                time_ms = v.time_ms,
                hp = v.character.hp,
                max_hp = v.character.stats[0],
                conditions = v.character.conditions,
                blue = remedy.map(|p| p.id),
                blue_distance = remedy.map(|p| v.position.distance(p.position)),
                threatened,
                retreat,
                outmatched,
                "autonomous event safety observation"
            );
        }
        let s = Scene {
            nearest: nearest.copied(),
            quarry: quarry.copied(),
            enemies,
            perception,
            inspecting,
            range,
            passes_cover: projectile.passes_cover,
            health,
            threatened,
            hit,
            retreat,
            outmatched,
            grid,
            remedy,
        };
        if self.reflex(v, content, &s, &mut decision)? {
            return Ok((Activity::Survive, decision));
        }
        let (activity, mut decision) =
            self.choose(v, content, &s, decision, now, reserved, equipping)?;
        self.tactics(v, content, &s, &mut decision)?;
        Ok((activity, decision))
    }
    /// Live: dodging replaced the plan for 35-40% of level-20 decisions, so trips stalled; tactics now run under it.
    fn tactics(
        &mut self,
        v: &PlayerView,
        content: &Content,
        s: &Scene,
        decision: &mut Decision,
    ) -> Result<()> {
        if !s.threatened {
            return Ok(());
        }
        // Near the planned fight, keep weapon range from it; otherwise keep making way toward the plan.
        let engaged = s.enemies.iter().find(|e| {
            matches!(decision.goal, Goal::Fight(id) | Goal::Hunt(id) if id == e.id)
                && e.position.distance(v.position) < s.range * 1.3
        });
        let pull = match engaged {
            Some(enemy) => Some((enemy.position, s.range)),
            None => decision.movement.map(|step| (step, 0.)),
        };
        decision.movement =
            combat::movement(v, &s.grid, &s.enemies, pull, false, &s.perception, None);
        if decision.aim.is_none()
            && let Some(enemy) =
                combat::target(v, content, &s.grid, &s.enemies, s.range, s.passes_cover)
        {
            combat::attack(
                v,
                content,
                &s.grid,
                enemy,
                self.tracks
                    .get(&enemy.id)
                    .map_or(Pos { x: 0., y: 0. }, |track| track.2),
                &s.perception,
                decision,
            )?;
        }
        Ok(())
    }
    /// The plan: errands, social life, consumption and adventuring, by priority.
    #[allow(clippy::too_many_arguments)]
    fn choose(
        &mut self,
        v: &PlayerView,
        content: &Content,
        s: &Scene,
        mut decision: Decision,
        now: u64,
        reserved: bool,
        equipping: bool,
    ) -> Result<(Activity, Decision)> {
        // Consent and receipts outrank every errand; an advertised quote does not preempt a started one.
        if self.mind.market.locked()
            || reserved && self.plan.owner().is_none_or(|o| o == plan::Owner::Trade)
        {
            decision.item = None;
            if let Some(market) = self.mind.market.decide(
                v,
                content,
                self.mind.future_class,
                &s.grid,
                &mut self.rng,
            )? {
                if self.plan.owner() != Some(plan::Owner::Trade) {
                    self.plan.abort(now, "superseded by a reserved exchange");
                    self.plan
                        .start(now, plan::Owner::Trade, "reserved exchange".into());
                }
                return Ok((Activity::Trade, market));
            }
        }
        if (equipping && s.remedy.is_none()) || (v.map.name == "Vault" && s.inspecting) {
            return Ok((Activity::Equip, decision));
        }
        if self.plan.overdue(now) {
            self.plan.abort(now, "deadline passed");
        }
        // The current errand runs first, so no other policy can undo it half-way.
        if let Some(owner) = self.plan.owner() {
            match self.errand(owner, v, content, s, &decision)? {
                plan::Errand::Act(activity, d) => return Ok((activity, d)),
                plan::Errand::Done(reason) => self.plan.complete(now, reason),
                plan::Errand::Abort(reason) => self.plan.abort(now, reason),
                plan::Errand::Yield => {}
            }
        }
        if self.plan.idle() {
            for owner in plan::Owner::ERRANDS {
                if self.plan.resting(owner, now) {
                    continue;
                }
                if let plan::Errand::Act(activity, d) =
                    self.errand(owner, v, content, s, &decision)?
                {
                    self.plan
                        .start(now, owner, cctv::reason(activity, &d, v, content));
                    return Ok((activity, d));
                }
            }
        }
        if decision.item.is_none()
            && !reserved
            && s.nearest
                .is_none_or(|e| e.position.distance(v.position) > 5.)
        {
            decision.item =
                progression::consume(v, content, &self.mind.market, self.mind.future_class)?;
            if decision.item.is_some() {
                return Ok((Activity::Consume, decision));
            }
        }
        if (self.proposing
            || self
                .offered
                .as_ref()
                .is_some_and(|o| o.until > v.elapsed_ms))
            && peaceful(&s.enemies, v.position)
        {
            self.destination = None;
            self.stationary = v.tick;
            return Ok((Activity::Social, decision));
        }
        if v.map.name == "Vault" && self.agreement.is_none() {
            if self.mind.social.trying_loadout(v) {
                decision.aim = Some(Pos {
                    x: v.position.x + self.heading.cos() * 6.,
                    y: v.position.y + self.heading.sin() * 6.,
                });
            } else {
                decision.journey = Some(Journey::Nexus);
            }
            return Ok((Activity::Linger, decision));
        }
        if self.agreement.is_none()
            && peaceful(&s.enemies, v.position)
            && let Some(social) = self.mind.social.decide(v)
            && (!matches!(social.goal, Goal::Follow(_)) || s.quarry.is_none())
        {
            return Ok((Activity::Social, social));
        }
        if v.map.name == "Nexus" && self.agreement.is_none() && v.time_ms < self.nexus_until {
            if decision.item.is_none()
                && self.group.is_none()
                && self.companion.is_none()
                && s.enemies.is_empty()
            {
                let free = |p: Pos, room: f32| {
                    v.player_markers.iter().all(|peer| {
                        peer.id == v.session.player || peer.position.distance(p) >= room
                    })
                };
                let available = |p: Pos| {
                    p.distance(v.position) <= 7. && s.grid.segment(v.position, p) && free(p, 2.)
                };
                // Live: spots were re-picked whenever a step broke the line to them or a peer passed by, so
                // Nexus bots shuffled around each other; a chosen spot is kept until someone takes it.
                if self
                    .nexus_spot
                    .is_none_or(|p| !free(p, 0.8) || p.distance(v.position) > 10.)
                {
                    let (x, y) = tile(v.position);
                    self.nexus_spot = (x.saturating_sub(7)..=x.saturating_add(7))
                        .flat_map(|x| {
                            (y.saturating_sub(7)..=y.saturating_add(7)).map(move |y| (x, y))
                        })
                        .map(center)
                        .filter(|p| available(*p))
                        .choose(&mut self.rng);
                }
                if let Some(spot) = self.nexus_spot {
                    decision.goal = Goal::Linger;
                    if v.position.distance(spot) > 0.3 {
                        decision.movement = s.grid.next(v.position, spot);
                    }
                } else {
                    decision.goal = Goal::NexusNoRoom;
                }
            }
            return Ok((Activity::Linger, decision));
        }
        Ok((
            Activity::Adventure,
            self.adventure(v, content, s, decision)?,
        ))
    }
    /// One errand's policy: act now, yield this decision to the outing, or report completion.
    fn errand(
        &mut self,
        owner: plan::Owner,
        v: &PlayerView,
        content: &Content,
        s: &Scene,
        decision: &Decision,
    ) -> Result<plan::Errand> {
        use plan::Errand::{Abort, Act, Done, Yield};
        let peaceful = peaceful(&s.enemies, v.position);
        let idle = || Decision {
            goal: Goal::Wait,
            movement: None,
            aim: None,
            item: decision.item.clone(),
            journey: None,
            chat: None,
        };
        Ok(match owner {
            plan::Owner::Trade => {
                if !matches!(v.map.name.as_str(), "Nexus" | "Vault")
                    && (!peaceful || self.committed(v))
                {
                    return Ok(Yield);
                }
                let started = self.plan.owner() == Some(owner);
                if v.map.name == "Realm"
                    && s.health >= 0.75
                    && if started {
                        self.mind.market.sought
                    } else {
                        self.mind
                            .market
                            .trip_due(v, content, self.mind.future_class)?
                    }
                {
                    self.mind.market.sought = true;
                    let mut d = idle();
                    d.journey = Some(Journey::Nexus);
                    return Ok(Act(Activity::Trade, d));
                }
                match self.mind.market.decide(
                    v,
                    content,
                    self.mind.future_class,
                    &s.grid,
                    &mut self.rng,
                )? {
                    Some(mut market) => {
                        if market.item.is_none() {
                            market.item = decision.item.clone();
                        }
                        Act(Activity::Trade, market)
                    }
                    None => Done("market visit finished"),
                }
            }
            plan::Owner::Event => {
                let allow_idle = !self.committed(v) && decision.item.is_none();
                match self
                    .mind
                    .events
                    .decide(v, content, &s.grid, self.profile, allow_idle)?
                {
                    Some(mut event) => {
                        if decision.item.is_some() {
                            event.item = decision.item.clone();
                        }
                        Act(Activity::Event, event)
                    }
                    None => Done("event needs nothing more"),
                }
            }
            plan::Owner::Bank => {
                let started = self.plan.owner() == Some(owner);
                // Live: 59 of 70 banking trips from the Realm came back with the same full bag; the Vault was full.
                let due = started
                    || !self.mind.market.reserves_stock()
                        && !self.mind.market.vault_full()
                        && self.agreement.is_none()
                        && progression::stash(v, content)?;
                match v.map.name.as_str() {
                    _ if !due => Yield,
                    "Vault" => {
                        match progression::bank(
                            v,
                            content,
                            &s.grid,
                            self.mind.future_class,
                            self.mind.market.reserved_kind(),
                        )? {
                            Some(d) if matches!(d.goal, Goal::Bank(_)) => Act(Activity::Bank, d),
                            _ => Done("own Vault organized"),
                        }
                    }
                    "Nexus" => match v.objects.iter().find(|p| p.kind == 0x720 && p.usable) {
                        Some(portal) => {
                            let mut d = idle();
                            d.goal = Goal::Portal(portal.id);
                            if v.position.distance(portal.position) <= 2. {
                                d.journey = Some(Journey::Portal(portal.id));
                            } else {
                                d.movement = s.grid.towards(v.position, portal.position);
                            }
                            Act(Activity::Bank, d)
                        }
                        None if started => Abort("no usable Vault portal observed"),
                        None => Yield,
                    },
                    "Realm"
                        if peaceful
                            && s.health >= 0.75
                            && (started
                                || v.character
                                    .items
                                    .iter()
                                    .skip(4)
                                    .filter(|k| **k >= 0)
                                    .count()
                                    >= 6) =>
                    {
                        let mut d = idle();
                        d.journey = Some(Journey::Nexus);
                        if self.group.is_some() {
                            d.chat = Some("nexus");
                        }
                        Act(Activity::Bank, d)
                    }
                    _ => Yield,
                }
            }
            plan::Owner::Call => {
                if self.plan.owner() == Some(owner) {
                    return Ok(Done("teleport requested"));
                }
                match self.call_target(v, content)? {
                    Some(target) => {
                        self.next_teleport = v.time_ms + 10_000;
                        let mut d = idle();
                        d.goal = Goal::Follow(target);
                        d.journey = Some(Journey::Regroup(target));
                        Act(Activity::Call, d)
                    }
                    None => Yield,
                }
            }
            plan::Owner::Gift => {
                if self.mind.social.gift_active() {
                    Act(Activity::Social, idle())
                } else if self.plan.owner() == Some(owner) {
                    Done("gift offer resolved")
                } else if decision.item.is_none() && self.equipment_opportunity(v, content)? {
                    Act(Activity::Social, idle())
                } else {
                    Yield
                }
            }
            plan::Owner::Goal => Yield,
        })
    }

    fn committed(&self, v: &PlayerView) -> bool {
        self.agreement.is_some()
            || self.companion.is_some()
            || self.group.is_some()
            || self.proposing
            || self
                .offered
                .as_ref()
                .is_some_and(|o| o.until > v.elapsed_ms)
            || self.mind.social.gift_active()
            || self.mind.market.reserves_stock()
    }

    /// Safety always outranks any activity: retreat, dodge, or leave for the Nexus.
    // Live: a dozen bots shuffled between two tiles for minutes; the stall check saw movement and never fired.
    fn snapshot_stalled_travel(&mut self, v: &PlayerView) {
        self.trail
            .push_back((v.time_ms, v.position, self.destination, self.goal.clone()));
        while self
            .trail
            .front()
            .is_some_and(|(t, _, _, _)| v.time_ms.saturating_sub(*t) > 10_000)
        {
            self.trail.pop_front();
        }
        let Some((start, from)) = self.trail.front().map(|(t, p, _, _)| (*t, *p)) else {
            return;
        };
        let steps: BTreeSet<_> = self
            .trail
            .iter()
            .filter_map(|(_, _, s, _)| s.map(tile))
            .collect();
        if v.time_ms.saturating_sub(start) < 9_000
            || v.time_ms < self.next_snapshot
            || from.distance(v.position) >= 2.
            || steps.len() < 2
            || !matches!(
                self.goal,
                Goal::Venture | Goal::Explore(..) | Goal::Hunt(_) | Goal::Follow(_)
            )
                    // Circling a target being fought is combat, not a stalled trip.
            || matches!(self.goal, Goal::Hunt(_)) && self.travel.is_none()
        {
            return;
        }
        let Some(grid) = self.navigation.as_ref() else {
            return;
        };
        self.next_snapshot = v.time_ms + 300_000;
        let (x0, y0) = tile(v.position);
        let map: Vec<String> = (y0 - 5..=y0 + 5)
            .map(|y| {
                (x0 - 5..=x0 + 5)
                    .map(|x| {
                        let p = (x, y);
                        if p == (x0, y0) {
                            '@'
                        } else if !grid.known.contains(&p) {
                            '?'
                        } else if grid.full_occupy.contains(&p) {
                            'T'
                        } else if self.blocked.contains_key(&p) {
                            'x'
                        } else if !grid.walkable.contains(&p) {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect()
            })
            .collect();
        let recent: Vec<_> = self
            .trail
            .iter()
            .rev()
            .filter_map(|(_, p, s, g)| s.map(|s| (p.x, p.y, s.x, s.y, format!("{g:?}"))))
            .take(12)
            .collect();
        tracing::warn!(
            player = v.session.player,
            account = v.session.account,
            world = v.session.world,
            layout = ?v.map.layout,
            goal = ?self.goal,
            trip = ?self.travel.as_ref().map(|(_, _, to)| (to.x, to.y)),
            x = v.position.x,
            y = v.position.y,
            recent = ?recent,
            map = map.join("/"),
            "autonomous travel made no net progress"
        );
    }
    fn reflex(
        &mut self,
        v: &PlayerView,
        content: &Content,
        s: &Scene,
        decision: &mut Decision,
    ) -> Result<bool> {
        let Scene {
            enemies,
            nearest,
            range,
            threatened,
            retreat,
            outmatched,
            remedy,
            ..
        } = s;
        let (range, threatened, retreat, outmatched, nearest, remedy) = (
            *range,
            *threatened,
            *retreat,
            *outmatched,
            *nearest,
            *remedy,
        );
        let (grid, perception) = (&s.grid, &s.perception);
        let here = tile(v.position);
        if hazards_near(v, content, 0).next().is_some() {
            let hazards: BTreeSet<_> = hazards_near(v, content, 8).collect();
            let shore = (here.0 - 8..=here.0 + 8)
                .flat_map(|x| (here.1 - 8..=here.1 + 8).map(move |y| (x, y)))
                .filter(|p| grid.walkable.contains(p) && !hazards.contains(p))
                .min_by(|a, b| {
                    center(*a)
                        .distance(v.position)
                        .total_cmp(&center(*b).distance(v.position))
                })
                .context("standing in damaging ground with no known safe tile within 8")?;
            decision.goal = Goal::Dodge;
            decision.movement = Some(center(shore));
            return Ok(true);
        }
        if retreat || threatened {
            let emergency = retreat || remedy.is_some();
            // Live: 32 of 32 sampled Recover frames never fired. A 2012 player backing off kept shooting.
            let opponent = combat::target(v, content, grid, enemies, range, s.passes_cover);
            let pressured = threatened
                || nearest.is_some_and(|e| e.position.distance(v.position) < range * 1.3);
            if emergency {
                decision.goal = if retreat {
                    Goal::Recover
                } else {
                    Goal::Remedy(remedy.expect("emergency remedy").id)
                };
            }
            // Live: wounded melee bots stood still beside a Cube God for 15 minutes, never out of reach or in range.
            if retreat || remedy.is_some() && pressured {
                let focus = if retreat {
                    nearest.map(|e| e.position)
                } else if let Some(blue) = remedy {
                    Some(blue.position)
                } else {
                    opponent.map(|e| e.position)
                };
                let spacing = if !retreat && remedy.is_some() {
                    0.
                } else {
                    range
                };
                decision.movement = combat::movement(
                    v,
                    grid,
                    enemies,
                    focus.map(|p| (p, spacing)),
                    retreat,
                    perception,
                    None,
                );
            }
            let held_heal = v
                .character
                .items
                .iter()
                .filter(|kind| **kind >= 0)
                .any(|kind| content.objects[&(*kind as u16)].activate == "Heal");
            let incoming = combat::incoming(v);
            // A Nexus exit restarts the whole journey, so only a lethal hit or real overmatch justifies it.
            let escape =
                if outmatched && nearest.is_some_and(|e| e.position.distance(v.position) < 4.) {
                    Some("near_outmatched")
                } else if incoming >= v.character.hp {
                    Some("lethal_volley")
                } else if !held_heal && v.character.hp <= s.hit {
                    Some("next_hit_lethal")
                } else {
                    None
                };
            if let Some(reason) = escape
                && decision.item.is_none()
                && v.map.name != "Nexus"
            {
                tracing::info!(
                    account = v.session.account,
                    character = v.session.character,
                    hp = v.character.hp,
                    max_hp = v.character.stats[0],
                    incoming,
                    hit = s.hit,
                    reason,
                    "autonomous recovery escape selected"
                );
                self.plan
                    .record(plan::now(), plan::Kind::Escape, None, reason.into());
                decision.journey = Some(Journey::Nexus);
                return Ok(true);
            }
            if retreat {
                for (slot, kind) in v
                    .character
                    .items
                    .iter()
                    .enumerate()
                    .skip(4)
                    .filter(|(_, k)| **k >= 0)
                {
                    let item = content.get(*kind as u16)?;
                    if decision.item.is_none()
                        && item.activate == "Heal"
                        && v.character.stats[0] - v.character.hp >= (item.heal as f32 * 0.35) as i32
                    {
                        decision.item = Some(Action::Use {
                            slot,
                            x: v.position.x,
                            y: v.position.y,
                        });
                        break;
                    }
                }
            }
            if retreat
                && decision.item.is_none()
                && let Some(enemy) = nearest
            {
                decision.item = combat::ability(v, content, grid, enemy)?;
            }
            if emergency && let Some(enemy) = opponent {
                combat::attack(
                    v,
                    content,
                    grid,
                    enemy,
                    self.tracks
                        .get(&enemy.id)
                        .map_or(Pos { x: 0., y: 0. }, |track| track.2),
                    perception,
                    decision,
                )?;
            }
            return Ok(emergency);
        }
        Ok(false)
    }
    /// Default activity: fight, loot, quest, take portals, keep company and explore, by score.
    fn adventure(
        &mut self,
        v: &PlayerView,
        content: &Content,
        s: &Scene,
        mut decision: Decision,
    ) -> Result<Decision> {
        let steering = self.steering.take();
        let (enemies, grid, perception) = (&s.enemies, &s.grid, &s.perception);
        let (nearest, quarry, range, health) = (s.nearest, s.quarry, s.range, s.health);
        let mut choices = Vec::new();
        let event_area = self.mind.events.area(v);
        if let Some(area) = event_area {
            let (x, y) = tile(area);
            choices.push((97., Goal::Survey(x, y), area));
        }
        let speed = self.mind.intentions.as_ref().is_some_and(|i| i.speed());
        // Level 20s weigh whether a Realm quest is worth it; lower levels keep following their leveling quests.
        let veteran_realm = v.map.name == "Realm" && v.character.level == 20;
        let realm_quest = if let Some(quest) = enemies.iter().find(|e| v.quest == Some(e.id))
            && let Some(appeal) = self.quest_appeal(v, content, quest)
        {
            let joined = appeal.joins(progression::can_fight(v, content, quest));
            if self.quest_interest != Some((quest.id, joined)) {
                self.quest_interest = Some((quest.id, joined));
                tracing::info!(
                    account = v.session.account,
                    quest = quest.kind,
                    score = appeal.score,
                    value = appeal.value,
                    crowd = appeal.crowd,
                    habit = ?appeal.habit,
                    joined,
                    "autonomous realm quest appeal"
                );
            }
            joined.then(|| {
                choices.push((appeal.score, Goal::Hunt(quest.id), quest.position));
                Goal::Hunt(quest.id)
            })
        } else {
            None
        };
        // A quest worth following is where this player goes; otherwise it heads for its own band, a level 20's Godlands.
        if v.map.name == "Realm"
            && realm_quest.is_none()
            && health >= (self.profile.recovery + 0.1).max(0.75)
        {
            let snakes = self
                .crowd
                .bands
                .snake_source
                .filter(|p| speed && self.crowd.bands.survivable(v, *p));
            if let Some(ground) = snakes.or_else(|| self.crowd.bands.venture(v)) {
                choices.push((60. + self.profile.resources, Goal::Venture, ground));
            }
        }
        // Live: level 20s heading for the Godlands or their arrow stopped for every enemy in range until the trip timed out.
        let heading = realm_quest.is_some() || choices.iter().any(|(_, g, _)| *g == Goal::Venture);
        // Grated rooms cannot stay cleared; geared players push past snakes: https://whyigame.wordpress.com/2012/05/31/rotmg-oops-1-snakes-in-a-pit/
        let dungeon =
            !matches!(v.map.name.as_str(), "Realm" | "Nexus" | "Vault") && v.quest.is_some();
        let rushing = dungeon
            && health >= 0.75
            && v.character.level == 20
            && v.character.stats[3] >= 20
            && self.mind.cognition.directions >= 8;
        for enemy in enemies {
            if !progression::can_fight(v, content, enemy)
                || events::shared(&v.map.name) && !progression::vulnerable(enemy)
            {
                continue;
            }
            let d = enemy.position.distance(v.position);
            if event_area.is_some() && !events::objective(enemy.kind) && d > range {
                continue;
            }
            let protected = !progression::vulnerable(enemy);
            // Travellers shoot what is below their band in passing instead of stopping for it.
            let passing = heading
                && content.objects[&enemy.kind].terrain != 0
                && bands::tier(content.objects[&enemy.kind].terrain) < self.crowd.bands.limit;
            let score = 45. - d * 2.
                + if d < range
                    && !passing
                    && (!dungeon
                        || !rushing && content.objects[&enemy.kind].xp_multiplier > 0.
                        || v.quest == Some(enemy.id))
                {
                    60. + (enemy.max_hp as f32).sqrt().min(20.)
                } else {
                    0.
                }
                // Live: bots at a Cube God shot its Overseers for minutes; the chosen quest is the fight.
                + if d < range && realm_quest == Some(Goal::Hunt(enemy.id)) {
                    30.
                } else {
                    0.
                };
            choices.push((score, Goal::Fight(enemy.id), enemy.position));
            if v.map.name == "Realm"
                && content.objects[&enemy.kind].god
                && content.objects[&enemy.kind].terrain == 1
                && self.crowd.bands.survivable(v, enemy.position)
            {
                choices.push((
                    76. + self.profile.resources,
                    Goal::Hunt(enemy.id),
                    enemy.position,
                ));
            }
            if speed
                && crate::dungeon::portal_drop(enemy.kind)
                    .is_some_and(|(portal, _)| portal == 0x718)
            {
                choices.push((
                    76. + self.profile.resources,
                    Goal::Hunt(enemy.id),
                    enemy.position,
                ));
            }
            if events::shared(&v.map.name)
                && progression::vulnerable(enemy)
                && events::objective(enemy.kind)
            {
                choices.push((110., Goal::Hunt(enemy.id), enemy.position));
            }
            // Stheno's shield cannot prevent approaching the quest that wakes her encounter.
            if v.quest == Some(enemy.id)
                && !veteran_realm
                && (!protected || v.map.name == "Snake Pit")
                && (v.character.level >= 3 || d <= 48.)
            {
                choices.push((
                    if v.map.name == "Snake Pit" || events::shared(&v.map.name) {
                        97.
                    } else if v.map.name == "Realm" {
                        70. + self.profile.resources
                    } else {
                        55. + self.profile.resources
                    },
                    Goal::Hunt(enemy.id),
                    enemy.position,
                ));
            }
        }
        {
            let needs_mana = v.character.items[1] >= 0
                && v.character.mp / 2 < content.get(v.character.items[1] as u16)?.mana_cost;
            for bag in v
                .objects
                .iter()
                .filter(|o| content.objects[&o.kind].class == "Container")
            {
                if progression::pickup(
                    v,
                    content,
                    bag,
                    self.mind.future_class,
                    Some(&self.mind.market),
                )?
                .is_some()
                {
                    // A distant objective must not outrank replenishing an exhausted ability.
                    let replenishing = needs_mana
                        && peaceful(enemies, v.position)
                        && bag
                            .contents
                            .iter()
                            .filter(|kind| **kind >= 0)
                            .any(|kind| content.objects[&(*kind as u16)].activate == "Magic");
                    let score = if replenishing {
                        98.
                    } else {
                        (40. + 16. * self.profile.resources
                            / (1. + v.position.distance(bag.position)))
                        .min(96.)
                    };
                    choices.push((score, Goal::Loot(bag.id), bag.position));
                }
            }
        }
        let peaceful = nearest.is_none_or(|e| e.position.distance(v.position) > 5.);
        let realm = if v.map.name == "Nexus" && peaceful && health >= 0.75 {
            self.realm_portal(v, grid)?
        } else {
            None
        };
        for portal in v
            .objects
            .iter()
            .filter(|p| p.usable)
            .filter(|p| match v.map.name.as_str() {
                "Nexus" => {
                    matches!(p.kind, 0x704 | 0x70e | 0x71c)
                        || p.kind == 0x712 && Some(p.id) == realm
                }
                // The committed exit is public even when a straggler missed personal clear credit.
                "Pirate Cave" => p.kind == 0x704,
                "Snake Pit" => matches!(p.kind, 0x704 | 0x71d),
                "Oryx's Castle" => p.kind == 0xd7b && events::ready(v, content),
                "Oryx's Chamber" => p.kind == 0x71d || p.kind == 0x700 && events::ready(v, content),
                "Wine Cellar" | "Mad Lab" => matches!(p.kind, 0x71d | 0x712),
                "Realm" => {
                    progression::ready(v, content, p.kind)
                        && self.group.as_ref().is_none_or(|g| {
                            p.kind == 0x890 || !g.finished && g.purpose.portal() == p.kind
                        })
                }
                _ => false,
            })
        {
            let exiting = portal.kind == 0x71d
                || matches!(v.map.name.as_str(), "Pirate Cave" | "Snake Pit")
                || portal.kind == 0x712 && events::shared(&v.map.name);
            if exiting || ((peaceful || matches!(portal.kind, 0xd7b | 0x700)) && health >= 0.75) {
                let reward_waiting = choices.iter().any(|(_, goal, _)| {
                    if let Goal::Loot(id) = goal {
                        v.objects
                            .iter()
                            .find(|bag| bag.id == *id)
                            .is_some_and(|bag| {
                                bag.contents.iter().filter(|kind| **kind >= 0).any(|kind| {
                                    content.objects[&(*kind as u16)].stat_increase.is_some()
                                })
                            })
                    } else {
                        false
                    }
                });
                choices.push((
                    if exiting {
                        if reward_waiting
                            || events::shared(&v.map.name)
                                && choices
                                    .iter()
                                    .any(|(_, goal, _)| matches!(goal, Goal::Loot(_)))
                        {
                            0.
                        } else {
                            99.
                        }
                    } else if portal.kind == 0x700 {
                        101.
                    } else {
                        95.
                    },
                    Goal::Portal(portal.id),
                    portal.position,
                ));
            }
        }
        let committed = self.committed(v);
        let witnessed_portal = self.witnessed_portal(v, content, &choices, range);
        if let Some(portal) = witnessed_portal {
            let position = v
                .objects
                .iter()
                .find(|p| p.id == portal)
                .expect("observed portal exists")
                .position;
            choices.push((104., Goal::Portal(portal), position));
        }
        if let Some(agreement) = &self.agreement {
            let destination = match agreement.waiting {
                Some(position) => Some((Goal::Gather(v.session.player), position)),
                None => v
                    .objects
                    .iter()
                    .find(|p| p.name == agreement.partner && p.hp > 0)
                    .map(|p| (Goal::Follow(p.id), p.position)),
            };
            if let Some((goal, position)) = destination {
                choices.push((110., goal, position));
            }
        }
        for player in v
            .objects
            .iter()
            .filter(|o| content.objects[&o.kind].class == "Player")
        {
            let d = player.position.distance(v.position);
            if d > 4. && (self.companion == Some(player.id)) {
                let score = 100.;
                choices.push((score, Goal::Follow(player.id), player.position));
            }
        }
        // Players go where the people are; exploring alone is what is left when nobody suitable is on the minimap.
        if !committed
            // Approaching the quest's fight is the quest logic's job, landing well outside it.
            && let Some((marker, crowd)) = self.crowd.gathering(v, self.ambitious(), |position| {
                let quest = enemies.iter().find(|e| v.quest == Some(e.id));
                // Live: level 20s joined lowland crowds on unseen ground and farmed Hobbit Mages in groups of eight.
                let veteran_ground = v.character.level < 20
                    || quest.is_some_and(|q| q.position.distance(position) < 45.)
                    || self.crowd.bands.known(position).is_some_and(|band| band >= 3);
                !self.blocked.contains_key(&tile(position))
                    && quest.is_none_or(|q| q.position.distance(position) >= 20.)
                    && veteran_ground
            })
        {
            let score = 55. + 5. * crowd.min(6) as f32 + self.profile.companions;
            let distance = marker.position.distance(v.position);
            if !choices.iter().any(|(s, _, _)| *s > score)
                && crowd >= 2
                && peaceful
                && health >= 0.75
                && distance > 24.
                && v.map.allow_teleport
                && v.time_ms >= self.next_teleport
            {
                tracing::info!(
                    account = v.session.account,
                    target = marker.id,
                    crowd,
                    distance,
                    world = v.session.world,
                    "autonomous gathering teleport"
                );
                self.next_teleport = v.time_ms + 10000;
                decision.goal = Goal::Follow(marker.id);
                decision.journey = Some(Journey::Regroup(marker.id));
                return Ok(decision);
            }
            choices.push((score, Goal::Follow(marker.id), marker.position));
        }
        // Revisiting an emptied room must not compete with progressing through the dungeon.
        if v.map.name == "Realm"
            && let Some(place) = self.hunting
            && v.time_ms.saturating_sub(self.last_encounter)
                > ((if v.character.level < 3 {
                    60000.
                } else {
                    240000.
                }) / self.profile.resources) as u64
        {
            if v.position.distance(place) > 3. {
                let (x, y) = tile(place);
                choices.push((30., Goal::Revisit(x, y), center((x, y))));
            } else {
                self.hunting = None;
            }
        }
        let intentions = self.mind.intentions.as_ref().expect("observed intentions");
        for (score, goal, _) in &mut choices {
            *score += intentions.bonus(v, goal);
        }
        // A chosen journey is kept: no other journey replaces it before arrival, route failure or deadline.
        let travel = |goal: &Goal, p: Pos| match goal {
            Goal::Venture | Goal::Revisit(..) | Goal::Explore(..) => true,
            Goal::Survey(..) => p.distance(v.position) > 8.,
            Goal::Hunt(_) | Goal::Fight(_) => p.distance(v.position) >= range,
            _ => false,
        };
        // Live: a Venture trip kept a destination it had already reached, and the bot wandered around it.
        if self
            .travel
            .as_ref()
            .is_some_and(|(kept, _, to)| *kept == Goal::Venture && to.distance(v.position) < 4.)
        {
            self.travel = None;
        }
        if let Some((kept, deadline, to)) = self.travel.clone() {
            // Live: Venture's destination was re-chosen every decision from the current 8-tile region and flipped
            // at region edges, so bots walked back and forth; a trip keeps the destination it started with.
            for choice in choices
                .iter_mut()
                .filter(|c| c.1 == kept && kept == Goal::Venture)
            {
                choice.2 = to;
            }
            let offered = choices.iter().find(|(_, g, _)| *g == kept).map(|c| c.2);
            if v.time_ms >= deadline {
                self.travel = None;
                if let Goal::Hunt(id) | Goal::Fight(id) = kept {
                    self.unreachable.insert(id, v.time_ms + 300_000);
                }
                if let Some(p) = offered {
                    self.blocked.insert(tile(p), v.tick + 100);
                }
                self.plan.record(
                    plan::now(),
                    plan::Kind::Aborted,
                    self.plan.owner(),
                    "travel deadline passed".into(),
                );
            } else if offered.is_some() || matches!(kept, Goal::Explore(..)) {
                // A worthwhile quest is the reason to travel, not one more journey waiting its turn.
                choices.retain(|(_, g, p)| {
                    *g == kept || !travel(g, *p) || Some(g) == realm_quest.as_ref()
                });
            }
        }
        self.unreachable.retain(|_, until| v.time_ms < *until);
        choices.retain(|(_, g, p)| match g {
            Goal::Hunt(id) | Goal::Fight(id) => {
                !self.unreachable.contains_key(id) || !travel(g, *p)
            }
            _ => true,
        });
        // Exploration's maximum score is 20; a clear higher-ranked route makes its full-map search irrelevant.
        if !choices.iter().any(|(score, goal, position)| {
            score + if *goal == self.goal { 8. } else { 0. } > 20.
                && !self.blocked.contains_key(&tile(*position))
                && grid.segment(v.position, *position)
        }) {
            let retained = if let Goal::Explore(x, y) = self.goal {
                Some((x, y))
            } else {
                None
            };
            let reachable = grid.nearby_reachable(v.position, retained);
            if let Goal::Explore(x, y) = self.goal {
                let pos = center((x, y));
                if pos.distance(v.position) > 0.7 && reachable.contains(&(x, y)) {
                    choices.push((12., self.goal.clone(), pos));
                }
            }
            if !choices
                .iter()
                .any(|(_, g, _)| matches!(g, Goal::Explore(..)))
            {
                let next = grid
                    .nearby(&reachable, v.position)
                    .filter(|t| {
                        !self.blocked.contains_key(t) && self.crowd.bands.survivable(v, center(*t))
                    })
                    .filter(|t| {
                        center(*t).distance(v.position) > 3.
                            && center(*t).distance(v.position) < 24.
                    })
                    .max_by(|a, b| {
                        let frontier = |t| grid.frontier(t, v.map.width, v.map.height);
                        let priority = frontier(*a)
                            .cmp(&frontier(*b))
                            .then_with(|| self.visits.get(b).cmp(&self.visits.get(a)));
                        if !priority.is_eq() {
                            return priority;
                        }
                        let value = |t| {
                            let p = center(t);
                            quarry.map_or_else(
                                || {
                                    (p.x - v.position.x) * self.heading.cos()
                                        + (p.y - v.position.y) * self.heading.sin()
                                },
                                |enemy| -p.distance(enemy.position),
                            )
                        };
                        value(*a).total_cmp(&value(*b))
                    });
                if let Some((x, y)) = next {
                    choices.push((12., Goal::Explore(x, y), center((x, y))));
                }
            }
        }
        for (score, goal, _) in &mut choices {
            if *goal == self.goal {
                *score += 8.;
            }
        }
        choices.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, goal, destination) in choices {
            let shortcut = if matches!(goal, Goal::Hunt(_))
                && self.agreement.is_none()
                && self.companion.is_none()
                && health >= 0.75
                && peaceful
                && decision.item.is_none()
                && v.time_ms >= self.next_teleport
            {
                let clear = |p: Pos| enemies.iter().all(|e| e.position.distance(p) > 3.);
                if Some(&goal) == realm_quest.as_ref() {
                    self.crowd.nearest_to(v, destination, clear)
                } else {
                    self.crowd
                        .shortcut(v, destination)
                        .filter(|p| clear(p.position))
                }
                .map(|p| p.id)
            } else {
                None
            };
            if shortcut.is_none() && self.blocked.contains_key(&tile(destination)) {
                if self.agreement.as_ref().is_some_and(|a| a.waiting.is_some()) {
                    return Ok(decision);
                }
                continue;
            }
            let movement = match goal {
                Goal::Hunt(quest) if shortcut.is_some() => {
                    let target = shortcut.expect("selected dungeon shortcut missing");
                    self.next_teleport = v.time_ms + 10_000;
                    decision.journey = Some(Journey::Regroup(target));
                    tracing::info!(
                        account = v.session.account,
                        target,
                        quest,
                        world = v.session.world,
                        map = v.map.name,
                        "autonomous quest shortcut"
                    );
                    None
                }
                Goal::Gather(_) if v.position.distance(destination) < 0.7 => None,
                Goal::Survey(..) if v.position.distance(destination) <= 8. => None,
                Goal::Portal(id) if v.position.distance(destination) <= 2. => {
                    decision.journey = Some(Journey::Portal(id));
                    None
                }
                Goal::Fight(target) | Goal::Hunt(target)
                    if v.position.distance(destination) < range
                        && shot_clear(v, grid, destination, s.passes_cover) =>
                {
                    let heading = steering.filter(|(id, _)| *id == target).map_or_else(
                        || {
                            (v.position.y - destination.y).atan2(v.position.x - destination.x)
                                + std::f32::consts::FRAC_PI_2
                                    * if v.session.account % 2 == 0 { 1. } else { -1. }
                        },
                        |(_, heading)| heading,
                    );
                    let movement = combat::movement(
                        v,
                        grid,
                        enemies,
                        Some((destination, range)),
                        false,
                        perception,
                        Some(combat::Steering {
                            heading,
                            distance: (combat::speed(v) * self.profile.reaction as f32 * 0.05
                                + 0.5)
                                .max(1.25),
                            clearance: self.mind.cognition.pickup_clearance,
                        }),
                    );
                    self.steering =
                        movement.map(|p| (target, (p.y - v.position.y).atan2(p.x - v.position.x)));
                    movement
                }
                Goal::Follow(_) if v.position.distance(destination) < 4.5 => None,
                Goal::Loot(id) if v.position.distance(destination) < 1.4 => {
                    let bag = v
                        .objects
                        .iter()
                        .find(|o| o.id == id)
                        .context("selected bag missing")?;
                    if decision.item.is_none() {
                        decision.item = Some(
                            progression::pickup(
                                v,
                                content,
                                bag,
                                self.mind.future_class,
                                Some(&self.mind.market),
                            )?
                            .context("selected bag has no collectable item")?,
                        );
                    }
                    if let Some(action) = &decision.item {
                        self.mind.events.collecting(v, action);
                    }
                    if let Some(Action::Swap { from, to }) = &decision.item
                        && to.kind != -1
                    {
                        tracing::info!(
                            player = v.session.player,
                            held = to.kind,
                            collected = from.kind,
                            "autonomous exchanging carried spare for needed consumable"
                        );
                    }
                    None
                }
                _ => match if matches!(goal, Goal::Hunt(_) | Goal::Venture | Goal::Survey(..)) {
                    grid.towards_kept(v.position, destination, &mut self.edge)
                } else {
                    grid.next(v.position, destination)
                } {
                    Some(pos) => Some(pos),
                    None => {
                        if self.agreement.as_ref().is_some_and(|a| a.waiting.is_some()) {
                            return Ok(decision);
                        }
                        if matches!(goal, Goal::Hunt(_))
                            && let Some(mut clearing) = progression::clear_cover(
                                v,
                                content,
                                grid,
                                destination,
                                range,
                                s.passes_cover,
                            )
                        {
                            clearing.item = decision.item;
                            return Ok(clearing);
                        }
                        // An unreachable trip stays set aside long enough for the bot to do something else.
                        let aside = if travel(&goal, destination) {
                            6000
                        } else {
                            100
                        };
                        self.blocked.insert(tile(destination), v.tick + aside);
                        if travel(&goal, destination) {
                            self.travel = None;
                            if let Goal::Hunt(id) | Goal::Fight(id) = &goal {
                                self.unreachable.insert(*id, v.time_ms + 300_000);
                            }
                            self.plan.record(
                                plan::now(),
                                plan::Kind::Aborted,
                                self.plan.owner(),
                                "no route".into(),
                            );
                        }
                        continue;
                    }
                },
            };
            if let Goal::Portal(portal) = goal
                && witnessed_portal != Some(portal)
                && v.map.name == "Realm"
                && v.position.distance(destination) <= 2.
                && self.portal.is_none_or(|(seen, _)| seen != portal)
            {
                self.portal = Some((portal, if self.rng.gen_bool(0.5) { u64::MAX } else { 0 }));
            }
            if let Goal::Portal(id) = goal
                && let Some((called, until)) = self.portal
                && called == id
            {
                if until == u64::MAX {
                    let portal = v
                        .objects
                        .iter()
                        .find(|p| p.id == id)
                        .context("selected portal missing")?;
                    decision.chat = Some(match portal.kind {
                        0x717 => "pc here",
                        0x718 => "snake here",
                        0x890 => "lab here",
                        kind => anyhow::bail!("unsupported autonomous dungeon call {kind:#x}"),
                    });
                }
                if v.time_ms < until {
                    decision.journey = None;
                }
            }
            if let Goal::Follow(id) = goal
                && v.position.distance(destination) > 8.
                && v.time_ms >= self.next_teleport
                && decision.item.is_none()
                && v.objects
                    .iter()
                    .any(|p| p.id == id && p.hp * 4 >= p.max_hp * 3)
                && enemies
                    .iter()
                    .all(|e| e.position.distance(destination) > 3.)
                && self.crowd.bands.teleportable(v, destination)
            {
                decision.journey = Some(Journey::Regroup(id));
                self.next_teleport = v.time_ms + 10000;
            }
            if let Goal::Follow(id) = goal
                && self.goal != goal
            {
                tracing::info!(
                    account = v.session.account,
                    target = id,
                    world = v.session.world,
                    "autonomous follow selected"
                );
            }
            self.travel = travel(&goal, destination).then(|| match self.travel.take() {
                Some(kept) if kept.0 == goal => kept,
                _ => (
                    goal.clone(),
                    v.time_ms + 10_000 + (destination.distance(v.position) * 1000.) as u64,
                    destination,
                ),
            });
            decision.goal = goal;
            decision.movement = movement;
            if (self.agreement.is_some()
                && matches!(decision.goal, Goal::Follow(_) | Goal::Gather(_))
                || matches!(decision.goal, Goal::Survey(..) | Goal::Venture))
                && let Some(enemy) =
                    combat::target(v, content, grid, enemies, range, s.passes_cover)
            {
                combat::attack(
                    v,
                    content,
                    grid,
                    enemy,
                    self.tracks
                        .get(&enemy.id)
                        .map_or(Pos { x: 0., y: 0. }, |track| track.2),
                    perception,
                    &mut decision,
                )?;
            }
            if let Goal::Fight(id) | Goal::Hunt(id) = decision.goal {
                let enemy = enemies
                    .iter()
                    .find(|e| e.id == id)
                    .copied()
                    .context("selected enemy missing")?;
                let enemy = if matches!(decision.goal, Goal::Hunt(_))
                    && v.position.distance(enemy.position) >= range
                {
                    combat::target(v, content, grid, enemies, range, s.passes_cover)
                } else {
                    Some(enemy)
                };
                if let Some(enemy) = enemy {
                    combat::attack(
                        v,
                        content,
                        grid,
                        enemy,
                        self.tracks
                            .get(&enemy.id)
                            .map_or(Pos { x: 0., y: 0. }, |track| track.2),
                        perception,
                        &mut decision,
                    )?;
                }
            }
            // A crowd fighting gods advertises itself, as 2012 trains did with "train here".
            if let Goal::Fight(id) | Goal::Hunt(id) = decision.goal
                && v.map.name == "Realm"
                && v.character.level == 20
                && decision.chat.is_none()
                && v.time_ms >= self.next_train_call
                && enemies.iter().any(|e| {
                    e.id == id
                        && content.objects[&e.kind].god
                        && content.objects[&e.kind].terrain == 1
                })
                && self.crowd.near(v, v.position) >= 2
            {
                self.next_train_call = v.time_ms + self.rng.gen_range(120_000..300_000);
                decision.chat = Some("train here");
            }
            return Ok(decision);
        }
        self.heading += 0.7;
        Ok(decision)
    }
}
fn angle(a: Pos, b: Pos) -> f32 {
    (b.y - a.y).atan2(b.x - a.x)
}
fn tile(p: Pos) -> (i16, i16) {
    (p.x.floor() as i16, p.y.floor() as i16)
}
fn center(t: (i16, i16)) -> Pos {
    Pos {
        x: f32::from(t.0) + 0.5,
        y: f32::from(t.1) + 0.5,
    }
}
fn peaceful(enemies: &[&crate::world::view::VisibleObject], position: Pos) -> bool {
    enemies
        .iter()
        .all(|enemy| enemy.position.distance(position) > 5.)
}
fn hostile(v: &PlayerView, owner: i32) -> bool {
    owner != v.session.player
        && !v
            .objects
            .iter()
            .any(|o| o.id == owner && !o.equipped.is_empty())
}
fn shot_clear(v: &PlayerView, grid: &Grid, target: Pos, passes_cover: bool) -> bool {
    sight(v, grid, target, passes_cover, None)
}
fn sight(
    v: &PlayerView,
    grid: &Grid,
    target: Pos,
    passes_cover: bool,
    endpoint: Option<i32>,
) -> bool {
    let steps = (v.position.distance(target) * 8.).ceil().max(1.) as u32;
    (0..=steps).all(|step| {
        let f = step as f32 / steps as f32;
        let point = tile(Pos {
            x: v.position.x + (target.x - v.position.x) * f,
            y: v.position.y + (target.y - v.position.y) * f,
        });
        grid.known.contains(&point)
            && (passes_cover
                || !v
                    .objects
                    .iter()
                    .any(|o| o.blocks && Some(o.id) != endpoint && tile(o.position) == point))
    })
}
fn own(v: &PlayerView, slot: usize) -> Slot {
    Slot {
        object: v.session.player,
        slot,
        kind: v.character.items[slot],
    }
}
type TilePoint = (i16, i16);
/// Known damaging tiles within `radius` of the player, which a 2012 player sees as lava.
fn hazards_near<'a>(
    v: &'a PlayerView,
    content: &'a Content,
    radius: i16,
) -> impl Iterator<Item = TilePoint> + 'a {
    let (x, y) = tile(v.position);
    (x - radius..=x + radius)
        .flat_map(move |x| (y - radius..=y + radius).map(move |y| (x, y)))
        .filter(|p| {
            v.tiles
                .ground(*p)
                .is_some_and(|g| content.hazards.contains_key(&g))
        })
}
mod movement;
use movement::Grid;

pub(crate) mod runtime;
pub(crate) const OUTPUT_CAPACITY: usize = 512;
pub use runtime::start;
pub fn persona_count() -> usize {
    persona::PERSONAS.len()
}
pub fn profiles() -> Vec<&'static str> {
    persona::PERSONAS
        .iter()
        .chain(persona::GUESTS.iter())
        .map(|p| p.name)
        .collect()
}
fn selected_profile(name: &str) -> Result<Profile> {
    persona::PERSONAS
        .iter()
        .chain(persona::GUESTS.iter())
        .copied()
        .find(|p| p.name == name)
        .context("unknown actual gameplay profile")
}
pub fn validate_test_profile(profile: &str) -> Result<()> {
    ensure!(
        !selected_profile(profile)?.is_guest(),
        "guest profiles cannot use generic prepared Spawn; use Persona starting context"
    );
    Ok(())
}
pub async fn start_test_persona(
    app: &crate::server::App,
    profile: &str,
    world: i32,
    position: Pos,
) -> Result<crate::store::Character> {
    runtime::start_test_persona(app, selected_profile(profile)?, world, position).await
}
pub async fn start_test(
    app: &crate::server::App,
    character: crate::store::Character,
    world: i32,
    position: Pos,
    profile: &str,
    owner: crate::server::Owner,
) -> Result<()> {
    runtime::start_selected(
        app,
        selected_profile(profile)?,
        character,
        world,
        position,
        owner,
    )
    .await
}
