use super::*;
use crate::store::{Character, experience_goal, level_start};
use rand::seq::SliceRandom;
use serde::Deserialize;

#[cfg(test)]
mod stock_tests;
use std::sync::LazyLock;

// Class quest thresholds; each reached threshold is one account star.
const THRESHOLDS: [i32; 5] = [20, 150, 400, 800, 2000];
// Potions raise attack and dexterity first, as players usually did.
const MAX_ORDER: [usize; 8] = [2, 5, 4, 3, 6, 7, 0, 1];

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Newbie,
    Grinder,
    Trader,
    Rusher,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    stars: i32,
    role: Role,
}

/// Historical names/rank bands; exact stars and roles are reconstructed presets, not measured lives.
pub(crate) static PERSONAS: LazyLock<Vec<Profile>> = LazyLock::new(|| {
    let entries: Vec<Entry> = serde_json::from_str(include_str!("../../data/bot-personas.json"))
        .expect("data/bot-personas.json is a valid persona list");
    entries.into_iter().map(profile).collect()
});

fn profile(e: Entry) -> Profile {
    assert!(
        (0..=65).contains(&e.stars),
        "{} has impossible stars",
        e.name
    );
    // Skill follows how much someone has played; risk and wants follow what they play for.
    let skill = e.stars as f32 / 65.;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed(&e.name));
    let mut jitter = |spread: f32| rng.gen_range(-spread..=spread);
    let (recovery, resources, companions) = match e.role {
        Role::Newbie => (0.5, 3., 7.),
        Role::Grinder => (0.48, 4., 6.),
        Role::Trader => (0.55, 8., 3.),
        Role::Rusher => (0.36, 6., 5.),
    };
    Profile {
        identity: Box::leak(format!("persona:v1:{}", e.name).into_boxed_str()),
        name: Box::leak(e.name.into_boxed_str()),
        recovery: (recovery + jitter(0.06)).clamp(0.3, 0.7),
        resources: resources + jitter(1.),
        companions: companions + jitter(1.),
        reaction: (6. - 3. * skill + jitter(0.6)).round().clamp(3., 7.) as u64,
        attention: (2. + 6. * skill + jitter(1.)).round().clamp(2., 8.) as usize,
        velocity_step: (2. - 1.75 * skill).clamp(0.25, 2.),
        directions: if skill > 0.5 {
            16
        } else if skill > 0.15 {
            8
        } else {
            4
        },
        pickup_clearance: (1.5 - 0.85 * skill + jitter(0.1)).clamp(0.65, 1.5),
        knows_blue: skill > 0.4,
        knows_minimap: e.role != Role::Newbie,
        stars: e.stars,
        role: e.role,
    }
}

pub(super) fn seed(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// The characters and vault a persona owns at this point of its 2012 life.
pub(super) fn life(
    p: &Profile,
    content: &Content,
    rng: &mut impl Rng,
) -> Result<(Vec<Character>, Vec<[i32; 8]>)> {
    // Gear and current-life sampling must not change synthetic account history on a later rebuild.
    let mut history = rand::rngs::StdRng::seed_from_u64(seed(p.identity));
    let mut classes: Vec<u16> = content.classes.keys().copied().collect();
    classes.shuffle(&mut history);
    let mut roster = Vec::new();
    if p.stars == 0 || p.role == Role::Trader {
        let level = if p.role == Role::Trader {
            1
        } else {
            rng.gen_range(1..=19)
        };
        let exp = level_start(level)? + rng.gen_range(0..experience_goal(level)?);
        roster.push(character(content, classes[0], level, exp, p.stars, rng)?);
    }
    let mut left = p.stars;
    let count = classes.len() as i32;
    for (index, class) in classes.into_iter().enumerate() {
        if left == 0 {
            break;
        }
        // Leave no more stars than the remaining classes can hold.
        let fewest = (left - 5 * (count - 1 - index as i32)).max(1);
        let stars = history.gen_range(fewest..=left.min(5));
        left -= stars;
        let floor = THRESHOLDS[stars as usize - 1];
        let ceiling = THRESHOLDS.get(stars as usize).copied().unwrap_or(4000);
        let fame = history.gen_range(floor..ceiling);
        let exp = fame * 1000 + history.gen_range(0..1000);
        roster.push(character(content, class, 20, exp, p.stars, rng)?);
    }
    ensure!(left == 0, "{} cannot hold {} stars", p.name, p.stars);
    let mut needed = BTreeMap::new();
    if p.stars >= 26 {
        // Veteran presets own an unlocked roster, not fabricated kills or deaths.
        for &class in content.classes.keys() {
            prerequisites(content, class, &mut needed);
        }
    } else {
        for ch in &roster {
            prerequisites(content, ch.class, &mut needed);
        }
    }
    for (class, level) in needed {
        if roster
            .iter()
            .any(|ch| ch.class == class && ch.level >= level)
        {
            continue;
        }
        let mut archived = character(content, class, level, level_start(level)?, 0, rng)?;
        archived.alive = false;
        archived.hp = 0;
        archived.items.fill(-1);
        roster.push(archived);
    }
    Ok((roster, vault(p, content, rng)?))
}

pub(super) fn starting_stock(p: &Profile, content: &Content) -> Result<Option<i32>> {
    (p.role == Role::Trader)
        .then(|| {
            named(
                content,
                ["Potion of Speed", "Potion of Defense", "Potion of Life"]
                    [(seed(p.identity) % 3) as usize],
            )
        })
        .transpose()
}

fn prerequisites(content: &Content, class: u16, needed: &mut BTreeMap<u16, i32>) {
    for &(required, level) in &content.classes[&class].requires {
        let prior = needed.entry(required).or_insert(0);
        if *prior < level {
            *prior = level;
            prerequisites(content, required, needed);
        }
    }
}

fn character(
    content: &Content,
    class: u16,
    level: i32,
    exp: i32,
    stars: i32,
    rng: &mut impl Rng,
) -> Result<Character> {
    let definition = content.get(class)?;
    let rules = &content.classes[&class];
    let mut ch = Character::fresh(definition)?;
    ch.lifetime = crate::store::FameLifetime::known_only();
    ch.level = level;
    ch.exp = exp;
    ch.fame = exp / 1000;
    for (i, (min, max)) in rules.growth.iter().enumerate() {
        for _ in 1..level {
            ch.base[i] += rng.gen_range(*min..=*max);
        }
        ch.base[i] = ch.base[i].min(rules.caps[i]);
    }
    let maxed = if level == 20 {
        (stars * 8 / 45).min(8)
    } else {
        0
    };
    for i in MAX_ORDER.into_iter().take(maxed as usize) {
        ch.base[i] = rules.caps[i];
    }
    let veteran = level == 20;
    let tiers = [
        if veteran {
            8 + stars / 13
        } else {
            (level - 1) / 3
        },
        if veteran {
            2 + stars / 16
        } else {
            (level - 1) / 6
        },
        if veteran {
            9 + stars / 13
        } else {
            (level - 1) / 3
        },
        if veteran {
            1 + stars / 16
        } else {
            (level - 10) / 4 + 1
        },
    ];
    for (slot, tier) in tiers.into_iter().enumerate() {
        ch.items[slot] = if slot == 2 && tier == 0 || slot == 3 && !veteran && level < 10 {
            -1
        } else {
            i32::from(equipment(content, definition.slots[slot], tier)?)
        };
    }
    let health = named(content, "Health Potion")?;
    let magic = named(content, "Magic Potion")?;
    ch.items[4..8].copy_from_slice(&[health, health, magic, -1]);
    let effective = ch.effective(content)?;
    (ch.hp, ch.mp) = (effective[0], effective[1]);
    Ok(ch)
}

/// The best tiered item for `slot_type` not above `tier`, matching the 2012 tier caps.
fn equipment(content: &Content, slot_type: i32, tier: i32) -> Result<u16> {
    let cap = match slot_type {
        6 | 7 | 14 => 13,
        1 | 2 | 3 | 8 | 17 => 12,
        _ => 5,
    };
    let tier = tier.clamp(0, cap);
    content
        .objects
        .values()
        .filter(|o| o.slot_type == slot_type && !o.unsupported_equipment)
        .filter_map(|o| o.tier.map(|t| (i32::from(t), o.kind)))
        .filter(|(t, _)| *t <= tier)
        .max()
        .map(|(_, kind)| kind)
        .with_context(|| format!("no tiered equipment for slot type {slot_type}"))
}

fn named(content: &Content, name: &str) -> Result<i32> {
    content
        .objects
        .values()
        .find(|o| o.name == name)
        .map(|o| i32::from(o.kind))
        .with_context(|| format!("{name} missing from objects.xml"))
}

fn vault(p: &Profile, content: &Content, rng: &mut impl Rng) -> Result<Vec<[i32; 8]>> {
    let mut items = Vec::new();
    if p.stars >= 13 {
        let wealth = p.stars as usize * if p.role == Role::Trader { 2 } else { 1 };
        for (name, per) in [
            ("Potion of Life", 12),
            ("Potion of Defense", 5),
            ("Potion of Speed", 4),
        ] {
            items.extend(std::iter::repeat_n(named(content, name)?, wealth / per));
        }
        for name in [
            "Potion of Attack",
            "Potion of Dexterity",
            "Potion of Vitality",
            "Potion of Wisdom",
        ] {
            items.extend(std::iter::repeat_n(
                named(content, name)?,
                rng.gen_range(0..=wealth / 8),
            ));
        }
    }
    let mut chests: Vec<[i32; 8]> = items
        .chunks(8)
        .map(|chunk| {
            let mut chest = [-1; 8];
            chest[..chunk.len()].copy_from_slice(chunk);
            chest
        })
        .collect();
    if chests.is_empty() {
        chests.push([-1; 8]);
    }
    ensure!(
        chests.len() <= 20,
        "{} owns more than the 20 purchasable vault chests",
        p.name
    );
    Ok(chests)
}

#[cfg(test)]
#[test]
fn every_persona_gets_a_valid_life_matching_its_stars() {
    let content = Content::load(std::path::Path::new("data")).unwrap();
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    let mut names = BTreeSet::new();
    for p in PERSONAS.iter() {
        assert!(
            names.insert(p.name.to_lowercase()),
            "duplicate persona {}",
            p.name
        );
        let (roster, vault) = life(p, &content, &mut rng).unwrap();
        let other = life(
            p,
            &content,
            &mut rand::rngs::StdRng::seed_from_u64(seed(p.identity) ^ 0x1235),
        )
        .unwrap()
        .0;
        let history = |characters: &[Character]| {
            characters
                .iter()
                .filter(|ch| ch.fame >= 20)
                .map(|ch| (ch.class, ch.fame))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(
            history(&roster),
            history(&other),
            "{} history depends on current-life randomness",
            p.name
        );
        let presentation = crate::store::AccountPresentation {
            guild: None,
            credits: 0,
            fame: 0,
            best_fame: roster.iter().map(|c| (c.class, c.fame)).collect(),
        };
        assert_eq!(presentation.stars(), p.stars, "{}", p.name);
        for c in &roster {
            c.effective(&content).unwrap();
            if c.alive {
                assert!(
                    c.items[..2].iter().all(|item| *item >= 0),
                    "{} lacks gear",
                    p.name
                );
            } else {
                assert!(c.items.iter().all(|item| *item == -1) && c.hp == 0);
            }
            let levels = roster.iter().map(|ch| (ch.class, ch.level)).collect();
            assert!(content.unlocked(c.class, &levels).unwrap());
        }
        if p.stars >= 26 {
            let levels = roster.iter().map(|ch| (ch.class, ch.level)).collect();
            assert!(
                content
                    .classes
                    .keys()
                    .all(|class| content.unlocked(*class, &levels).unwrap())
            );
        }
        if p.role == Role::Trader && p.stars >= 13 {
            assert_eq!(
                roster[0].level, 1,
                "account rank is not the trader's current level"
            );
            for (name, per) in [
                ("Potion of Life", 12),
                ("Potion of Defense", 5),
                ("Potion of Speed", 4),
            ] {
                let kind = named(&content, name).unwrap();
                let total = vault
                    .iter()
                    .flatten()
                    .chain(roster.iter().flat_map(|ch| &ch.items))
                    .filter(|item| **item == kind)
                    .count();
                assert_eq!(
                    total,
                    p.stars as usize * 2 / per,
                    "{} currency duplicated by starting withdrawal",
                    p.name
                );
            }
        }
        assert!(!vault.is_empty());
    }
}

#[cfg(test)]
#[tokio::test]
async fn persona_snapshot_survives_reopen_and_regeneration_without_duplicate_grants() {
    use crate::store::Store;
    let content = Content::load(std::path::Path::new("data")).unwrap();
    let profile = *PERSONAS.iter().find(|p| p.stars >= 26).unwrap();
    let dir =
        std::env::temp_dir().join(format!("persona-roundtrip-{:016x}", rand::random::<u64>()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("game.db");
    let store = Store::open(&path, &content).unwrap();
    let build = || {
        life(
            &profile,
            &content,
            &mut rand::rngs::StdRng::seed_from_u64(seed(profile.identity)),
        )
        .unwrap()
    };
    let (roster, vault) = build();
    let original = store
        .persona_character(profile.identity.into(), roster, vault, 0, None)
        .await
        .unwrap();
    let account = original.account;
    let before = store.presentation(account).await.unwrap();
    assert_eq!(before.stars(), profile.stars);
    let original_vault = store.vaults(account).await.unwrap();
    let read =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let guid: String = read
        .query_row("SELECT guid FROM accounts WHERE id=?1", [account], |r| {
            r.get(0)
        })
        .unwrap();
    let living = store.list(guid.clone()).await.unwrap().characters;
    let (roster, vault) = build();
    assert_eq!(
        store
            .persona_character(profile.identity.into(), roster, vault, 0, None)
            .await
            .unwrap()
            .id,
        original.id
    );
    for ch in living {
        store.delete_character(account, ch.id).await.unwrap();
    }
    let (roster, vault) = build();
    let next = store
        .persona_character(profile.identity.into(), roster, vault, 0, None)
        .await
        .unwrap();
    assert_ne!(next.id, original.id);
    assert_eq!(
        store.presentation(account).await.unwrap().stars(),
        before.stars()
    );
    assert_eq!(
        serde_json::to_value(store.vaults(account).await.unwrap()).unwrap(),
        serde_json::to_value(original_vault).unwrap()
    );
    let mut dead = next.clone();
    dead.alive = false;
    dead.hp = 0;
    store
        .death(dead, "persona regression".into())
        .await
        .unwrap();
    let raw: String = read
        .query_row(
            "SELECT receipt FROM deaths WHERE character=?1",
            [next.id],
            |r| r.get(0),
        )
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for row in receipt["rows"].as_array().unwrap() {
        assert!(
            ![
                "pacifist",
                "thirsty",
                "mundane",
                "boots_on_the_ground",
                "friend_of_the_cubes"
            ]
            .contains(&row["kind"].as_str().unwrap()),
            "unobserved lifetime must not earn absence bonuses: {row}"
        );
    }
    let collision = PERSONAS.iter().find(|p| p.name != profile.name).unwrap();
    let human = store.list("persona-collision-human".into()).await.unwrap();
    store.rename(human.id, collision.name.into()).await.unwrap();
    let (roster, vault) = life(
        collision,
        &content,
        &mut rand::rngs::StdRng::seed_from_u64(seed(collision.identity)),
    )
    .unwrap();
    assert!(
        store
            .persona_character(collision.identity.into(), roster, vault, 0, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("already belongs")
    );
    assert_eq!(
        store
            .list("persona-collision-human".into())
            .await
            .unwrap()
            .name,
        collision.name
    );
    drop(read);
    drop(store);
    let reopened = Store::open(&path, &content).unwrap();
    assert_eq!(reopened.list(guid).await.unwrap().id, account);
    println!("persona fixture for backup/restore: {}", path.display());
}
