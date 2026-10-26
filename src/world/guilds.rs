use super::*;
use crate::store::Guild;
use std::time::Duration;

pub(super) struct Invitation {
    guild: i32,
    name: String,
    inviter: i32,
    expires: Instant,
}

impl Directory {
    pub fn guild_actor(&self, id: i32) -> Result<(i32, String, Option<i32>)> {
        let world = self
            .worlds
            .get(&self.location(id)?)
            .context("Guild sender world missing.")?;
        let p = &world.players[&id];
        ensure!(
            p.active() && p.character.hp > 0 && !p.pending,
            "Busy, dead or departing players cannot manage guilds."
        );
        Ok((
            p.character.account,
            p.character.name.clone(),
            p.presentation.guild.as_ref().map(|g| g.id),
        ))
    }

    pub fn guild_creation(&self, id: i32) -> Result<()> {
        self.guild_actor(id)?;
        let w = &self.worlds[&self.location(id)?];
        let p = &w.players[&id];
        ensure!(
            w.entities.values().any(|e| e.kind == 0x1df
                && e.published
                && p.known.contains(&e.id)
                && p.pos.distance(e.pos) <= 2.),
            "Approach the Guild Register to create a guild."
        );
        Ok(())
    }

    pub fn guild_invite(&mut self, id: i32, name: &str, now: Instant) -> Result<()> {
        self.guild_actor(id)?;
        self.chat_permit(id, now)?;
        self.guild_invites.retain(|_, i| i.expires > now);
        let world = &self.worlds[&self.location(id)?];
        let p = &world.players[&id];
        let guild = p
            .presentation
            .guild
            .as_ref()
            .context("You are not in a guild.")?;
        ensure!(
            guild.rank >= 20,
            "Only Officers, Leaders and the Founder may invite players."
        );
        let (target, q) = world
            .players
            .iter()
            .find(|(_, q)| q.character.name.eq_ignore_ascii_case(name))
            .context("Player is not nearby.")?;
        ensure!(
            *target != id
                && q.active()
                && q.character.hp > 0
                && p.known.contains(target)
                && p.pos.distance(q.pos) <= 20.,
            "Player is not available nearby."
        );
        ensure!(
            !q.account_lists[1].contains(&p.character.account),
            "Player is not available."
        );
        ensure!(
            q.presentation.guild.is_none(),
            "Player is already in a guild."
        );
        ensure!(
            crate::store::name_chosen(q.character.account, &q.character.name),
            "Player must choose a name first."
        );
        ensure!(
            q.presentation.stars() >= 1,
            "Player must earn their first class star before joining a guild."
        );
        ensure!(
            self.guild_invites.len() < 4096 || self.guild_invites.contains_key(target),
            "Guild invitation registry is full."
        );
        let mut out = Writer::new();
        out.utf(&p.character.name).utf(&guild.name);
        let output = q.tx.as_ref().context("Player is not connected.")?;
        if let Err(error) = output.packets.try_send(Packet {
            id: wire::INVITED_TO_GUILD,
            body: out.0,
        }) {
            lose(&output.lost, "guild_invite_queue");
            bail!("Player cannot receive guild invitations: {error}");
        }
        self.guild_invites.insert(
            *target,
            Invitation {
                guild: guild.id,
                name: guild.name.clone(),
                inviter: p.character.account,
                expires: now + Duration::from_secs(60),
            },
        );
        tracing::info!(
            player = id,
            target,
            guild = guild.id,
            "guild invitation queued"
        );
        Ok(())
    }

    pub fn guild_invitation(&self, id: i32, name: &str, now: Instant) -> Result<(i32, i32)> {
        self.guild_actor(id)?;
        let invite = self
            .guild_invites
            .get(&id)
            .context("You have not been invited to that guild.")?;
        ensure!(
            invite.expires > now && invite.name.eq_ignore_ascii_case(name),
            "Guild invitation expired or is for a different guild."
        );
        Ok((invite.guild, invite.inviter))
    }

    pub fn sync_guild(
        &mut self,
        account: i32,
        presentation: AccountPresentation,
        created: bool,
    ) -> Result<()> {
        let mut revoked = Vec::new();
        for (world_id, w) in &mut self.worlds {
            for (id, p) in &mut w.players {
                if p.character.account == account {
                    // Another member's rank change must not overwrite concurrent shop wallet updates.
                    p.presentation.guild = presentation.guild.clone();
                    if created {
                        p.presentation.fame = presentation.fame;
                    }
                    self.guild_invites.remove(id);
                    if let Some((guild, _)) = w.guild
                        && p.presentation.guild.as_ref().is_none_or(|m| m.id != guild)
                    {
                        revoked.push((*id, *world_id));
                    }
                }
            }
        }
        for (id, origin) in revoked {
            self.return_to_nexus(id, origin, false);
        }
        Ok(())
    }

    pub fn guild_fault(&mut self, account: i32, guild: Option<i32>, reason: String) -> Result<()> {
        for w in self.worlds.values_mut() {
            for (id, p) in &mut w.players {
                if p.character.account == account
                    || guild
                        .is_some_and(|g| p.presentation.guild.as_ref().is_some_and(|m| m.id == g))
                {
                    p.quarantine(*id, reason.clone());
                }
            }
        }
        Ok(())
    }

    pub fn guild_sign_on(&self, id: i32) -> Result<()> {
        let w = &self.worlds[&self.location(id)?];
        let p = &w.players[&id];
        if let Some(guild) = &p.presentation.guild {
            self.guild_notice(guild.id, &format!("{} has come online.", p.character.name));
        }
        Ok(())
    }

    pub fn guild_notice(&self, guild: i32, message: &str) {
        let mut out = Writer::new();
        out.utf("")
            .i32(-1)
            .i32(-1)
            .u8(0)
            .utf("*Guild*")
            .utf(message)
            .utf("");
        for p in self
            .worlds
            .values()
            .flat_map(|w| w.players.values())
            .filter(|p| p.active() && p.presentation.guild.as_ref().is_some_and(|m| m.id == guild))
        {
            send(p, wire::TEXT, out.0.clone());
        }
    }

    pub fn guild_instance(&mut self, guild: Guild) -> Result<i32> {
        ensure!((1..=4).contains(&guild.level), "Invalid Guild Hall level.");
        if let Some((id, _)) = self
            .worlds
            .iter()
            .find(|(_, w)| w.guild.is_some_and(|(id, _)| id == guild.id))
        {
            return Ok(*id);
        }
        self.reserve_worlds(1, &[])?;
        let mut world = self.make(
            &format!("guild-hall-{}", guild.level),
            "Guild Hall",
            true,
            None,
        )?;
        world.guild = Some((guild.id, guild.level));
        world.max_players = self.max_players.min(50);
        let id = self.next_world;
        self.next_world = id
            .checked_add(1)
            .context("World identity space exhausted.")?;
        self.worlds.insert(id, world);
        self.idle.insert(id, self.tick);
        tracing::info!(
            world = id,
            guild = guild.id,
            level = guild.level,
            "private Guild Hall created"
        );
        Ok(id)
    }

    pub fn guild_upgrade_level(&self, id: i32, merchant: i32) -> Result<i32> {
        self.guild_actor(id)?;
        let w = &self.worlds[&self.location(id)?];
        let p = &w.players[&id];
        let (guild, level) = w
            .guild
            .context("Guild upgrades can only be purchased in your Guild Hall.")?;
        ensure!(
            p.presentation.guild.as_ref().is_some_and(|m| m.id == guild),
            "This is not your Guild Hall."
        );
        let e = w
            .entities
            .get(&merchant)
            .context("Guild merchant is missing.")?;
        ensure!(
            e.published && p.known.contains(&merchant) && p.pos.distance(e.pos) <= 2.,
            "Guild merchant is out of reach."
        );
        ensure!(
            (1..=3).contains(&level) && e.kind == 0x735 + level as u16,
            "This merchant cannot upgrade the hall."
        );
        Ok(level)
    }
}
