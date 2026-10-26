use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Cache {
    statuses: BTreeMap<i32, BTreeMap<u8, Vec<u8>>>,
    pub elided: u64,
}
impl Cache {
    pub fn encode(&mut self, id: i32, status: Vec<u8>) -> Vec<u8> {
        assert!(status.len() >= 14, "truncated object status for {id}");
        let count = i16::from_be_bytes(status[12..14].try_into().unwrap());
        assert!(count >= 0, "negative stat count for {id}");
        let previous = self.statuses.entry(id).or_default();
        let mut seen = [false; 256];
        let mut out = status[..14].to_vec();
        let mut cursor = 14;
        let mut fields = Vec::new();
        for _ in 0..count {
            let key = *status.get(cursor).expect("truncated internal stat key");
            assert!(!seen[key as usize], "duplicate stat {key} for {id}");
            seen[key as usize] = true;
            cursor += 1;
            let length = if matches!(key, 31 | 62) {
                let prefix = status
                    .get(cursor..cursor + 2)
                    .expect("truncated stat string length");
                2 + usize::from(u16::from_be_bytes(prefix.try_into().unwrap()))
            } else {
                4
            };
            let value = status
                .get(cursor..cursor + length)
                .expect("truncated internal stat value");
            let changed = previous.get(&key).is_none_or(|old| old != value);
            if changed {
                previous.insert(key, value.to_vec());
            }
            fields.push((key, value, changed));
            cursor += length;
        }
        assert_eq!(cursor, status.len(), "trailing stat bytes for {id}");
        // Flash caches the star badge in its name bitmap; stat 56 invalidates it.
        let stars_changed = fields
            .iter()
            .any(|(key, _, changed)| *key == 30 && *changed);
        let mut changed = 0i16;
        for (key, value, different) in fields {
            if different || key == 56 && stars_changed {
                out.push(key);
                out.extend(value);
                changed += 1;
            }
        }
        previous.retain(|key, _| seen[*key as usize]);
        out[12..14].copy_from_slice(&changed.to_be_bytes());
        if changed == 0 {
            self.elided += 1;
        }
        out
    }
    pub fn retain(&mut self, known: &BTreeSet<i32>) {
        self.statuses.retain(|id, _| known.contains(id));
    }
}
