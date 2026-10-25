use super::*;

fn fixture() -> (Content, PlayerView, Brain, Request) {
    let (content, mut view) = super::super::quest_tests::fixture();
    let peer = &mut view.objects[0];
    peer.kind = crate::content::WIZARD;
    peer.name = "Peer".into();
    peer.account = Some(2);
    peer.position = view.position;
    peer.equipped = vec![0xa97];
    peer.level = Some(14);
    let brain = Brain::new(ARCHETYPES[0], &view);
    let request = brain
        .adventure_request(
            adventures::Plan::Invite {
                peer: adventures::Member {
                    account: 2,
                    name: "Peer".into(),
                },
                purpose: adventures::Purpose::Equipment,
            },
            &view,
            &content,
        )
        .unwrap();
    (content, view, brain, request)
}

#[tokio::test]
async fn session_cancellation_aborts_pending_publication() {
    let (_, _, _, request) = fixture();
    let task = tokio::spawn(std::future::pending::<std::result::Result<Reply, Failure>>());
    let aborted = task.abort_handle();
    let mut pending = Some(Pending {
        request,
        id: "cancel-test".into(),
        task,
    });
    cancel(&mut pending);
    assert!(pending.is_none());
    tokio::time::timeout(Duration::from_secs(1), async {
        while !aborted.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn publication_deadline_is_stable_length_sensitive_and_staggers_speakers() {
    let now = Instant::now();
    let deadlines: Vec<_> = ARCHETYPES
        .iter()
        .map(|p| publication_at(now, *p, "hi", "hi"))
        .collect();
    for (i, at) in deadlines.iter().enumerate() {
        assert_eq!(*at, publication_at(now, ARCHETYPES[i], "hi", "hi"));
        assert!(*at > now + Duration::from_millis(750));
        assert!(*at < now + Duration::from_secs(4));
    }
    assert!(deadlines.iter().collect::<BTreeSet<_>>().len() > 1);
    let longer = publication_at(
        now,
        ARCHETYPES[0],
        "can we level together?",
        "Peer, want to look for gear together?",
    );
    assert!(longer > deadlines[0]);
    // Fast generation waits; a five-second generation never adds another typing delay.
    assert!(longer.saturating_duration_since(now + Duration::from_millis(300)) > Duration::ZERO);
    assert_eq!(
        longer.saturating_duration_since(now + Duration::from_secs(5)),
        Duration::ZERO
    );
}

#[test]
fn optional_history_is_room_scoped_bounded_and_not_a_permanent_silence() {
    let mut speech = PublicSpeech::default();
    speech.record(-2, 1000, "want to level up together?");
    assert!(speech.repeated(-2, 1000, " WANT TO LEVEL UP TOGETHER? "));
    assert!(!speech.repeated(-3, 1000, "want to level up together?"));
    assert!(speech.repeated(-2, 90_999, "want to level up together?"));
    assert!(!speech.repeated(-2, 91_000, "want to level up together?"));
    for i in 0..5 {
        speech.record(-2, 1000, &format!("offer {i}"));
    }
    assert!(speech.repeated(-2, 1000, "want to level up together?"));
    for i in 0..40 {
        speech.record(-2, 1000, &format!("offer {i}"));
    }
    assert_eq!(speech.recent.len(), 32);
    assert!(!speech.repeated(-2, 1000, "want to level up together?"));
}

#[test]
fn delayed_dialogue_revalidates_world_peer_lifecycle_intent_and_expiry() {
    let (_, mut view, brain, mut request) = fixture();
    let invite = adventures::policy_reply(&request, Intent::Invite).unwrap();
    assert_eq!(invite.reply, "Peer, want to look for better gear together?");
    assert!(request.payload.get("recent_public_utterances").is_none());
    assert!(brain.validate_dialogue(&request, &invite, &view).is_ok());
    let obsolete = |error: anyhow::Error, reason| {
        assert_eq!(error.downcast_ref::<Obsolete>().unwrap().0, reason);
    };
    view.session.world -= 1;
    obsolete(
        brain
            .validate_dialogue(&request, &invite, &view)
            .unwrap_err(),
        "session_changed",
    );
    view.session.world += 1;
    let peer = view.objects.remove(0);
    obsolete(
        brain
            .validate_dialogue(&request, &invite, &view)
            .unwrap_err(),
        "speaker_left_perception",
    );
    view.objects.push(peer);
    view.objects[0].name = "SomeoneElse".into();
    obsolete(
        brain
            .validate_dialogue(&request, &invite, &view)
            .unwrap_err(),
        "speaker_left_perception",
    );
    view.objects[0].name = request.name.clone();
    view.lifecycle.busy = true;
    obsolete(
        brain
            .validate_dialogue(&request, &invite, &view)
            .unwrap_err(),
        "controller_busy",
    );
    view.lifecycle.quarantined = true;
    assert!(
        !brain
            .validate_dialogue(&request, &invite, &view)
            .unwrap_err()
            .is::<Obsolete>()
    );
    view.lifecycle.quarantined = false;
    view.lifecycle.busy = false;
    let unrequested = Reply {
        intent: Intent::Join,
        reply: "yes".into(),
    };
    assert!(
        !brain
            .validate_dialogue(&request, &unrequested, &view)
            .unwrap_err()
            .is::<Obsolete>()
    );
    let peer = view.objects.remove(0);
    assert!(
        !brain
            .validate_dialogue(&request, &unrequested, &view)
            .unwrap_err()
            .is::<Obsolete>()
    );
    view.lifecycle.busy = true;
    assert!(
        !brain
            .validate_dialogue(&request, &unrequested, &view)
            .unwrap_err()
            .is::<Obsolete>()
    );
    view.lifecycle.busy = false;
    view.objects.push(peer);
    let hp = view.character.hp;
    view.character.hp = 1;
    obsolete(
        brain
            .validate_dialogue(&request, &invite, &view)
            .unwrap_err(),
        "adventurer_no_longer_ready",
    );
    view.character.hp = hp;
    request.created = Instant::now() - Duration::from_secs(11);
    obsolete(
        brain
            .validate_dialogue(&request, &invite, &view)
            .unwrap_err(),
        "response_expired",
    );
    assert!(brain.agreement.is_none());
    assert_eq!(brain.invitation_until, 0);
}

#[test]
fn pending_speech_does_not_hold_up_critical_recovery() {
    let (content, mut view, mut brain, _) = fixture();
    brain.proposing = true;
    view.character.hp = 100;
    assert_eq!(brain.decide(&view, &content).unwrap().goal, Goal::Recover);
}

#[tokio::test]
async fn policy_speech_wait_is_enforced_but_does_not_stack_after_a_past_deadline() {
    let (_, _, _, request) = fixture();
    let reply = adventures::policy_reply(&request, Intent::Invite).unwrap();
    let at = publication_at(
        request.created,
        ARCHETYPES[0],
        request.payload["message"].as_str().unwrap(),
        &reply.reply,
    );
    let mut pending = Pending::start(None, request, ARCHETYPES[0]);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut pending.task)
            .await
            .is_err()
    );
    let reply = tokio::time::timeout(Duration::from_secs(5), &mut pending.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(Instant::now() >= at);
    assert_eq!(reply.intent, Intent::Invite);
    let (_, _, _, mut request) = fixture();
    request.created -= Duration::from_secs(5);
    let mut pending = Pending::start(None, request, ARCHETYPES[0]);
    assert!(
        tokio::time::timeout(Duration::from_millis(500), &mut pending.task)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
}

pub(super) async fn provider(
    status: axum::http::StatusCode,
    body: Value,
) -> (Arc<Service>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::post(move || {
                    let body = body.clone();
                    async move { (status, axum::Json(body)) }
                }),
            ),
        )
        .await
        .unwrap();
    });
    (
        Arc::new(Service::new("local-test-only", &endpoint, Duration::from_secs(2)).unwrap()),
        server,
    )
}

fn addressed(brain: &Brain, view: &PlayerView, content: &Content, text: &str) -> Request {
    let peer = &view.objects[0];
    let mut w = wire::Writer::new();
    w.utf(&peer.name)
        .i32(peer.id)
        .i32(0)
        .u8(0)
        .utf("")
        .utf(text)
        .utf("");
    brain
        .dialogue_request(
            &Packet {
                id: wire::TEXT,
                body: w.0,
            },
            view,
            content,
        )
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn same_bound_speaker_correction_cancels_the_old_ready_action_even_inside_cooldown() {
    let (content, view, _, _) = fixture();
    let profile = *persona::PERSONAS
        .iter()
        .max_by_key(|p| persona::seed(p.name) % 20)
        .unwrap();
    let brain = Brain::new(profile, &view);
    let text = "a".repeat(60);
    let envelope = json!({"model":MODEL,"choices":[{"finish_reason":"tool_calls","message":{
        "provider_metadata":{"gateway":{"routing":{"canonicalSlug":MODEL,"finalProvider":"nvidia"},"cost":"0"}},
        "tool_calls":[{"type":"function","function":{"name":"reply_to_player","arguments":json!({"intent":"follow","reply":text}).to_string()}}]}}]});
    let (service, server) = provider(axum::http::StatusCode::OK, envelope).await;
    let message = format!("{} follow me", profile.name);
    let old = addressed(&brain, &view, &content, &message);
    let at = publication_at(old.created, profile, &message, &text);
    let correction_at = old.created + Duration::from_millis(2200);
    let mut pending = Some(Pending::start(Some(service.clone()), old, profile));
    tokio::time::sleep_until(correction_at.into()).await;
    assert_eq!(
        service.health()["successes"],
        1,
        "generation completed; only presentation is pending"
    );
    assert!(!pending.as_ref().unwrap().task.is_finished());
    assert!(Instant::now() < at);
    let mut correction = addressed(
        &brain,
        &view,
        &content,
        &format!("{} wait here", profile.name),
    );
    correction.session.0 -= 1;
    assert!(!supersede(&mut pending, &correction));
    correction.session.0 += 1;
    correction.speaker += 1;
    assert!(!supersede(&mut pending, &correction));
    correction.speaker -= 1;
    correction.name = "SomeoneElse".into();
    assert!(!supersede(&mut pending, &correction));
    correction.name = "Peer".into();
    let abort = pending.as_ref().unwrap().task.abort_handle();
    assert!(supersede(&mut pending, &correction));
    tokio::time::timeout(Duration::from_millis(100), async {
        while !abort.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        Instant::now() < at,
        "old worker was aborted before its publication deadline"
    );
    tokio::time::sleep_until((at + Duration::from_millis(100)).into()).await;
    assert!(
        pending.is_none(),
        "no superseded reply remains for runtime publication/application"
    );
    assert!(brain.companion.is_none() && brain.agreement.is_none());
    let created = correction.created;
    pending = Some(Pending::start(Some(service), correction, profile));
    let newest = addressed(&brain, &view, &content, &format!("{} leave", profile.name));
    assert!(newest.created.duration_since(created) < Duration::from_secs(2));
    assert!(supersede(&mut pending, &newest));
    assert!(pending.is_none());
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn provider_failure_remains_observable_and_never_becomes_policy_speech() {
    let (service, server) = provider(axum::http::StatusCode::SERVICE_UNAVAILABLE, json!({})).await;
    let (_, _, _, request) = fixture();
    let mut pending = Pending::start(Some(service.clone()), request, ARCHETYPES[0]);
    let failure = (&mut pending.task).await.unwrap().unwrap_err();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert_eq!(failure.0, "provider_rejected");
    let health = service.health();
    assert_eq!(health["model"], MODEL);
    assert_eq!(health["successes"], 0);
    assert_eq!(health["failures"], 1);
    assert_eq!(health["last_error"], "provider_rejected");
}

#[test]
fn market_service_health_labels_process_budget_restart_reset_and_unknown_provider_quota() {
    let service = Service::new(
        "local-test-only",
        "http://127.0.0.1:1",
        Duration::from_secs(1),
    )
    .unwrap();
    {
        let mut b = service.budget.lock().unwrap();
        b.daily = 12;
        b.contract_failed = true;
        b.blocked_until = Instant::now() + Duration::from_secs(10);
    }
    let h = service.health();
    assert_eq!(h["budget_scope"], "process-local");
    assert_eq!(h["budget_resets_on_restart"], true);
    assert_eq!(h["provider_quota"], "unknown");
    assert_eq!(h["automatic_remaining"], 788);
    assert_eq!(h["requests_remaining"], 988);
    assert_eq!(h["contract_failed"], true);
    assert!(h["cooldown_seconds"].as_u64().unwrap() > 0);
    assert!(h["budget_window_resets_in_seconds"].as_u64().unwrap() <= 86400);
    service.budget.lock().unwrap().day = Instant::now() - Duration::from_secs(86401);
    let h = service.health();
    assert_eq!(h["daily_requests"], 0);
    assert_eq!(h["automatic_remaining"], 800);
    assert_eq!(h["requests_remaining"], 1000);
    assert_eq!(h["budget_window_resets_in_seconds"], 0);
}

#[tokio::test]
async fn market_process_budget_reserves_human_headroom_and_caps_total_requests_before_network_use()
{
    let service = Service::new(
        "local-test-only",
        "http://127.0.0.1:1",
        Duration::from_secs(1),
    )
    .unwrap();
    service.budget.lock().unwrap().daily = 800;
    assert_eq!(
        service
            .ask(&json!({"automatic":true}), "auto-cap")
            .await
            .unwrap_err()
            .0,
        "request_budget_exhausted"
    );
    assert_eq!(service.health()["daily_requests"], 800);
    service.budget.lock().unwrap().daily = 1000;
    assert_eq!(
        service.ask(&json!({}), "total-cap").await.unwrap_err().0,
        "request_budget_exhausted"
    );
    assert_eq!(service.health()["daily_requests"], 1000);
}
