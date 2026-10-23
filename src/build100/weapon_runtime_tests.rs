use super::*;
use crate::{content::Grounds, realm_map};
use std::{path::PathBuf, time::Duration};

fn setup(repaired: bool, class: i16) -> (Content, Room, Instant) {
    let path = PathBuf::from(
        std::env::var_os(if repaired {
            "BUILD100_CONTENT_REPAIRED_DIR"
        } else {
            "BUILD100_CONTENT_ORIGINAL_DIR"
        })
        .expect("qualified content required"),
    );
    let c = Content::load(&path).unwrap();
    let g = Arc::new(Grounds::load(&path).unwrap());
    let map = Arc::new(realm_map::tutorial(100, &c, g).unwrap());
    let mut r = Room::from_map(map, &c, 1_000_000).unwrap();
    let mut p = Character::new(&c, class, "Weapon".into(), 1).unwrap();
    (p.x, p.y) = r.landing();
    r.attach(1, p, &c).unwrap();
    (c, r, Instant::now())
}
fn fire(c: &Content, r: &Room, id: u8, angle: f32) -> Packet {
    let p = r.character(1);
    let kind = p.stats[&8] as i16;
    let proj = c.get(kind).unwrap().normal_projectile(0).unwrap();
    packet(8, |w| {
        w.u8(id)
            .i32(1)
            .u8(c.named(&proj.visual).unwrap().id as u8)
            .i16(kind)
            .f32(p.x + 0.3)
            .f32(p.y)
            .f32(angle)
            .i16(32767);
    })
}
fn send(c: &Content, r: &mut Room, id: u8, angle: f32, now: Instant) -> Result<Vec<Packet>> {
    let p = fire(c, r, id, angle);
    r.handle_at(1, p, c, now, now)
}
fn assert_unchanged(before: &Room, after: &Room) {
    assert_eq!(before.character(1), after.character(1));
    assert_eq!(before.bullets.len(), after.bullets.len());
    assert_eq!(
        before.rng.clone().gen_range(0.0..1.0),
        after.rng.clone().gen_range(0.0..1.0)
    );
    let a = &before.players[&1].weapon;
    let b = &after.players[&1].weapon;
    assert_eq!(a.started, b.started);
    assert_eq!(
        a.pending.as_ref().map(|p| &p.shots),
        b.pending.as_ref().map(|p| &p.shots)
    );
}

#[tokio::test]
async fn weapon_runtime_transport_handler_stall_does_not_reject_source_spaced_staff() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let (c, mut room, base) = setup(false, 0x30e);
    let clock_ms = Arc::new(AtomicU64::new(0));
    let marked = Arc::new(tokio::sync::Notify::new());
    let (source, mut source_rx) = tokio::sync::mpsc::channel(3);
    let stream = futures_util::stream::poll_fn(move |cx| source_rx.poll_recv(cx));
    let mut input = crate::input::receive_with_clock(stream, {
        let clock_ms = clock_ms.clone();
        let marked = marked.clone();
        move || {
            let now = base + Duration::from_millis(clock_ms.load(Ordering::SeqCst));
            marked.notify_one();
            now
        }
    });
    let mut encoder = crate::wire::Codec::server_with_keys(b"hPvJ(R}Mp$-cg", b"zCV2tsY0]s;|]");
    let mut frames = Vec::new();
    for first in [1, 3, 5] {
        let mut bytes = Vec::new();
        for id in [first, first + 1] {
            let p = fire(&c, &room, id, 0.0);
            bytes.extend(encoder.frame(p.id, p.body));
        }
        frames.push(axum::extract::ws::Message::Binary(bytes.into()));
    }
    source
        .send(Ok::<_, std::io::Error>(frames.remove(0)))
        .await
        .unwrap();
    let first = input.recv().await.unwrap().unwrap();
    marked.notified().await;
    // Source frames arrive at 0/360/720; the handler cannot dispatch the second until 400.
    clock_ms.store(360, Ordering::SeqCst);
    source.send(Ok(frames.remove(0))).await.unwrap();
    marked.notified().await;
    clock_ms.store(400, Ordering::SeqCst);
    let second = input.recv().await.unwrap().unwrap();
    clock_ms.store(720, Ordering::SeqCst);
    source.send(Ok(frames.remove(0))).await.unwrap();
    let third = input.recv().await.unwrap().unwrap();
    let receipts = [first.received, second.received, third.received];
    let mut codec = crate::protocol::server_codec();
    let mut published = Vec::new();
    let mut errors = Vec::new();
    for frame in [first, second, third] {
        let axum::extract::ws::Message::Binary(bytes) = frame.message else {
            panic!("binary");
        };
        for p in codec.feed(&bytes).unwrap() {
            match room.handle_at(1, p, &c, frame.received, frame.received) {
                Ok(packets) => published.extend(packets.into_iter().map(|p| p.body[0])),
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    assert_eq!(
        published,
        [1, 2, 3, 4, 5, 6],
        "receipts={:?}; errors={errors:?}",
        receipts.map(|r| r.duration_since(base).as_millis())
    );
    assert!(errors.is_empty());
    assert_eq!(
        receipts.map(|r| r.duration_since(base).as_millis()),
        [0, 360, 720]
    );
}

#[test]
fn cadence_pacing_measured_firebrand_pair_does_not_rephase() {
    let (c, mut r, base) = setup(true, 0x30e);
    r.character_mut(1).stats.extend([(28, 17), (8, 0xa98)]);
    send(&c, &mut r, 21, 0., base).unwrap();
    send(&c, &mut r, 22, 0., base + Duration::from_nanos(77_104)).unwrap();
    let before = r.rng.clone().gen_range(0.0..1.0);
    assert!(
        send(&c, &mut r, 23, 0., base + Duration::from_nanos(335_496_014))
            .unwrap()
            .is_empty()
    );
    assert!(
        send(&c, &mut r, 24, 0., base + Duration::from_nanos(335_937_358))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        r.players[&1].weapon.pending.as_ref().unwrap().shots,
        [(23, 0.), (24, 0.)]
    );
    assert_eq!(r.bullets.len(), 2);
    assert_eq!(r.rng.clone().gen_range(0.0..1.0), before);
    assert!(
        r.release_weapon(1, &c, base + Duration::from_nanos(336_077_103))
            .is_err()
    );
    assert_eq!(r.rng.clone().gen_range(0.0..1.0), before);
    let output = r
        .release_weapon(1, &c, base + Duration::from_nanos(336_077_104))
        .unwrap();
    assert_eq!(
        output.iter().map(|p| p.body[0]).collect::<Vec<_>>(),
        [23, 24]
    );
    assert_eq!(
        r.players[&1].weapon.started,
        Some(base + Duration::from_nanos(336_077_104))
    );
    assert!(
        r.release_weapon(1, &c, base + Duration::from_millis(337))
            .is_err()
    );
}

#[test]
fn cadence_pacing_cascades_and_receipt_relative_partial_expiry() {
    for (first, a, b) in [
        (93, 336_064_288, 336_503_887),
        (13, 336_183_211, 336_570_830),
    ] {
        let (c, mut r, base) = setup(true, 0x30e);
        r.character_mut(1).stats.insert(28, 17);
        send(&c, &mut r, 1, 0., base).unwrap();
        send(&c, &mut r, 2, 0., base).unwrap();
        assert!(
            send(&c, &mut r, first, 0., base + Duration::from_nanos(a))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            send(&c, &mut r, first + 1, 0., base + Duration::from_nanos(b))
                .unwrap()
                .len(),
            2
        );
        r.character_mut(1).x += 0.1;
        let next = base + Duration::from_nanos(b + 335_798_330);
        assert!(send(&c, &mut r, first + 2, 0., next).unwrap().is_empty());
        assert!(
            send(
                &c,
                &mut r,
                first + 3,
                0.,
                next + Duration::from_nanos(56_635)
            )
            .unwrap()
            .is_empty()
        );
        let due = r.weapon_deadline(1).unwrap();
        let events = r.release_weapon(1, &c, due).unwrap();
        assert_eq!(
            events.iter().map(|p| p.body[0]).collect::<Vec<_>>(),
            [first + 2, first + 3]
        );
    }
    let (c, mut r, base) = setup(true, 0x30e);
    let late = base + Duration::from_secs(10);
    let p = fire(&c, &r, 8, 0.);
    r.handle_at(1, p, &c, base, late).unwrap();
    let p = fire(&c, &r, 9, 0.);
    assert_eq!(
        r.handle_at(
            1,
            p,
            &c,
            base + Duration::from_millis(1),
            late + Duration::from_millis(1)
        )
        .unwrap()
        .len(),
        2
    );
    let p = fire(&c, &r, 10, 0.);
    r.handle_at(
        1,
        p,
        &c,
        base + Duration::from_millis(2),
        late + Duration::from_millis(2),
    )
    .unwrap();
    let p = fire(&c, &r, 11, 0.);
    r.handle_at(
        1,
        p,
        &c,
        base + Duration::from_millis(3),
        late + Duration::from_millis(3),
    )
    .unwrap();
    assert_eq!(
        r.players[&1]
            .weapon
            .pending
            .as_ref()
            .unwrap()
            .first_received,
        base + Duration::from_millis(2)
    );
    assert_eq!(
        r.release_weapon(1, &c, base + Duration::from_secs(100))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn cadence_pacing_delayed_funding_capacity_and_live_id_races_are_atomic() {
    for failure in 0..4 {
        let (c, mut r, base) = setup(true, 0x30e);
        r.character_mut(1).stats.insert(28, 17);
        send(&c, &mut r, 21, 0., base).unwrap();
        send(&c, &mut r, 22, 0., base).unwrap();
        send(&c, &mut r, 23, 0., base).unwrap();
        send(&c, &mut r, 24, 0., base).unwrap();
        let template = r.bullets[0].clone();
        if failure == 3 {
            r.character_mut(1).x += 3.;
        } else if failure == 2 {
            r.bullets.push(Bullet {
                owner: 1,
                id: 23,
                ..template
            });
        } else {
            r.bullets = (0..if failure == 0 { 4095 } else { 255 })
                .map(|i| Bullet {
                    owner: if failure == 0 { 999 } else { 1 },
                    id: (i % 90) as u8,
                    ..template.clone()
                })
                .collect();
        }
        let before = r.clone();
        let due = r.weapon_deadline(1).unwrap();
        assert!(r.release_weapon(1, &c, due).is_err());
        assert_unchanged(&before, &r);
        assert_eq!(r.weapon_deadline(1), Some(due));
    }
}

#[test]
fn cadence_pacing_stunned_and_paused_masks_fail_closed_at_collection_and_release() {
    for condition in [
        crate::content::Condition::Stunned,
        crate::content::Condition::Paused,
    ] {
        let (c, mut r, base) = setup(true, 0x30e);
        let bit = combat::ConditionMask::bit(condition) as i32;
        r.character_mut(1).stats.insert(25, bit);
        let before = r.clone();
        assert!(send(&c, &mut r, 1, 0., base).is_err());
        assert_unchanged(&before, &r);
        r.character_mut(1).stats.insert(25, 0);
        send(&c, &mut r, 1, 0., base).unwrap();
        send(&c, &mut r, 2, 0., base).unwrap();
        send(&c, &mut r, 3, 0., base).unwrap();
        send(&c, &mut r, 4, 0., base).unwrap();
        r.character_mut(1).stats.insert(25, bit);
        let before = r.clone();
        assert!(
            r.release_weapon(1, &c, r.weapon_deadline(1).unwrap())
                .is_err()
        );
        assert_unchanged(&before, &r);
    }
}

#[test]
fn weapon_runtime_shot_trace_is_bounded_nonmutating_and_not_admission() {
    let mut input = packet(8, |w| {
        w.u8(255)
            .i32(-7)
            .u8(41)
            .i16(2711)
            .f32(0.0)
            .f32(0.0)
            .f32(0.0)
            .i16(-1);
    });
    input.body[8..12].copy_from_slice(&f32::NAN.to_be_bytes());
    let original = input.body.clone();
    assert_eq!(crate::shot_trace(&input), Some((255, -7, 2711)));
    assert_eq!(input.body, original);
    for len in 0..original.len() {
        input.body.truncate(len);
        assert_eq!(crate::shot_trace(&input), None);
        input.body = original.clone();
    }
    input.body.push(0);
    assert_eq!(crate::shot_trace(&input), None);
    input.body = original;
    input.id = 17;
    assert_eq!(crate::shot_trace(&input), None);
}

#[test]
fn weapon_runtime_original_repaired_root_continuous_rolls_all_starters_and_staff_atomic_waves() {
    // Player.use rolls root Number then int((min+(max-min)*u)*(.5+ATK/75*1.5)).
    for repaired in [false, true] {
        for (class, original, repaired_range, attack) in [
            (0x300, (400, 500), (20, 60), 10),
            (0x307, (450, 500), (10, 40), 12),
            (0x30e, (10, 25), (10, 25), 12),
            (0x310, (400, 500), (20, 40), 10),
            (0x31d, (400, 500), (30, 60), 15),
            (0x31e, (400, 500), (30, 60), 15),
            (0x31f, (400, 500), (30, 60), 12),
        ] {
            let (c, mut r, now) = setup(repaired, class);
            let p = r.character(1).clone();
            let item = c.get(p.stats[&8] as i16).unwrap();
            let range = if repaired { repaired_range } else { original };
            assert_eq!(item.root_damage, Some(range));
            let mut rng = r.rng.clone();
            let expected = (0..item.num_projectiles)
                .map(|_| {
                    let u: f64 = rng.gen_range(0.0..1.0);
                    ((f64::from(range.0) + f64::from(range.1 - range.0) * u)
                        * (0.5 + f64::from(attack) / 75.0 * 1.5)) as i32
                })
                .collect::<Vec<_>>();
            let mut output = vec![];
            for i in 0..item.num_projectiles {
                let events = send(&c, &mut r, if i == 0 { 96 } else { 1 }, 0.0, now).unwrap();
                if i + 1 < item.num_projectiles {
                    assert!(events.is_empty() && r.bullets.is_empty());
                    assert_eq!(r.rng.clone().gen_range(0.0..1.0), {
                        let (_, original, _) = setup(repaired, class);
                        original.rng.clone().gen_range(0.0..1.0)
                    });
                }
                output.extend(events);
            }
            assert_eq!(output.len(), usize::from(item.num_projectiles));
            assert_eq!(
                r.bullets.iter().map(|b| b.damage).collect::<Vec<_>>(),
                expected
            );
            assert_eq!(r.character(1), &p);
            for (i, shot) in output.iter().enumerate() {
                let mut read = Reader::new(&shot.body);
                assert_eq!(read.u8().unwrap(), if i == 0 { 96 } else { 1 });
                assert_eq!(read.i32().unwrap(), 1);
                assert_eq!(read.u8().unwrap(), 0); // Incoming visual identity is not server selector.
                assert_eq!(read.i16().unwrap(), item.id);
                read.f32().unwrap();
                read.f32().unwrap();
                assert_eq!(read.f32().unwrap(), 0.0);
                assert_eq!(read.i16().unwrap(), expected[i] as i16);
                read.done().unwrap();
            }
            if class == 0x30e {
                assert_eq!(
                    r.bullets
                        .iter()
                        .map(|b| (b.id % 2, b.amplitude, b.frequency))
                        .collect::<Vec<_>>(),
                    [(0, 0.5, 2.0), (1, 0.5, 2.0)]
                );
            }
        }
    }
}

#[test]
fn weapon_runtime_actual_funding_deadline_no_elapsed_clock_bank_and_effective_dex_rof() {
    let (mut c, mut r, now) = setup(true, 0x307);
    r.character_mut(1).stats.insert(28, 75);
    send(&c, &mut r, 1, 0.0, now).unwrap();
    assert!(
        send(&c, &mut r, 2, 0.0, now + Duration::from_nanos(124_999_999))
            .unwrap()
            .is_empty()
    );
    assert!(
        r.release_weapon(1, &c, now + Duration::from_nanos(124_999_999))
            .is_err()
    );
    assert_eq!(
        r.release_weapon(1, &c, now + Duration::from_millis(125))
            .unwrap()
            .len(),
        1
    );
    r.elapsed = 1.0e20; // Simulation time cannot authorize a weapon packet.
    assert!(
        send(&c, &mut r, 3, 0.0, now + Duration::from_millis(125))
            .unwrap()
            .is_empty()
    );
    assert_eq!(r.weapon_deadline(1), Some(now + Duration::from_millis(250)));
    assert_eq!(
        r.release_weapon(1, &c, now + Duration::from_secs(100))
            .unwrap()
            .len(),
        1
    );
    assert!(
        send(&c, &mut r, 4, 0.0, now + Duration::from_secs(100))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        r.weapon_deadline(1),
        Some(now + Duration::from_millis(100_125))
    );
    r.release_weapon(1, &c, now + Duration::from_millis(100_125))
        .unwrap();
    let (c0, mut r0, now) = setup(true, 0x307);
    r0.character_mut(1).stats.extend([(28, 75), (9, 0xa63)]); // Iron Quiver +2 DEX, no75 cap.
    send(&c0, &mut r0, 1, 0.0, now).unwrap();
    assert!(
        send(
            &c0,
            &mut r0,
            2,
            0.0,
            now + Duration::from_nanos(121_999_999)
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        r0.release_weapon(1, &c0, now + Duration::from_millis(122))
            .unwrap()
            .len(),
        1
    );
    let (original, mut sword, now) = setup(false, 0x31d);
    send(&original, &mut sword, 1, 0.0, now).unwrap();
    assert!(
        send(
            &original,
            &mut sword,
            2,
            0.0,
            now + Duration::from_nanos(13_999_999)
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        sword
            .release_weapon(1, &original, now + Duration::from_millis(14))
            .unwrap()
            .len(),
        1
    );
    c.objects.get_mut(&0xa1a).unwrap().rate_of_fire = None;
    assert!(
        send(&c, &mut r, 5, 0.0, now + Duration::from_secs(200))
            .err()
            .expect("rejected")
            .to_string()
            .contains("RateOfFire")
    );
}

#[test]
fn weapon_runtime_two_three_spread_native_number_order_and_wrong_angle_rejected_atomically() {
    for n in [2, 3] {
        let (mut c, mut r, now) = setup(true, 0x307);
        let item = c.objects.get_mut(&0xa1a).unwrap();
        item.num_projectiles = n;
        item.arc_gap_radians = 0.1;
        // Source XML Number .1, not f64::from(typed f32 .1).
        let mut angle = 0.45 - 0.1 * f64::from(n - 1) / 2.0;
        for i in 0..n {
            let before = r.clone();
            if i > 0 {
                assert!(send(&c, &mut r, 17 + i, (angle as f32).next_up().next_up(), now).is_err());
                assert_unchanged(&before, &r);
            }
            let output = send(&c, &mut r, 17 + i, angle as f32, now).unwrap();
            assert_eq!(output.len(), if i + 1 == n { usize::from(n) } else { 0 });
            angle += 0.1;
        }
        assert_eq!(
            r.bullets.iter().map(|b| b.id).collect::<Vec<_>>(),
            (0..n).map(|i| 17 + i).collect::<Vec<_>>()
        );
    }
}

#[test]
fn weapon_runtime_pending_expiry_replacement_is_not_late_second_shot_or_cadence_refund() {
    let (c, mut r, now) = setup(true, 0x30e);
    r.character_mut(1).stats.insert(28, 75); // 125ms source interval.
    let mut expected_rng = r.rng.clone();
    send(&c, &mut r, 8, 0.0, now).unwrap();
    let before = r.character(1).clone();
    r.advance(0.5, &c).unwrap();
    assert!(r.bullets.is_empty());
    // Exactly at expiry the second packet is a new FIRST, never a funded late pair.
    assert!(
        send(&c, &mut r, 19, 1.0, now + Duration::from_millis(125))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        r.players[&1].weapon.pending.as_ref().unwrap().shots,
        [(19, 1.0)]
    );
    assert_eq!(
        send(&c, &mut r, 20, 1.0, now + Duration::from_millis(126))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(r.bullets.iter().map(|b| b.id).collect::<Vec<_>>(), [19, 20]);
    for bullet in &r.bullets {
        let u: f64 = expected_rng.gen_range(0.0..1.0);
        assert_eq!(bullet.damage, ((10.0 + 15.0 * u) * 0.74) as i32);
    }
    assert_eq!(
        r.rng.clone().gen_range(0.0..1.0),
        expected_rng.gen_range(0.0..1.0)
    );
    assert!(
        send(&c, &mut r, 44, 1.0, now + Duration::from_millis(249))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        r.players[&1].weapon.pending.as_ref().unwrap().due,
        now + Duration::from_millis(251)
    );
    r.cancel_weapon(1);
    send(&c, &mut r, 47, 1., now + Duration::from_millis(249)).unwrap();
    assert_eq!(
        r.players[&1].weapon.pending.as_ref().unwrap().due,
        now + Duration::from_millis(376)
    );
    assert_eq!(r.character(1).stats[&6], before.stats[&6]);
    assert_eq!(r.character(1).stats[&4], before.stats[&4]);
    assert_eq!(r.character(1).stats[&8], before.stats[&8]);
}

#[test]
fn weapon_runtime_malformed_group_final_packet_and_capacity_have_no_partial_roll_or_output() {
    let (c, mut r, now) = setup(true, 0x30e);
    send(&c, &mut r, 8, 0.0, now).unwrap();
    for mutation in 0..10 {
        let mut p = fire(&c, &r, 9, 0.0);
        match mutation {
            0 => p.body[0] = 8,  // Duplicate buffered ID.
            1 => p.body[4] = 2,  // Owner.
            2 => p.body[5] ^= 1, // Visual.
            3 => p.body[7] ^= 1, // Item.
            4 => p.body[8..12].copy_from_slice(&(r.character(1).x + 0.4).to_be_bytes()),
            5 => p.body[16..20].copy_from_slice(&f32::NAN.to_be_bytes()),
            6 => p.body[8..12].copy_from_slice(&f32::INFINITY.to_be_bytes()),
            7 => p.body[16..20].copy_from_slice(&1.0_f32.to_be_bytes()),
            8 => {
                p.body.pop();
            }
            9 => p.body.push(0),
            _ => unreachable!(),
        }
        let before = r.clone();
        assert!(
            r.handle_at(1, p, &c, now, now).is_err(),
            "mutation {mutation}"
        );
        assert_unchanged(&before, &r);
    }
    r.character_mut(1).stats.insert(11, 0xa29);
    let before = r.clone();
    assert!(
        send(&c, &mut r, 9, 0.0, now)
            .err()
            .expect("rejected")
            .to_string()
            .contains("equipment")
    );
    assert_unchanged(&before, &r);
    r.character_mut(1).stats.insert(11, -1);
    assert_eq!(send(&c, &mut r, 9, 0.0, now).unwrap().len(), 2);
    let template = r.bullets[0].clone();
    for owner_capacity in [true, false] {
        let (c, mut r, now) = setup(true, 0x30e);
        let count = if owner_capacity { 255 } else { 4095 };
        r.bullets = (0..)
            .filter(|i| !owner_capacity || *i != 95)
            .take(count)
            .map(|i| Bullet {
                owner: if owner_capacity { 1 } else { 999 },
                id: i as u8,
                ..template.clone()
            })
            .collect();
        let before = r.clone();
        assert!(send(&c, &mut r, 95, 0.0, now).is_err());
        assert_unchanged(&before, &r);
    }
}

#[test]
fn weapon_runtime_final_damage_overflow_missing_metadata_and_unsupported_selector_are_atomic() {
    let (mut c, mut r, now) = setup(true, 0x30e);
    send(&c, &mut r, 1, 0.0, now).unwrap();
    r.players
        .get_mut(&1)
        .unwrap()
        .weapon
        .pending
        .as_mut()
        .unwrap()
        .weapon
        .root_damage = Some((32767, 32767));
    r.players
        .get_mut(&1)
        .unwrap()
        .weapon
        .pending
        .as_mut()
        .unwrap()
        .attack = 100;
    let before = r.clone();
    assert!(send(&c, &mut r, 2, 0.0, now).is_err());
    assert_unchanged(&before, &r);
    for mutation in 0..5 {
        let (_, mut r, now) = setup(true, 0x30e);
        let item = c.objects.get_mut(&0xa97).unwrap();
        match mutation {
            0 => item.root_damage = None,
            1 => {
                item.root_damage = Some((10, 25));
                item.rate_of_fire = None;
            }
            2 => {
                item.rate_of_fire = Some(1.0);
                item.num_projectiles = 0;
            }
            3 => {
                item.num_projectiles = 2;
                item.projectiles[0]
                    .1
                    .features
                    .push("UnsupportedMotion".into());
            }
            4 => {
                item.projectiles[0].1.features.clear();
                item.projectiles[0].0 = 1;
            }
            _ => unreachable!(),
        }
        // Build wire without selecting a deliberately unsupported server projectile.
        let p = packet(8, |w| {
            let p = r.character(1);
            w.u8(1)
                .i32(1)
                .u8(41)
                .i16(0xa97)
                .f32(p.x)
                .f32(p.y)
                .f32(0.0)
                .i16(0);
        });
        let before = r.clone();
        let error = r
            .handle_at(1, p, &c, now, now)
            .err()
            .expect("metadata rejected")
            .to_string();
        assert!(
            error.contains(
                [
                    "missing root damage",
                    "RateOfFire",
                    "empty weapon burst",
                    "unsupported normal projectile semantics",
                    "missing projectile 0"
                ][mutation]
            ),
            "{error}"
        );
        assert_unchanged(&before, &r);
    }
}

#[test]
fn weapon_runtime_detach_death_clear_pending_no_attacks_while_dead_and_invisible_does_not_cancel() {
    let (c, mut r, now) = setup(true, 0x30e);
    send(&c, &mut r, 9, 0.0, now).unwrap();
    let saved = r.character(1).clone();
    r.detach(1);
    r.attach(1, saved, &c).unwrap();
    assert!(r.players[&1].weapon.pending.is_none());
    assert!(send(&c, &mut r, 11, 0.0, now).unwrap().is_empty());
    let pos = (r.character(1).x, r.character(1).y);
    r.bullets.push(Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(0.1, 1.),
        owner: 999,
        id: 1,
        origin: pos,
        age: 0.0,
        amplitude: 0.0,
        frequency: 1.0,
        x: pos.0,
        y: pos.1,
        vx: 0.0,
        vy: 0.0,
        damage: 1000,
        life: 1.0,
    });
    r.advance(0.05, &c).unwrap();
    assert!(!r.character(1).alive);
    assert!(r.players[&1].weapon.pending.is_none());
    let p = fire(&c, &r, 12, 0.0);
    assert!(r.handle_at(1, p, &c, now, now).unwrap().is_empty());
    let (c, mut r, now) = setup(true, 0x300);
    let cloak = packet(15, |w| {
        w.i32(1).u8(1).i32(1).f32(0.0).f32(0.0);
    });
    r.handle_at(1, cloak, &c, now, now).unwrap();
    assert_eq!(send(&c, &mut r, 1, 0.0, now).unwrap().len(), 1);
    assert_eq!(r.character(1).stats[&25], 4096);
    assert!(send(&c, &mut r, 2, 0.0, now).unwrap().is_empty());
    assert_eq!(r.bullets.len(), 1);
    assert_eq!(r.character(1).stats[&25], 4096);
}

#[test]
fn weapon_runtime_capacity_exact_full_batch_and_final_capacity_race_are_atomic() {
    let (c, mut template_room, now) = setup(true, 0x30e);
    send(&c, &mut template_room, 1, 0.0, now).unwrap();
    send(&c, &mut template_room, 2, 0.0, now).unwrap();
    let template = template_room.bullets[0].clone();
    for global in [false, true] {
        let (c, mut r, now) = setup(true, 0x30e);
        send(&c, &mut r, 94, 0.0, now).unwrap();
        let count = if global { 4094 } else { 254 };
        r.bullets = (0..)
            .filter(|i| global || ![94, 95].contains(i))
            .take(count)
            .map(|i| Bullet {
                owner: if global { 999 } else { 1 },
                id: i as u8,
                ..template.clone()
            })
            .collect();
        let mut excessive = r.clone();
        excessive.bullets.push(Bullet {
            owner: if global { 999 } else { 1 },
            id: 94,
            ..template.clone()
        });
        let before = excessive.clone();
        assert!(send(&c, &mut excessive, 95, 0.0, now).is_err());
        assert_unchanged(&before, &excessive);
        assert_eq!(send(&c, &mut r, 95, 0.0, now).unwrap().len(), 2);
        assert_eq!(r.bullets.len(), count + 2);
    }
}
