use super::*;

impl Directory {
    pub(crate) fn sync_gold(
        &mut self,
        account: i32,
        gold: &crate::store::gold::Gold,
    ) -> Result<bool> {
        let Some(p) = self
            .worlds
            .values_mut()
            .flat_map(|w| w.players.values_mut())
            .find(|p| p.character.account == account && p.active())
        else {
            return Ok(false);
        };
        p.presentation.credits = gold.balance;
        if gold.rewards.is_empty() {
            return Ok(false);
        }
        let packet = chat::chat_info(&gold.message()?);
        let Some(output) = &p.tx else {
            return Ok(false);
        };
        if output.packets.try_send(packet).is_err() {
            lose(&output.lost, "gold_notice_queue");
            tracing::warn!(
                account,
                "gold notification deferred: connection queue full or closed"
            );
            return Ok(false);
        }
        Ok(true)
    }
}
