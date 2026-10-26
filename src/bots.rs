use crate::{
    content::Content,
    wire::{self, Packet, Reader},
    world::{
        Action, Pos, Slot,
        view::{OutputState, PlayerView},
    },
};
use anyhow::{Context, Result, ensure};
use rand::{Rng, SeedableRng};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

pub(crate) mod adventures;
mod calls;
pub(crate) mod cctv;
mod combat;
mod crowds;
pub(crate) mod dialogue;
mod director;
mod entries;
mod events;

mod persona;
mod progression;
pub(crate) mod pursuits;
#[cfg(test)]
mod quest_tests;
mod realms;
mod social;

/// Who a bot is: identity, temperament and skill. Built from `persona::PERSONAS`.
#[derive(Clone, Copy)]
struct Profile {
    // Keep persisted registry keys independent of the names visible to players.
    identity: &'static str,
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
    aim: Option<f32>,
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
    Supply,
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
            Self::Supply => "restocking",
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
    danger: f32,
    threatened: bool,
    retreat: bool,
    outmatched: bool,
    grid: Arc<Grid>,
    remedy: Option<&'a crate::world::view::VisibleObject>,
}
struct Brain {
    profile: Profile,
    activity: Activity,
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
    previous: Pos,
    stationary: u64,
    destination: Option<Pos>,
    blocked: BTreeMap<(i16, i16), u64>,
    cover: BTreeSet<(i16, i16)>,
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
    next_bank: u64,
    inland_reached: bool,
    nexus_until: u64,
    portal: Option<(i32, u64)>,
    hunting: Option<Pos>,
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
        mind.events.knows_blue = profile.knows_blue;
        Self {
            profile,
            activity: Activity::Busy,
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
            previous: view.position,
            stationary: view.tick,
            destination: None,
            blocked: BTreeMap::new(),
            cover: BTreeSet::new(),
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
            next_bank: 0,
            inland_reached: false,
            nexus_until: if view.map.name == "Nexus" {
                view.time_ms + 10000
            } else {
                0
            },
            portal: None,
            hunting: None,
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
            && text.eq_ignore_ascii_case(&format!("{} follow me", self.profile.name))
        {
            self.companion = Some(actor);
            self.invitation_until = view.time_ms + 60000;
            return Ok(true);
        }
        Ok(false)
    }

    fn navigation_grid(&mut self, v: &PlayerView, content: &Content, avoid: &[Pos]) -> Arc<Grid> {
        let measuring = crate::performance::Measure::start();
        let observed: BTreeSet<_> = v
            .objects
            .iter()
            .filter(|o| o.blocks && !content.objects[&o.kind].enemy)
            .map(|o| tile(o.position))
            .collect();
        // Only nearby observed absence clears remembered cover, not an out-of-view removal.
        self.cover
            .retain(|point| center(*point).distance(v.position) >= 19. || observed.contains(point));
        self.cover.extend(observed);
        let mut blockers: BTreeSet<_> = self
            .cover
            .iter()
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
            self.blocked.insert(tile(destination), v.tick + 100);
            tracing::warn!(player=v.session.player,tick=v.tick,goal=?self.goal,x=destination.x,y=destination.y,"autonomous route made no progress; temporarily excluded");
            self.heading = self.rng.gen_range(0.0..std::f32::consts::TAU);
            self.stationary = v.tick;
        }
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
        if v.lifecycle.busy {
            self.set_activity(v, Activity::Busy);
            return Ok(decision);
        }
        let (activity, decision) = self.arbitrate(v, content, decision)?;
        self.set_activity(v, activity);
        Ok(decision)
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
    ) -> Result<(Activity, Decision)> {
        self.mind.reconcile(v)?;
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
        let perception = combat::Perception::attend(v, &self.mind.cognition, inspecting);
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
                    )
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
        let danger = combat::clearance(v, v.position, &perception);
        let threatened = danger < self.mind.cognition.pickup_clearance;
        if health < self.profile.recovery {
            self.recover_until = v.tick + 60;
        }
        let outmatched = enemies.iter().any(|enemy| {
            !progression::can_fight(v, content, enemy) && enemy.position.distance(v.position) < 8.
        });
        let retreat = outmatched
            || v.tick < self.recover_until
            || (matches!(self.goal, Goal::Recover)
                && health < (self.profile.recovery + 0.15).min(0.9));
        let avoid: Vec<_> = enemies
            .iter()
            .filter(|enemy| !retreat && !progression::can_fight(v, content, enemy))
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
            danger,
            threatened,
            retreat,
            outmatched,
            grid,
            remedy,
        };
        if self.reflex(v, content, &s, &mut decision)? {
            return Ok((Activity::Survive, decision));
        }
        // Reflexes can escape a deal, but only durable receipts can settle it.
        if reserved {
            decision.item = None;
            if let Some(market) = self.mind.market.decide(
                v,
                content,
                self.mind.future_class,
                &s.grid,
                &mut self.rng,
            )? {
                return Ok((Activity::Trade, market));
            }
        }
        if (equipping && s.remedy.is_none()) || (v.map.name == "Vault" && s.inspecting) {
            return Ok((Activity::Equip, decision));
        }
        if let Some(mut market) =
            self.mind
                .market
                .decide(v, content, self.mind.future_class, &s.grid, &mut self.rng)?
        {
            if market.item.is_none() {
                market.item = decision.item;
            }
            return Ok((Activity::Trade, market));
        }
        if let Some(project) = self.mind.supply(v, content, &s.grid)? {
            return Ok((Activity::Supply, project));
        }
        if let Some(mut event) =
            self.mind
                .events
                .decide(v, content, &s.grid, self.mind.cognition.attention)?
        {
            if decision.item.is_some() {
                event.item = decision.item;
            }
            return Ok((Activity::Event, event));
        }
        if decision.item.is_none()
            && !reserved
            && s.nearest
                .is_none_or(|e| e.position.distance(v.position) > 5.)
        {
            decision.item = progression::consume(v, content)?;
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
        if v.map.name == "Vault"
            && self.agreement.is_none()
            && let Some(banking) = progression::bank(v, content, &s.grid, self.mind.future_class)?
        {
            self.next_bank = v.time_ms + 300000;
            if matches!(banking.journey, Some(Journey::Nexus)) && self.mind.social.trying_loadout(v)
            {
                decision.aim = Some(self.heading);
                return Ok((Activity::Bank, decision));
            }
            return Ok((Activity::Bank, banking));
        }
        let banking = v.time_ms >= self.next_bank && progression::stash(v, content)?;
        if self.agreement.is_none()
            && peaceful(&s.enemies, v.position)
            && let Some(social) =
                self.mind
                    .social
                    .decide(v, &s.grid, self.profile.companions, banking)
        {
            return Ok((Activity::Social, social));
        }
        if v.map.name == "Nexus" && self.agreement.is_none() && v.time_ms < self.nexus_until {
            return Ok((Activity::Linger, decision));
        }
        if v.map.name == "Realm"
            && self.agreement.is_none()
            && banking
            && v.character
                .items
                .iter()
                .skip(4)
                .filter(|k| **k >= 0)
                .count()
                >= 6
            && peaceful(&s.enemies, v.position)
            && s.health >= 0.75
        {
            decision.journey = Some(Journey::Nexus);
            if self.group.is_some() {
                decision.chat = Some("nexus");
            }
            return Ok((Activity::Bank, decision));
        }
        if let Some(target) = self.call_target(v, content)? {
            self.next_teleport = v.time_ms + 10_000;
            decision.goal = Goal::Follow(target);
            decision.journey = Some(Journey::Regroup(target));
            return Ok((Activity::Call, decision));
        }
        Ok((
            Activity::Adventure,
            self.adventure(v, content, &s, decision, banking)?,
        ))
    }

    /// Safety always outranks any activity: retreat, dodge, or leave for the Nexus.
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
            health,
            danger,
            threatened,
            retreat,
            outmatched,
            remedy,
            ..
        } = s;
        let (range, health, danger, threatened, retreat, outmatched, nearest, remedy) = (
            *range,
            *health,
            *danger,
            *threatened,
            *retreat,
            *outmatched,
            *nearest,
            *remedy,
        );
        let (grid, perception) = (&s.grid, &s.perception);
        if threatened
            && v.objects.iter().any(|o| o.contents.contains(&0xa22))
            && v.map.name == "Realm"
        {
            tracing::info!(
                account = v.session.account,
                danger,
                acceptance = self.mind.cognition.pickup_clearance,
                "autonomous exposed supply declined"
            );
            decision.goal = Goal::Recover;
            decision.journey = Some(Journey::Nexus);
            return Ok(true);
        }
        if retreat || threatened {
            decision.goal = if retreat {
                Goal::Recover
            } else if let Some(blue) = remedy {
                Goal::Remedy(blue.id)
            } else {
                Goal::Dodge
            };
            let opponent = if retreat {
                None
            } else {
                combat::target(v, content, grid, enemies, range, s.passes_cover)
            };
            let pressured = threatened
                || nearest.is_some_and(|e| e.position.distance(v.position) < range * 1.3);
            if pressured {
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
                decision.movement =
                    combat::movement(v, grid, enemies, focus, spacing, retreat, perception);
            }
            let held_heal = v
                .character
                .items
                .iter()
                .filter(|kind| **kind >= 0)
                .any(|kind| content.objects[&(*kind as u16)].activate == "Heal");
            let emergency = self.profile.recovery * 0.5;
            let separation = |p: Pos| {
                enemies
                    .iter()
                    .map(|e| e.position.distance(p))
                    .fold(f32::INFINITY, f32::min)
            };
            let escape =
                if outmatched && nearest.is_some_and(|e| e.position.distance(v.position) < 4.) {
                    Some("near_outmatched")
                } else if !held_heal && health < emergency {
                    Some("critical_health")
                } else if !held_heal
                    && retreat
                    && pressured
                    && !decision.movement.is_some_and(|p| {
                        combat::speed(v) > 0.
                            && recovery_step_valid(v, content, grid, p)
                            && (enemies.is_empty() || separation(p) > separation(v.position))
                            && combat::clearance(v, p, perception)
                                >= self.mind.cognition.pickup_clearance
                    })
                {
                    Some("no_viable_backstep")
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
                    emergency_threshold = emergency,
                    reason,
                    "autonomous recovery escape selected"
                );
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
            if let Some(enemy) = opponent {
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
            return Ok(true);
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
        banking: bool,
    ) -> Result<Decision> {
        let (enemies, grid, perception) = (&s.enemies, &s.grid, &s.perception);
        let (nearest, quarry, range, inspecting, health) =
            (s.nearest, s.quarry, s.range, s.inspecting, s.health);
        let mut choices = Vec::new();
        let event_area = self.mind.events.area(v);
        if let Some(area) = event_area {
            let (x, y) = tile(area);
            choices.push((97., Goal::Survey(x, y), area));
        }
        let inland = Pos {
            x: v.map.width as f32 * 0.5,
            y: v.map.height as f32 * 0.5,
        };
        if v.map.name == "Realm" && v.position.distance(inland) <= 12. {
            self.inland_reached = true;
        }
        if v.map.name == "Realm" && progression::ready(v, content, 0x718) && !self.inland_reached {
            choices.push((60. + self.profile.resources, Goal::Venture, inland));
        }
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
            let score = 45. - d * 2.
                + if d < range {
                    // Nearby reachable mobs are shared XP; equipment does not make them invisible.
                    60. + (enemy.max_hp as f32).sqrt().min(20.)
                } else {
                    0.
                };
            choices.push((score, Goal::Fight(enemy.id), enemy.position));
            if events::shared(&v.map.name)
                && progression::vulnerable(enemy)
                && events::objective(enemy.kind)
            {
                choices.push((110., Goal::Hunt(enemy.id), enemy.position));
            }
            if v.quest == Some(enemy.id) && !protected && (v.character.level >= 3 || d <= 48.) {
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
                if progression::pickup(v, content, bag, self.mind.future_class)?.is_some() {
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
        let realm = if v.map.name == "Nexus" && !banking && peaceful && health >= 0.75 {
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
                    if banking {
                        p.kind == 0x720
                    } else {
                        matches!(p.kind, 0x704 | 0x70e | 0x71c)
                            || p.kind == 0x712 && Some(p.id) == realm
                    }
                }
                // The committed exit is public even when a straggler missed personal clear credit.
                "Pirate Cave" => p.kind == 0x704,
                "Snake Pit" => p.kind == 0x71d,
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
                || v.map.name == "Pirate Cave"
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
        self.crowd
            .attend(v, content, &self.mind.cognition, inspecting);
        let recent_company = self
            .crowd
            .cooperator(v, |position| !self.blocked.contains_key(&tile(position)));
        let cooperative_goal = if self.agreement.is_none()
            && self.companion.is_none()
            && crowds::cooperative_available(&choices, v.position, range, |position| {
                !self.blocked.contains_key(&tile(position))
            })
            && let Some(peer) = &recent_company
        {
            choices.push((
                80. + self.profile.companions,
                Goal::Follow(peer.id),
                peer.position,
            ));
            Some(peer.id)
        } else {
            None
        };
        if self.agreement.is_none()
            && self.companion.is_none()
            && recent_company.is_none()
            && let Some(marker) = self.crowd.destination(v, self.group.as_ref(), |position| {
                !self.blocked.contains_key(&tile(position))
            })
        {
            let distance = marker.position.distance(v.position);
            if distance > 7. || self.crowd.keeping_company(marker.id) {
                let score = if self.crowd.keeping_company(marker.id) {
                    80.
                } else {
                    64.
                } + self.profile.companions;
                if !choices.iter().any(|(s, _, _)| *s > score)
                    && peaceful
                    && distance > 24.
                    && v.map.allow_teleport
                    && v.time_ms >= self.next_teleport
                {
                    self.next_teleport = v.time_ms + 10000;
                    decision.goal = Goal::Follow(marker.id);
                    decision.journey = Some(Journey::Regroup(marker.id));
                    tracing::info!(
                        account = v.session.account,
                        target = marker.id,
                        world = v.session.world,
                        reason = "seek_shared_xp",
                        "autonomous attended crowd travel"
                    );
                    let peer = marker.id;
                    self.crowd.approach_selected(peer);
                    return Ok(decision);
                }
                choices.push((score, Goal::Follow(marker.id), marker.position));
            }
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
                    .filter(|t| !self.blocked.contains_key(t))
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
            if self.blocked.contains_key(&tile(destination)) {
                if self.agreement.as_ref().is_some_and(|a| a.waiting.is_some()) {
                    return Ok(decision);
                }
                continue;
            }
            let movement = match goal {
                Goal::Gather(_) if v.position.distance(destination) < 0.7 => None,
                Goal::Survey(..) if v.position.distance(destination) <= 8. => None,
                Goal::Portal(id) if v.position.distance(destination) <= 2. => {
                    decision.journey = Some(Journey::Portal(id));
                    None
                }
                Goal::Fight(_) | Goal::Hunt(_)
                    if v.position.distance(destination) < range
                        && shot_clear(v, grid, destination, s.passes_cover) =>
                {
                    combat::movement(
                        v,
                        grid,
                        enemies,
                        Some(destination),
                        range,
                        false,
                        perception,
                    )
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
                            progression::pickup(v, content, bag, self.mind.future_class)?
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
                    grid.towards(v.position, destination)
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
                        self.blocked.insert(tile(destination), v.tick + 100);
                        continue;
                    }
                },
            };
            if let Goal::Portal(portal) = goal
                && witnessed_portal != Some(portal)
                && self.goal != goal
                && v.map.name == "Realm"
            {
                let portal = v
                    .objects
                    .iter()
                    .find(|p| p.id == portal)
                    .context("selected portal missing")?;
                decision.chat = Some(match portal.kind {
                    0x717 => "pc here",
                    0x718 => "snake here",
                    0x890 => "lab here",
                    kind => anyhow::bail!("unsupported autonomous dungeon call {kind:#x}"),
                });
            }
            if let Goal::Follow(id) = goal
                && cooperative_goal != Some(id)
                && v.position.distance(destination) > 8.
                && v.time_ms >= self.next_teleport
                && decision.item.is_none()
                && v.objects
                    .iter()
                    .any(|p| p.id == id && p.hp * 4 >= p.max_hp * 3)
                && enemies
                    .iter()
                    .all(|e| e.position.distance(destination) > 3.)
            {
                decision.journey = Some(Journey::Regroup(id));
                self.next_teleport = v.time_ms + 10000;
            }
            if self.agreement.is_none()
                && self.companion.is_none()
                && let Goal::Follow(id) = goal
                && cooperative_goal != Some(id)
            {
                if self.goal != goal {
                    tracing::info!(
                        account = v.session.account,
                        target = id,
                        reason = if self.crowd.keeping_company(id) {
                            "keep_shared_xp_company"
                        } else {
                            "seek_shared_xp"
                        },
                        "autonomous crowd approach selected"
                    );
                }
                self.crowd.approach_selected(id);
            }
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
                    && v.map.name == "Snake Pit"
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
fn recovery_step_valid(v: &PlayerView, content: &Content, grid: &Grid, pos: Pos) -> bool {
    if !pos.x.is_finite() || !pos.y.is_finite() || !grid.walkable.contains(&tile(pos)) {
        return false;
    }
    let own = tile(pos);
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
    // World::player_position_valid checks the half-cell footprint, not just the route centerline.
    neighbors.iter().all(|point| {
        v.tiles
            .iter()
            .find(|t| (t.x, t.y) == *point)
            .is_some_and(|t| t.ground != 255)
    }) && v.objects.iter().all(|o| {
        let point = tile(o.position);
        if point == own {
            !content.objects[&o.kind].blocks
        } else {
            !neighbors.contains(&point) || !content.objects[&o.kind].full_occupy
        }
    })
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
mod movement;
use movement::Grid;

mod runtime;
pub(crate) const OUTPUT_CAPACITY: usize = 512;
pub use runtime::start;
pub fn persona_count() -> usize {
    persona::PERSONAS.len()
}
pub fn profiles() -> Vec<&'static str> {
    persona::PERSONAS.iter().map(|p| p.name).collect()
}
fn selected_profile(name: &str) -> Result<Profile> {
    persona::PERSONAS
        .iter()
        .copied()
        .find(|p| p.name == name)
        .context("unknown actual gameplay profile")
}
pub fn validate_test_profile(profile: &str) -> Result<()> {
    selected_profile(profile).map(|_| ())
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
