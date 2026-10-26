use super::*;
use crate::store::NameChoice;

pub(crate) struct NameReservation {
    player: i32,
    account: i32,
    character: i32,
    revision: i32,
    name: String,
}

impl World {
    pub(crate) fn begin_name_choice(
        &mut self,
        player: i32,
        account: i32,
    ) -> Result<std::result::Result<NameReservation, &'static str>> {
        let trading = self.trade_key(player).is_some();
        let p = self
            .players
            .get_mut(&player)
            .context("name choice player missing")?;
        ensure!(p.character.account == account, "name choice owner mismatch");
        ensure!(
            p.persistence_failure.is_none(),
            "name choice owner quarantined"
        );
        if trading {
            return Ok(Err("Finish your trade before choosing a name."));
        }
        if !p.active() || p.pending || p.character.hp <= 0 {
            return Ok(Err("Cannot choose a name while busy or dead."));
        }
        let plan = NameReservation {
            player,
            account,
            character: p.character.id,
            revision: p.character.revision,
            name: p.character.name.clone(),
        };
        p.pending = true;
        Ok(Ok(plan))
    }

    pub(crate) fn finish_name_choice(
        &mut self,
        plan: NameReservation,
        name: String,
        result: Result<NameChoice>,
    ) -> Result<Option<&'static str>> {
        let published = (|| {
            let p = self
                .players
                .get_mut(&plan.player)
                .context("name choice owner disappeared")?;
            ensure!(
                p.pending
                    && p.persistence_failure.is_none()
                    && p.character.account == plan.account
                    && p.character.id == plan.character
                    && p.character.revision == plan.revision
                    && p.character.name == plan.name,
                "name choice reservation, owner or revision changed"
            );
            match result? {
                NameChoice::Rejected(reason) => {
                    p.pending = false;
                    Ok(Some(reason))
                }
                NameChoice::Committed { balance } => {
                    ensure!(balance >= 0, "invalid committed name choice balance");
                    // Pending defers departure/death snapshots, not damage or their lifecycle flags.
                    p.character.name = name;
                    p.presentation.credits = balance;
                    p.pending = false;
                    Ok(None)
                }
            }
        })();
        if let Err(error) = &published {
            self.poison(
                plan.player,
                format!("name choice publication failed: {error:#}"),
            )?;
        }
        published
    }
}
