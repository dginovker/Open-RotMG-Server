#[derive(Clone, Copy, Debug)]
pub(crate) enum Aim {
    Target,
    Fixed(f32),
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Prediction {
    None,
    Lead(f32),
    Chance(f64),
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Alternative {
    pub projectile: u8,
    pub count: usize,
    pub gap: f32,
    pub offset: f32,
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Action {
    Shoot {
        projectile: u8,
        choice: Option<Alternative>,
    },
    Grenade {
        radius: f32,
        damage: i32,
    },
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Attack {
    pub action: Action,
    pub aim: Aim,
    pub count: usize,
    pub gap: f32,
    pub offset: f32,
    pub interval: Option<u64>,
    pub delay: u64,
    pub range: f32,
    pub predict: Prediction,
    pub enabled: bool,
}
impl Attack {
    pub(super) fn fan(projectile: u8, count: usize, gap: f32, interval: u64, range: f32) -> Self {
        Self {
            action: Action::Shoot {
                projectile,
                choice: None,
            },
            aim: Aim::Target,
            count,
            gap: gap.to_radians(),
            offset: 0.,
            interval: Some(interval),
            delay: 0,
            range,
            predict: Prediction::None,
            enabled: true,
        }
    }
    pub(super) fn ring(projectile: u8, count: usize, interval: u64, range: f32) -> Self {
        Self::fan(projectile, count, 360. / count as f32, interval, range)
            .offset(180. - 180. / count as f32)
    }
    pub(super) fn grenade(radius: f32, damage: i32, interval: u64, range: f32) -> Self {
        Self {
            action: Action::Grenade { radius, damage },
            ..Self::fan(0, 1, 0., interval, range)
        }
    }
    pub(super) fn offset(mut self, degrees: f32) -> Self {
        self.offset += degrees.to_radians();
        self
    }
    pub(super) fn delay(mut self, ms: u64) -> Self {
        self.delay = ms;
        self
    }
    fn predict(mut self, factor: f32) -> Self {
        self.predict = Prediction::Lead(factor);
        self
    }
    pub(super) fn chance(mut self, chance: f64) -> Self {
        self.predict = Prediction::Chance(chance);
        self
    }
    pub(super) fn fixed(mut self, angle: f32) -> Self {
        self.aim = Aim::Fixed(angle);
        self
    }
    pub(super) fn once(mut self) -> Self {
        self.interval = None;
        self
    }
    pub(super) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    fn choice(mut self, index: u8) -> Self {
        let Action::Shoot { choice, .. } = &mut self.action else {
            unreachable!()
        };
        *choice = Some(Alternative {
            projectile: index,
            count: self.count,
            gap: self.gap,
            offset: self.offset,
        });
        self
    }
    pub(super) fn alternate(mut self, projectile: u8, count: usize, gap: f32, offset: f32) -> Self {
        let Action::Shoot { choice, .. } = &mut self.action else {
            panic!("grenade cannot have an alternate volley")
        };
        *choice = Some(Alternative {
            projectile,
            count,
            gap: gap.to_radians(),
            offset: offset.to_radians(),
        });
        self
    }
    pub fn shots(self, projectile: u8, heading: f32) -> Vec<(u8, f32)> {
        let heading = match self.aim {
            Aim::Target => heading,
            Aim::Fixed(angle) => angle,
        };
        (0..self.count)
            .map(|i| {
                (
                    projectile,
                    heading + self.offset + (i as f32 - (self.count - 1) as f32 / 2.) * self.gap,
                )
            })
            .collect()
    }
}
#[derive(Debug)]
pub(crate) struct Schedule {
    pub state: u64,
    pub age: u64,
    pub attacks: Vec<Attack>,
}
impl Schedule {
    pub fn snapshot(&self, heading: f32) -> Vec<(u8, f32)> {
        self.attacks
            .iter()
            .filter(|a| {
                a.enabled
                    && self.age >= a.delay
                    && match a.interval {
                        Some(ms) => (self.age - a.delay).is_multiple_of(ms),
                        None => self.age == a.delay,
                    }
            })
            .flat_map(|a| match a.action {
                Action::Shoot { projectile, .. } => a.shots(projectile, heading),
                Action::Grenade { .. } => vec![],
            })
            .collect()
    }
}
pub(crate) fn abyss_schedule(kind: u16, elapsed: u64) -> Schedule {
    use crate::dungeon::abyss::{self, IDOL, MALPHAS};
    let fan = Attack::fan;
    let mut state = 0;
    let mut age = elapsed;
    let attacks = match kind {
        0x66d => vec![fan(0, 5, 9., 1000, 20.)],
        0x66e => vec![fan(0, 3, 7.2, 1000, 15.)],
        0x66f | 0x670 => vec![fan(0, 3, 9., 1000, 15.)],
        0x671 | 0x672 => vec![fan(0, 3, 6., 500, 4.8)],
        0x908 => vec![fan(0, 3, 5., 1200, 5.).chance(0.45)],
        0x909 => vec![fan(0, 8, 45., 1, 0.).fixed(0.).delay(600).once()],
        0x90b => vec![
            fan(0, 1, 0., 200, 4.),
            fan(0, 8, 45., 1, 0.).fixed(0.).once().enabled(false),
        ],
        0x651 => vec![fan(0, 3, 15., 500, 10.)],
        MALPHAS => {
            let phase = abyss::phase(elapsed);
            state = elapsed / 50200 * 7 + phase;
            age = elapsed % 50200 - [0, 9000, 11500, 23500, 36500, 39000, 47000][phase as usize];
            let large = phase == 3;
            let rotation = age.saturating_sub(1800) / 1400 % 3;
            // Rotation must not rearm the other weapons, including the continuously firing radial slot.
            vec![
                fan(
                    if large { 3 } else { 0 },
                    1,
                    0.,
                    if phase == 5 { 2100 } else { 900 },
                    8.,
                )
                .offset(match phase {
                    0 => 1.,
                    2 => 0.6,
                    _ => 0.,
                })
                .chance(match phase {
                    0 | 2 => 0.15,
                    3 => 0.2,
                    5 => 0.25,
                    _ => 0.,
                })
                .delay(if large { 1800 } else { 0 })
                .enabled(matches!(phase, 0 | 2 | 3 | 5)),
                fan(2, 6, 60., 1200, 0.).fixed(0.).enabled(phase == 2),
                fan(4, 3, 120., 700, 0.)
                    .fixed((rotation as f32 * 40.).to_radians())
                    .offset(0.7)
                    .delay(1800)
                    .enabled(large),
                fan(3, 1, 0., 2000, 8.)
                    .chance(0.25)
                    .delay(3200)
                    .enabled(large && age >= 3200 && rotation == 1),
            ]
        }
        IDOL => vec![
            fan(0, 6, 60., 400, 0.).fixed(age as f32 / 1500.),
            fan(2, 4, 90., 1200, 0.).fixed(0.),
            fan(3, 3, 10., 1400, 20.),
            fan(3, 3, 10., 1400, 20.).delay(400),
        ],
        _ => unreachable!("unregistered Abyss schedule {kind:#x}"),
    };
    Schedule {
        state,
        age,
        attacks,
    }
}
fn manor_schedule(kind: u16, hp: i32, elapsed: u64) -> Schedule {
    use crate::dungeon::manor::*;
    let mut state = 0;
    let mut age = elapsed;
    let fan = Attack::fan;
    let attacks = match kind {
        BOSS => {
            let (phase, t) = crate::dungeon::manor::phase(elapsed);
            age = t;
            state = phase;
            match phase % 3 {
                0 => vec![fan(0, blades(hp), 10., 700, 30.)],
                1 => [(750, 20), (1450, 6), (2250, 15), (2650, 25)]
                    .into_iter()
                    .map(|(delay, n)| Attack::ring(1, n, 50, 30.).fixed(0.).delay(delay).once())
                    .collect(),
                2 => vec![],
                _ => unreachable!(),
            }
        }
        BAT | ESCORT | CHASER => vec![fan(0, 1, 0., 900, 1.)],
        0x1727 => vec![fan(0, 1, 0., 100, 1.)],
        CREATURE => vec![fan(0, 1, 0., 200, 4.).delay(1500)],
        FERATU => vec![fan(0, 5, 9., 1200, 8.)],
        0x1728 => vec![fan(0, 3, 20., 1500, 8.), fan(1, 5, 5., 1700, 9.)],
        0x1729 => vec![fan(0, 2, 180., 1500, 10.), fan(1, 1, 0., 750, 8.)],
        ROCK => vec![Attack::ring(0, 8, 50, 0.).fixed(0.).delay(3000).once()],
        0x172b => vec![fan(0, 5, 10., 1500, 10.)],
        0x172c => vec![fan(0, 3, 5., 1500, 9.)],
        COFFIN | SWARM | DECOY | KAGE | MINE | GRAVE => vec![],
        _ => unreachable!("unregistered Manor attack"),
    };
    Schedule {
        state,
        age,
        attacks,
    }
}
pub(crate) fn manor_entry_schedule(kind: u16, stage: u64, elapsed: u64) -> Schedule {
    use crate::dungeon::manor::{KAGE, MINE};
    let mut state = stage << 32;
    let mut age = elapsed;
    let attacks = match (kind, stage) {
        (KAGE, 2) => {
            let t = elapsed % 1400;
            let aimed = t >= 1000;
            state += elapsed / 1400 * 2 + u64::from(aimed);
            age = if aimed { t - 1000 } else { t };
            if aimed {
                vec![Attack::fan(1, 2, 180., 400, 20.)]
            } else {
                vec![
                    Attack::fan(1, 2, 180., 300, 0.)
                        .fixed((t / 300) as f32 * 30f32.to_radians())
                        .offset(90.),
                ]
            }
        }
        (KAGE, 3) => vec![Attack::fan(0, 2, 50., 400, 20.)],
        (MINE, 1) => vec![Attack::fan(0, 4, 45., 50, 60.).once()],
        (KAGE, 0 | 1) | (MINE, 0) => vec![],
        _ => unreachable!("invalid Manor entrance phase"),
    };
    Schedule {
        state,
        age,
        attacks,
    }
}

pub(crate) fn schedule(kind: u16, hp: i32, elapsed: u64) -> Option<Schedule> {
    if crate::dungeon::abyss::ROSTER.contains(&kind) {
        return Some(abyss_schedule(kind, elapsed));
    }
    if crate::dungeon::manor::ROSTER.contains(&kind) {
        return Some(manor_schedule(kind, hp, elapsed));
    }
    if crate::dungeon::davy::profile(kind).is_some() {
        return Some(davy_schedule(kind, 1, elapsed));
    }
    if super::landmarks::ROSTER.contains(&kind) {
        return Some(super::landmarks::schedule(kind, hp, 0, 0, elapsed));
    }
    if super::fauna::ROSTER.contains(&kind) {
        return Some(super::fauna::schedule(kind, hp, elapsed));
    }
    let fan = Attack::fan;
    let ring = Attack::ring;
    let grenade = Attack::grenade;
    let mut state = 0;
    let mut age = elapsed;
    let attacks = match kind {
        0x21b => vec![],
        0x21c => vec![fan(0, 1, 0., 1000, 10.).predict(1.)],
        0x21d..=0x21f => vec![fan(0, 1, 0., 500, 10.).predict(1.)],
        0x220 => vec![fan(0, 3, 20., 500, 10.)],
        0x936 => {
            state = u64::from(elapsed >= 2000);
            age = elapsed.saturating_sub(2000);
            vec![
                ring(0, 12, 1000, 3000.)
                    .fixed(22.5f32.to_radians())
                    .enabled(state == 1),
                fan(0, 1, 0., 1000, 10.).predict(1.).enabled(state == 1),
                fan(1, 1, 0., 2000, 10.).predict(1.).enabled(state == 1),
            ]
        }
        0x937..=0x93f => {
            let center: f32 = match kind {
                0x937 | 0x93d => 180.,
                0x938 => 240.,
                0x939 | 0x93e => 300.,
                0x93a => 360.,
                0x93b | 0x93f => 60.,
                0x93c => 120.,
                _ => unreachable!(),
            };
            // Equivalent wrapped angles keep all three threads in one native volley/ACK.
            vec![
                fan(0, 3, 60., crate::dungeon::spider::WEB_INTERVAL, 0.).fixed(center.to_radians()),
            ]
        }
        0x954 => vec![fan(0, 1, 0., 2000, 8.).choice(1)],
        0x60c => vec![fan(0, 1, 0., 5000, 10.), fan(1, 1, 0., 400, 5.)],
        0x953 => vec![fan(0, 1, 0., 1000, 8.), grenade(2., 12, 2500, 5.)],
        0x614 => {
            vec![
                fan(0, 1, 0., 1000, 10.),
                fan(1, 1, 0., if hp <= 182 { 1000 } else { 1300 }, 10.),
            ]
        }
        0x609 => {
            state = elapsed / 2000;
            age = elapsed % 2000;
            match state % 3 {
                0 => vec![fan(0, 3, 14., 1000, 10.).chance(0.3)],
                1 => vec![fan(0, 3, 10., 1000, 10.).chance(0.5)],
                _ => vec![],
            }
        }
        0x617 | 0x61a => {
            let mut attacks = (0..3)
                .map(|i| {
                    ring(i, 15, 1200, 25.)
                        .fixed(i as f32 * 8f32.to_radians())
                        .delay(i as u64 * 400)
                })
                .collect::<Vec<_>>();
            if kind == 0x61a {
                attacks.push(fan(3, 1, 0., 1000, 12.));
            }
            attacks
        }
        0x632 => vec![
            fan(0, 3, 15., 2000, 10.),
            fan(1, 1, 0., 2000, 10.).delay(1000),
        ],
        0x633 => {
            state = elapsed / 5000;
            age = elapsed % 5000;
            vec![fan(
                (state % 2) as u8,
                1,
                0.,
                if state.is_multiple_of(2) { 250 } else { 500 },
                10.,
            )]
        }
        0x640 => vec![
            fan(0, 3, 7., 1000, 10.),
            fan(1, 1, 0., 1000, 10.).delay(500).predict(1.),
        ],
        0x645 => vec![
            fan(0, 1, 0., 1000, 10.),
            grenade(2., 55, 1000, 5.).delay(500),
        ],
        0x649 => vec![
            fan(0, 1, 0., 2000, 10.).predict(0.5),
            fan(1, 1, 0., 2000, 10.).delay(1000).predict(1.),
        ],
        // Dungeon Quiet also predicts; its purple fan does not: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db/BehaviorDb.SpriteWorld.cs#L65-L70
        0xd05 => vec![
            fan(0, 4, 10., 1000, 12.),
            fan(1, 1, 0., 1000, 12.).predict(1.),
        ],
        // Only Quiet leads movement: https://steamcommunity.com/sharedfiles/filedetails/?id=181796604
        0x652 => vec![
            fan(0, 4, 10., 1000, 12.),
            fan(1, 1, 0., 1000, 10.).predict(1.),
        ],
        0x656 => vec![
            ring(0, 5, 750, 12.).predict(0.5),
            fan(1, 1, 0., 1000, 10.).predict(1.),
        ],
        0x658 => vec![
            fan(0, 5, 10., 1000, 12.).predict(1.),
            fan(1, 1, 0., 650, 10.).predict(1.),
        ],
        0x91a if hp >= 400 => {
            let phase = elapsed % 3750;
            state = elapsed / 3750 * 3
                + if phase < 2500 {
                    0
                } else if phase > 3500 {
                    2
                } else {
                    1
                };
            if phase < 2500 {
                age = phase;
                vec![]
            } else {
                age = phase - 2500;
                let step = (age.saturating_sub(200) / 200).min(4) as f32;
                if age > 1000 {
                    vec![]
                } else {
                    vec![
                        ring(0, 4, 200, 10.)
                            .fixed((step * 10.).to_radians())
                            .delay(200),
                        ring(0, 4, 200, 10.)
                            .fixed((90. - step * 10.).to_radians())
                            .delay(200),
                    ]
                }
            }
        }
        0x6d8 => {
            state = elapsed / 1500;
            age = elapsed % 1500;
            match state % 3 {
                0 => (0..3)
                    .map(|i| fan(0, 3, 10., 300, 15.).offset(i as f32 * 120.))
                    .collect(),
                1 => (0..3)
                    .map(|i| fan(0, 4, 10., 300, 15.).offset(60. + i as f32 * 120.))
                    .collect(),
                _ => vec![fan(1, 2, 15., 100, 15.).predict(1.)],
            }
        }
        0x906 | 0x907 if hp < 400 => vec![ring(0, 8, 50, 0.).fixed(0.)],
        crate::dungeon::snake::GUARD => {
            state = u64::from(hp < crate::dungeon::snake::GUARD_RAGE_HP);
            vec![
                fan(0, 3, 25., 1000, 25.),
                fan(1, 6, 60., 1000, 10.),
                fan(2, 3, 120., 2000, 15.).enabled(state == 1),
            ]
        }
        crate::dungeon::snake::DART => vec![fan(0, 1, 0., 2000, 25.)],
        crate::dungeon::snake::BUTTON | crate::dungeon::snake::SPAWNER => vec![],
        0x917 => {
            (state, age) = crate::dungeon::snake::phase(elapsed);
            match state % 3 {
                0 => vec![
                    fan(0, 2, 10., 1000, 10.).offset(45.),
                    fan(0, 1, 0., 1000, 10.).offset(135.),
                    fan(0, 1, 0., 1000, 10.).offset(225.),
                    fan(0, 2, 10., 1000, 10.).offset(315.),
                ],
                1 => vec![
                    fan(0, 1, 0., 1000, 10.).enabled(age < 16000),
                    fan(0, 1, 0., 1000, 10.).offset(90.).enabled(age < 16000),
                    fan(0, 1, 0., 1000, 10.).offset(270.).enabled(age < 16000),
                ],
                _ => vec![ring(1, 6, 500, 10.), Attack::grenade(2.5, 100, 1000, 10.)],
            }
        }
        0xd06 => {
            let t = elapsed % 43300;
            let cycle = elapsed / 43300 * 6;
            match t {
                0..2000 => {
                    state = cycle;
                    age = t;
                    vec![]
                }
                2000..2300 => {
                    state = cycle + 1;
                    age = t - 2000;
                    vec![]
                }
                2300..12300 => {
                    state = cycle + 2;
                    age = t - 2300;
                    vec![
                        fan(0, 2, 10., 400, 8.).offset(0.7).chance(0.4),
                        fan(0, 2, 180., 400, 8.).offset(0.7).chance(0.4),
                        fan(0, 18, 20., 1500, 8.)
                            .offset(0.4)
                            .chance(0.4)
                            .enabled(false),
                    ]
                }
                12300..32300 => {
                    let local = (t - 12300) % 5000;
                    if local < 1500 {
                        state = cycle + 3;
                        age = local;
                        vec![
                            fan(0, 1, 0., 1000, 8.).chance(0.1),
                            fan(0, 3, 120., 500, 8.).offset(0.3).chance(0.1),
                        ]
                    } else {
                        state = cycle + 4;
                        age = local - 1500;
                        vec![fan(0, 3, 120., 800, 8.).offset(0.3).chance(0.2)]
                    }
                }
                _ => {
                    state = cycle + 5;
                    age = t - 32300;
                    vec![fan(0, 3, 15., 1300, 8.)]
                }
            }
        }
        0xd07..=0xd0a => {
            let corner: f32 = match kind {
                0xd07 => 90.,
                0xd08 => 0.,
                0xd09 => 270.,
                0xd0a => 180.,
                _ => unreachable!(),
            };
            if elapsed < 2000 {
                vec![]
            } else {
                state = if elapsed < 8000 {
                    1
                } else if elapsed < 14000 {
                    2
                } else {
                    3
                };
                age = elapsed - [0, 2000, 8000, 14000][state as usize];
                let mut attacks = vec![
                    fan(0, 1, 0., 300, 0.).fixed(corner.to_radians()),
                    fan(0, 1, 0., 300, 0.).fixed((corner + 90.).to_radians()),
                ];
                if state == 2 {
                    attacks.push(fan(0, 1, 0., 300, 0.).fixed((corner + 45.).to_radians()));
                }
                attacks
            }
        }
        0xd90 => {
            let phase = elapsed / 13000;
            let local = elapsed % 13000;
            state = phase * 2 + u64::from(local >= 3000);
            if local < 3000 {
                age = local;
                vec![]
            } else {
                age = local - 3000;
                match phase % 4 {
                    0 => (0..5)
                        .map(|i| {
                            fan(0, 3, 120., 1000, 0.)
                                .fixed((i as f32 * 24.).to_radians())
                                .delay(i * 200)
                        })
                        .collect(),
                    1 => vec![fan(4, 12, 30., 2000, 10.)],
                    2 => vec![
                        fan(1, 8, 45., 1000, 10.),
                        fan(1, 8, 45., 1000, 10.).offset(22.5).delay(500),
                        fan(2, 3, 20., 2000, 8.),
                    ],
                    _ => vec![fan(1, 3, 10., 1000, 8.)],
                }
            }
        }
        0xd99 => vec![fan(0, 3, 20., 1000, 10.), fan(1, 1, 0., 1000, 10.)],
        0xd9a => vec![
            fan(0, 3, 20., 1000, 10.),
            fan(1, 3, 20., 2000, 10.).delay(100),
        ],
        0xda5 | 0xda6 => vec![fan(0, 1, 0., 500, 10.), fan(0, 1, 0., 1000, 3.)],
        _ => return None,
    };
    Some(Schedule {
        state,
        age,
        attacks,
    })
}

pub(crate) fn davy_schedule(kind: u16, stage: u64, age: u64) -> Schedule {
    use crate::dungeon::davy::{BOSS, LIT};
    let fan = Attack::fan;
    let attacks = match kind {
        BOSS if stage == 1 => vec![fan(0, 5, 10., 2000, 10.), fan(1, 1, 0., 4000, 10.)],
        LIT if stage == 2 => vec![fan(0, 6, 60., 1000, 10.).once().delay(100)],
        0xe31 => vec![fan(0, 1, 0., 2000, 10.)],
        0xe50 => vec![fan(0, 3, 10., 2000, 10.)],
        0xe3a => vec![fan(0, 3, 10., 1750, 10.)],
        0xe44 => vec![fan(0, 6, 60., 4000, 10.)],
        0xe2f => vec![fan(0, 1, 0., 1750, 10.)],
        0xe42 => vec![fan(0, 3, 10., 2500, 10.)],
        0xe43 => vec![fan(0, 2, 10., 2000, 10.)],
        0xe45 => vec![
            fan(0, 1, 0., 1100, 13.),
            fan(0, 1, 0., 1100, 13.).delay(50),
            fan(0, 1, 0., 1100, 13.).delay(100),
            fan(1, 5, 10., 2000, 7.),
        ],
        0xe48 if stage == 2 => vec![fan(0, 6, 60., 1000, 10.).once()],
        _ => vec![],
    };
    Schedule {
        state: stage,
        age,
        attacks,
    }
}

pub(crate) fn armor_schedule(stage: u64, age: u64) -> Schedule {
    let attacks = match stage {
        0 => vec![],
        1 => vec![Attack::fan(0, 2, 15., 600, 10.)],
        2 => vec![Attack::fan(0, 6, 60., 200, 10.)],
        _ => panic!("invalid Suit of Armor phase"),
    };
    Schedule {
        state: stage,
        age,
        attacks,
    }
}
pub(crate) fn endgame_schedule(kind: u16, hp: i32, max_hp: i32, elapsed: u64) -> Option<Schedule> {
    use crate::endgame::{ORYX, OryxPhase, oryx_phase};
    let fan = Attack::fan;
    let ring = Attack::ring;
    let mut state = 0;
    let mut age = elapsed;
    let attacks = match kind {
        0x668 => vec![
            fan(0, 5, 5., 1000, 10.).predict(1.),
            fan(1, 1, 0., 500, 10.),
        ],
        0x66a | 0x66b => vec![fan(0, 3, 5., 1000, 10.).predict(0.5)],
        0xd7e => {
            return Some(armor_schedule(
                if hp == max_hp {
                    0
                } else if hp < max_hp / 5 {
                    2
                } else {
                    1
                },
                elapsed,
            ));
        }
        0xd82 => {
            state = elapsed / 9000 * 2 + u64::from(elapsed % 9000 >= 5000);
            if elapsed % 9000 < 5000 {
                age = elapsed % 9000;
                vec![ring(1, 5, 1000, 10.)]
            } else {
                age = elapsed % 9000 - 5000;
                vec![
                    ring(1, 5, 1000, 10.),
                    ring(2, 5, 750, 10.),
                    fan(0, 3, 10., 300, 10.),
                ]
            }
        }
        0xd83 => {
            state = elapsed / 7500 * 2 + u64::from(elapsed % 7500 >= 5000);
            if elapsed % 7500 < 5000 {
                age = elapsed % 7500;
                vec![fan(0, 3, 20., 350, 10.)]
            } else {
                age = elapsed % 7500 - 5000;
                vec![
                    fan(0, 1, 0., 50, 10.),
                    fan(1, 1, 0., 1000, 10.),
                    fan(2, 1, 0., 450, 10.),
                ]
            }
        }
        0x1748 => vec![ring(0, 12, 700, 50.).delay(200)],
        0x174a if elapsed >= 10000 => {
            state = 1;
            age = elapsed - 10000;
            vec![fan(0, 1, 0., 700, 3.)]
        }
        0x174a | 0x174b => vec![fan(0, 3, 10., 1000, 25.)],
        0x944 => vec![
            fan(0, 1, 0., 1500, 8.).chance(1.),
            fan(1, 3, 20., 1500, 8.).delay(500),
        ],
        ORYX if hp <= max_hp / 5 => {
            state = u64::MAX;
            vec![
                fan(1, 2, 10., 1500, 7.).delay(2000),
                fan(16, 5, 10., 1500, 7.).delay(2000),
            ]
        }
        ORYX => {
            let phase = oryx_phase(elapsed);
            if phase != OryxPhase::Opening {
                let t = (elapsed - 20000) % 85000;
                let (index, start) = match phase {
                    OryxPhase::Dance => (1, 0),
                    OryxPhase::Artifacts => (2, 25000),
                    OryxPhase::Gaze => (3, 50000),
                    OryxPhase::DanceTwo => (4, 60000),
                    _ => unreachable!(),
                };
                state = (elapsed - 20000) / 85000 * 4 + index;
                age = t - start;
            }
            match phase {
                OryxPhase::Opening => vec![fan(5, 4, 10., 1000, 25.)],
                OryxPhase::Dance => vec![ring(6, 8, 700, 50.).delay(200)],
                OryxPhase::Artifacts => vec![
                    ring(7, 10, 500, 50.).delay(200),
                    ring(9, 3, 1500, 50.).delay(200),
                    ring(8, 10, 2000, 50.).delay(200),
                ],
                OryxPhase::Gaze => vec![fan(1, 2, 10., 1000, 7.).delay(800)],
                OryxPhase::DanceTwo => vec![
                    ring(8, 4, 200, 0.)
                        .fixed([0f32, 30., 15., 45.][(age / 200 % 4) as usize].to_radians()),
                ],
                _ => unreachable!(),
            }
        }
        _ => return None,
    };
    Some(Schedule {
        state,
        age,
        attacks,
    })
}
pub(crate) fn oryx2_schedule(phase: crate::endgame::OryxPhase, age: u64) -> Schedule {
    use crate::endgame::OryxPhase;
    let fan = Attack::fan;
    if phase == OryxPhase::PrepareRage {
        return Schedule {
            state: 1,
            age,
            attacks: vec![
                fan(13, 1, 0., 4000, 0.).fixed(0.).delay(4000),
                fan(14, 1, 0., 4000, 0.)
                    .fixed(30f32.to_radians())
                    .delay(4000),
            ],
        };
    }
    assert!(
        matches!(phase, OryxPhase::Opening | OryxPhase::Rage),
        "invalid Oryx2 attack phase"
    );
    let mut attacks = vec![Attack::ring(0, 8, 1500, 25.)];
    attacks.extend(
        [(1, 3), (2, 3), (3, 2), (4, 3), (5, 2), (6, 3)]
            .into_iter()
            .map(|(index, n)| fan(index, n, 10., 1000, 25.)),
    );
    if phase == OryxPhase::Rage {
        attacks.push(fan(13, 1, 0., 50, 25.).delay(8000).once());
        attacks.push(fan(14, 1, 0., 50, 25.).delay(8500).once());
    }
    Schedule {
        state: if phase == OryxPhase::Rage { 2 } else { 0 },
        age,
        attacks,
    }
}

#[derive(Clone, Copy, Debug)]
enum Deadline {
    Unarmed,
    At(u64),
    Finished,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Clocks {
    state: Option<u64>,
    deadlines: Vec<Deadline>,
}
impl Clocks {
    pub fn due(&mut self, schedule: &Schedule, now: u64) -> Vec<usize> {
        if self.state != Some(schedule.state) {
            self.state = Some(schedule.state);
            self.deadlines = vec![Deadline::Unarmed; schedule.attacks.len()];
        }
        assert_eq!(
            self.deadlines.len(),
            schedule.attacks.len(),
            "attack channels changed without a phase transition"
        );
        self.deadlines
            .iter_mut()
            .enumerate()
            .filter_map(|(i, deadline)| {
                let attack = schedule.attacks[i];
                if !attack.enabled {
                    *deadline = Deadline::Unarmed;
                    return None;
                }
                if matches!(deadline, Deadline::Unarmed) {
                    *deadline = Deadline::At(
                        now.checked_sub(schedule.age)
                            .expect("attack phase age exceeds world time")
                            + attack.delay,
                    );
                }
                let Deadline::At(at) = *deadline else {
                    return None;
                };
                if now < at {
                    return None;
                }
                *deadline = match attack.interval {
                    Some(ms) => Deadline::At(now + ms),
                    None => Deadline::Finished,
                };
                Some(i)
            })
            .collect()
    }
    pub fn suspend(&mut self, delta: u64) {
        for clock in &mut self.deadlines {
            if let Deadline::At(at) = clock {
                *at += delta;
            }
        }
    }
}
