use crate::content::Content;
use anyhow::{Result, ensure};

pub(crate) mod attacks;
pub(crate) mod fauna;
pub(crate) mod jungle;
pub(crate) mod landmarks;
pub(crate) mod tomb;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    Player,
    Parent,
    Kind(u16),
    Home,
    CampLeader,
    CampMember(u16),
    CampEntity(i32),
    BoundEntity(i32),
    QuestMember(u16),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    Stationary,
    Wander,
    WanderHome {
        radius: f32,
    },
    Chase {
        sight: f32,
        stop: f32,
    },
    Follow {
        sight: f32,
        stop: f32,
        idle: f32,
        duration: u64,
        cooldown: u64,
    },
    Charge {
        sight: f32,
        cooldown: u64,
        delay: u64,
        stop: f32,
        idle: f32,
    },
    Circle,
    Pace {
        distance: f32,
    },
    Tangential,
    Maintain {
        sight: f32,
        distance: f32,
    },
    Retreat {
        distance: f32,
    },
    Orbit {
        anchor: Anchor,
        radius: f32,
    },
    StrictOrbit {
        anchor: Anchor,
        radius: f32,
    },
    Protect {
        anchor: Anchor,
        distance: f32,
    },
}
#[derive(Clone, Copy)]
pub struct Profile {
    pub motion: Motion,
    pub speed: f32,
    pub interval: u64,
    pub range: f32,
}
// Kithio d15f321, compiled BehaviorDb.Mountain/Shoot: cooldown is checked before decrement.
// At our 50 ms tick, a 500 ms cooldown fires again after 550 ms, without an entry delay.
pub(crate) fn white_demon_schedule(age: u64) -> attacks::Schedule {
    let mut attack = attacks::Attack::fan(0, 3, 20., 550, 10.);
    attack.predict = attacks::Prediction::Lead(1.);
    attacks::Schedule {
        state: 0,
        age,
        attacks: vec![attack],
    }
}
pub(crate) fn nr_speed(rate: f32) -> f32 {
    (5.55 * rate + 0.74) * 1.5
}
pub fn profile(kind: u16) -> Option<Profile> {
    if crate::dungeon::lab::ROSTER.contains(&kind) {
        return Some(crate::dungeon::lab::profile(kind));
    }
    if matches!(
        kind,
        crate::dungeon::beach::BOSS | crate::dungeon::beach::BUM
    ) {
        return Some(crate::dungeon::beach::profile(kind));
    }
    if crate::dungeon::jungle::ROSTER.contains(&kind) {
        return Some(jungle::profile(kind));
    }
    if crate::dungeon::manor::ROSTER.contains(&kind) {
        return Some(crate::dungeon::manor::profile(kind));
    }
    if let Some(profile) = crate::dungeon::davy::profile(kind) {
        return Some(profile);
    }
    if let Some(profile) = crate::dungeon::tomb::profile(kind) {
        return Some(profile);
    }
    if let Some(profile) = crate::dungeon::trench::profile(kind) {
        return Some(profile);
    }
    if landmarks::ROSTER.contains(&kind) {
        return Some(landmarks::profile(kind));
    }
    if fauna::ROSTER.contains(&kind) {
        return Some(fauna::profile(kind));
    }
    use Motion::*;
    if crate::dungeon::spider::ROSTER.contains(&kind) {
        return Some(crate::dungeon::spider::profile(kind));
    }
    let chase = |stop| Chase { sight: 10., stop };
    let (motion, speed, interval, range) = match kind {
        0x172
        | 0x173
        | 0x175..=0x179
        | crate::dungeon::snake::BUTTON
        | crate::dungeon::snake::SPAWNER => (Stationary, 0., 0, 0.),
        crate::dungeon::snake::DART => (Stationary, 0., 2000, 25.),
        // Qualified Guard/button/dart comparator; selected stats/projectiles remain authoritative: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db/BehaviorDb.SnakePit.cs
        crate::dungeon::snake::GUARD => (WanderHome { radius: 4. }, nr_speed(0.2), 1000, 25.),
        0x600 => (
            Chase {
                sight: 6.,
                stop: 0.,
            },
            8.5,
            2500,
            3.,
        ),
        0x601 => (
            Chase {
                sight: 6.,
                stop: 1.,
            },
            11.,
            2500,
            3.,
        ),
        0x602 => (Wander, 6., 2000, 10.),
        0x603 => (Wander, 4., 2000, 8.),
        0x604 => (Wander, 2., 0, 0.),
        0x953 => (
            Chase {
                sight: 6.,
                stop: 1.,
            },
            4.,
            1000,
            6.,
        ),
        0x954 => (
            Chase {
                sight: 6.,
                stop: 0.,
            },
            8.,
            2000,
            6.,
        ),
        0x612 | 0x613 => (
            Chase {
                sight: 8.,
                stop: 2.,
            },
            8.,
            1000,
            3.,
        ),
        0x614 => (
            Chase {
                sight: 10.5,
                stop: 4.,
            },
            5.,
            100,
            10.,
        ),
        0x618 => (
            Chase {
                sight: 8.,
                stop: 1.,
            },
            8.5,
            1000,
            3.,
        ),
        0x619 => (
            Chase {
                sight: 8.,
                stop: 4.,
            },
            8.,
            1000,
            8.,
        ),
        0x61a => (
            Chase {
                sight: 10.5,
                stop: 6.,
            },
            7.5,
            200,
            10.5,
        ),
        0x60e => (Wander, 4., 1000, 5.),
        0x60f => (Wander, 4., 600, 5.),
        0x610 => (Wander, 4., 1800, 5.),
        0x683..=0x686 => (Circle, 6., 500, 100.),
        0x687..=0x689 => (
            Chase {
                sight: 11.,
                stop: 1.,
            },
            8.,
            500,
            100.,
        ),
        0x68a..=0x68f => (Wander, 2., 0, 0.),
        0x927 => (Wander, 2., 500, 10.),
        0x605 | 0x60a | 0x616 => (chase(4.), 8., 1000, 10.),
        0x606 | 0x615 => (chase(1.), 8., 1000, 3.),
        0x607 => (chase(6.), 6., 300, 10.),
        0x60c => (chase(4.), 6., 200, 10.),
        0x60d => (chase(4.), 6., 10000, 10.),
        0x609 => (chase(2.), 6., 1000, 10.),
        0x617 => (chase(6.), 7.5, 400, 12.),
        0x61c | 0x61e | 0x61f | 0x63a | 0x63b | 0x63e => (chase(1.), 10., 1000, 3.),
        0x61d | 0x63c => (chase(7.), 10., 1000, 10.),
        0x629 | 0x62c => (chase(1.), 12., 1000, 3.),
        0x62a => (Wander, 12., 1000, 10.),
        0x62b => (chase(7.), 12., 0, 0.),
        0x6c6 => (chase(2.), 4., 200, 10.),
        0x647 | 0x648 => (chase(7.), 12., 1000, 10.),
        0x649 => (chase(7.), 10., 1000, 10.),
        0x900..=0x902 => (Wander, 1., 500, 8.),
        0x63f => (Wander, 4., 1000, 10.),
        0x624 | 0xd00 => (Wander, 15., 300, 10.),
        0x625 | 0x626 | 0xd01..=0xd04 => (Wander, 15., 1000, 10.),
        0x62d | 0x631 => (chase(1.), 8., 1000, 10.),
        0x62e => (chase(1.), 8., 2000, 10.),
        0x62f => (chase(1.), 8., 1000, 3.),
        0x6d2..=0x6d4 => (Stationary, 0., 0, 0.),
        0x6d5..=0x6d7 => (Stationary, 0., 500, 5.),
        0x632 => (chase(1.), 10., 1000, 10.),
        0x633 => (chase(1.), 10., 250, 10.),
        0x640 => (chase(1.), 10., 500, 10.),
        0x642 => (chase(7.), 4., 500, 10.),
        0x644 => (chase(7.), 4., 200, 10.),
        0x643 => (chase(1.), 12., 500, 10.),
        0x645 => (chase(1.), 4., 1000, 10.),
        0x646 => (chase(1.), 10., 500, 10.),
        0x64a | 0x64c | 0x64d => (chase(7.), 12., 500, 10.),
        0x64b => (chase(1.), 12., 500, 3.),
        0x64f => (chase(7.), 15., 500, 10.),
        0x650 => (chase(1.), 10., 500, 10.),
        0x651 => (
            Follow {
                sight: 10.,
                stop: 7.,
                idle: nr_speed(0.4),
                duration: 0,
                cooldown: 0,
            },
            nr_speed(1.),
            550,
            10.,
        ),
        0x654 => (chase(4.), 6., 1000, 12.),
        0x652 | 0xd05 => (Wander, 2., 1000, 12.),
        0x653 => (
            Protect {
                anchor: Anchor::Parent,
                distance: 2.,
            },
            4.,
            0,
            0.,
        ),
        0x655 => (chase(4.), 10., 1250, 12.),
        0x656 => (chase(4.), 10., 250, 12.),
        0x657 => (chase(4.), 6., 500, 12.),
        0x658 => (chase(4.), 10., 50, 12.),
        0x659 => (chase(4.), 4., 900, 12.),
        0x91a => (chase(0.), 6., 50, 10.),
        0x6d8 => (chase(5.), 8., 300, 15.),
        0x905 => (Wander, 2., 1000, 15.),
        0x906 | 0x907 => (Wander, 4., 800, 15.),
        0x223 | 0x224 => (Wander, 8., 1000, 10.),
        0x225 | 0x227 => (chase(1.), 10., 1000, 10.),
        0x226 => (Wander, 5., 1000, 10.),
        0x228 => (chase(5.), 10., 1000, 10.),
        0x229 => (chase(5.), 10., 300, 10.),
        0x917 => (Stationary, 0., 500, 15.),
        0x918 => (
            Orbit {
                anchor: Anchor::Parent,
                radius: 10.,
            },
            nr_speed(7.5),
            1000,
            25.,
        ),
        0x919 => (
            Protect {
                anchor: Anchor::Parent,
                distance: 2.,
            },
            4.,
            1000,
            20.,
        ),
        0xd06 => (Wander, 3., 400, 12.),
        0xd07..=0xd0a => (Stationary, 0., 300, 100.),
        // Wandering runs alongside Septavius's attack queue: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db/BehaviorDb.Undead_Lair.cs
        0xd90 => (Wander, 0.5, 150, 12.),
        0xd91..=0xd93 => (chase(1.), 10., 1000, 6.),
        0xd94..=0xd98 => (chase(7.), 9., 1000, 10.),
        0xd99 | 0xd9a => (chase(7.), 8., 1000, 10.),
        0xd9b..=0xd9d => (Wander, 1., 1000, 10.),
        0xd9e | 0xd9f => (Wander, 1., 500, 10.),
        0xda2 | 0xda3 => (Stationary, 0., 1000, 3.),
        0xda4 => (chase(1.), 12., 1000, 10.),
        0xda5 | 0xda6 => (chase(1.), 10., 500, 10.),
        0xdaa | 0xdac => (chase(7.), 9., 1000, 8.),
        0xdab | 0xdad..=0xdaf | 0xdb0 | 0xdb2 => (chase(1.), 12., 1000, 10.),
        0xdb1 => (
            Protect {
                anchor: Anchor::Parent,
                distance: 6.,
            },
            10.,
            1000,
            10.,
        ),
        kind if crate::dungeon::abyss::ROSTER.contains(&kind) => {
            return Some(crate::dungeon::abyss::profile(kind));
        }
        kind if crate::quest::ROSTER.contains(&kind) => return crate::quest::profile(kind),

        kind if crate::endgame::ROSTER.contains(&kind) => return crate::endgame::profile(kind),
        _ => return None,
    };
    Some(Profile {
        motion,
        speed,
        interval,
        range,
    })
}
pub fn stage(kind: u16, hp: i32, elapsed: u64) -> u64 {
    match kind {
        0x609 => elapsed / 2000,
        0x614 => u64::from(hp <= 182),
        0x953 => u64::from(hp <= 80),
        0x61f => (elapsed / 2500) % 2,
        0x6c6 => (elapsed / 1000) % 2,
        0x649 => (elapsed / 1000) % 2,
        0x91a if hp < 400 => 2,
        0x91a => u64::from(elapsed % 3750 >= 2500),
        0x6d8 => (elapsed / 1500) % 3 + 3 * ((elapsed / 2000) % 3),
        0x906 | 0x907 => u64::from(hp < 400),
        0x917 => crate::dungeon::snake::phase(elapsed).0,
        0xd06 => {
            elapsed / 43300 * 5
                + match elapsed % 43300 {
                    0..2000 => 0,
                    2000..2300 => 1,
                    2300..12300 => 2,
                    12300..32300 => 3,
                    _ => 4,
                }
        }
        0xd90 => elapsed / 13000,
        _ => 0,
    }
}
pub fn movement(kind: u16, hp: i32, elapsed: u64) -> (Motion, f32) {
    if kind == crate::dungeon::spider::BOSS && elapsed < 2000 {
        return (Motion::Stationary, 0.);
    }
    if crate::dungeon::abyss::ROSTER.contains(&kind) {
        let p = crate::dungeon::abyss::profile(kind);
        return (p.motion, p.speed);
    }
    let p = profile(kind).expect("validated encounter motion");
    match kind {
        crate::dungeon::snake::GUARD if hp < crate::dungeon::snake::GUARD_RAGE_HP => (
            Motion::Follow {
                sight: 10.,
                stop: 3.,
                idle: nr_speed(0.2),
                duration: 0,
                cooldown: 0,
            },
            nr_speed(0.2),
        ),
        0x917 => match stage(kind, hp, elapsed) % 3 {
            1 => (Motion::WanderHome { radius: 2. }, nr_speed(0.4)),
            2 => (
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 0.1,
                },
                nr_speed(0.3),
            ),
            _ => (Motion::Stationary, 0.),
        },
        0xd06 => match stage(kind, hp, elapsed) % 5 {
            0 | 4 => (Motion::Wander, 3.),
            1 => (
                Motion::Chase {
                    sight: 10.,
                    stop: 2.,
                },
                36.,
            ),
            2 => (
                Motion::Orbit {
                    anchor: Anchor::Player,
                    radius: 4.,
                },
                10.8,
            ),
            // Center the box phase rather than freezing at the orbit endpoint: https://github.com/ossimc82/fabiano-swagger-of-doom/blob/6fd20aad4a7905b13f25389c68368a942a2b68cb/wServer/logic/db/BehaviorDb.SpriteWorld.cs#L45-L58
            3 => (
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 0.1,
                },
                nr_speed(1.4),
            ),
            _ => unreachable!(),
        },
        _ => (p.motion, p.speed),
    }
}
fn spread(index: u8, count: usize, gap: f32, heading: f32) -> Vec<(u8, f32)> {
    (0..count)
        .map(|i| {
            (
                index,
                heading + (i as f32 - (count - 1) as f32 / 2.) * gap.to_radians(),
            )
        })
        .collect()
}
fn ring(index: u8, count: usize, offset: f32) -> Vec<(u8, f32)> {
    (0..count)
        .map(|i| {
            (
                index,
                offset + i as f32 * std::f32::consts::TAU / count as f32,
            )
        })
        .collect()
}

pub fn phased_attacks(kind: u16, hp: i32, heading: f32, elapsed: u64) -> Vec<(u8, f32)> {
    if let Some(schedule) = attacks::schedule(kind, hp, elapsed) {
        return schedule.snapshot(heading);
    }
    match kind {
        0x927 if hp < 640 => vec![(0, heading), (1, heading)],
        0x172 | 0x173 | 0x175..=0x179 | 0x927 | 0x68a..=0x68f | 0x653 => vec![],
        0x604 | 0x6d2..=0x6d4 | crate::dungeon::beach::BOSS | crate::dungeon::beach::BUM => vec![],
        0x62f => spread(0, 2, 7., heading),
        0x601 | 0x602 | 0x603 | 0x612 | 0x613 | 0x618 | 0x619 => vec![(0, heading)],
        0x60e => spread(0, 2, 10., heading),
        0x610 => ring(0, 5, heading),
        0x600
        | 0x60f
        | 0x683..=0x689
        | 0x605..=0x607
        | 0x60a
        | 0x60d
        | 0x615
        | 0x616
        | 0x61c..=0x61f
        | 0x63a..=0x63c
        | 0x63e
        | 0x63f
        | 0x642
        | 0x643
        | 0x644
        | 0x629
        | 0x62c
        | 0x62d
        | 0x62e
        | 0x631
        | 0x6d5..=0x6d7
        | 0x647
        | 0x648
        | 0x900..=0x902
        | 0x64a..=0x64d
        | 0x64f
        | 0x223
        | 0x224
        | 0x226
        | 0x227
        | 0x229
        | 0x918
        | 0x919
        | 0xd91..=0xd94
        | 0xd96
        | 0xd97
        | 0xd9b..=0xd9d
        | 0xda4
        | 0xdaa..=0xdb2 => vec![(0, heading)],
        0x624 | 0xd00 => spread(0, 2, 7., heading),
        0x625 | 0xd01 => spread(0, 3, 7., heading),
        0x626 | 0xd02 => spread(0, 4, 7., heading),
        0xd03 | 0xd04 => spread(0, 5, 7., heading),
        0x646 => {
            if (elapsed / 500).is_multiple_of(3) {
                ring(0, 20, 0.)
            } else {
                vec![(0, heading)]
            }
        }
        0x6c6 => {
            if elapsed % 1000 % 400 < 200 {
                if (elapsed / 1000).is_multiple_of(2) {
                    spread(0, 3, 15., heading)
                } else {
                    vec![(1, heading)]
                }
            } else {
                vec![]
            }
        }
        0x62a => spread(0, 3, 15., heading),
        0x62b => vec![],

        0x650 => spread(0, 3, 7., heading),
        0x651 => spread(0, 3, 20., heading),
        0x654 | 0x655 => spread(0, 5, 10., heading),
        0x657 => ring(0, 5, heading),
        0x91a => vec![],
        0x905 => vec![(0, heading)],
        0x906 | 0x907 => spread(0, 3, 20., heading),
        0x659 => spread(0, 7, 25., heading),
        0x228 | 0xd95 | 0xd98 | 0xd9e | 0xd9f => spread(0, 3, 6., heading),
        0x225 => spread(0, 3, 6., heading),
        0xda2 | 0xda3 => ring(0, 12, heading),
        kind if crate::quest::ROSTER.contains(&kind) => vec![],
        _ => panic!("attack requested without an explicit encounter profile"),
    }
}
pub fn validate(content: &Content, kind: u16) -> Result<()> {
    if crate::dungeon::lab::ROSTER.contains(&kind) {
        return crate::dungeon::lab::validate(content, kind);
    }
    if matches!(
        kind,
        crate::dungeon::manor::KAGE | crate::dungeon::manor::MINE
    ) {
        let last = if kind == crate::dungeon::manor::MINE {
            1
        } else {
            3
        };
        for stage in 0..=last {
            for attack in attacks::manor_entry_schedule(kind, stage, 0).attacks {
                if let attacks::Action::Shoot { projectile, .. } = attack.action {
                    ensure!(
                        content.get(kind)?.projectiles.contains_key(&projectile),
                        "missing Manor entrance projectile {kind:#x}/{projectile}"
                    );
                }
            }
        }
    }
    if crate::dungeon::jungle::ROSTER.contains(&kind) {
        for stage in 0..=6 {
            for attack in jungle::schedule(kind, stage, 0).attacks {
                if let attacks::Action::Shoot { projectile, .. } = attack.action {
                    ensure!(
                        content.get(kind)?.projectiles.contains_key(&projectile),
                        "missing Jungle projectile {kind:#x}/{projectile}"
                    );
                }
            }
        }
        return Ok(());
    }
    if crate::dungeon::tomb::contains(kind) {
        return crate::dungeon::tomb::validate(content, kind);
    }
    if crate::dungeon::trench::ROSTER.contains(&kind) {
        return crate::dungeon::trench::validate(content, kind);
    }
    if crate::quest::ROSTER.contains(&kind) {
        return crate::quest::validate(content, kind);
    }
    if crate::endgame::ROSTER.contains(&kind) {
        return crate::endgame::validate_kind(content, kind);
    }
    ensure!(
        profile(kind).is_some(),
        "missing encounter profile for {kind:#x}"
    );
    for hp in [1, 2000, 4000, 6000, content.get(kind)?.hp] {
        for elapsed in (0..60000).step_by(500) {
            if let Some(schedule) = attacks::schedule(kind, hp, elapsed) {
                for attack in schedule.attacks {
                    ensure!(
                        attack
                            .interval
                            .is_none_or(|ms| ms > 0 && ms.is_multiple_of(50))
                            && attack.delay.is_multiple_of(50)
                            && (1..=255).contains(&attack.count)
                            && attack.gap.is_finite()
                            && attack.offset.is_finite()
                            && attack.range.is_finite()
                            && attack.range >= 0.,
                        "invalid attack channel for {kind:#x}"
                    );
                    match attack.action {
                        attacks::Action::Shoot { projectile, choice } => {
                            if let Some(other) = choice {
                                ensure!(
                                    (1..=255).contains(&other.count)
                                        && other.gap.is_finite()
                                        && other.offset.is_finite(),
                                    "invalid alternate volley for {kind:#x}"
                                );
                            }
                            for index in std::iter::once(projectile)
                                .chain(choice.map(|other| other.projectile))
                            {
                                ensure!(
                                    content.get(kind)?.projectiles.contains_key(&index),
                                    "encounter {kind:#x} references absent selected projectile {index}"
                                );
                            }
                        }
                        attacks::Action::Grenade { radius, damage } => {
                            ensure!(radius > 0. && damage > 0, "invalid grenade for {kind:#x}")
                        }
                    }
                }
            }
            for (index, _) in phased_attacks(kind, hp, 0., elapsed) {
                ensure!(
                    content.get(kind)?.projectiles.contains_key(&index),
                    "encounter {kind:#x} references absent selected projectile {index}"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn white_demon_does_not_inherit_medusas_longer_firing_radius() {
        for (kind, range) in [(0x651, 10.), (0x654, 12.), (0x657, 12.), (0x659, 12.)] {
            assert_eq!(super::profile(kind).unwrap().range, range, "{kind:#x}");
        }
    }
}
