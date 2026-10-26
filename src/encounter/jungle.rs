use super::{
    Anchor, Motion, Profile,
    attacks::{Attack, Schedule},
    nr_speed,
};
use crate::dungeon::jungle::*;

pub fn profile(kind: u16) -> Profile {
    let (motion, rate, interval, range) = match kind {
        EGG | 0xdd8 => (Motion::Stationary, 0., 0, 0.),
        COIL => (Motion::WanderHome { radius: 5. }, 0.4, 1000, 10.),
        SNAKE => (Motion::Wander, 0.4, 1000, 4.),
        BOSS => (Motion::Wander, 0.3, 200, 20.),
        BOSS_TOTEM | TOTEM | FIRE => (Motion::Stationary, 0., 400, 10.),
        SPIRIT => (Motion::WanderHome { radius: 3. }, 0.4, 750, 8.),
        BABY => (
            Motion::Maintain {
                sight: 7.,
                distance: 4.,
            },
            0.4,
            750,
            7.,
        ),
        BASILISK => (
            Motion::Follow {
                sight: 6.,
                stop: 1.,
                idle: nr_speed(0.5),
                duration: 0,
                cooldown: 0,
            },
            0.6,
            750,
            6.,
        ),
        SHAMAN => (Motion::Wander, 0.4, 1000, 8.),
        WARRIOR | HUNTER => (
            Motion::Follow {
                sight: 8.,
                stop: 1.,
                idle: nr_speed(0.4),
                duration: if kind == WARRIOR { 1200 } else { 0 },
                cooldown: if kind == WARRIOR { 1000 } else { 0 },
            },
            if kind == HUNTER { 0.8 } else { 0.4 },
            1000,
            8.,
        ),
        _ => panic!("unregistered Jungle actor {kind:#x}"),
    };
    Profile {
        motion,
        speed: if rate == 0. { 0. } else { nr_speed(rate) },
        interval,
        range,
    }
}

pub fn movement(kind: u16, stage: u64, has_parent: bool) -> (Motion, f32) {
    match kind {
        COIL if stage == 1 => (
            Motion::Protect {
                anchor: Anchor::Home,
                distance: 0.2,
            },
            3.,
        ),
        BOSS if matches!(stage, 1 | 4) => (
            Motion::Protect {
                anchor: Anchor::Home,
                distance: 0.1,
            },
            nr_speed(1.),
        ),
        BOSS if stage == 5 => (Motion::Stationary, 0.),
        BOSS if stage == 6 => (
            Motion::Follow {
                sight: 20.,
                stop: 0.5,
                idle: 0.,
                duration: 0,
                cooldown: 0,
            },
            nr_speed(0.4),
        ),
        SHAMAN | WARRIOR | HUNTER if has_parent => (
            Motion::Orbit {
                anchor: Anchor::Parent,
                radius: 3.,
            },
            nr_speed(0.4),
        ),
        BABY if has_parent => (
            Motion::Protect {
                anchor: Anchor::Parent,
                distance: 4.,
            },
            nr_speed(0.4),
        ),
        _ => {
            let p = profile(kind);
            (p.motion, p.speed)
        }
    }
}

pub fn schedule(kind: u16, stage: u64, age: u64) -> Schedule {
    let fan = Attack::fan;
    let ring = Attack::ring;
    let spiral =
        || fan(1, 3, 10., 200, 20.).fixed((-90. + 45. * (age / 200 % 8) as f32).to_radians());
    let attacks = match kind {
        EGG | 0xdd8 => vec![],
        COIL => vec![
            fan(0, 1, 0., 4000, 10.).delay(1000).chance(1.),
            ring(1, 10, 4000, 10.).delay(2000).fixed(0.),
        ],
        SNAKE => vec![
            fan(0, 2, 7., 2000, 4.).delay(1000),
            ring(1, 6, 2000, 4.).delay(2000).fixed(0.),
        ],
        BOSS => match stage {
            2 => vec![
                fan(2, 1, 0., 1250, 10.),
                fan(2, 1, 0., 1250, 10.).delay(250),
            ],
            3 => vec![spiral()],
            5 => vec![ring(0, 8, 400, 20.).fixed(0.)],
            6 => vec![
                ring(0, 8, 800, 20.).fixed(0.),
                spiral().enabled(age % 800 >= 400),
            ],
            _ => vec![],
        },
        BOSS_TOTEM => match stage {
            5 => vec![ring(0, 6, 400, 20.).fixed(0.)],
            6 => vec![ring(0, 4, 800, 20.).fixed(0.)],
            _ => vec![],
        },
        TOTEM => vec![fan(0, 9, 22.5, 1500, 9.)],
        SPIRIT | BABY => vec![fan(0, 1, 0., 750, 7.)],
        BASILISK => (0..4).map(|i| fan(i, 1, 0., 750, 6.)).collect(),
        SHAMAN | WARRIOR => vec![ring(0, 8, 1500, 8.).fixed(0.)],
        HUNTER => vec![fan(0, 1, 0., 1000, 8.)],
        FIRE => vec![
            fan(0, 1, 0., 1000, 1.),
            fan(1, 1, 0., 1000, 7.).delay(500),
            fan(1, 1, 0., 1000, 7.).delay(600),
            fan(1, 1, 0., 1000, 7.).delay(700),
            fan(1, 1, 0., 1000, 7.).delay(800),
        ],
        _ => panic!("unregistered Jungle attack {kind:#x}"),
    };
    Schedule {
        state: if kind == COIL { 0 } else { stage },
        age,
        attacks,
    }
}
