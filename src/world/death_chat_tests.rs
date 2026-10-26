use super::*;

async fn fixture() -> (
    World,
    crate::store::Store,
    [i32; 2],
    Vec<mpsc::Receiver<Packet>>,
) {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let map = Arc::new(Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap());
    let mut world = World::new(content, map).unwrap();
    let mut ids = Vec::new();
    let mut inboxes = Vec::new();
    let (lost, _) = watch::channel(None);
    for role in ["victim", "witness"] {
        let account = store.list(format!("death-chat-{role}")).await.unwrap();
        let mut ch = store
            .create(
                account.id,
                world.content.get(crate::content::WIZARD).unwrap(),
            )
            .await
            .unwrap();
        if role == "victim" {
            ch.name = "Victim".into();
        }
        let (packets, mut inbox) = mpsc::channel(64);
        let output = Some(Output {
            packets,
            lost: lost.clone(),
        });
        ids.push(world.enter(ch, output).unwrap());
        while inbox.try_recv().is_ok() {}
        inboxes.push(inbox);
    }
    let origin = world.players[&ids[0]].pos;
    let witness = world.players.get_mut(&ids[1]).unwrap();
    witness.pos = Pos {
        x: origin.x,
        y: origin.y - 60.,
    };
    (world, store, [ids[0], ids[1]], inboxes)
}

fn messages(inbox: &mut mpsc::Receiver<Packet>) -> Vec<String> {
    std::iter::from_fn(|| inbox.try_recv().ok())
        .filter_map(|p| {
            if p.id != wire::TEXT {
                return None;
            }
            let mut r = wire::Reader::new(&p.body);
            assert_eq!(r.utf().unwrap(), "");
            assert_eq!(r.i32().unwrap(), -1);
            assert_eq!(r.i32().unwrap(), -1);
            assert_eq!(r.u8().unwrap(), 0);
            assert_eq!(r.utf().unwrap(), "");
            Some(r.utf().unwrap())
        })
        .collect()
}

#[tokio::test]
async fn death_chat_is_world_wide_once_and_only_after_successful_persistence() {
    for fail in [false, true] {
        let (mut world, store, [victim, witness], inboxes) = fixture().await;
        let [mut victim_inbox, mut inbox]: [_; 2] = inboxes.try_into().unwrap();
        assert!(world.finish_death(victim).is_err());
        assert!(messages(&mut inbox).is_empty());
        let p = world.players.get_mut(&victim).unwrap();
        p.character.hp = 0;
        p.death_pending = true;
        p.killer = "Medusa".into();
        let (_, ch, killer) = world.pending_deaths().pop().unwrap();
        assert!(messages(&mut inbox).is_empty());
        assert!(store.fame(ch.account, ch.id).await.is_err());
        if fail {
            assert!(
                world
                    .committed::<()>(
                        victim,
                        Err(anyhow::anyhow!("injected death commit failure"))
                    )
                    .is_err()
            );
            assert!(world.finish_death(victim).is_err());
            assert!(messages(&mut inbox).is_empty());
            continue;
        }
        world
            .committed(victim, store.death(ch.clone(), killer).await)
            .unwrap();
        assert!(store.fame(ch.account, ch.id).await.is_ok());
        world.finish_death(victim).unwrap();
        assert_eq!(victim_inbox.try_recv().unwrap().id, wire::TEXT);
        assert_eq!(victim_inbox.try_recv().unwrap().id, 41);
        assert!(victim_inbox.try_recv().is_err());
        assert_eq!(
            messages(&mut inbox),
            ["Victim died at level 1, killed by Medusa"]
        );
        assert!(!world.players[&victim].character.alive);
        assert!(world.players[&witness].character.alive);
        assert!(world.finish_death(victim).is_err());
        assert!(messages(&mut inbox).is_empty());
    }
}
