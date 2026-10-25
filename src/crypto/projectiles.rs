use anyhow::{Result, ensure};

const FILTER: &[u8] = b"\x09\x62\x0a\x62\x09\x23\x80\x35\x2a\x63\x05\x46\x9b\x1f\x00\x12\x0a\x00\x00\x62\x0c\x12\x56\x02\x00\x10\xb4\x02\x00\x62\x05\x46\xd1\x1f\x00\x12\x04\x00\x00\x10\xa6\x02\x00\xd0\x66\xa1\x2b\x76\x62\x0b\x11\xea\x01\x00\x2a\x62\x0b\x2a\x12\x03\x00\x00\x29\xd2\x76\x11\x3e\x00\x00\x12\x2b\x00\x00\x62\x0b\x2a\x12\x03\x00\x00\x29\xd3\x76\x11\x57\x00\x00\x29\x62\x05\x66\x6e\x62\x0b\x11\x3b\x00\x00\x66\xc3\x0c\x62\x0c\x2a\x11\x03\x00\x00\x29\xd0\x76\x12\x69\x00\x00\x76\x2a\x62\x0b\x2a\x12\x03\x00\x00\x29\xd3\x76\x11\x55\x00\x00\x11\x2e\x00\x00\x29\xd0\x66\xc4\x13\x76\x2a\x62\x0b\x11\x44\x00\x00\x12\x1d\x00\x00\x29\x62\x05\x66\x6e\x66\xbf\x36\x62\x0c\x2a\x11\x03\x00\x00\x29\xd0\x76\x12\x86\x01\x00\x76\x62\x0c\x12\x40\x01\x00\x11\x04\x00\x00\x10\x14\x02\x00\x62\x05\x66\xca\x0a\x62\x0b\x11\x6c\x01\x00\x76\x2a\x62\x0c\x2a\x11\x03\x00\x00\x29\xd1\x76\x12\x58\x01\x00\x11\x0e\x00\x00\x29\x62\x0c\x12\x9d\x01\x00\x62\x05\x46\xcd\x04\x00\x76\x12\x04\x00\x00\x10\xdf\x01\x00";
const PREFILTER: &[u8] = b"\x09\x62\x0a\x62\x09\x23\x80\x35\x63\x05\xd0\x66\xa1\x2b\x12\x0b\x00\x00\x62\x05\x66\x6e\x66\xc3\x0c\x11\x13\x00\x00\xd0\x66\xc4\x13\x12\xac\x02\x00\x62\x05\x66\x6e\x66\xbf\x36\x12\xa1\x02\x00\x62\x05\x46\x9b\x1f\x00\x11\x97\x02\x00\x62\x05\x46\xd1\x1f\x00\x11\x8d\x02\x00\x62\x05\x66\xca\x0a\x11\x84\x02\x00\x62\x05\x46\xcd\x04\x00\x11\x7a\x02\x00\x10\x97\x00\x00";

pub(super) fn patch(data: &mut [u8]) -> Result<usize> {
    let positions: Vec<_> = data
        .windows(FILTER.len())
        .enumerate()
        .filter_map(|(i, bytes)| (bytes == FILTER).then_some(i))
        .collect();
    ensure!(positions.len() <= 1, "ambiguous projectile target filter");
    if let Some(&offset) = positions.first() {
        // Reject impossible types before pure condition calls; keep target order/hit rules: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/com/company/assembleegameclient/objects/Projectile.as#L326-L413
        let filter = &mut data[offset..offset + FILTER.len()];
        filter.fill(0x02);
        filter[..PREFILTER.len()].copy_from_slice(PREFILTER);
    }
    Ok(positions.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projectile_filter_is_exact_unique_and_preserves_layout() {
        let mut source = [b"before".as_slice(), FILTER, b"after"].concat();
        let original = source.clone();
        assert_eq!(patch(&mut source).unwrap(), 1);
        assert_eq!(source.len(), original.len());
        assert_eq!(&source[..6], b"before");
        assert_eq!(&source[source.len() - 5..], b"after");
        assert_eq!(&source[6..6 + PREFILTER.len()], PREFILTER);
        assert!(
            source[6 + PREFILTER.len()..source.len() - 5]
                .iter()
                .all(|b| *b == 2)
        );
        assert_eq!(patch(&mut source).unwrap(), 0);
        let mut ambiguous = FILTER.repeat(2);
        assert!(patch(&mut ambiguous).is_err());
        assert_eq!(ambiguous, FILTER.repeat(2));
        for i in 0..FILTER.len() {
            let mut corrupt = FILTER.to_vec();
            corrupt[i] ^= 1;
            let before = corrupt.clone();
            assert_eq!(patch(&mut corrupt).unwrap(), 0);
            assert_eq!(corrupt, before);
        }
    }

    #[test]
    fn projectile_filter_bytecode_preserves_eligibility_without_irrelevant_calls() {
        #[derive(Debug, PartialEq)]
        enum Value {
            This,
            Target,
            Props,
            Bool(bool),
        }
        use Value::*;
        for bits in 0..256u32 {
            let flag = |i: u32| bits & (1u32 << i) != 0u32;
            let (mut pc, mut calls, mut stack) = (10, 0, Vec::new());
            while pc < PREFILTER.len() {
                let opcode = PREFILTER[pc];
                pc += 1;
                match opcode {
                    0xd0 => stack.push(This),
                    0x62 => {
                        assert_eq!(PREFILTER[pc], 5);
                        pc += 1;
                        stack.push(Target);
                    }
                    0x66 | 0x46 => {
                        let name = super::super::u30(PREFILTER, &mut pc).unwrap();
                        let receiver = stack.pop().unwrap();
                        let value = match (receiver, name) {
                            (This, 5537) => Bool(flag(0)),
                            (This, 2500) => Bool(flag(1)),
                            (Target, 110) => Props,
                            (Props, 1603) => Bool(flag(2)),
                            (Props, 6975) => Bool(flag(3)),
                            (Target, 1354) => Bool(flag(6)),
                            (Target, 3995) => {
                                calls += 1;
                                Bool(flag(4))
                            }
                            (Target, 4049) => {
                                calls += 1;
                                Bool(flag(5))
                            }
                            (Target, 589) => {
                                calls += 1;
                                Bool(flag(7))
                            }
                            other => panic!("unexpected bytecode property: {other:?}"),
                        };
                        if opcode == 0x46 {
                            assert_eq!(PREFILTER[pc], 0);
                            pc += 1;
                        }
                        stack.push(value);
                    }
                    0x10..=0x12 => {
                        let offset = i32::from_le_bytes([
                            PREFILTER[pc],
                            PREFILTER[pc + 1],
                            PREFILTER[pc + 2],
                            0,
                        ]) << 8
                            >> 8;
                        pc += 3;
                        let take = if opcode == 0x10 {
                            true
                        } else {
                            let Bool(value) = stack.pop().unwrap() else {
                                panic!("nonboolean branch")
                            };
                            value == (opcode == 0x11)
                        };
                        if take {
                            pc = (pc as i32 + offset) as usize;
                        }
                    }
                    other => panic!("unexpected bytecode opcode: {other:#x}"),
                }
            }
            let eligible = flag(0) && flag(2) || flag(1) && flag(3);
            let accepted = eligible && !(flag(4) || flag(5) || flag(6) || flag(7));
            assert_eq!(pc, if accepted { FILTER.len() } else { 0x313 - 0x42 });
            assert!(stack.is_empty());
            if !eligible {
                assert_eq!(calls, 0);
            }
        }
    }
}
