use super::*;
use crate::world::view::{VisibleObject, VisibleProjectile};

/// How finely a player reads motion; every visible hostile bullet is weighed.
pub(super) struct Perception {
    step: f32,
    directions: usize,
}
impl Perception {
    pub(super) fn of(cognition: &pursuits::Cognition) -> Self {
        Self {
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
    // Scanning every known tile here cost ~13% of bot CPU; clearance asks once per candidate move.
    let ground = v
        .tiles
        .tile(tile(v.position))
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

pub(super) fn clearance(v: &PlayerView, destination: Pos) -> f32 {
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
        for bullet in v.projectiles.iter().filter(|b| hostile(v, b.owner)) {
            if let Some(pos) = predicted(bullet, seconds) {
                minimum = minimum.min(player.distance(pos));
            }
        }
    }
    minimum
}

/// What a hit costs after own defense; shot damage is public in 2012's enemy-shot packet.
fn cost(v: &PlayerView, raw: i32, piercing: bool) -> i32 {
    crate::condition::damage(raw, v.character.stats[3], v.character.conditions, piercing)
}
/// Damage from visible bullets and bombs that reach this spot within half a second if the player stands still.
pub(super) fn incoming(v: &PlayerView) -> i32 {
    let bullets = v
        .projectiles
        .iter()
        .filter(|b| hostile(v, b.owner))
        .filter(|b| {
            (0..=10).any(|step| {
                predicted(b, step as f32 * 0.05).is_some_and(|p| p.distance(v.position) < 0.6)
            })
        })
        .map(|b| cost(v, b.damage, b.armor_piercing));
    let bombs = v
        .throws
        .iter()
        .filter(|t| t.lands_ms <= v.time_ms + 500 && bombed(t, v.position))
        .map(|t| cost(v, t.damage, false));
    bullets.chain(bombs).sum()
}
/// Inside a pending bomb's landing circle, with half a tile for the player's own size.
pub(super) fn bombed(t: &crate::world::view::VisibleThrow, p: Pos) -> bool {
    t.position.distance(p) < t.radius + 0.5
}
/// The hardest single hit anything nearby can land, from its bullets in flight and its known attacks.
pub(super) fn biggest_hit(v: &PlayerView, c: &Content, enemies: &[&VisibleObject]) -> i32 {
    let flying = v
        .projectiles
        .iter()
        .filter(|b| hostile(v, b.owner))
        .map(|b| cost(v, b.damage, b.armor_piercing));
    let known = enemies
        .iter()
        .filter(|e| e.position.distance(v.position) < 15.)
        .flat_map(|e| {
            c.objects[&e.kind]
                .projectiles
                .values()
                .map(|p| cost(v, p.max, p.armor_piercing))
        });
    flying.chain(known).max().unwrap_or(0)
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
        decision.aim = Some(point);
    }
    if decision.item.is_none() {
        decision.item = ability(v, content, grid, enemy)?;
    }
    Ok(())
}

fn berserk_ally(v: &PlayerView, content: &Content, grid: &Grid, range: f32) -> Result<bool> {
    use crate::condition::*;
    for ally in &v.objects {
        if ally.hp <= 0
            || ally.equipped.is_empty()
            || v.position.distance(ally.position) > range
            || !ally
                .condition_mask
                // Dazed overrides Berserk in the authoritative attack rate.
                .is_some_and(|mask| {
                    mask & (bit(BERSERK) | bit(DAZED) | bit(STUNNED) | (1 << 20)) == 0
                })
        {
            continue;
        }
        let Ok(kind) = u16::try_from(ally.equipped[0]) else {
            continue;
        };
        let Some(shot) = content.get(kind)?.projectiles.get(&0) else {
            continue;
        };
        for enemy in &v.objects {
            let distance = ally.position.distance(enemy.position);
            if !content.get(enemy.kind)?.enemy
                || enemy.hp <= shot.max
                || enemy.condition_mask.is_none()
                || !progression::vulnerable(enemy)
                || distance >= shot.speed * shot.lifetime / 10000.
            {
                continue;
            }
            // Public weapon reach and clear fire are evidence of a local fight, not peer intent.
            let steps = (distance * 8.).ceil().max(1.) as u32;
            if (0..=steps).all(|step| {
                let f = step as f32 / steps as f32;
                let point = tile(Pos {
                    x: ally.position.x + (enemy.position.x - ally.position.x) * f,
                    y: ally.position.y + (enemy.position.y - ally.position.y) * f,
                });
                grid.known.contains(&point)
                    && (shot.passes_cover
                        || !v
                            .objects
                            .iter()
                            .any(|o| o.blocks && tile(o.position) == point))
            }) {
                return Ok(true);
            }
        }
    }
    Ok(false)
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
            Ability::Nova => progression::vulnerable(enemy) && worthwhile(14., false),
            Ability::Poison { .. }
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
            Ability::Condition {
                effect: BERSERK,
                range: Some(range),
                ..
            } if v.character.class == 0x031d && berserk_ally(v, content, grid, *range)? => true,
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
        let direction =
            (direction / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
        let point = Pos {
            x: v.position.x + direction.cos() * distance,
            y: v.position.y + direction.sin() * distance,
        };
        if !grid.segment(v.position, point) {
            continue;
        }
        let safety = clearance(v, point);
        if steering.is_some_and(|s| safety < s.clearance) {
            continue;
        }
        // Live: Medusa bombs killed 17 level 20s a day; their landing circle is shown before impact.
        let bombs: f32 = v
            .throws
            .iter()
            .map(|t| (t.radius + 0.5 - t.position.distance(point)).max(0.) * -20.)
            .sum();
        let separation = enemies
            .iter()
            .map(|e| point.distance(e.position))
            .fold(20., f32::min);
        let spacing = if retreat {
            // Beyond the 15-tile pressed radius, so a recovering player leaves Recover and resumes its plan.
            separation.min(16.) * 3.
        } else {
            target.map_or(0., |(p, range)| {
                -(point.distance(p) - range * 0.8).abs() * 3.
            })
        };
        // Retain safe steering, not physical inertia; urgent avoidance stays first: https://www.red3d.com/cwr/steer/gdc99/
        let continuity = steering.map_or(0., |s| (direction - s.heading).cos() * 3.);
        let score = safety.min(4.) * 12. + separation.min(2.) * 5. + spacing + continuity + bombs;
        if best.is_none_or(|(prior, _)| score > prior) {
            best = Some((score, point));
        }
    }
    best.map(|(_, point)| point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::{BERSERK, DAZED, INVULNERABLE, QUIET, STUNNED, bit};

    #[test]
    fn speed_reads_the_own_known_tile() {
        let (_, v) = quest_tests::fixture();
        let own = tile(v.position);
        let scanned = v.tiles.iter().find(|t| (t.x, t.y) == own).unwrap();
        let looked_up = v.tiles.tile(own).unwrap();
        assert_eq!(
            (looked_up.x, looked_up.y, looked_up.ground),
            (scanned.x, scanned.y, scanned.ground)
        );
        assert_eq!(
            speed(&v),
            (4. + 5.6 * v.character.stats[4] as f32 / 75.) * scanned.ground_speed
        );
    }

    #[test]
    fn local_support_warrior_aura_beyond_self_reach_and_controls() {
        let (content, mut v) = quest_tests::fixture();
        v.character.class = 0x031d;
        v.character.items[0] = 0xa00;
        v.character.items[1] = 0xa66;
        v.objects[0].position = Pos { x: 19.5, y: 10.5 };
        let grid = Grid::terrain(&v);
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_none()
        );
        let mut ally = quest_tests::fixture().1.objects.remove(0);
        ally.id = 3;
        ally.kind = crate::content::WIZARD;
        ally.account = Some(3);
        ally.equipped = vec![0xa97, -1, -1, -1];
        ally.position = Pos { x: 14.5, y: 10.5 };
        v.objects.push(ally);
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_some(),
            "four-tile aura boundary is inclusive"
        );
        v.character.conditions = bit(BERSERK);
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_some(),
            "ally benefit is independent of own buff"
        );
        v.character.conditions = 0;
        for mask in [
            None,
            Some(bit(BERSERK)),
            Some(bit(DAZED)),
            Some(bit(STUNNED)),
            Some(1 << 20),
        ] {
            v.objects[1].condition_mask = mask;
            assert!(
                ability(&v, &content, &grid, &v.objects[0])
                    .unwrap()
                    .is_none()
            );
        }
        v.objects[1].condition_mask = Some(0);
        v.objects[1].position.x = 14.51;
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_none()
        );
        v.objects[1].position.x = 14.5;
        v.objects[1].hp = 0;
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_none()
        );
        v.objects[1].hp = 500;
        v.objects[0].condition_mask = Some(bit(INVULNERABLE));
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_none()
        );
        v.objects[0].condition_mask = Some(0);
        v.objects[0].position.x = 30.5;
        assert!(!berserk_ally(&v, &content, &grid, 4.).unwrap());
        v.objects[0].position.x = 19.5;
        v.objects[1].blocks = true;
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_none()
        );
        v.objects[1].blocks = false;
        v.character.mp = 59;
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_none()
        );
        v.character.mp = 200;
        v.character.ability_ready_ms = v.time_ms + 1;
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_none()
        );
        v.character.ability_ready_ms = 0;
        v.character.conditions = bit(QUIET);
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_none()
        );
        v.character.conditions = 0;
        v.objects[1].condition_mask = Some(bit(DAZED));
        v.objects[0].position.x = 14.;
        assert!(
            ability(&v, &content, &grid, &v.objects[0])
                .unwrap()
                .is_some(),
            "existing self-benefit remains despite Dazed ally"
        );
    }
}
