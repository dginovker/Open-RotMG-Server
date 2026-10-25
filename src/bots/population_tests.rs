use super::*;

#[test]
fn population_preserves_existing_bindings_and_has_unique_new_inhabitants() {
    let old = [
        ("Bayleaf", "Triceps"),
        ("Mossy", "Biceps"),
        ("Trellis", "BattleCrus"),
        ("Juniper", "Kadabu"),
        ("Reed", "ExElite"),
        ("Bramble", "Pixl"),
    ];
    assert_eq!(PROFILES.len(), 20);
    for (index, (identity, name)) in old.into_iter().enumerate() {
        let profile = PROFILES[index];
        assert_eq!(
            (profile.identity, profile.name, profile.archetype),
            (identity, name, index)
        );
    }
    assert_eq!(
        PROFILES
            .iter()
            .map(|p| p.identity)
            .collect::<BTreeSet<_>>()
            .len(),
        20
    );
    assert_eq!(
        PROFILES
            .iter()
            .map(|p| p.name)
            .collect::<BTreeSet<_>>()
            .len(),
        20
    );
    assert!(PROFILES.iter().all(|p| p.archetype < ARCHETYPES.len()));
}
