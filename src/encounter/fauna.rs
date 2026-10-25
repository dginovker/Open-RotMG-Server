use super::{
    Motion, Profile,
    attacks::{Attack, Schedule},
};

pub(crate) const ROSTER: &[u16] = &[
    0x620, 0x622, 0x623, 0x6c5, 0x7f22, 0x205, 0x947, 0x948, 0x949, 0x945, 0x946, 0x952, 0x202,
    0x204, 0x20d, 0x20e, 0x20f, 0x635, 0x636, 0x637, 0x639, 0x6c4, 0x6c7, 0x6c8, 0x6c9, 0x6d1,
    0x206, 0x207, 0x208, 0x209, 0xda0, 0xda1, 0xda7, 0xda8, 0xda9, 0x903, 0x904, 0x950, 0x951,
    0x955, 0x957, 0x958, 0x959, 0x960, 0x961,
];
pub(crate) fn profile(kind: u16) -> Profile {
    let charge = |sight, cooldown, delay, stop, idle| Motion::Charge {
        sight,
        cooldown,
        delay,
        stop,
        idle,
    };
    let chase = |sight, stop| Motion::Chase { sight, stop };
    let (motion, speed, interval, range) = match kind {
        0x903 | 0x904 => (chase(10., 2.), 7., 200, 10.),
        0x950 => (Motion::WanderHome { radius: 2. }, 4., 2000, 10.),
        0x951 => (charge(5., 1000, 1000, 1., 4.), 10., 500, 10.),
        0x955 => (Motion::Wander, super::nr_speed(0.4), 500, 8.),
        0x957 => (Motion::WanderHome { radius: 1. }, 4., 1000, 10.),
        0x958 | 0x959 | 0x960 | 0x961 => (Motion::Stationary, 0., 100, 0.),
        0x952 => (chase(10., 2.), 10., 500, 10.),
        0x202 => (Motion::Wander, 8., 1000, 9.),
        0x204 => (charge(10., 5000, 0, 1., 2.), 20., 1000, 10.),
        0x20d => (charge(15., 5000, 0, 1., 2.), 25., 50, 0.),
        0x20e => (Motion::Wander, 4., 500, 10.),
        0x20f => (Motion::WanderHome { radius: 2. }, 4., 500, 10.),
        0x635..=0x637 | 0x639 => (
            chase(10., if kind == 0x637 { 3. } else { 2. }),
            10.,
            1000,
            10.,
        ),
        0x6c4 => (chase(10., 2.), 8., 500, 10.),
        0x6c7 => (Motion::Wander, 8., 500, 10.),
        0x6c8 => (chase(10., 0.), 6., 0, 0.),
        0x6c9 => (chase(10., 0.), 8., 500, 8.),
        0x6d1 => (Motion::Wander, 6., 1500, 10.),
        0x206 => (Motion::Stationary, 0., 0, 0.),
        0x207..=0x209 => (
            Motion::Wander,
            20.,
            1000,
            if kind == 0x209 { 5. } else { 8. },
        ),
        0xda0 | 0xda1 => (
            charge(8., 2000, 0, 0.05, super::nr_speed(0.1)),
            super::nr_speed(3.),
            1000,
            3.,
        ),
        0xda7..=0xda9 => (Motion::Wander, super::nr_speed(0.1), 1000, 10.),
        0x620 | 0x622 | 0x623 => (Motion::Wander, 4., 500, 10.),
        0x6c5 | 0x7f22 => (
            Motion::Chase {
                sight: 10.,
                stop: 2.,
            },
            4.,
            500,
            10.,
        ),
        0x205 => (
            Motion::Chase {
                sight: 15.,
                stop: 5.,
            },
            8.,
            1000,
            6.,
        ),
        0x947 => (Motion::WanderHome { radius: 1. }, 4., 1000, 10.),
        0x948 | 0x949 => (Motion::Wander, 4., 1000, 10.),
        0x945 => (
            Motion::Chase {
                sight: 10.,
                stop: 1.,
            },
            4.,
            500,
            10.,
        ),
        0x946 => (Motion::Wander, 4., 1000, 10.),
        _ => panic!("unknown fauna profile {kind:#x}"),
    };
    Profile {
        motion,
        speed,
        interval,
        range,
    }
}
pub(crate) fn schedule(kind: u16, hp: i32, elapsed: u64) -> Schedule {
    let fan = Attack::fan;
    let ring = Attack::ring;
    let mut state = 0;
    let mut age = elapsed;
    let attacks = match kind {
        0x903 | 0x904 => vec![
            fan(0, 1, 0., 200, 10.).enabled(false),
            ring(0, 10, 50, 0.).fixed(0.).once().enabled(false),
        ],
        0x950 => vec![fan(0, 1, 0., 2000, 10.)],
        0x951 => vec![fan(0, 1, 0., 500, 10.)],
        0x955 => {
            let t = elapsed % 4600;
            let dart = t >= 3000;
            state = elapsed / 4600 * 2 + u64::from(dart);
            age = if dart { t - 3000 } else { t };
            if dart {
                vec![
                    ring(0, 5, 50, 0.)
                        .fixed(20f32.to_radians())
                        .delay(800)
                        .once(),
                    ring(0, 5, 50, 0.)
                        .fixed(56f32.to_radians())
                        .delay(1400)
                        .once(),
                ]
            } else {
                vec![fan(0, 1, 0., 500, 4.).chance(0.1)]
            }
        }
        0x957 => vec![fan(0, 1, 0., 1000, 10.)],
        0x958 | 0x959 | 0x960 | 0x961 => {
            state = elapsed / 1000;
            age = elapsed % 1000;
            let angle = match kind {
                0x958 => 0f32,
                0x959 => 90.,
                0x960 => 45.,
                0x961 => 135.,
                _ => unreachable!(),
            };
            // Descriptor indices 0–2 refer to selected projectile IDs 1–3 for these traps.
            vec![
                fan((state % 3 + 1) as u8, 1, 0., 100, 0.)
                    .fixed(angle.to_radians())
                    .delay(100),
            ]
        }
        0x952 => {
            let t = elapsed % 8100;
            let phase = if t < 5000 {
                0
            } else if t < 7100 {
                1
            } else {
                2
            };
            state = elapsed / 8100 * 3 + phase;
            age = t - [0, 5000, 7100][phase as usize];
            match phase {
                0 => vec![fan(0, 1, 0., 500, 10.).alternate(1, 2, 7., 0.)],
                1 => vec![ring(0, 20, 250, 0.).fixed(0.)],
                _ => vec![],
            }
        }
        0x202 => vec![fan(0, 1, 0., 1000, 9.)],
        0x204 => vec![fan(0, 1, 0., 1000, 10.)],
        0x20d => vec![ring(0, 12, 50, 0.).fixed(0.).once().enabled(false)],
        0x20e | 0x20f => vec![fan(0, 1, 0., 500, 10.)],
        0x635 => vec![fan(0, 3, 7., 1000, 10.)],
        0x636 | 0x639 => vec![fan(0, 1, 0., 1000, 10.)],
        0x637 => vec![
            fan(0, 3, 7., 2000, 10.).delay(1000),
            fan(1, 1, 0., 2000, 10.).delay(2000),
        ],
        0x6c4 => {
            let mut attacks = (0..4)
                .flat_map(|index| {
                    [1000, 2000, 3000, 4000, 5000]
                        .map(move |delay| fan(index, 1, 0., 6500, 10.).delay(delay))
                })
                .collect::<Vec<_>>();
            attacks.extend([5500, 6000, 6500].map(|delay| fan(4, 1, 0., 6500, 10.).delay(delay)));
            attacks
        }
        0x6c7 => vec![fan(0, 1, 0., 500, 10.)],
        0x6c8 | 0x206 => vec![],
        0x6c9 => vec![fan(0, 1, 0., 500, 8.)],
        0x6d1 => vec![fan(0, 1, 0., 1500, 10.).alternate(1, 6, 60., 150.)],
        0x207 | 0x208 => vec![fan(0, 1, 0., 1000, 8.)],
        0x209 => vec![fan(0, 3, 7., 1000, 5.)],
        0xda0 | 0xda1 => vec![fan(0, 1, 0., 1000, 3.)],
        0xda7..=0xda9 => vec![
            fan(0, 1, 0., 1000, 10.),
            Attack::grenade(2.5, 50 + i32::from(kind - 0xda7) * 20, 1000, 8.),
        ],
        0x620 | 0x622 | 0x6c5 | 0x7f22 => vec![fan(0, 1, 0., 500, 10.)],
        0x623 => {
            state = u64::from(hp < 200);
            vec![fan(
                state as u8,
                1,
                0.,
                if state == 1 { 250 } else { 500 },
                10.,
            )]
        }
        0x205 => vec![fan(0, 3, 15., 1000, 6.)],
        0x946..=0x949 => vec![fan(0, 1, 0., 1000, 10.)],
        0x945 => {
            let phase = elapsed % 2900;
            state = elapsed / 2900 * 2 + u64::from(phase >= 2000);
            age = if phase < 2000 { phase } else { phase - 2000 };
            vec![fan(
                0,
                if phase < 2000 { 3 } else { 1 },
                40.,
                if phase < 2000 { 500 } else { 800 },
                10.,
            )]
        }
        _ => panic!("unknown fauna attack {kind:#x}"),
    };
    Schedule {
        state,
        age,
        attacks,
    }
}
