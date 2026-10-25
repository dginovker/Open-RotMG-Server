use super::*;

#[tokio::test]
async fn quiver_and_wooden_shield_transport_encrypted_split_coalesced_serial_awaited_replies_no_adjacency_or_deadlock()
 {
    for item in [0xa61, 0xa63, 0xa64, 0xa65, 0xa08] {
        let remaining = match item {
            0xa61 => 40,
            0xa65 => 0,
            _ => 20,
        };
        for split in [false, true] {
            let h = Harness::normal();
            let mut a = equipped_shooter(&h, A, 0, item).await;
            let (mut b, _) = h.enter(B).await;
            drain(&mut a);
            drain(&mut b);
            let id = a.avatar;
            let position = h
                .inspect(move |s| {
                    let p = s.worlds[&0].character(id);
                    (p.x, p.y)
                })
                .await;
            let mut encoder =
                crate::wire::Codec::server_with_keys(b"hPvJ(R}Mp$-cg", b"zCV2tsY0]s;|]");
            let mut bytes = vec![];
            for p in [
                shot(&h, &a, 96).await,
                packet(5, |w| {
                    w.f32(position.0).f32(position.1);
                }),
                world::prediction_runtime_tests::report(99999, id, 96, 4),
                cast(id, 1, id, (0.5, 0.5)),
            ] {
                bytes.extend(encoder.frame(p.id, p.body));
            }
            let chunks = if split {
                vec![
                    bytes[..3].to_vec(),
                    bytes[3..19].to_vec(),
                    bytes[19..35].to_vec(),
                    bytes[35..].to_vec(),
                ]
            } else {
                vec![bytes]
            };
            let stream =
                futures_util::stream::iter(chunks.into_iter().map(|b| {
                    Ok::<_, std::io::Error>(axum::extract::ws::Message::Binary(b.into()))
                }));
            let mut input = crate::input::receive(stream);
            let mut codec = crate::protocol::server_codec();
            let mut count = 0;
            while let Some(frame) = input.recv().await.unwrap() {
                let axum::extract::ws::Message::Binary(ref bytes) = frame.message else {
                    panic!("binary")
                };
                for decoded in codec.feed(bytes).unwrap() {
                    let (reply, receive) = oneshot::channel();
                    h.commands
                        .send(Command::Input {
                            session: a.session,
                            packet: decoded,
                            received: frame.received,
                            reply,
                        })
                        .await
                        .unwrap();
                    tokio::time::timeout(Duration::from_secs(2), receive)
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                    count += 1;
                    if count == 1 {
                        assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], remaining);
                        // Native loop awaits8 before dispatching15; another owner remains responsive meanwhile.
                        let peer = b.avatar;
                        let pos = h
                            .inspect(move |s| {
                                let p = s.worlds[&0].character(peer);
                                (p.x, p.y)
                            })
                            .await;
                        tokio::time::timeout(
                            Duration::from_secs(2),
                            h.input(
                                b.session,
                                packet(5, |w| {
                                    w.f32(pos.0).f32(pos.1);
                                }),
                            ),
                        )
                        .await
                        .unwrap()
                        .unwrap();
                    }
                }
            }
            assert_eq!(count, 4);
            assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], remaining);
            assert!(shot_ids(&drain(&mut a), id).is_empty());
            assert_eq!(shot_ids(&drain(&mut b), id), [96]);
            assert!(h.input(a.session, cast(id, 1, id, (0., 0.))).await.is_err());
            h.stop().await;
        }
    }
}
