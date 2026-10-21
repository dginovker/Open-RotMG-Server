use super::*;
use crate::{content::Grounds, realm_map};

fn setup() -> (Content, Room) {
    let path = std::env::var_os("BUILD100_CLIENT_DIR").expect("qualified content required");
    let path = std::path::Path::new(&path);
    let content = Content::load(path).unwrap();
    let map = realm_map::realm(100, &content, Arc::new(Grounds::load(path).unwrap())).unwrap();
    let mut room = Room::from_map(Arc::new(map), &content, 1_000_000).unwrap();
    let mut character = Character::new(&content, 0x30e, "Dropper".into(), 1).unwrap();
    (character.x, character.y) = room.landing();
    room.attach(1, character, &content).unwrap();
    (content, room)
}
fn drop_item(owner: i32, slot: u8, kind: i16) -> Packet {
    packet(19, |w| {
        w.i32(owner).u8(slot).i16(kind);
    })
}

#[test]
fn inventory_drop_ordinary_transfer_at_authoritative_position() {
    let (content, mut room) = setup();
    let before = room.character(1).clone();
    assert_eq!(before.stats[&12], 0xa22);
    let next = room.next_id;
    let count = item_count(&room, 0xa22);
    let events = room.handle(1, drop_item(1, 4, 0xa22), &content).unwrap();
    let bag = &room.entities[&(next + 1)];
    assert_eq!(bag.kind, 0x500);
    assert_eq!((bag.x, bag.y, bag.age), (before.x, before.y, 0.));
    assert_eq!(bag.stats[&8], 0xa22);
    assert!((9..16).all(|slot| bag.stats[&slot] == -1));
    assert!(matches!(bag.provenance, Provenance::Loot { source: 1 }));
    let mut expected = before;
    expected.stats.insert(12, -1);
    assert_eq!(room.character(1), &expected);
    assert_eq!(item_count(&room, 0xa22), count);
    assert_eq!(events.iter().map(|p| p.id).collect::<Vec<_>>(), [10, 13]);
}

fn item_count(room: &Room, kind: i32) -> usize {
    room.players
        .values()
        .map(|p| (8..20).filter(|s| p.character.stats[s] == kind).count())
        .sum::<usize>()
        + room
            .entities
            .values()
            .filter(|e| matches!(e.kind, 0x500 | 0x806))
            .map(|e| (8..16).filter(|s| e.stats[s] == kind).count())
            .sum::<usize>()
}
fn unchanged(before: &Room, after: &Room) {
    use rand::RngCore;
    assert_eq!(before.next_id, after.next_id);
    assert_eq!(before.rng.clone().next_u64(), after.rng.clone().next_u64());
    assert_eq!(before.players.len(), after.players.len());
    for (&id, avatar) in &before.players {
        assert_eq!(avatar.character, after.players[&id].character);
    }
    assert_eq!(before.entities.len(), after.entities.len());
    for (&id, entity) in &before.entities {
        let got = &after.entities[&id];
        assert_eq!(
            (
                entity.id,
                entity.kind,
                entity.x,
                entity.y,
                entity.age,
                entity.provenance,
                &entity.stats
            ),
            (
                got.id,
                got.kind,
                got.x,
                got.y,
                got.age,
                got.provenance,
                &got.stats
            )
        );
    }
}
fn swap(a: (i32, u8, i16), b: (i32, u8, i16)) -> Packet {
    packet(14, |w| {
        w.i32(a.0).u8(a.1).i16(a.2).i32(b.0).u8(b.1).i16(b.2);
    })
}
fn bags(packets: &[Packet]) -> Vec<i32> {
    packets
        .iter()
        .filter(|p| p.id == 10)
        .filter_map(|p| {
            let mut r = Reader::new(&p.body);
            let kind = r.i16().unwrap();
            r.utf().unwrap();
            let id = r.i32().unwrap();
            (kind == 0x500).then_some(id)
        })
        .collect()
}

#[test]
fn inventory_drop_strict_body_owner_slot_type_rejections_are_atomic() {
    let (content, mut room) = setup();
    room.attach(2, room.character(1).clone(), &content).unwrap();
    let valid = drop_item(1, 4, 0xa22).body;
    assert_eq!(valid.len(), 7);
    let mut bodies: Vec<_> = (0..7).map(|n| valid[..n].to_vec()).collect();
    let mut suffix = valid.clone();
    suffix.push(0);
    bodies.push(suffix);
    let static_id = *room.entities.keys().next().unwrap();
    for owner in [2, -1, i32::MIN, i32::MAX, static_id, 9_999_999] {
        bodies.push(drop_item(owner, 4, 0xa22).body);
    }
    for (slot, kind) in [
        (12, 0xa22),
        (255, 0xa22),
        (5, -1),
        (5, 0xa22),
        (4, -1),
        (4, -2),
        (4, 0xa23),
    ] {
        bodies.push(drop_item(1, slot, kind).body);
    }
    for body in bodies {
        let before = room.clone();
        assert!(
            room.handle(
                1,
                Packet {
                    id: 19,
                    body: body.clone()
                },
                &content
            )
            .is_err(),
            "accepted {body:?}"
        );
        unchanged(&before, &room);
    }
    let events = room.handle(1, drop_item(1, 4, 0xa22), &content).unwrap();
    let bag = bags(&events)[0];
    let before = room.clone();
    for p in [drop_item(1, 4, 0xa22), drop_item(bag, 0, 0xa22)] {
        assert!(room.handle(1, p, &content).is_err());
        unchanged(&before, &room);
    }
}

#[test]
fn inventory_drop_signed_attached_avatar_ids_and_identical_occurrences() {
    let (content, mut room) = setup();
    let mut character = room.character(1).clone();
    character.stats.insert(13, 0xa22);
    room.detach(1);
    let owner = i32::MIN + 7;
    room.attach(owner, character, &content).unwrap();
    let first = bags(
        &room
            .handle(owner, drop_item(owner, 4, 0xa22), &content)
            .unwrap(),
    )[0];
    assert_eq!(room.character(owner).stats[&13], 0xa22);
    let second = bags(
        &room
            .handle(owner, drop_item(owner, 5, 0xa22), &content)
            .unwrap(),
    )[0];
    assert_ne!(first, second);
    assert_eq!(room.entities[&first].stats[&8], 0xa22);
    assert_eq!(room.entities[&second].stats[&8], 0xa22);
    assert!((12..14).all(|slot| room.character(owner).stats[&slot] == -1));
    room.detach(owner);
    let before = room.clone();
    assert!(
        room.handle(owner, drop_item(owner, 4, 0xa22), &content)
            .is_err()
    );
    unchanged(&before, &room);
}

#[test]
fn inventory_drop_bad_definition_item_and_id_capacity_do_not_consume() {
    let (content, room) = setup();
    let bag = content.named("Loot Bag").unwrap().id;
    for bad in 0..3 {
        let mut c = Content {
            objects: content.objects.clone(),
        };
        let def = c.objects.get_mut(&bag).unwrap();
        match bad {
            0 => {
                def.slots.pop();
            }
            1 => def.class = "Character".into(),
            _ => def.slots[0] = 1,
        }
        let mut candidate = room.clone();
        assert!(candidate.handle(1, drop_item(1, 4, 0xa22), &c).is_err());
        unchanged(&room, &candidate);
    }
    for limit in [room.next_id + 1, i32::MAX] {
        let mut candidate = room.clone();
        candidate.id_limit = limit;
        if limit == i32::MAX {
            candidate.next_id = i32::MAX;
        }
        let before = candidate.clone();
        assert!(
            candidate
                .handle(1, drop_item(1, 4, 0xa22), &content)
                .is_err()
        );
        unchanged(&before, &candidate);
    }
    for bad in 0..3 {
        let mut c = Content {
            objects: content.objects.clone(),
        };
        let item = c.objects.get_mut(&0xa22).unwrap();
        match bad {
            0 => item.class = "Container".into(),
            1 => item.slot_type = None,
            _ => item.slot_type = Some(99),
        }
        let mut candidate = room.clone();
        assert!(candidate.handle(1, drop_item(1, 4, 0xa22), &c).is_err());
        unchanged(&room, &candidate);
    }
}

#[test]
fn inventory_drop_equipment_rebases_maxima_preserves_other_carry_and_permanent_bases() {
    for (slot, item, hp, mp) in [(3, 0xa27, 139, 50), (2, 0xa5f, 50, 139), (4, 0xa22, 50, 50)] {
        let (content, mut room) = setup();
        super::shore_runtime_tests::isolate_population(&mut room);
        room.character_mut(1)
            .stats
            .extend([(8 + slot, item), (1, hp), (4, mp)]);
        room.advance(0.1, &content).unwrap();
        let before = room.character(1).clone();
        let old = before.resources_carry.unwrap();
        assert!(old.hp_remainder > 0. && old.mp_remainder > 0.);
        let events = room
            .handle(1, drop_item(1, slot, item as i16), &content)
            .unwrap();
        let bag = bags(&events)[0];
        let mut expected = before.clone();
        expected.stats.insert(8 + slot, -1);
        let carry = expected.resources_carry.as_mut().unwrap();
        if slot == 3 {
            expected.stats.insert(1, 100);
            carry.hp_remainder = 0.;
            carry.hp_roundoff = 0.;
        }
        if slot == 2 {
            expected.stats.insert(4, 100);
            carry.mp_remainder = 0.;
            carry.mp_roundoff = 0.;
        }
        assert_eq!(room.character(1), &expected);
        room.handle(1, swap((bag, 0, item as i16), (1, slot, -1)), &content)
            .unwrap();
        assert_eq!(room.character(1).stats[&1], expected.stats[&1]);
        assert_eq!(room.character(1).stats[&4], expected.stats[&4]);
        let saved = room.character(1).clone();
        room.detach(1);
        room.attach(
            1,
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap(),
            &content,
        )
        .unwrap();
        assert_eq!(room.character(1), &saved);
        assert_eq!(
            room.character(1).equipment_stats(&content).unwrap(),
            before.equipment_stats(&content).unwrap()
        );
    }
}

#[test]
fn inventory_drop_reuses_visibility_pickup_range_empty_retirement_and_expiry() {
    let (content, mut room) = setup();
    super::shore_runtime_tests::isolate_population(&mut room);
    room.attach(2, room.character(1).clone(), &content).unwrap();
    for viewer in [1, 2] {
        room.publication(viewer, &[], &content).unwrap();
    }
    let events = room.handle(1, drop_item(1, 4, 0xa22), &content).unwrap();
    let bag = bags(&events)[0];
    room.attach(3, room.character(1).clone(), &content).unwrap();
    assert_eq!(bags(&room.publication(3, &[], &content).unwrap()), [bag]);
    assert!(bags(&room.publication(3, &[], &content).unwrap()).is_empty());
    for viewer in [1, 2] {
        let out = room.publication(viewer, &events, &content).unwrap();
        assert_eq!(bags(&out), [bag]);
        let make = out.iter().position(|p| p.id == 10).unwrap();
        assert!(make < out.iter().position(|p| p.id == 13).unwrap());
        assert!(bags(&room.publication(viewer, &events, &content).unwrap()).is_empty());
    }
    let (x, y) = (room.entities[&bag].x, room.entities[&bag].y);
    room.character_mut(2).x = x + 17.;
    assert!(bags(&room.publication(2, &events, &content).unwrap()).is_empty());
    room.character_mut(2).x = x + 2.;
    assert!(
        room.handle(2, swap((bag, 0, 0xa22), (2, 5, -1)), &content)
            .is_err()
    );
    room.character_mut(2).x = x + 1.5;
    room.character_mut(2).y = y;
    assert_eq!(bags(&room.publication(2, &[], &content).unwrap()), [bag]);
    let mut expiry = room.clone();
    expiry.entities.get_mut(&bag).unwrap().age = 60.;
    expiry.advance(0., &content).unwrap();
    assert!(expiry.entities.contains_key(&bag));
    expiry.advance(0.01, &content).unwrap();
    assert!(!expiry.entities.contains_key(&bag));
    room.handle(2, swap((bag, 0, 0xa22), (2, 5, -1)), &content)
        .unwrap();
    assert_eq!(room.character(2).stats[&13], 0xa22);
    room.advance(0., &content).unwrap();
    assert!(!room.entities.contains_key(&bag));
}

#[test]
fn inventory_drop_same_type_reoccupied_slot_has_no_invented_request_nonce() {
    let (content, mut room) = setup();
    room.character_mut(1).stats.insert(13, 0xa22);
    let count = item_count(&room, 0xa22);
    room.handle(1, drop_item(1, 4, 0xa22), &content).unwrap();
    assert!(room.handle(1, drop_item(1, 4, 0xa22), &content).is_err());
    room.handle(1, swap((1, 5, 0xa22), (1, 4, -1)), &content)
        .unwrap();
    room.handle(1, drop_item(1, 4, 0xa22), &content).unwrap();
    assert_eq!(item_count(&room, 0xa22), count);
    assert_eq!(
        room.entities.values().filter(|e| e.kind == 0x500).count(),
        2
    );
    assert_eq!(
        (room.character(1).stats[&12], room.character(1).stats[&13]),
        (-1, -1)
    );
}
