use super::{Pos, Writer};

#[derive(Clone, Copy)]
pub(super) struct EnemyShot {
    pub bullet: u8,
    pub projectile: u8,
    pub start: Pos,
    pub angle: f32,
    pub damage: i32,
}

pub(super) fn dazed(shots: Vec<(u8, f32)>) -> Vec<(u8, f32)> {
    if shots.len() <= 1 {
        return shots;
    }
    let count = (shots.len() / 2).max(1);
    (0..count)
        .map(|i| shots[(2 * i + 1) * shots.len() / (2 * count)])
        .collect()
}

pub(super) fn packets(owner: i32, shots: &[EnemyShot]) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut offset = 0;
    while offset < shots.len() {
        let first = shots[offset];
        let mut count = 1;
        let mut gap = 0.;
        while count < usize::from(u8::MAX) {
            let Some(next) = shots.get(offset + count) else {
                break;
            };
            if next.bullet != first.bullet.wrapping_add(count as u8)
                || next.projectile != first.projectile
                || next.damage != first.damage
                || next.start.x != first.start.x
                || next.start.y != first.start.y
            {
                break;
            }
            if count == 1 {
                gap = next.angle - first.angle;
            } else {
                // Native count/gap reconstruction rounds complete rings in f32.
                let tolerance = 8. * f32::EPSILON;
                if (next.angle - (first.angle + gap * count as f32)).abs() > tolerance {
                    break;
                }
            }
            count += 1;
        }
        let mut out = Writer::new();
        out.u8(first.bullet)
            .i32(owner)
            .u8(first.projectile)
            .f32(first.start.x)
            .f32(first.start.y)
            .f32(first.angle)
            .i16(first.damage as i16);
        if count > 1 {
            out.u8(count as u8).f32(gap);
        }
        packets.push(out.0);
        offset += count;
    }
    packets
}
