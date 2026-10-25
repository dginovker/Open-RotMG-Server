use super::*;

pub(super) struct ChatBudget {
    updated: Instant,
    tokens: f32,
}

pub(crate) const ANNOUNCEMENT: &str = "@ANNOUNCEMENT";

pub(crate) fn chat_private(text: &str) -> Packet {
    text_packet("*Discord*", -1, 0, text)
}

pub(crate) fn chat_error(reason: &str) -> Packet {
    text_packet("*Error*", -1, 0, reason)
}

pub(crate) fn chat_info(text: &str) -> Packet {
    text_packet("", -1, 0, text)
}

pub(super) fn text_packet(name: &str, object: i32, bubble: u8, text: &str) -> Packet {
    // The client draws a class star for every numStars >= 0 (client §_-0At§), so non-player
    // lines use -1; realm-src's Mob/Oryx/Taunt/SendInfo all send -1 for the same reason.
    text_to(name, object, bubble, "", text, -1)
}

fn text_to(name: &str, object: i32, bubble: u8, recipient: &str, text: &str, stars: i32) -> Packet {
    let mut out = Writer::new();
    out.utf(name)
        .i32(object)
        .i32(stars)
        .u8(bubble)
        .utf(recipient)
        .utf(text)
        .utf("");
    Packet {
        id: wire::TEXT,
        body: out.0,
    }
}

pub(crate) fn validate(text: &str) -> Result<()> {
    ensure!(
        !text.trim().is_empty(),
        controller("Chat message is empty.")
    );
    ensure!(
        text.encode_utf16().count() <= 128,
        controller("Chat messages are limited to 128 characters.")
    );
    ensure!(
        !text
            .chars()
            .any(|c| c.is_control()
                || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')),
        controller("Chat message contains control characters.")
    );
    Ok(())
}

impl Player {
    fn can_chat(&self) -> bool {
        self.character.alive
            && self.character.hp > 0
            && !self.death_pending
            && !self.departing
            && self.persistence_failure.is_none()
    }
}

impl World {
    pub(crate) fn dialogue_participants(&self, bot: i32, speaker: i32) -> Result<()> {
        let sender = self.players.get(&bot).context("dialogue bot left world")?;
        let peer = self
            .players
            .get(&speaker)
            .context("dialogue speaker left world")?;
        ensure!(
            sender.controller == Controller::Autonomous
                && sender.can_chat()
                && peer.can_chat()
                && sender.known.contains(&speaker)
                && !peer.account_lists[1].contains(&sender.character.account)
                && sender.pos.distance(peer.pos) <= 12.,
            "dialogue participants unavailable"
        );
        Ok(())
    }
    pub(crate) fn dialogue_notice(&self, speaker: i32, text: &str, failed: bool) -> Result<()> {
        validate(text)?;
        let p = self
            .players
            .get(&speaker)
            .context("dialogue recipient left world")?;
        ensure!(p.can_chat(), "dialogue recipient unavailable");
        let packet = if failed {
            chat_error(text)
        } else {
            chat_info(text)
        };
        send(p, packet.id, packet.body);
        Ok(())
    }
    pub fn account_list_target(&self, id: i32, target: i32) -> Result<i32> {
        let sender = self.players.get(&id).context("player missing")?;
        let target_player = self
            .players
            .get(&target)
            .ok_or_else(|| controller("player not found"))?;
        ensure!(
            sender.persistence_failure.is_none(),
            "account list sender is quarantined"
        );
        ensure!(
            sender.can_chat()
                && target_player.can_chat()
                && id != target
                && sender.known.contains(&target),
            controller("account list target is not an available nearby player")
        );
        Ok(target_player.character.account)
    }

    pub fn sync_account_lists(&mut self, id: i32, lists: Option<AccountLists>) -> Result<()> {
        let player = self.players.get_mut(&id).context("player missing")?;
        if let Some(lists) = lists {
            player.account_lists = lists;
        }
        for (list, accounts) in player.account_lists.iter().enumerate() {
            let mut out = Writer::new();
            out.i32(list as i32).i16(i16::try_from(accounts.len())?);
            for account in accounts {
                out.i32(*account);
            }
            send(player, wire::ACCOUNT_LIST, out.0);
        }
        Ok(())
    }
}

impl Directory {
    pub(crate) fn prepare_actor_pause(&mut self, player: i32, paused: bool) -> Result<()> {
        ensure!(
            self.clock.paused(),
            controller("actor pause requires a paused test session")
        );
        let w = self.resolve(Target::Player(player))?;
        let p = w.players.get(&player).context("pause actor missing")?;
        ensure!(
            p.controller == Controller::Qa
                && p.character.alive
                && !p.pending
                && !p.progress_started
                && !p.death_pending
                && !p.departing,
            controller("pause preparation requires an available direct actor")
        );
        ensure!(
            p.paused != paused,
            controller("actor already has the requested pause state")
        );
        self.chat(player, "/pause".into(), Instant::now())
    }

    pub fn chat_permit(&mut self, id: i32, now: Instant) -> Result<()> {
        let sender = &self.resolve(Target::Player(id))?.players[&id];
        ensure!(
            sender.persistence_failure.is_none(),
            "Dead, departing or quarantined players cannot chat."
        );
        ensure!(
            sender.can_chat(),
            controller("Dead, departing or quarantined players cannot chat.")
        );
        let account = sender.character.account;
        // Account budgets survive world transfers and reconnects without touching durable game saves.
        self.chat_budgets
            .retain(|_, b| now.saturating_duration_since(b.updated).as_secs() < 3);
        ensure!(
            self.chat_budgets.len() < 4096 || self.chat_budgets.contains_key(&account),
            controller("Chat rate registry is full.")
        );
        let budget = self.chat_budgets.entry(account).or_insert(ChatBudget {
            updated: now,
            tokens: 3.,
        });
        let elapsed = now
            .checked_duration_since(budget.updated)
            .context("chat receive clock regressed")?;
        budget.tokens = (budget.tokens + elapsed.as_secs_f32()).min(3.);
        budget.updated = now;
        ensure!(
            budget.tokens >= 1.,
            controller("Chat rate limit: wait one second before sending again.")
        );
        budget.tokens -= 1.;
        Ok(())
    }

    pub fn chat(&mut self, id: i32, text: String, now: Instant) -> Result<()> {
        validate(&text)?;
        let trimmed = text.trim();
        let (command, rest) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        let who = command.eq_ignore_ascii_case("/who");
        let server = command.eq_ignore_ascii_case("/server");
        let teleport = command.eq_ignore_ascii_case("/teleport");
        let trade = command.eq_ignore_ascii_case("/trade");
        let yell = command.eq_ignore_ascii_case("/yell");
        let tell = command.eq_ignore_ascii_case("/tell") || command.eq_ignore_ascii_case("/t");
        let guild_chat =
            command.eq_ignore_ascii_case("/g") || command.eq_ignore_ascii_case("/guild");
        ensure!(
            !trimmed.starts_with('/')
                || tell
                || who
                || server
                || teleport
                || trade
                || yell
                || guild_chat
                || trimmed.eq_ignore_ascii_case("/pause"),
            controller("Unknown chat command.")
        );
        let world = self.resolve(Target::Player(id))?;
        let sender = &world.players[&id];
        ensure!(
            sender.persistence_failure.is_none(),
            "Dead, departing or quarantined players cannot chat."
        );
        ensure!(
            sender.can_chat(),
            controller("Dead, departing or quarantined players cannot chat.")
        );
        if text.trim().eq_ignore_ascii_case("/pause") {
            ensure!(
                sender.paused
                    || !world.entities.values().any(|e| e.hp > 0
                        && world.content.objects[&e.kind].enemy
                        && sender.pos.distance(e.pos) <= 8.),
                controller("Not safe enough to pause!")
            );
            let p = world.players.get_mut(&id).unwrap();
            p.paused = !p.paused;
            p.last_move_at = world.now;
            let text = format!("Game {}.", if p.paused { "paused" } else { "resumed" });
            let reply = text_packet("", -1, 0, &text);
            send(p, reply.id, reply.body);
            return Ok(());
        }
        let account = sender.character.account;
        let name = sender.character.name.clone();
        let guild = sender.presentation.guild.clone();
        let stars = sender.presentation.stars();
        ensure!(
            !tell || crate::store::name_chosen(account, &name),
            controller("Choose a name before using /tell.")
        );
        self.chat_permit(id, now)?;
        if server {
            ensure!(rest.trim().is_empty(), controller("Usage: /server"));
            let world = self.resolve(Target::Player(id))?;
            let packet = chat_info(&format!("Current world: {}", world.name));
            send(&world.players[&id], packet.id, packet.body);
            tracing::info!(player=id, account, world=%world.name, "server location sent");
            return Ok(());
        }
        if trade {
            let name = rest.trim();
            ensure!(
                !name.is_empty() && !name.contains(char::is_whitespace),
                controller("Usage: /trade <player name>")
            );
            return self
                .resolve(Target::Player(id))?
                .request_trade(id, name.into());
        }
        if teleport {
            let name = rest.trim();
            ensure!(
                !name.is_empty() && !name.contains(char::is_whitespace),
                controller("Usage: /teleport <player name>")
            );
            let target = self
                .resolve(Target::Player(id))?
                .players
                .iter()
                .find(|(_, p)| p.character.name.eq_ignore_ascii_case(name))
                .map(|(id, _)| *id)
                .ok_or_else(|| controller("Player not found in this world."))?;
            self.teleport(id, target, now)?;
            return Ok(());
        }
        if who {
            ensure!(rest.trim().is_empty(), controller("Usage: /who"));
            // Period /who lists the current realm, not the cross-world tell/guild audience.
            let world = self.resolve(Target::Player(id))?;
            let names: Vec<_> = world
                .players
                .values()
                .filter(|p| p.can_chat())
                .map(|p| p.character.name.as_str())
                .collect();
            let packet = chat_info(&format!("Players online: {}", names.join(", ")));
            send(&world.players[&id], packet.id, packet.body);
            tracing::info!(player=id, account, world=%world.name, players=names.len(), "who roster sent");
            return Ok(());
        }
        if guild_chat {
            let guild = guild.ok_or_else(|| controller("You are not in a guild."))?;
            let message = rest.trim();
            ensure!(!message.is_empty(), controller("Usage: /g <message>"));
            let packet = text_to(&name, -1, 0, "*Guild*", message, stars);
            let mut recipients = 0;
            for p in self
                .worlds
                .values()
                .flat_map(|w| w.players.values())
                .filter(|p| {
                    p.can_chat()
                        && p.presentation
                            .guild
                            .as_ref()
                            .is_some_and(|g| g.id == guild.id)
                        && !p.account_lists[1].contains(&account)
                })
            {
                send(p, packet.id, packet.body.clone());
                recipients += 1;
            }
            tracing::info!(
                player = id,
                account,
                guild = guild.id,
                recipients,
                bytes = message.len(),
                "guild chat delivered"
            );
            return Ok(());
        }
        if tell {
            let (recipient, message) = rest
                .trim_start()
                .split_once(char::is_whitespace)
                .filter(|(_, message)| !message.trim().is_empty())
                .ok_or_else(|| controller("Usage: /tell <player> <message>"))?;
            let target = self
                .worlds
                .values()
                .flat_map(|w| w.players.values())
                .find(|p| p.can_chat() && p.character.name.eq_ignore_ascii_case(recipient))
                .ok_or_else(|| controller("Player not found."))?;
            ensure!(
                target.character.account != account,
                controller("You cannot tell yourself.")
            );
            let packet = text_to(&name, -1, 0, &target.character.name, message.trim(), 0);
            let ignored = target.account_lists[1].contains(&account);
            if !ignored {
                send(target, packet.id, packet.body.clone());
            }
            let sender = &self.resolve(Target::Player(id))?.players[&id];
            send(sender, packet.id, packet.body);
            tracing::info!(
                player = id,
                account,
                delivered = !ignored,
                bytes = message.len(),
                "private chat processed"
            );
            return Ok(());
        }
        let text = if yell {
            // Period sources establish the Nexus audience, not a cross-world relay.
            ensure!(
                self.resolve(Target::World(-2))?.players.contains_key(&id),
                controller("You can only /yell in the Nexus.")
            );
            let message = rest.trim();
            ensure!(!message.is_empty(), controller("Usage: /yell <message>"));
            message
        } else {
            &text
        };
        let packet = text_to(&name, id, 5, "", text, stars);
        let world = self.resolve(Target::Player(id))?;
        world.ocean_response(id, text)?;
        let pos = world.players[&id].pos;
        let mut recipients = 0;
        for player in world.players.values().filter(|p| {
            p.can_chat()
                && !p.account_lists[1].contains(&account)
                && (yell || pos.distance(p.pos) < 15.)
        }) {
            send(player, packet.id, packet.body.clone());
            recipients += 1;
        }
        tracing::info!(player=id, account, world=%world.name, yell, recipients, bytes=text.len(), "local chat delivered");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stars(packet: Packet) -> i32 {
        let mut r = crate::wire::Reader::new(&packet.body);
        r.utf().unwrap();
        r.i32().unwrap();
        r.i32().unwrap()
    }

    #[test]
    fn non_player_text_does_not_carry_a_class_star() {
        for packet in [
            chat_info("Players online: Test"),
            chat_error("Unknown chat command."),
            chat_private("Discord"),
            text_packet("#Oryx the Mad God", 4, 5, "Uhh. So... sleepy..."),
        ] {
            assert_eq!(stars(packet), -1);
        }
    }
}
