use super::{
    Motion, Profile,
    attacks::{Attack, Schedule},
    nr_speed,
};

pub(crate) const ROSTER: &[u16] = &[
    0x678, 0x679, 0x67a, 0x67b, 0x67c, 0x925, 0x65a, 0x65b, 0x65c, 0x65d, 0x65e, 0x935, 0x941,
    0x942, 0x943, 0xd50, 0xd51, 0xd52, 0xd53, 0xd54, 0xd55, 0xe37, 0xe38, 0xe39, 0xe3b, 0xe3c,
    0xe3d, 0xe4f,
];
pub(crate) const ROOTS: &[u16] = &[0x678, 0x925, 0x935, 0x941, 0xd50, 0xd54, 0xe37];
pub(crate) fn owner(kind: u16) -> u16 {
    match kind {
        0x678..=0x67c => 0x678,
        0x925 | 0x65a..=0x65e => 0x925,
        0x935 => 0x935,
        0x941..=0x943 => 0x941,
        0xd50..=0xd53 => 0xd50,
        0xd54..=0xd55 => 0xd54,
        0xe37 | 0xe38 | 0xe39 | 0xe3b | 0xe3c | 0xe3d | 0xe4f => 0xe37,
        _ => panic!("unselected landmark identity {kind:#x}"),
    }
}
pub(crate) fn profile(kind: u16) -> Profile {
    let (motion, speed, interval, range) = match kind {
        0x678 => (Motion::WanderHome { radius: 2. }, nr_speed(0.4), 1000, 10.),
        0x679..=0x67c => (
            Motion::Wander,
            5.,
            if kind == 0x67b { 400 } else { 1000 },
            10.,
        ),
        0x925 => (Motion::WanderHome { radius: 3. }, 3., 1000, 10.),
        0x65a..=0x65e => (
            Motion::Chase {
                sight: 10.,
                stop: 2.,
            },
            8.,
            1000,
            10.,
        ),
        0x935 => (Motion::Stationary, 0., 50, 0.),
        0x941 => (Motion::WanderHome { radius: 7. }, nr_speed(0.3), 50, 10.),
        0x942 => (Motion::WanderHome { radius: 5. }, nr_speed(0.85), 1400, 10.),
        0x943 => (Motion::WanderHome { radius: 12. }, nr_speed(1.), 50, 10.),
        0xd54 => (Motion::Wander, nr_speed(0.5), 50, 20.),
        0xd55 => (
            Motion::Chase {
                sight: 15.,
                stop: 0.,
            },
            4.,
            50,
            10.,
        ),
        0xd50 => (Motion::Stationary, 0., 50, 20.),
        0xd51 => (
            Motion::Orbit {
                anchor: super::Anchor::QuestMember(0xd50),
                radius: 4.,
            },
            nr_speed(0.3),
            500,
            8.,
        ),
        0xd52 => (
            Motion::Follow {
                sight: 20.,
                stop: 4.,
                idle: nr_speed(0.3),
                duration: 0,
                cooldown: 0,
            },
            nr_speed(1.),
            700,
            13.,
        ),
        0xd53 => (
            Motion::Follow {
                sight: 20.,
                stop: 6.,
                idle: nr_speed(0.2),
                duration: 0,
                cooldown: 0,
            },
            nr_speed(0.6),
            50,
            10.,
        ),
        0xe37 => (Motion::Stationary, 0., 50, 20.),
        0xe38 | 0xe3c => (Motion::Stationary, 0., 0, 0.),
        0xe39 => (
            Motion::Charge {
                sight: 8.,
                cooldown: 2000,
                delay: 0,
                stop: 0.05,
                idle: nr_speed(0.1),
            },
            nr_speed(3.),
            1000,
            5.,
        ),
        0xe3b => (
            Motion::Orbit {
                anchor: super::Anchor::QuestMember(0xe37),
                radius: 8.,
            },
            nr_speed(0.4),
            50,
            0.,
        ),
        0xe3d => (Motion::Wander, nr_speed(0.1), 1300, 5.),
        0xe4f => (
            Motion::Follow {
                sight: 10.,
                stop: 0.,
                idle: 0.,
                duration: 0,
                cooldown: 0,
            },
            nr_speed(0.2),
            50,
            0.,
        ),
        _ => panic!("unselected landmark profile {kind:#x}"),
    };
    Profile {
        motion,
        speed,
        interval,
        range,
    }
}
pub(crate) fn schedule(kind: u16, _hp: i32, stage: u64, choice: u8, age: u64) -> Schedule {
    let fan = Attack::fan;
    let ring = Attack::ring;
    let mut state = 0;
    let mut at = age;
    let attacks = match kind {
        0x678 => {
            let mut a = fan(0, 4, 7., 1000, 10.);
            a.offset = 1.;
            vec![a]
        }
        0x679..=0x67c => {
            let mut a = fan(0, 1, 0., if kind == 0x67b { 400 } else { 1000 }, 10.);
            if matches!(kind, 0x67a | 0x67c) {
                a.offset = 0.5;
            }
            vec![a]
        }
        0x925 => vec![fan(0, 3, 6., 1000, 10.)],
        0x65a..=0x65e => vec![fan(0, 1, 0., 1000, 10.)],
        0x935 => {
            state = stage;
            match stage {
                0..=4 => vec![],
                5 | 6 => vec![fan(0, 16, 22.5, 100000, 0.).fixed(0.).once()],
                _ => panic!("invalid crystal stage"),
            }
        }
        0x941 => {
            state = stage;
            match stage {
                0 | 2 | 4 | 7 => vec![],
                1 => {
                    state = 100 + age / 200;
                    at = age % 200;
                    let step = age / 200 % 8;
                    vec![
                        fan(0, 4, 90., 300, 0.).fixed((step as f32 * 15.).to_radians()),
                        fan(5, 1, 0., 1000, 10.).enabled(matches!(step, 3 | 7)),
                    ]
                }
                3 => vec![fan(0, 1, 0., 1000, 10.)],
                5 => vec![
                    fan(5, 3, 120., 900, 10.),
                    fan(3, 3, 15., 1600, 0.).fixed(40f32.to_radians()),
                    fan(3, 3, 15., 1600, 0.).fixed(220f32.to_radians()),
                    fan(3, 3, 15., 1600, 0.)
                        .fixed(130f32.to_radians())
                        .delay(800),
                    fan(3, 3, 15., 1600, 0.)
                        .fixed(310f32.to_radians())
                        .delay(800),
                ],
                6 => [(40f32, 400), (60., 800), (50., 1200), (70., 1600)]
                    .into_iter()
                    .map(|(angle, delay)| {
                        fan(2, 9, 40., 2000, 0.)
                            .fixed(angle.to_radians())
                            .delay(delay)
                    })
                    .collect(),
                _ => panic!("invalid prisoner stage"),
            }
        }
        0x942 => vec![fan(0, 1, 0., 1400, 10.)],
        0x943 => {
            state = u64::from(age >= 800);
            at = age.saturating_sub(800);
            vec![fan(0, 1, 0., 500, 10.).chance(0.3).enabled(age >= 800)]
        }
        // BehaviorDb.Sphinx.cs, realm-src@1855c017; GPL-3.0, NR-Core and Skilly (data/REALM-SRC-LICENSE).
        0xd54 => {
            state = stage;
            match stage {
                0 | 2 | 4 | 6 | 7 => vec![],
                // Count-one source channels ignore shootAngle and remain independent.
                1 => vec![
                    fan(0, 1, 0., 800, 12.),
                    fan(0, 3, 10., 1000, 12.),
                    fan(0, 1, 0., 1000, 12.),
                    fan(0, 1, 0., 1000, 12.),
                ],
                3 => (0..4)
                    .flat_map(|n| {
                        [0., 180.].map(move |opposite| {
                            fan(1, 8, 10., 100000, 0.)
                                .fixed((n as f32 * 70. + opposite).to_radians())
                                .delay(n * 2000)
                                .once()
                        })
                    })
                    .collect(),
                5 => vec![
                    ring(2, 9, 2300, 0.).fixed(40f32.to_radians()),
                    fan(2, 2, 4., 700, 20.).enabled(choice == 0),
                    fan(2, 8, 5., 1100, 20.).enabled(choice == 1),
                ],
                _ => panic!("invalid Sphinx phase"),
            }
        }
        0xd55 => {
            state = stage;
            match stage {
                0 | 3 => vec![],
                1 => vec![ring(0, 6, 700, 0.).fixed(60f32.to_radians())],
                2 => vec![fan(0, 1, 0., 700, 7.)],
                _ => panic!("invalid Reaper phase"),
            }
        }
        0xd50 => {
            state = stage;
            match stage {
                0 | 2 | 5 | 7..=9 => vec![],
                1 => vec![
                    fan(0, 7, 10., 2000, 0.).fixed(0.),
                    fan(0, 7, 10., 2000, 0.).fixed(std::f32::consts::PI),
                ],
                3 => {
                    state = 100 + u64::from(choice);
                    if choice == 0 {
                        vec![fan(0, 7, 10., 2000, 12.), fan(0, 7, 190., 2000, 12.)]
                    } else {
                        vec![
                            fan(0, 7, 10., 3000, 0.).fixed(0.),
                            fan(0, 7, 10., 3000, 0.).fixed(std::f32::consts::PI),
                        ]
                    }
                }
                4 => vec![fan(0, 7, 10., 1300, 20.)],
                6 => {
                    state = 1000 + age / 1500;
                    at = age % 1500;
                    vec![
                        fan(1, 4, 90., 2000, 0.).fixed(if (age / 1500).is_multiple_of(2) {
                            std::f32::consts::FRAC_PI_2
                        } else {
                            std::f32::consts::FRAC_PI_4
                        }),
                    ]
                }
                10 => vec![
                    fan(1, 8, 45., 50, 0.)
                        .fixed(std::f32::consts::FRAC_PI_4)
                        .once(),
                ],
                _ => panic!("invalid lost-lands stage"),
            }
        }
        0xd51 => vec![fan(0, 4, 7., 500, 8.)],
        0xd52 => vec![fan(0, 1, 0., 700, 13.)],
        0xd53 => vec![
            fan(1, 8, 45., 3000, 0.).fixed(std::f32::consts::FRAC_PI_4),
            fan(0, 5, 10., 1500, 10.),
        ],
        0xe37 => {
            state = stage;
            match stage {
                0..=2 | 4 | 8 | 10 => vec![],
                3 => vec![fan(0, 3, 6., 2000, 10.), fan(0, 1, 0., 800, 10.)],
                5 => {
                    let t = age % 9500;
                    let starts = [0, 1000, 2000, 3000, 4500, 5500, 6500];
                    let phase = starts.iter().rposition(|start| t >= *start).unwrap();
                    state = 100 + age / 9500 * 7 + phase as u64;
                    at = t - starts[phase];
                    match phase {
                        0 | 2 => [90f32, 270.]
                            .into_iter()
                            .map(|a| fan(0, 3, 20., 3000, 0.).fixed(a.to_radians()))
                            .collect(),
                        1 => [0f32, 180.]
                            .into_iter()
                            .map(|a| fan(0, 3, 20., 3000, 0.).fixed(a.to_radians()))
                            .collect(),
                        3 => vec![fan(0, 12, 30., 3000, 0.).fixed(30f32.to_radians())],
                        4 | 6 => [0f32, 90., 180., 270.]
                            .into_iter()
                            .map(|a| {
                                fan(1, 1, 0., if phase == 6 { 5000 } else { 3000 }, 0.)
                                    .fixed(a.to_radians())
                            })
                            .collect(),
                        5 => [45f32, 135., 225., 315.]
                            .into_iter()
                            .map(|a| fan(1, 1, 0., 3000, 0.).fixed(a.to_radians()))
                            .collect(),
                        _ => unreachable!(),
                    }
                }
                6 | 9 => vec![
                    fan(0, 4, 90., 1000, 0.).fixed((90. + 35. * (age / 1000) as f32).to_radians()),
                    fan(1, 1, 0., 1400, 10.).chance(1.),
                ],
                7 | 11 => vec![
                    fan(0, 2, 8., 800, 20.),
                    fan(0, 3, 8., 1300, 20.),
                    fan(1, 2, 8., 2000, 20.),
                ],
                _ => panic!("invalid ship stage"),
            }
        }
        0xe38 | 0xe3c => vec![],
        0xe39 => vec![fan(0, 3, 8., 1000, 5.)],
        0xe3b => {
            state = stage;
            vec![
                fan(0, 10, 36., 700, 0.)
                    .fixed(36f32.to_radians())
                    .enabled(stage == 1),
            ]
        }
        0xe3d => {
            state = stage;
            vec![fan(0, 3, 12., 1300, 5.).enabled(stage == 0)]
        }
        0xe4f => {
            state = stage;
            vec![
                fan(0, 10, 36., 50, 0.)
                    .fixed(36f32.to_radians())
                    .once()
                    .enabled(stage == 1),
            ]
        }
        _ => panic!("unselected landmark attack {kind:#x}"),
    };
    Schedule {
        state,
        age: at,
        attacks,
    }
}
