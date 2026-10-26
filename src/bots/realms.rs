use super::*;
use rand::seq::SliceRandom;

pub(super) fn choose(account: i32, realms: &[(i32, usize)], rng: &mut impl Rng) -> Result<i32> {
    let busiest = realms
        .iter()
        .map(|(_, count)| *count)
        .max()
        .context("no open Realm has capacity")?;
    // Attract bots to crowds without eliminating quieter outings: https://github.com/OpenRotMG/Open-RotMG-Server/issues/365
    let selected = realms
        .choose_weighted(rng, |(_, count)| if *count == busiest { 6u32 } else { 1 })
        .context("invalid Realm crowd preference weights")?
        .0;
    tracing::info!(account, choices = ?realms, busiest, selected, "autonomous Realm crowd preference sampled");
    Ok(selected)
}

fn population(name: &str) -> Result<usize> {
    let (_, counts) = name
        .rsplit_once(" (")
        .context("missing population suffix")?;
    let (count, capacity) = counts
        .strip_suffix(')')
        .and_then(|s| s.split_once('/'))
        .context("expected (population/capacity)")?;
    let count = count.parse::<usize>()?;
    let capacity = capacity.parse::<usize>()?;
    ensure!(
        count < capacity,
        "usable Realm entrance is full or has zero capacity"
    );
    Ok(count)
}

impl Brain {
    pub(super) fn realm_portal(&mut self, v: &PlayerView, grid: &Grid) -> Result<Option<i32>> {
        let choices = v
            .objects
            .iter()
            .filter(|p| {
                p.kind == 0x712 && p.usable && !self.blocked.contains_key(&tile(p.position))
            })
            .filter(|p| {
                v.position.distance(p.position) <= 2. || grid.next(v.position, p.position).is_some()
            })
            .map(|p| {
                Ok((
                    p.id,
                    population(&p.name)
                        .with_context(|| format!("Realm portal {} {:?}", p.id, p.name))?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        if choices.is_empty() {
            return Ok(None);
        }
        if let Goal::Portal(id) = self.goal
            && choices.iter().any(|(candidate, _)| *candidate == id)
        {
            return Ok(Some(id));
        }
        choose(v.session.account, &choices, &mut self.rng).map(Some)
    }
}
