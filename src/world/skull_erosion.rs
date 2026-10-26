// GPL-3.0 reference; data/REALM-SRC-LICENSE: realm-src@1855c017eab870d13f0b61b1cf0b9e66a01922ff, wServer/realm/setpieces/Noise.cs.
pub(super) struct SkullErosion([u8; 256]);

impl SkullErosion {
    pub(super) fn new(seed: u64) -> Self {
        // Keep the world's single seed draw instead of importing wall-clock nondeterminism.
        let seed = seed as i32;
        let subtraction = if seed == i32::MIN {
            i32::MAX
        } else {
            seed.abs()
        };
        let mut previous = 161_803_398_i32 - subtraction;
        let mut current = 1_i32;
        let mut state = [0_i32; 56];
        state[55] = previous;
        for i in 1..55 {
            let index = 21 * i % 55;
            state[index] = current;
            current = previous.wrapping_sub(current);
            if current < 0 {
                current += i32::MAX;
            }
            previous = state[index];
        }
        for _ in 0..4 {
            for i in 1..56 {
                state[i] = state[i].wrapping_sub(state[1 + (i + 30) % 55]);
                if state[i] < 0 {
                    state[i] += i32::MAX;
                }
            }
        }
        let (mut next, mut next_pair) = (0, 21);
        // The source draws bytes with replacement; shuffling a permutation changes the field.
        Self(std::array::from_fn(|_| {
            next = if next == 55 { 1 } else { next + 1 };
            next_pair = if next_pair == 55 { 1 } else { next_pair + 1 };
            let mut value = state[next] - state[next_pair];
            if value == i32::MAX {
                value -= 1;
            }
            if value < 0 {
                value += i32::MAX;
            }
            state[next] = value;
            value as u8
        }))
    }

    pub(super) fn erodes(&self, x: usize, y: usize) -> bool {
        self.sample(
            f64::from(x as f32 / 33. * 8.),
            f64::from(y as f32 / 33. * 8.),
            0.5,
        ) < 0.2
    }

    fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        const GRADIENTS: [[i32; 3]; 12] = [
            [1, 1, 0],
            [-1, 1, 0],
            [1, -1, 0],
            [-1, -1, 0],
            [1, 0, 1],
            [-1, 0, 1],
            [1, 0, -1],
            [-1, 0, -1],
            [0, 1, 1],
            [0, -1, 1],
            [0, 1, -1],
            [0, -1, -1],
        ];
        let point = [x, y, z];
        let skew = (x + y + z) * (1. / 3.);
        let cell = point.map(|v| {
            if v + skew > 0. {
                (v + skew) as i32
            } else {
                (v + skew) as i32 - 1
            }
        });
        let unskew = f64::from(cell[0] + cell[1] + cell[2]) * (1. / 6.);
        let origin: [f64; 3] = std::array::from_fn(|i| point[i] - (f64::from(cell[i]) - unskew));
        let [x, y, z] = origin;
        let (first, second) = if x >= y {
            if y >= z {
                ([1, 0, 0], [1, 1, 0])
            } else if x >= z {
                ([1, 0, 0], [1, 0, 1])
            } else {
                ([0, 0, 1], [1, 0, 1])
            }
        } else if y < z {
            ([0, 0, 1], [0, 1, 1])
        } else if x < z {
            ([0, 1, 0], [0, 1, 1])
        } else {
            ([0, 1, 0], [1, 1, 0])
        };
        let wrapped = cell.map(|v| (v & 255) as usize);
        let mut contributions = [0.; 4];
        for (rank, offset) in [[0; 3], first, second, [1; 3]].into_iter().enumerate() {
            let delta: [f64; 3] =
                std::array::from_fn(|i| origin[i] - f64::from(offset[i]) + rank as f64 * (1. / 6.));
            let hash = usize::from(self.0[(wrapped[2] + offset[2] as usize) & 255]);
            let hash = usize::from(self.0[(wrapped[1] + offset[1] as usize + hash) & 255]);
            let gradient =
                GRADIENTS[usize::from(self.0[(wrapped[0] + offset[0] as usize + hash) & 255]) % 12];
            let [x, y, z] = delta;
            let mut attenuation = 0.6 - x * x - y * y - z * z;
            if attenuation >= 0. {
                attenuation *= attenuation;
                contributions[rank] = attenuation
                    * attenuation
                    * (f64::from(gradient[0]) * x
                        + f64::from(gradient[1]) * y
                        + f64::from(gradient[2]) * z);
            }
        }
        let [a, b, c, d] = contributions;
        f64::from((32. * (a + b + c + d) as f32 + 1.) * 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_simplex_field() {
        let c = f64::from(16_f32 / 33. * 8.);
        for (seed, x, y, z, bits) in [
            (0, 0., 0., 0.5, 0x3f48_022f),
            (1, c, c, 0.5, 0x3ed5_46a9),
            (
                42,
                f64::from(32_f32 / 33. * 8.),
                f64::from(7_f32 / 33. * 8.),
                0.5,
                0x3f38_c6c4,
            ),
            (0, 0., 0., 0., 0x3f00_0000),
            (0, -1., -1., -1., 0x3f00_0000),
            (0x8000_0000, c, c, 0.5, 0x3f2a_9e06),
        ] {
            assert_eq!(
                (SkullErosion::new(seed).sample(x, y, z) as f32).to_bits(),
                bits,
                "seed={seed}"
            );
        }
        let erosion = SkullErosion::new(0);
        let erased = (0..33)
            .map(|x| (0..33).filter(|&y| erosion.erodes(x, y)).count())
            .sum::<usize>();
        assert_eq!(erased, 113);
    }

    #[test]
    fn source_seeded_bytes_and_explicit_low_word_adapter() {
        let zero = SkullErosion::new(0);
        assert_eq!(
            &zero.0[..16],
            &[
                26, 12, 70, 111, 93, 117, 228, 216, 173, 103, 101, 213, 245, 25, 219, 200
            ]
        );
        assert_eq!(
            zero.0
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            158
        );
        assert_eq!(SkullErosion::new(1 << 32).0, zero.0);
        assert_eq!(
            SkullErosion::new(1).0,
            SkullErosion::new(u64::from(u32::MAX)).0
        );
        assert_eq!(
            SkullErosion::new(0x8000_0000).0,
            SkullErosion::new(i32::MAX as u64).0
        );
        assert_ne!(SkullErosion::new(i32::MAX as u64).0, zero.0);
    }
}
