use super::*;
use crate::world::view::VisibleObject;

#[test]
fn snake_exit_requires_observed_usable_return_and_waits_for_reward() {
    let (content, mut view) = quest_tests::fixture();
    view.map.name = "Snake Pit".into();
    view.quest = None;
    view.objects.clear();
    let decide = |view: &PlayerView| {
        Brain::new(ARCHETYPES[0], view)
            .decide(view, &content)
            .unwrap()
    };
    assert!(
        decide(&view).journey.is_none(),
        "no exit before publication"
    );
    view.objects.push(VisibleObject {
        id: 3,
        account: None,
        kind: 0x704,
        name: "Realm Portal".into(),
        position: Pos { x: 11.5, y: 10.5 },
        hp: 0,
        max_hp: 0,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: true,
        condition_mask: None,
    });
    let decision = decide(&view);
    assert_eq!(decision.goal, Goal::Portal(3));
    assert!(matches!(decision.journey, Some(Journey::Portal(3))));
    view.objects[0].usable = false;
    assert!(!matches!(decide(&view).journey, Some(Journey::Portal(_))));
    view.objects[0].usable = true;
    view.objects[0].kind = 0x712;
    assert!(!matches!(decide(&view).journey, Some(Journey::Portal(_))));
    view.objects[0].kind = 0x71d;
    assert!(matches!(decide(&view).journey, Some(Journey::Portal(3))));
    view.objects[0].kind = 0x704;
    view.objects.push(VisibleObject {
        id: 4,
        account: None,
        kind: 0x500,
        name: String::new(),
        position: view.position,
        hp: 1,
        max_hp: 1,
        level: None,
        equipped: vec![],
        contents: vec![0xa21],
        blocks: false,
        usable: true,
        condition_mask: None,
    });
    let decision = decide(&view);
    assert_eq!(decision.goal, Goal::Loot(4));
    assert!(
        decision.item.is_some(),
        "take the real visible reward before returning"
    );
    assert!(decision.journey.is_none());
    view.objects.pop();
    assert!(matches!(decide(&view).journey, Some(Journey::Portal(3))));
}
