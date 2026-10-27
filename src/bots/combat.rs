use super::*;
use crate::world::view::{VisibleObject, VisibleProjectile};

pub(super) struct Perception {
    attended: Vec<(i32, u8)>,
    step: f32,
    directions: usize,
}
impl Perception {
    pub(super) fn attend(v: &PlayerView, cognition: &pursuits::Cognition, occupied: bool) -> Self {
        let budget = if occupied {
            cognition.attention.saturating_sub(1).max(1)
        } else {
            cognition.attention
        };
        let mut candidates: Vec<_> = v
            .projectiles
            .iter()
            .filter(|b| hostile(v, b.owner))
            .collect();
        if events::shared(&v.map.name) {
            let approaching = |b: &VisibleProjectile| {
                (0..=3)
                    .filter_map(|step| predicted(b, step as f32 * 0.2))
                    .map(|p| p.distance(v.position))
                    .fold(f32::INFINITY, f32::min)
            };
            candidates.sort_by(|a, b| approaching(a).total_cmp(&approaching(b)));
        }
        let attended: Vec<_> = candidates
            .into_iter()
            .take(budget)
            .map(|b| (b.owner, b.id))
            .collect();
        if occupied || !attended.is_empty() {
            tracing::info!(
                account = v.session.account,
                player = v.session.player,
                tick = v.tick,
                occupied,
                ?attended,
                budget,
                velocity_step = cognition.velocity_step,
                directions = cognition.directions,
                "autonomous attended threat estimates"
            );
        }
        Self {
            attended,
            step: cognition.velocity_step,
            directions: cognition.directions,
        }
    }
    pub(super) fn velocity(&self, v: Pos) -> Pos {
        Pos {
            x: (v.x / self.step).round() * self.step,
            y: (v.y / self.step).round() * self.step,
        }
    }
}
pub(super) fn predicted(b: &VisibleProjectile, seconds: f32) -> Option<Pos> {
    let direction = Pos {
        x: b.angle.cos(),
        y: b.angle.sin(),
    };
    // Wave displacement is perpendicular, so projection recovers age without the private birth clock.
    let age = ((b.position.x - b.start.x) * direction.x + (b.position.y - b.start.y) * direction.y)
        * 10000.
        / b.speed;
    let age = age.max(0.) + seconds * 1000.;
    if age >= b.lifetime_ms {
        return None;
    }
    let phase = if b.id.is_multiple_of(2) {
        0.
    } else {
        std::f32::consts::PI
    };
    let wave =
        b.amplitude * (phase + age / b.lifetime_ms * b.frequency * std::f32::consts::TAU).sin();
    let distance = age * b.speed / 10000.;
    Some(Pos {
        x: b.start.x + distance * direction.x - wave * direction.y,
        y: b.start.y + distance * direction.y + wave * direction.x,
    })
}

pub(super) fn speed(v: &PlayerView) -> f32 {
    let ground = v
        .tiles
        .iter()
        .find(|t| (t.x, t.y) == tile(v.position))
        .expect("own tile missing from discovered terrain")
        .ground_speed;
    let flags = v.character.conditions;
    let speed = if flags & crate::content::PARALYZED != 0 {
        0.
    } else if flags & crate::content::SLOWED != 0 {
        4.
    } else {
        (4. + 5.6 * v.character.stats[4] as f32 / 75.)
            * if flags & crate::content::SPEEDY != 0 {
                1.5
            } else {
                1.
            }
    };
    speed * ground
}

pub(super) fn clearance(v: &PlayerView, destination: Pos, perception: &Perception) -> f32 {
    let speed = speed(v);
    let distance = v.position.distance(destination);
    let mut minimum = 20_f32;
    for step in 0..=24 {
        let seconds = step as f32 * 0.025;
        let fraction = if distance > 0. {
            (seconds * speed / distance).min(1.)
        } else {
            0.
        };
        let player = Pos {
            x: v.position.x + (destination.x - v.position.x) * fraction,
            y: v.position.y + (destination.y - v.position.y) * fraction,
        };
        for bullet in v
            .projectiles
            .iter()
            .filter(|b| perception.attended.contains(&(b.owner, b.id)))
        {
            if let Some(pos) = predicted(bullet, seconds) {
                minimum = minimum.min(player.distance(pos));
            }
        }
    }
    minimum
}

pub(super) fn lead(
    v: &PlayerView,
    target: &VisibleObject,
    velocity: Pos,
    projectile: &crate::content::Projectile,
) -> Pos {
    let mut point = target.position;
    for _ in 0..3 {
        let seconds =
            (v.position.distance(point) * 10. / projectile.speed).min(projectile.lifetime / 1000.);
        point = Pos {
            x: target.position.x + velocity.x * seconds,
            y: target.position.y + velocity.y * seconds,
        };
    }
    point
}

pub(super) fn target<'a>(
    v: &PlayerView,
    content: &Content,
    grid: &Grid,
    enemies: &[&'a VisibleObject],
    range: f32,
    passes_cover: bool,
) -> Option<&'a VisibleObject> {
    enemies
        .iter()
        .copied()
        .filter(|enemy| {
            progression::can_fight(v, content, enemy)
                && progression::vulnerable(enemy)
                && v.position.distance(enemy.position) < range
                && shot_clear(v, grid, enemy.position, passes_cover)
        })
        .max_by(|a, b| {
            (a.max_hp as f32 / (1. + v.position.distance(a.position)))
                .total_cmp(&(b.max_hp as f32 / (1. + v.position.distance(b.position))))
        })
}

pub(super) fn attack(
    v: &PlayerView,
    content: &Content,
    grid: &Grid,
    enemy: &VisibleObject,
    velocity: Pos,
    perception: &Perception,
    decision: &mut Decision,
) -> Result<()> {
    if !progression::vulnerable(enemy) {
        return Ok(());
    }
    let weapon = content.get(v.character.items[0] as u16)?;
    let projectile = weapon
        .projectiles
        .get(&0)
        .context("weapon projectile missing")?;
    let lead = lead(v, enemy, perception.velocity(velocity), projectile);
    let estimate = perception.velocity(Pos {
        x: lead.x - v.position.x,
        y: lead.y - v.position.y,
    });
    let point = Pos {
        x: v.position.x + estimate.x,
        y: v.position.y + estimate.y,
    };
    tracing::info!(player=v.session.player,target=enemy.id,velocity=?velocity,estimate=?estimate,aim=angle(v.position,point),"autonomous execution aim estimate");
    let range = projectile.speed * projectile.lifetime / 10000.;
    if v.character.conditions & crate::content::STUNNED == 0
        && v.position.distance(point) < range
        && shot_clear(v, grid, point, projectile.passes_cover)
    {
        decision.aim = Some(angle(v.position, point));
    }
    if decision.item.is_none() {
        decision.item = ability(v, content, grid, enemy)?;
    }
    Ok(())
}

/// Uses the equipped class ability against or away from `enemy`; healing tomes belong to `social`.
pub(super) fn ability(
    v: &PlayerView,
    content: &Content,
    grid: &Grid,
    enemy: &VisibleObject,
) -> Result<Option<Action>> {
    use crate::condition::*;
    use crate::content::Ability;
    let Ok(kind) = u16::try_from(v.character.items[1]) else {
        return Ok(None);
    };
    let item = content.get(kind)?;
    let distance = v.position.distance(enemy.position);
    if v.time_ms < v.character.ability_ready_ms
        || v.character.mp < item.mana_cost
        || v.character.conditions & bit(QUIET) != 0
        || distance > 14.
    {
        return Ok(None);
    }
    let weapon = content.get(v.character.items[0] as u16)?.projectiles[&0].clone();
    let health = v.character.hp as f32 / v.character.stats[0] as f32;
    let endangered = health < 0.6 && distance < 8.;
    let teleport = item.abilities.iter().find_map(|a| match a {
        Ability::Teleport { distance } => Some(*distance),
        _ => None,
    });
    // A teleporting item's single cursor is the escape landing, so it cannot also aim at the enemy.
    let aimed = teleport.is_none();
    // Snake Pit keeps a second cast in reserve for its boss.
    let worthwhile = |reach: f32, passes_cover: bool| {
        aimed
            && enemy.hp > weapon.max * 2
            && distance <= reach
            && shot_clear(v, grid, enemy.position, passes_cover)
            && (v.map.name != "Snake Pit"
                || v.quest == Some(enemy.id)
                || v.character.mp - item.mana_cost >= item.mana_cost)
    };
    let reach = |p: &crate::content::Projectile| p.speed * p.lifetime / 10000.;
    let mut wanted = false;
    for activation in &item.abilities {
        wanted |= match activation {
            Ability::Shoot => {
                let shot = item
                    .projectiles
                    .get(&0)
                    .context("ability projectile missing")?;
                worthwhile(reach(shot), shot.passes_cover)
            }
            // Area abilities land at the cursor, not after the weapon's flight time.
            Ability::Nova
            | Ability::Poison { .. }
            | Ability::Drain { .. }
            | Ability::Lightning { .. }
            | Ability::Daze { .. } => worthwhile(14., false),
            Ability::Trap { .. } => {
                worthwhile(14., false) && grid.walkable.contains(&tile(enemy.position))
            }
            Ability::SpiritBomb => worthwhile(3., false),
            // The server charges casts that only reach stasised or immune enemies.
            Ability::Stasis { .. } => {
                endangered
                    && aimed
                    && v.objects.iter().any(|o| {
                        content.objects[&o.kind].enemy
                            && o.hp > 0
                            && o.position.distance(enemy.position) <= 3.
                            && o.condition_mask
                                .is_some_and(|m| m & (bit(STASIS) | bit(STASIS_IMMUNE)) == 0)
                    })
            }
            Ability::Decoy { .. } | Ability::Teleport { .. } => endangered,
            Ability::Condition { effect, .. } if v.character.conditions & bit(*effect) != 0 => {
                false
            }
            Ability::Condition { effect, .. } => match *effect {
                INVISIBLE | INVULNERABLE | ARMORED => endangered,
                HEALING => health < 0.75,
                // Buffs start just before a melee weapon reaches its target.
                BERSERK | DAMAGING | SPEEDY => {
                    enemy.hp > weapon.max && distance <= reach(&weapon) + 2.
                }
                effect => anyhow::bail!("unsupported autonomous condition ability {effect}"),
            },
            Ability::Heal { .. } | Ability::Clear { .. } | Ability::Cleanse { .. } => false,
        };
    }
    if !wanted {
        return Ok(None);
    }
    let point = match teleport {
        Some(distance) => {
            let away = angle(enemy.position, v.position);
            // Leave margin for movement made while this decision was computed.
            let Some(landing) = (1..=3)
                .rev()
                .map(|step| Pos {
                    x: v.position.x + away.cos() * distance * step as f32 / 4.,
                    y: v.position.y + away.sin() * distance * step as f32 / 4.,
                })
                .find(|p| grid.walkable.contains(&tile(*p)))
            else {
                return Ok(None);
            };
            landing
        }
        None => enemy.position,
    };
    Ok(Some(Action::Use {
        slot: 1,
        x: point.x,
        y: point.y,
    }))
}

#[derive(Clone, Copy)]
pub(super) struct Steering {
    pub heading: f32,
    pub distance: f32,
    pub clearance: f32,
}

pub(super) fn movement(
    v: &PlayerView,
    grid: &Grid,
    enemies: &[&VisibleObject],
    target: Option<(Pos, f32)>,
    retreat: bool,
    perception: &Perception,
    steering: Option<Steering>,
) -> Option<Pos> {
    if v.character.conditions & crate::content::PARALYZED != 0 {
        return None;
    }
    let mut best = None;
    let distance = steering.map_or(1.25, |s| s.distance);
    for index in 0..perception.directions + usize::from(steering.is_some()) {
        let direction = if index == perception.directions {
            steering.expect("retained heading candidate").heading
        } else {
            index as f32 * std::f32::consts::TAU / perception.directions as f32
        };
        let point = Pos {
            x: v.position.x + direction.cos() * distance,
            y: v.position.y + direction.sin() * distance,
        };
        if !grid.segment(v.position, point) {
            continue;
        }
        let safety = clearance(v, point, perception);
        if steering.is_some_and(|s| safety < s.clearance) {
            continue;
        }
        let separation = enemies
            .iter()
            .map(|e| point.distance(e.position))
            .fold(20., f32::min);
        let spacing = if retreat {
            separation.min(12.) * 3.
        } else {
            target.map_or(0., |(p, range)| {
                -(point.distance(p) - range * 0.8).abs() * 3.
            })
        };
        // Retain safe steering, not physical inertia; urgent avoidance stays first: https://www.red3d.com/cwr/steer/gdc99/
        let continuity = steering.map_or(0., |s| (direction - s.heading).cos() * 3.);
        let score = safety.min(4.) * 12. + separation.min(2.) * 5. + spacing + continuity;
        if best.is_none_or(|(prior, _)| score > prior) {
            best = Some((score, point));
        }
    }
    best.map(|(_, point)| point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::{DAMAGING, HEALING, INVISIBLE, STASIS, bit};

    fn scene(weapon: i32, ability: i32, health: f32) -> (Content, PlayerView) {
        let (content, mut v) = super::super::quest_tests::fixture();
        v.position = Pos { x: 40.5, y: 40.5 };
        v.character.items[0] = weapon;
        v.character.items[1] = ability;
        v.character.hp = (v.character.stats[0] as f32 * health) as i32;
        v.objects[0].position = Pos { x: 44.5, y: 40.5 };
        (content, v)
    }
    fn cast(content: &Content, v: &PlayerView) -> Option<Pos> {
        let grid = Grid::terrain(v);
        match ability(v, content, &grid, &v.objects[0]).unwrap()? {
            Action::Use { slot: 1, x, y } => Some(Pos { x, y }),
            other => panic!("unexpected ability action {other:?}"),
        }
    }

    #[test]
    fn rogue_cloaks_only_when_injured_and_visible() {
        let (content, mut v) = scene(2675, 0xa56, 0.4);
        assert!(cast(&content, &v).is_some());
        v.character.conditions = bit(INVISIBLE);
        assert!(cast(&content, &v).is_none());
        let (content, v) = scene(2675, 0xa56, 1.);
        assert!(cast(&content, &v).is_none());
    }

    #[test]
    fn casts_skip_active_buffs_stasised_targets_and_aimless_teleports() {
        let (content, mut v) = scene(2561, 0xa52, 0.5);
        v.character.conditions = bit(HEALING) | bit(DAMAGING);
        assert!(cast(&content, &v).is_none());
        v.character.conditions = bit(HEALING);
        assert!(cast(&content, &v).is_some());

        let (content, mut v) = scene(2714, 0xae4, 0.4);
        v.objects[0].condition_mask = Some(bit(STASIS));
        assert!(cast(&content, &v).is_none());
        v.objects[0].condition_mask = Some(0);
        assert!(cast(&content, &v).unwrap().distance(v.objects[0].position) < 1e-3);

        let (content, v) = scene(2565, 0xae2, 1.);
        assert!(cast(&content, &v).is_none());
        let (content, v) = scene(2565, 0xae2, 0.4);
        let landing = cast(&content, &v).unwrap();
        assert!(
            landing.distance(Pos { x: 25.5, y: 40.5 }) < 1e-3,
            "{landing:?}"
        );
    }
}
