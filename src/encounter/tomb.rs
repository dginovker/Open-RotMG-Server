use super::attacks::{Attack, Schedule};
use crate::dungeon::tomb::{BES, GEB, NUT};
use anyhow::{Result, bail};

pub const SHIELD_MS: u64 = 5000;
pub const SPECIAL_AFTER_MS: u64 = 20000;
pub const SPECIAL_MS: u64 = 10000;
pub const SPECIAL_ARMOR_BREAK_MS: u64 = 3000;
pub const RAGE: u64 = 7;
pub const SPECIAL_TWO: u64 = 8;
pub const SPECIAL_THREE: u64 = 9;

pub fn thresholds(kind: u16) -> Result<[i32; 7]> {
    Ok(match kind {
        BES => [98900, 97900, 70000, 55000, 40000, 25000, 6000],
        NUT => [79000, 78300, 72000, 56000, 40000, 24000, 4800],
        GEB => [89000, 88100, 64800, 54000, 36000, 18000, 5400],
        _ => bail!("not a Tomb boss: {kind:#x}"),
    })
}

pub fn phase_for_hp(kind: u16, hp: i32) -> Result<u64> {
    Ok(thresholds(kind)?
        .iter()
        .take_while(|limit| hp <= **limit)
        .count() as u64)
}

pub fn schedule(kind: u16, phase: u64, age: u64, guarding: bool) -> Schedule {
    let fan = Attack::fan;
    let ring =
        |index, count, cooldown, range| Attack::ring(index, count, cooldown, range).fixed(0.);
    let grenade = Attack::grenade;
    let special = matches!(phase, SPECIAL_TWO | SPECIAL_THREE);
    let mut attacks = if guarding && phase == 0 {
        match kind {
            NUT => vec![ring(0, 6, 2000, 20.)],
            GEB => vec![fan(2, 2, 10., 500, 20.)],
            BES => vec![ring(2, 8, 1000, 4.)],
            _ => vec![],
        }
    } else if matches!(kind, BES | GEB | NUT) && phase == 1 {
        vec![ring(if kind == NUT { 7 } else { 3 }, 20, 6000, 60.).delay(2000)]
    } else if kind == BES && phase >= 2 {
        if special {
            vec![
                fan(4, 5, 10., 500, 12.),
                fan(4, 5, 10., 750, 12.).offset(180.),
            ]
        } else if phase == RAGE {
            vec![
                fan(4, 5, 10., 500, 12.),
                fan(4, 5, 10., 750, 12.).offset(180.),
                fan(1, 5, 15., 2000, 30.),
                fan(1, 2, 90., 2000, 30.),
                ring(0, 6, 3000, 12.),
                fan(0, 1, 0., 1000, 12.),
            ]
        } else {
            let boomerangs = match phase {
                2 => 3,
                3 => 7,
                4 => 6,
                _ => 9,
            };
            let bolts = match phase {
                3 => 1,
                4 | 5 => 2,
                6 => 3,
                _ => 0,
            };
            let mut a = vec![
                ring(2, if phase >= 5 { 10 } else { 8 }, 1000, 4.),
                if phase == 2 {
                    ring(1, boomerangs, 5000, 30.)
                } else {
                    fan(1, boomerangs, 15., 3000, 30.)
                },
            ];
            if phase <= 3 {
                a.push(ring(0, 6, 3000, 12.));
            }
            if bolts > 0 {
                a.push(fan(0, bolts, 10., 3000, 12.));
            }
            a
        }
    } else if kind == NUT && phase >= 2 {
        if special {
            vec![
                fan(8, 6, 10., 500, 20.),
                fan(8, 6, 10., 500, 20.).offset(180.),
            ]
        } else {
            let rage = phase == RAGE;
            let mut a = vec![fan(5, if phase >= 6 { 2 } else { 1 }, 15., 1000, 20.)];
            if phase >= 3 {
                a.push(fan(6, if phase >= 6 { 2 } else { 1 }, 15., 3000, 20.));
            }
            for (index, count, delay) in if rage {
                [(1, 4, 1000), (2, 4, 1500), (3, 6, 2000), (4, 8, 2500)]
            } else {
                [(1, 3, 1000), (2, 4, 1500), (3, 5, 2000), (4, 6, 2500)]
            } {
                a.push(ring(index, count, 2500, 14.).delay(delay));
            }
            if rage {
                a.push(fan(8, 6, 10., 500, 20.).offset(180.));
                a.push(fan(0, 1, 0., 750, 20.));
            }
            a
        }
    } else if kind == GEB && phase >= 2 {
        if special {
            (0..4)
                .map(|n| fan(4, 1, 0., 250, 20.).fixed((n as f32 * 90.).to_radians()))
                .collect()
        } else {
            let mut a = vec![fan(2, 2, 10., 500, 20.)];
            if phase >= 3 {
                a.push(fan(2, 5, 90., 1000, 20.));
            }
            if phase == 2 {
                a.push(grenade(3., 160, 3000, 10.));
                a.push(grenade(4., 120, 3000, 10.).delay(1500));
            } else {
                a.push(ring(1, if phase == 3 { 8 } else { 10 }, 5000, 12.));
                a.push(fan(
                    0,
                    if phase == 3 {
                        2
                    } else if phase == 4 {
                        3
                    } else {
                        4
                    },
                    15.,
                    2000,
                    16.,
                ));
                if phase < RAGE {
                    a.push(fan(4, 1, 0., 500, 20.));
                    if phase >= 4 {
                        a.push(fan(4, 1, 0., 500, 20.).offset(90.));
                    }
                }
                if (5..RAGE).contains(&phase) {
                    a.push(grenade(5., 45, 4500, 10.));
                    a.push(grenade(4., 100, 4500, 10.).delay(1500));
                    a.push(grenade(3., 120, 4500, 10.).delay(3000));
                } else {
                    a.push(grenade(
                        3.,
                        if phase == RAGE { 150 } else { 120 },
                        3000,
                        10.,
                    ));
                    a.push(
                        grenade(4., if phase == RAGE { 120 } else { 100 }, 3000, 10.).delay(1500),
                    );
                }
                if phase == RAGE {
                    a.push(fan(5, 2, 10., 500, 20.));
                }
            }
            a
        }
    } else {
        match kind {
            0xd10 | 0xd12 | 0xd14 => vec![fan(0, 1, 0., 1400, 3.)],
            0xd11 | 0xd13 | 0xd15 => (0..4)
                .map(|index| fan(index, 1, 0., [1400, 1700, 2000, 2100][index as usize], 18.))
                .collect(),
            0xd16..=0xd18 => vec![fan(0, 1, 0., 1400, 8.)],
            0xd19 => vec![fan(0, 3, 10., 1400, 8.)],
            0xd1a if phase == 1 => vec![ring(1, 36, 50, 20.).once()],
            0xd1a => vec![ring(0, 25, 1860, 14.), fan(1, 4, 30., 1100, 14.)],
            0xd1b => vec![fan(0, 1, 0., 300, 1.4), fan(1, 1, 0., 300, 1.4)],
            0xd1c..=0xd1e => vec![ring(0, 3, 2500, 12.), ring(1, 3, 5000, 12.)],
            0xd1f..=0xd24 => vec![ring(0, 3, 2500, 12.)],
            0xd29 if phase == 1 => vec![
                ring(0, 8, 1, 24.).delay(500).once(),
                ring(0, 8, 1, 24.).delay(1000).once(),
                ring(0, 8, 1, 24.).delay(1500).once(),
            ],
            0xd2a | 0xd2b if matches!(phase, 1 | 2 | 4) => vec![],
            0xd2a | 0xd2b if phase == 5 => vec![ring(0, 9, 50, 20.).once()],
            0xd2a | 0xd2b => vec![
                fan(0, 1, 0., if phase == 3 { 700 } else { 1100 }, 14.),
                fan(1, 1, 0., if phase == 3 { 1400 } else { 2000 }, 14.),
            ],
            0xd31 => vec![fan(0, 1, 0., 300, 5.)],
            0xd32 => vec![ring(0, 2, 300, 20.).fixed((age / 300) as f32 * 15f32.to_radians())],
            0xd33 => vec![fan(0, 1, 0., 300, 11.2)],
            0xd34 => vec![ring(0, 3, 300, 6.4)],
            0xd35 => vec![ring(0, 4, 300, 20.).fixed((age / 300) as f32 * 15f32.to_radians())],
            0xd36 => vec![ring(0, 5, 300, 7.2)],
            0xd3a => vec![
                fan(0, 1, 0., 960, 6.3),
                fan(1, 1, 0., 1160, 6.3),
                fan(2, 1, 0., 1360, 6.3),
            ],
            0xd3b => vec![fan(0, 1, 0., 1000, 18.), fan(1, 1, 0., 1200, 18.)],
            0xd3c => vec![
                fan(0, 3, 12., 1000, 8.4),
                fan(1, 3, 12., 1500, 8.4),
                fan(2, 1, 0., 1700, 12.),
                ring(3, 5, 2000, 15.),
            ],
            _ => vec![],
        }
    };
    if matches!(kind, BES | GEB | NUT) && phase == 0 && !guarding {
        attacks.clear();
    }
    Schedule {
        state: phase + if guarding { 100 } else { 0 },
        age,
        attacks,
    }
}
