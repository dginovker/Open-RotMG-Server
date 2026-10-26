use super::*;
use crate::bots::runtime::tests::{actor, nearby};
use crate::server;

fn member(account: i32) -> Member {
    Member {
        account,
        name: format!("Member{account}"),
    }
}
fn trip(organizer: i32) -> Adventure {
    Adventure {
        members: vec![member(organizer)],
        purpose: Purpose::Equipment,
        until: 600000,
        finished: false,
    }
}

#[tokio::test]
async fn society_publication_cancels_valid_issued_plans_after_context_drift() {
    for answering in [false, true] {
        for drift in [
            "finished",
            "full",
            "purpose",
            "occupied",
            "offer_removed",
            "offer_expired",
        ] {
            if !answering && drift.starts_with("offer_") {
                continue;
            }
            let (dir, app) = server::test_app();
            app.world.clock.set_paused(true);
            let (_owner, id, mut output, mut brain) =
                actor(&app, "outing-bot", "Trader", [-1; 8]).await;
            let (_peer_owner, peer, mut peer_output, _) =
                actor(&app, "outing-peer", "Peer", [-1; 8]).await;
            nearby(&app, id, peer).await;
            let view = app.world.player_view(id).await.unwrap();
            let own = view.session.account;
            let other = app.world.player_view(peer).await.unwrap().session.account;
            let plan = {
                let mut state = app.society.lock().unwrap();
                state.register(own, Default::default());
                state.register(other, Default::default());
                if answering {
                    state.apply(
                        Member {
                            account: other,
                            name: "Peer".into(),
                        },
                        Plan::Invite {
                            peer: member(own),
                            purpose: Purpose::Equipment,
                        },
                        false,
                        "Trader, want to look for gear?".into(),
                        0,
                    );
                }
                let plan = state
                    .next(&brain, &view, &app.content, false)
                    .expect("real planner must issue a valid plan");
                state.validate(own, &plan, 0).unwrap();
                plan
            };
            let request = brain.adventure_request(plan, &view, &app.content).unwrap();
            let intent = if answering {
                dialogue::Intent::Join
            } else {
                dialogue::Intent::Invite
            };
            let reply = policy_reply(&request, intent).unwrap();
            {
                let mut state = app.society.lock().unwrap();
                let organizer = if answering { other } else { own };
                let mut changed = trip(organizer);
                match drift {
                    "finished" => changed.finished = true,
                    "full" => changed.members.extend((100..105).map(member)),
                    "purpose" => changed.purpose = Purpose::Experience,
                    "occupied" => {
                        changed.members = vec![member(if answering { own } else { other })]
                    }
                    "offer_removed" => state.offers.clear(),
                    "offer_expired" => {}
                    _ => unreachable!(),
                }
                if !drift.starts_with("offer_") {
                    state.trips.push(changed);
                }
            }
            if drift == "offer_expired" {
                app.world
                    .directory(|d| {
                        d.tick = 1200;
                        Ok(())
                    })
                    .await
                    .unwrap();
            }
            while output.try_recv().is_ok() {}
            while peer_output.try_recv().is_ok() {}
            let error = dialogue::publish(&app, &mut brain, &request, &reply, &view)
                .await
                .unwrap_err();
            assert!(
                error.is::<dialogue::Obsolete>(),
                "answering={answering} drift={drift}: {error:#}"
            );
            assert!(
                !std::iter::from_fn(|| peer_output.try_recv().ok()).any(|p| p.id == wire::TEXT)
            );
            assert!(!std::iter::from_fn(|| output.try_recv().ok()).any(|p| p.id == wire::TEXT));
            assert!(brain.agreement.is_none());
            assert_eq!(brain.invitation_until, 0);
            let state = app.society.lock().unwrap();
            assert!(
                !state
                    .speech
                    .repeated(-2, 0, "want to look for better gear together?")
            );
            drop(state);
            drop(app);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[test]
fn society_immutable_plan_errors_precede_obsolete_trip_state() {
    let mut state = Society::default();
    let mut ended = trip(1);
    ended.finished = true;
    state.trips.push(ended);
    let self_invite = Plan::Invite {
        peer: member(1),
        purpose: Purpose::Equipment,
    };
    assert!(
        !state
            .validate(1, &self_invite, 0)
            .unwrap_err()
            .is::<dialogue::Obsolete>()
    );
    let answer = Plan::Answer(Offer {
        from: member(1),
        to: member(2),
        purpose: Purpose::Equipment,
        until: 60000,
        text: "want to hunt?".into(),
        answer_started: true,
    });
    assert!(
        !state
            .validate(99, &answer, 0)
            .unwrap_err()
            .is::<dialogue::Obsolete>()
    );
    let malformed = Plan::Answer(Offer {
        from: member(1),
        to: member(1),
        purpose: Purpose::Equipment,
        until: 60000,
        text: "want to hunt?".into(),
        answer_started: true,
    });
    assert!(state.validate(1, &malformed, 0).is_err());
}

#[tokio::test]
async fn society_publication_rejects_unrequested_intent_even_after_trip_finishes() {
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (_owner, id, _output, mut brain) =
        actor(&app, "invalid-outing-bot", "Trader", [-1; 8]).await;
    let (_peer_owner, peer, _peer_output, _) =
        actor(&app, "invalid-outing-peer", "Peer", [-1; 8]).await;
    nearby(&app, id, peer).await;
    let view = app.world.player_view(id).await.unwrap();
    let other = app.world.player_view(peer).await.unwrap().session.account;
    let request = brain
        .adventure_request(
            Plan::Invite {
                peer: Member {
                    account: other,
                    name: "Peer".into(),
                },
                purpose: Purpose::Equipment,
            },
            &view,
            &app.content,
        )
        .unwrap();
    let mut ended = trip(view.session.account);
    ended.finished = true;
    app.society.lock().unwrap().trips.push(ended);
    let reply = dialogue::Reply {
        intent: dialogue::Intent::Join,
        reply: "yes".into(),
    };
    let error = dialogue::publish(&app, &mut brain, &request, &reply, &view)
        .await
        .unwrap_err();
    assert!(!error.is::<dialogue::Obsolete>());
    assert_eq!(error.to_string(), "dialogue intention not requested");
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
