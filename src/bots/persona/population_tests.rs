use super::*;
use crate::store::Store;

#[tokio::test]
async fn new_cohorts_mix_current_levels_independently_of_account_rank() {
    let content = Content::load(std::path::Path::new("data")).unwrap();
    let mut classes = BTreeSet::new();
    let mut histograms = Vec::new();
    let mut twinks = 0;
    let mut poor = 0;
    let mut wealthy = 0;
    for cohort in [7, 42, 2012, 12351] {
        let mut rng = rand::rngs::StdRng::seed_from_u64(cohort);
        let mut candidates = PERSONAS.clone();
        candidates.shuffle(&mut rng);
        let dir = std::env::temp_dir().join(format!(
            "persona-cohort-{cohort}-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir(&dir).unwrap();
        let store = Store::open(&dir.join("game.db"), &content).unwrap();
        let mut histogram = [0; 4];
        for p in candidates.iter().take(40) {
            let (roster, vault) = life(
                p,
                &content,
                &mut rand::rngs::StdRng::seed_from_u64(seed(p.identity)),
            )
            .unwrap();
            let ch = store
                .persona_character(
                    p.identity.into(),
                    roster,
                    vault,
                    if p.role == Role::Trader {
                        0
                    } else {
                        rng.r#gen()
                    },
                    starting_stock(p, &content).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                store.presentation(ch.account).await.unwrap().stars(),
                p.stars
            );
            let wealth = store
                .vaults(ch.account)
                .await
                .unwrap()
                .iter()
                .flat_map(|v| &v.items)
                .filter(|item| **item >= 0)
                .count();
            poor += usize::from(wealth == 0);
            wealthy += usize::from(wealth >= 20);
            twinks += usize::from(ch.level < 20 && p.stars >= 26);
            classes.insert(ch.class);
            histogram[match ch.level {
                1..=6 => 0,
                7..=13 => 1,
                14..=19 => 2,
                20 => 3,
                _ => panic!("invalid starting level {}", ch.level),
            }] += 1;
        }
        println!("new cohort {cohort}: levels 1-6/7-13/14-19/20 = {histogram:?}");
        histograms.push(histogram);
    }
    println!(
        "selected classes={}, twinks={twinks}, poor={poor}, wealthy={wealthy}",
        classes.len()
    );
    let level_twenty: usize = histograms.iter().map(|histogram| histogram[3]).sum();
    assert!(
        (40..=64).contains(&level_twenty),
        "L20 admission share: {level_twenty}/160"
    );
    for histogram in histograms {
        assert!(
            (1..=20).contains(&histogram[3]),
            "L20 must remain present without dominating: {histogram:?}"
        );
        assert!(
            histogram[..3].iter().all(|n| *n > 0),
            "missing leveling band: {histogram:?}"
        );
    }
    assert_eq!(classes.len(), 13);
    assert!(twinks > 0 && poor > 0 && wealthy > 0);
}

#[test]
fn requested_names_are_unique_without_inventing_aliases() {
    for name in [
        "X", "Bastard", "Tupac", "Mike", "RayJack", "BRUTUS", "Rob", "Amitp", "TKGP", "Rave",
        "Tellah", "Apollo", "BMJ", "Skoad", "Cronus", "Liam", "Vulcan", "zxcv", "Doc", "AntiLOL",
        "MSellers", "Bluebolt", "Redbolt", "Henez", "Isti", "LSevv", "GHZD", "SLVRDLLR",
    ] {
        assert_eq!(
            PERSONAS.iter().filter(|p| p.name == name).count(),
            1,
            "{name}"
        );
    }
    let mut names = BTreeSet::new();
    assert!(PERSONAS.iter().all(|p| names.insert(p.name.to_lowercase())));
}
