use super::*;
use rand::seq::IteratorRandom;
use std::time::{Duration, Instant};

mod incoming;
use incoming::Incoming;
#[cfg(test)]
mod incoming_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("market wall clock predates epoch")
        .as_secs()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Goods {
    pub kind: i32,
    pub quantity: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Terms {
    pub version: u64,
    pub accounts: [i32; 2],
    pub names: [String; 2],
    pub goods: [Goods; 2],
    pub meeting: [i32; 3],
    pub expires: u64,
    pub batch: u8,
}
impl Terms {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.version > 0
                && self.accounts.iter().all(|a| *a > 0)
                && self.accounts[0] != self.accounts[1]
                && self.names.iter().all(|n| !n.is_empty() && n.len() <= 16)
                && self.names[0] != self.names[1]
                && (1..=8).contains(&self.batch)
                && self
                    .goods
                    .iter()
                    .all(|g| (0..=65535).contains(&g.kind) && (1..=16).contains(&g.quantity))
                && self.goods[0].kind != self.goods[1].kind
                && self.meeting[0] == -2
                && self.meeting[1..].iter().all(|n| (0..=65535).contains(n)),
            "invalid market terms"
        );
        Ok(())
    }
    fn body(&self) -> String {
        format!(
            "{:x} {}x{}/{}x{} @{},{},{} !{} b{}",
            self.version,
            self.goods[0].kind,
            self.goods[0].quantity,
            self.goods[1].kind,
            self.goods[1].quantity,
            self.meeting[0],
            self.meeting[1],
            self.meeting[2],
            self.expires,
            self.batch
        )
    }
    pub(in crate::bots) fn text(&self, recipient: &str, verb: &str) -> String {
        format!("{recipient}, {verb} {} p0 r0", self.body())
    }
    pub(crate) fn delivery(&self, ordinal: u8) -> Result<Delivery> {
        self.validate()?;
        let quantities = self.goods.each_ref().map(|g| {
            g.quantity
                .saturating_sub(ordinal.saturating_mul(self.batch))
                .min(self.batch)
        });
        ensure!(quantities != [0, 0], "market delivery already complete");
        Ok(Delivery {
            terms: self.clone(),
            ordinal,
            quantities,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Delivery {
    pub terms: Terms,
    pub ordinal: u8,
    pub quantities: [u8; 2],
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Contact {
    account: i32,
    name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Bargain {
    pub terms: Terms,
    pub own_consent: bool,
    pub agreed: bool,
    pub peer_consent: Option<[i32; 3]>,
    pub confirmed: u8,
    #[serde(default)]
    pub consent_revision: u8,
    pub rounds: u8,
    pub resolution: Option<String>,
}
impl Bargain {
    fn body(&self) -> String {
        // Paid ordinal and revocation revision stop a prior assent authorizing remaining deliveries.
        format!(
            "{} p{} r{}",
            self.terms.body(),
            self.confirmed,
            self.consent_revision
        )
    }
    pub(in crate::bots) fn text(&self, recipient: &str, verb: &str) -> String {
        format!("{recipient}, {verb} {}", self.body())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct MarketSession {
    anchor: TilePoint,
    until: u64,
    next_move: u64,
    destination: Option<TilePoint>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Market {
    #[serde(skip)]
    session: Option<MarketSession>,
    #[serde(skip)]
    session_used: bool,
    #[serde(skip)]
    pub(in crate::bots) name: String,
    #[serde(skip)]
    pub(in crate::bots) guild: bool,
    #[serde(skip)]
    pub(in crate::bots) next_message_ms: u64,
    #[serde(skip)]
    next_request_ms: u64,
    #[serde(skip)]
    announced: bool,
    contacts: Vec<Contact>,
    pub bargain: Option<Bargain>,
    history: VecDeque<Bargain>,
    vault: Option<Vec<i32>>,
    next_ad: u64,
    #[serde(skip)]
    pub(in crate::bots) outgoing: Option<Message>,
    #[serde(skip)]
    pub(in crate::bots) trade: Option<SessionTrade>,
    #[serde(skip)]
    pub(in crate::bots) incoming: Option<Incoming>,
    #[serde(skip)]
    pub(in crate::bots) revoke_trade: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::bots) enum Channel {
    Local,
    Tell(String),
    Guild,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::bots) struct Message {
    pub text: String,
    pub channel: Channel,
    pub consent: bool,
}
impl Message {
    pub(in crate::bots) fn optional_public(&self) -> Option<&str> {
        (!self.consent && self.text.starts_with("sell ") && self.channel == Channel::Local)
            .then_some(self.text.as_str())
    }
    pub(in crate::bots) fn chat(&self) -> String {
        match &self.channel {
            Channel::Local => self.text.clone(),
            Channel::Tell(n) => format!("/tell {n} {}", self.text),
            Channel::Guild => format!("/g {}", self.text),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::bots) struct SessionTrade {
    pub binding: [i32; 3],
    pub peer: String,
    pub items: [Vec<i32>; 2],
    pub tradeable: [bool; 12],
    pub offers: [[bool; 12]; 2],
    pub ready: u64,
    pub ready_at: Instant,
    pub changed: bool,
}
impl Market {
    pub(crate) fn validate(&self, account: i32) -> Result<()> {
        ensure!(
            self.contacts.len() <= 32
                && self.history.len() <= 8
                && self
                    .vault
                    .as_ref()
                    .is_none_or(|v| v.len() == 8 && v.iter().all(|k| (-1..=65535).contains(k))),
            "invalid market memory bounds"
        );
        for b in self.bargain.iter().chain(self.history.iter()) {
            b.terms.validate()?;
            ensure!(
                b.terms.accounts.contains(&account)
                    && b.confirmed <= 16
                    && b.consent_revision <= 4
                    && b.resolution.as_ref().is_none_or(|r| r.len() <= 128),
                "invalid account-bound bargain"
            );
        }
        Ok(())
    }
    pub(in crate::bots) fn bind(&mut self) {
        self.session = None;
        self.session_used = false;
        self.incoming = None;
        self.outgoing = None;
        self.trade = None;
        self.announced = false;
        self.revoke_trade = false;
        self.next_request_ms = 0;
        if let Some(b) = &mut self.bargain {
            b.own_consent = false;
            b.peer_consent = None;
        }
    }
    pub(in crate::bots) fn request_due(&mut self, elapsed_ms: u64) -> bool {
        if elapsed_ms < self.next_request_ms {
            return false;
        }
        self.next_request_ms = elapsed_ms + 20_000;
        true
    }
    pub(in crate::bots) fn observe(&mut self, v: &PlayerView) {
        // Publication runs before decide can retire an expired session.
        if self
            .session
            .as_ref()
            .is_some_and(|s| v.elapsed_ms >= s.until)
            && self
                .outgoing
                .as_ref()
                .is_some_and(|m| m.text.starts_with("sell "))
        {
            self.outgoing = None;
        }
        if v.map.name != "Nexus" {
            self.end_session(v, "left_nexus");
            self.session_used = false;
        }
        for p in v
            .objects
            .iter()
            .filter(|p| p.hp > 0 && !p.equipped.is_empty())
        {
            if let Some(account) = p.account {
                self.contacts
                    .retain(|c| c.account != account && c.name != p.name);
                self.contacts.push(Contact {
                    account,
                    name: p.name.clone(),
                });
                if self.contacts.len() > 32 {
                    self.contacts.remove(0);
                }
            }
        }
        if v.map.name == "Vault"
            && let Some(chest) = v.objects.iter().find(|p| p.kind == 0x504)
        {
            if self.vault.as_ref() != Some(&chest.contents) {
                tracing::info!(account=v.session.account,revision=v.character.inventory_revision,items=?v.character.items,vault=?chest.contents,"market own private stock observed");
            }
            self.vault = Some(chest.contents.clone());
        }
    }
    fn count(&self, v: &PlayerView, kind: i32) -> usize {
        v.character
            .items
            .iter()
            .skip(4)
            .chain(self.vault.iter().flatten())
            .filter(|k| **k == kind)
            .count()
    }
    fn wanted(&self, v: &PlayerView, c: &Content, future: Option<u16>, kind: i32) -> Result<u8> {
        let item = c.get(u16::try_from(kind)?)?;
        if item.soulbound || item.unsupported_equipment {
            return Ok(0);
        }
        let desired = if let Some(index) = progression::potion_index(c, kind)? {
            (c.classes[&v.character.class].caps[index] - v.character.base_stats[index]).clamp(0, 16)
                as usize
        } else if item.activate == "Heal" || (item.activate == "Magic" && v.character.items[1] >= 0)
        {
            2
        } else if progression::priority(v, c, kind, false, future)? >= 60 {
            1
        } else {
            0
        };
        Ok(desired.saturating_sub(self.count(v, kind)).min(16) as u8)
    }
    fn spare(&self, v: &PlayerView, c: &Content, future: Option<u16>, kind: i32) -> Result<u8> {
        let item = c.get(u16::try_from(kind)?)?;
        if item.soulbound || item.unsupported_equipment || item.class != "Equipment" {
            return Ok(0);
        }
        let count = self.count(v, kind);
        let keep = if let Some(index) = progression::potion_index(c, kind)? {
            (c.classes[&v.character.class].caps[index] - v.character.base_stats[index]).clamp(0, 16)
                as usize
        } else if item.activate == "Heal" || (item.activate == "Magic" && v.character.items[1] >= 0)
        {
            2
        } else if progression::priority(v, c, kind, true, future)? >= 60 {
            1
        } else {
            0
        };
        Ok(count.saturating_sub(keep).min(16) as u8)
    }
    fn stock(&self, v: &PlayerView, c: &Content, future: Option<u16>) -> Result<Vec<Goods>> {
        let mut goods = Vec::new();
        for kind in v
            .character
            .items
            .iter()
            .skip(4)
            .chain(self.vault.iter().flatten())
            .filter(|k| **k >= 0)
            .copied()
            .collect::<BTreeSet<_>>()
        {
            let quantity = self.spare(v, c, future, kind)?;
            if quantity > 0 {
                goods.push(Goods { kind, quantity });
            }
        }
        Ok(goods)
    }
    fn want(&self, v: &PlayerView, c: &Content, future: Option<u16>) -> Result<Option<i32>> {
        // Supply deficits precede speculative loadouts, without assigning prices to either.
        for kind in [0xa22, 0xa23, 0xa1f, 0xa20, 0xa21, 0xa34, 0xa35, 0xa4c]
            .into_iter()
            .chain(c.objects.keys().copied().map(i32::from))
        {
            if c.objects[&(kind as u16)].class == "Equipment"
                && self.wanted(v, c, future, kind)? > 0
            {
                return Ok(Some(kind));
            }
        }
        Ok(None)
    }
    fn suitable(
        &self,
        v: &PlayerView,
        c: &Content,
        future: Option<u16>,
        t: &Terms,
    ) -> Result<bool> {
        let side = t
            .accounts
            .iter()
            .position(|a| *a == v.session.account)
            .context("foreign market terms")?;
        Ok(
            self.spare(v, c, future, t.goods[side].kind)? >= t.goods[side].quantity
                && self.wanted(v, c, future, t.goods[1 - side].kind)? >= t.goods[1 - side].quantity,
        )
    }
    fn queue(&mut self, text: String, channel: Channel, consent: bool) {
        self.outgoing = Some(Message {
            text,
            channel,
            consent,
        });
    }
    pub(in crate::bots) fn finish(&mut self, reason: &str) {
        self.outgoing = None;
        if let Some(b) = &mut self.bargain
            && b.resolution.is_none()
        {
            b.resolution = Some(reason.into());
            b.own_consent = false;
            b.peer_consent = None;
            self.revoke_trade = reason != "settled_all_installments_transaction_identified";
            tracing::info!(terms=%serde_json::to_string(&b).expect("bargain serialization"),reason,"market remaining balance resolved explicitly");
        }
        self.trade = None;
    }
    fn replace(&mut self, t: Terms) {
        self.next_request_ms = 0;
        let rounds = self.bargain.as_ref().map_or(1, |b| {
            if b.terms.accounts.iter().all(|a| t.accounts.contains(a)) {
                b.rounds.saturating_add(1)
            } else {
                1
            }
        });
        if let Some(mut old) = self.bargain.take() {
            self.revoke_trade = true;
            self.trade = None;
            old.own_consent = false;
            old.peer_consent = None;
            if old.resolution.is_none() {
                old.resolution = Some("counteroffer_invalidated_previous_consents".into());
            }
            self.history.push_back(old);
            if self.history.len() > 8 {
                self.history.pop_front();
            }
        }
        self.bargain = Some(Bargain {
            terms: t,
            own_consent: false,
            agreed: false,
            peer_consent: None,
            confirmed: 0,
            consent_revision: 0,
            rounds,
            resolution: None,
        });
    }
    pub(in crate::bots) fn receive(
        &mut self,
        packet: &Packet,
        v: &PlayerView,
        c: &Content,
        future: Option<u16>,
    ) -> Result<bool> {
        self.receive_observed(packet, v, &v.objects, c, future)
    }
    pub(in crate::bots) fn receive_observed(
        &mut self,
        packet: &Packet,
        v: &PlayerView,
        observed_objects: &[crate::world::view::VisibleObject],
        c: &Content,
        future: Option<u16>,
    ) -> Result<bool> {
        if packet.id != wire::TEXT {
            return self.trade_packet(packet, v);
        }
        let mut r = Reader::new(&packet.body);
        let name = r.utf()?;
        let speaker = r.i32()?;
        r.i32()?;
        r.u8()?;
        let recipient = r.utf()?;
        let text = r.utf()?;
        r.utf()?;
        r.done()?;
        if speaker == v.session.player || name == self.name.as_str() {
            return Ok(false);
        }
        let channel = if recipient.is_empty() {
            Channel::Local
        } else if recipient == "*Guild*" {
            Channel::Guild
        } else if recipient == self.name.as_str() {
            Channel::Tell(name.clone())
        } else {
            return Ok(false);
        };
        if speaker < 0 && matches!(channel, Channel::Local) {
            return Ok(false);
        }
        let peer = observed_objects.iter().find(|p| {
            p.hp > 0
                && !p.equipped.is_empty()
                && p.name == name
                && (speaker == -1 || p.id == speaker)
        });
        let observed = peer.and_then(|p| p.account.map(|a| (a, [v.session.world, p.id, 0])));
        let remembered = self
            .contacts
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.account);
        let addressed = format!("{}, ", self.name.as_str());
        let body = if let Some(b) = text.strip_prefix(&addressed) {
            b
        } else if text.starts_with("sell ") {
            text.as_str()
        } else {
            return Ok(false);
        };
        let active = self
            .bargain
            .as_ref()
            .is_some_and(|b| b.resolution.is_none());
        tracing::info!(
            account = v.session.account,
            speaker,
            name,
            recipient,
            ?observed,
            remembered,
            text,
            "market message received by real audience"
        );
        if let Some(ad) = body.strip_prefix("sell ") {
            if active || self.outgoing.is_some() {
                return Ok(true);
            }
            let parsed = parse_ad(ad, c);
            let Some((goods, want)) = parsed else {
                return Ok(true);
            };
            // An unknown private name can inform assessment but cannot bind an account or meeting.
            if !(0..=65535).contains(&want)
                || !c.objects.contains_key(&(goods.kind as u16))
                || !c.objects.contains_key(&(want as u16))
            {
                tracing::warn!(
                    account = v.session.account,
                    name,
                    ?goods,
                    want,
                    "market advertisement contains an unknown item; no terms accepted"
                );
                return Ok(true);
            }
            let desired = self.wanted(v, c, future, goods.kind)?;
            let available = self.spare(v, c, future, want)?;
            if let Some((account, _)) = observed {
                let payment = if available > 0 {
                    Some(Goods {
                        kind: want,
                        quantity: available,
                    })
                } else {
                    self.stock(v, c, future)?
                        .into_iter()
                        .find(|g| g.kind != goods.kind)
                };
                if desired > 0
                    && let Some(payment) = payment
                {
                    let n = desired.min(payment.quantity).min(goods.quantity);
                    let t = Terms {
                        version: rand::random::<u64>().max(1),
                        accounts: [v.session.account, account],
                        names: [self.name.as_str().into(), name.clone()],
                        goods: [
                            Goods {
                                kind: payment.kind,
                                quantity: n,
                            },
                            Goods {
                                kind: goods.kind,
                                quantity: n,
                            },
                        ],
                        meeting: [-2, tile(v.position).0 as i32, tile(v.position).1 as i32],
                        expires: now() + 180,
                        batch: capacity(v, payment.kind),
                    };
                    let text = t.text(&name, "offer");
                    self.replace(t);
                    let channel = if channel == Channel::Local {
                        Channel::Tell(name.clone())
                    } else {
                        channel
                    };
                    self.queue(text, channel, false);
                } else {
                    self.queue(
                        format!("{name}, no deal: those goods do not meet my needs or stock."),
                        channel,
                        false,
                    );
                }
            }
            return Ok(true);
        }
        if body.starts_with("no deal") || body.starts_with("abandon ") {
            if let Some(b) = &self.bargain
                && observed.is_some_and(|(a, _)| b.terms.accounts.contains(&a))
            {
                self.finish("peer_refused_or_departed_remaining_balance_unenforced");
            }
            return Ok(true);
        }
        let (verb, raw) = match body.split_once(' ') {
            Some((verb, raw)) if matches!(verb, "offer" | "counter" | "agree") => (verb, raw),
            _ => return Ok(false),
        };
        let Some((account, binding)) = observed else {
            return Ok(true);
        };
        let parsed = parse_terms(raw);
        let Some((version, goods, meeting, expires, batch, paid, consent_revision)) = parsed else {
            return Ok(true);
        };
        if expires <= now() || expires > now() + 600 {
            return Ok(true);
        }
        if verb == "offer" || verb == "counter" {
            let t = Terms {
                version,
                accounts: [account, v.session.account],
                names: [name.clone(), self.name.as_str().into()],
                goods,
                meeting,
                expires,
                batch,
            };
            if t.validate().is_err()
                || t.goods
                    .iter()
                    .any(|g| !c.objects.contains_key(&(g.kind as u16)))
            {
                tracing::warn!(
                    account = v.session.account,
                    name,
                    "market offer has invalid terms or unknown goods; no consent"
                );
                return Ok(true);
            }
            if active
                && self
                    .bargain
                    .as_ref()
                    .is_some_and(|b| !b.terms.accounts.contains(&account))
            {
                return Ok(true);
            }
            if let Some(b) = &mut self.bargain
                && active
                && b.confirmed > 0
            {
                let same_terms = version == b.terms.version && t.body() == b.terms.body();
                if same_terms && (paid != b.confirmed || consent_revision < b.consent_revision) {
                    return Ok(true);
                }
                let reaffirm = same_terms
                    && (consent_revision > b.consent_revision
                        || (!b.own_consent && b.peer_consent.is_none()));
                if !reaffirm || consent_revision > b.consent_revision {
                    if b.consent_revision >= 4 || consent_revision > 4 {
                        self.finish("renegotiation_limit_remaining_balance_unenforced");
                        self.queue(
                            format!(
                                "{name}, no deal: original remaining balance abandoned, not paid."
                            ),
                            channel,
                            false,
                        );
                        return Ok(true);
                    }
                    b.consent_revision = if reaffirm {
                        consent_revision
                    } else {
                        b.consent_revision + 1
                    };
                    b.own_consent = false;
                    b.peer_consent = None;
                    self.revoke_trade = true;
                    self.next_request_ms = 0;
                    self.trade = None;
                    tracing::info!(terms=?b,"market changed terms revoked consent; original paid terms and remaining balance retained");
                }
                let text = b.text(&name, if reaffirm { "agree" } else { "counter" });
                self.queue(text, channel, reaffirm);
                return Ok(true);
            }
            if paid != 0 || consent_revision != 0 {
                return Ok(true);
            }
            if verb == "offer"
                && active
                && self.bargain.as_ref().is_some_and(|b| {
                    !b.agreed
                        && !b.own_consent
                        && b.peer_consent.is_none()
                        && b.terms.accounts[0] == v.session.account
                        && v.session.account < account
                })
            {
                return Ok(true);
            }
            if self
                .bargain
                .as_ref()
                .is_some_and(|b| b.terms.accounts.contains(&account) && b.rounds >= 4)
            {
                self.finish("negotiation_limit_no_agreement_remaining_balance_unenforced");
                self.queue(
                    format!("{name}, no deal: incompatible wants."),
                    channel,
                    false,
                );
                return Ok(true);
            }
            if self.suitable(v, c, future, &t)? {
                let text = t.text(&name, "agree");
                self.replace(t);
                self.queue(text, channel, true);
            } else {
                self.finish("unsuitable_terms_refused_no_payment_due");
                let [offered, asked] = &t.goods;
                let available = self.spare(v, c, future, asked.kind)?.min(asked.quantity);
                let desired = self.wanted(v, c, future, offered.kind)?;
                // Smaller trades must preserve the offered exchange rate, not invent another want.
                let quantities = (1..=available).rev().find_map(|pay| {
                    let receive = u16::from(pay) * u16::from(offered.quantity);
                    let denominator = u16::from(asked.quantity);
                    (receive % denominator == 0 && receive / denominator <= u16::from(desired))
                        .then_some((pay, (receive / denominator) as u8))
                });
                if let Some((pay, receive)) = quantities {
                    let counter = Terms {
                        version: rand::random::<u64>().max(1),
                        accounts: [v.session.account, account],
                        names: [self.name.as_str().into(), name.clone()],
                        goods: [
                            Goods {
                                kind: asked.kind,
                                quantity: pay,
                            },
                            Goods {
                                kind: offered.kind,
                                quantity: receive,
                            },
                        ],
                        meeting,
                        expires,
                        batch: capacity(v, asked.kind),
                    };
                    tracing::info!(account=v.session.account,requested=?t.goods,counter=?counter.goods,"market counteroffer reduced to actual needs and spare stock");
                    let text = counter.text(&name, "counter");
                    self.replace(counter);
                    self.queue(text, channel, false);
                } else {
                    self.queue(
                        format!("{name}, no deal: unsuitable goods or insufficient stock."),
                        channel,
                        false,
                    );
                }
            }
        } else if let Some(b) = &mut self.bargain
            && b.resolution.is_none()
            && b.terms.accounts.contains(&account)
            && b.body() == raw
        {
            b.peer_consent = Some(binding);
            if b.own_consent {
                b.agreed = true;
            }
            if !b.own_consent {
                let text = b.text(&name, "agree");
                self.queue(text, channel, true);
            }
        }
        Ok(true)
    }
    pub(in crate::bots) fn published(&mut self, m: &Message) {
        self.announced = true;
        if m.consent
            && let Some(b) = &mut self.bargain
            && m.text.ends_with(&b.body())
        {
            b.own_consent = true;
            if b.peer_consent.is_some() {
                b.agreed = true;
            }
        }
        self.outgoing = None;
    }
    pub(in crate::bots) fn recover(&mut self, receipts: Vec<Delivery>) -> Result<()> {
        let Some(b) = &mut self.bargain else {
            return Ok(());
        };
        for (ordinal, r) in receipts.iter().enumerate() {
            ensure!(
                r.terms == b.terms
                    && r.ordinal as usize == ordinal
                    && *r == b.terms.delivery(r.ordinal)?,
                "stale, duplicate or foreign settlement receipt"
            );
        }
        ensure!(
            receipts.len() >= b.confirmed as usize,
            "committed market receipts regressed"
        );
        if b.confirmed as usize != receipts.len() {
            self.trade = None;
        }
        b.confirmed = u8::try_from(receipts.len())?;
        if b.terms.delivery(b.confirmed).is_err()
            && b.resolution.as_deref() != Some("settled_all_installments_transaction_identified")
        {
            // A death before the mind save cannot undo a committed final installment.
            b.resolution = None;
            self.finish("settled_all_installments_transaction_identified");
        }
        Ok(())
    }
    fn end_session(&mut self, v: &PlayerView, reason: &str) {
        if let Some(s) = self.session.take() {
            tracing::info!(account=v.session.account,elapsed_ms=v.elapsed_ms,anchor=?s.anchor,until=s.until,reason,"market intentional session ended");
        }
        if self
            .outgoing
            .as_ref()
            .is_some_and(|m| m.text.starts_with("sell "))
        {
            self.outgoing = None;
        }
    }
    pub(in crate::bots) fn decide(
        &mut self,
        v: &PlayerView,
        c: &Content,
        future: Option<u16>,
        grid: &Grid,
        rng: &mut impl Rng,
    ) -> Result<Option<Decision>> {
        self.observe(v);
        if !super::settled(v) {
            return Ok(None);
        }
        if let Some(b) = self.bargain.clone().filter(|b| b.resolution.is_none()) {
            let side = b
                .terms
                .accounts
                .iter()
                .position(|a| *a == v.session.account)
                .context("foreign bargain")?;
            let peer = 1 - side;
            let delivery = b.terms.delivery(b.confirmed)?;
            let remaining = b.terms.goods[side]
                .quantity
                .saturating_sub(b.confirmed * b.terms.batch);
            let failure = if now() >= b.terms.expires {
                Some("expired_remaining_balance_unenforced")
            } else if self.count(v, b.terms.goods[side].kind) < remaining as usize {
                Some("promised_stock_changed_remaining_balance_unenforced")
            } else {
                None
            };
            if let Some(reason) = failure {
                self.finish(reason);
                self.queue(
                    format!(
                        "{}, abandon {:x}: {reason}",
                        b.terms.names[peer], b.terms.version
                    ),
                    Channel::Tell(b.terms.names[peer].clone()),
                    false,
                );
                return Ok(None);
            }
            let mut d = Decision {
                goal: Goal::Wait,
                movement: None,
                aim: None,
                item: None,
                journey: None,
                chat: None,
            };
            if v.map.name != "Nexus" && v.map.name != "Vault" {
                d.journey = Some(Journey::Nexus);
                return Ok(Some(d));
            }
            if !b.agreed {
                if !self.announced && self.outgoing.is_none() {
                    let verb = if side == 0 { "offer" } else { "agree" };
                    self.queue(
                        b.text(&b.terms.names[peer], verb),
                        Channel::Tell(b.terms.names[peer].clone()),
                        side != 0,
                    );
                }
                return Ok(Some(d));
            }
            let own = delivery.quantities[side] as usize;
            let needed = delivery.quantities[peer] as usize;
            let carried = v
                .character
                .items
                .iter()
                .skip(4)
                .filter(|k| **k == b.terms.goods[side].kind)
                .count();
            let free = v.character.items.iter().skip(4).filter(|k| **k < 0).count();
            let bank = carried < own || free + own < needed;
            if v.map.name == "Vault" {
                let Some(chest) = v.objects.iter().find(|o| o.kind == 0x504) else {
                    return Ok(Some(d));
                };
                let transfer = if carried < own
                    && let Some(from) = chest
                        .contents
                        .iter()
                        .position(|k| *k == b.terms.goods[side].kind)
                    && let Some(to) = v
                        .character
                        .items
                        .iter()
                        .enumerate()
                        .skip(4)
                        .find(|(_, k)| **k < 0)
                        .map(|(i, _)| i)
                {
                    Some(Action::Swap {
                        from: Slot {
                            object: chest.id,
                            slot: from,
                            kind: chest.contents[from],
                        },
                        to: super::own(v, to),
                    })
                } else if bank
                    && let Some(to) = chest.contents.iter().position(|k| *k < 0)
                    && let Some(from) = v
                        .character
                        .items
                        .iter()
                        .enumerate()
                        .skip(4)
                        .find(|(_, k)| **k >= 0 && **k != b.terms.goods[side].kind)
                        .map(|(i, _)| i)
                {
                    Some(Action::Swap {
                        from: super::own(v, from),
                        to: Slot {
                            object: chest.id,
                            slot: to,
                            kind: -1,
                        },
                    })
                } else {
                    None
                };
                if let Some(action) = transfer {
                    d.goal = Goal::Bank(chest.id);
                    if v.position.distance(chest.position) < 1.4 {
                        d.item = Some(action)
                    } else {
                        d.movement = grid.towards(v.position, chest.position)
                    }
                } else if bank {
                    self.finish("vault_capacity_or_stock_unavailable_remaining_balance_unenforced");
                } else {
                    d.journey = Some(Journey::Nexus)
                }
                return Ok(Some(d));
            }
            if bank {
                if let Some(p) = v.objects.iter().find(|p| p.kind == 0x720 && p.usable) {
                    d.goal = Goal::Portal(p.id);
                    if v.position.distance(p.position) < 1.4 {
                        d.journey = Some(Journey::Portal(p.id))
                    } else {
                        d.movement = grid.towards(v.position, p.position)
                    }
                }
                return Ok(Some(d));
            }
            let meeting = Pos {
                x: b.terms.meeting[1] as f32 + 0.5,
                y: b.terms.meeting[2] as f32 + 0.5,
            };
            if v.position.distance(meeting) >= 1. {
                d.movement = grid.towards(v.position, meeting);
                return Ok(Some(d));
            }
            if let Some(p) = v.objects.iter().find(|p| {
                p.account == Some(b.terms.accounts[peer]) && p.name == b.terms.names[peer]
            }) {
                if b.peer_consent
                    .is_some_and(|s| s[0] != v.session.world || s[1] != p.id)
                {
                    let b = self.bargain.as_mut().unwrap();
                    b.peer_consent = None;
                    b.own_consent = false;
                }
                if !b.own_consent && self.outgoing.is_none() {
                    self.queue(
                        b.text(&b.terms.names[peer], "agree"),
                        Channel::Tell(b.terms.names[peer].clone()),
                        true,
                    );
                }
            }
            return Ok(Some(d));
        }
        if v.map.name != "Nexus" {
            return Ok(None);
        }
        let mut d = Decision {
            goal: Goal::Wait,
            movement: None,
            aim: None,
            item: None,
            journey: None,
            chat: None,
        };
        if self.trade.is_some() {
            return Ok(Some(d));
        }
        if self.session.is_none() && self.session_used {
            return Ok(None);
        }
        if self
            .session
            .as_ref()
            .is_some_and(|s| v.elapsed_ms >= s.until)
        {
            self.end_session(v, "elapsed");
            return Ok(None);
        }
        let Some(goods) = self.stock(v, c, future)?.into_iter().next() else {
            self.end_session(v, "no_spare_stock");
            return Ok(None);
        };
        let Some(want) = self.want(v, c, future)? else {
            self.end_session(v, "needs_satisfied");
            return Ok(None);
        };
        if self.session.is_none() {
            self.session_used = true;
            let admitted = rng.gen_ratio(1, 5);
            tracing::info!(
                account = v.session.account,
                admitted,
                probability = 0.2,
                reason = if admitted {
                    "visit_admitted"
                } else {
                    "visit_declined"
                },
                "market autonomous session admission"
            );
            if !admitted {
                return Ok(None);
            }
            let duration_ms = rng.gen_range(60..=1800) * 1000u64;
            self.session = Some(MarketSession {
                anchor: tile(v.position),
                until: v
                    .elapsed_ms
                    .checked_add(duration_ms)
                    .context("market session deadline overflow")?,
                next_move: v.elapsed_ms + 10_000,
                destination: None,
            });
            tracing::info!(account=v.session.account,elapsed_ms=v.elapsed_ms,duration_ms,session=?self.session,?goods,want,"market intentional session started");
        }
        let s = self
            .session
            .as_mut()
            .expect("market session just initialized");
        if v.position.distance(center(s.anchor)) > 2. {
            self.end_session(v, "displaced_from_local_area");
            return Ok(None);
        }
        if v.elapsed_ms >= s.next_move {
            s.next_move = v.elapsed_ms + 10_000;
            let (x, y) = tile(v.position);
            // The half-tile margin leaves room for ordinary controller reaction steps.
            s.destination = [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
                .into_iter()
                .filter(|p| {
                    center(*p).distance(center(s.anchor)) <= 1.5
                        && grid.segment(v.position, center(*p))
                })
                .choose(rng);
            tracing::info!(account=v.session.account,anchor=?s.anchor,target=?s.destination,"market intentional session local step");
        }
        d.movement = s
            .destination
            .map(center)
            .filter(|p| v.position.distance(*p) > 0.2 && grid.segment(v.position, *p));
        if now() >= self.next_ad && self.outgoing.is_none() {
            // Chat renders text literally, not item types: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/kabam/rotmg/messaging/impl/GameServerConnection.as#L1635-L1663
            let ad = format!(
                "{} x{}; want {}",
                c.get(u16::try_from(goods.kind)?)?.name,
                goods.quantity,
                c.get(u16::try_from(want)?)?.name
            );
            ensure!(
                parse_ad(&ad, c) == Some((goods.clone(), want)),
                "market advertisement cannot resolve its item names: {ad}"
            );
            self.queue(
                format!("sell {ad}"),
                if self.guild {
                    Channel::Guild
                } else {
                    Channel::Local
                },
                false,
            );
            self.next_ad = now() + 60;
        }
        Ok(Some(d))
    }
    fn trade_packet(&mut self, p: &Packet, v: &PlayerView) -> Result<bool> {
        let mut r = Reader::new(&p.body);
        match p.id {
            61 => {
                let name = r.utf()?;
                r.done()?;
                self.incoming_request(name, v);
            }
            67 => {
                let read_items = |r: &mut Reader<'_>| -> Result<(Vec<i32>, [bool; 12])> {
                    ensure!(r.i16()? == 12, "market trade start shape");
                    let mut items = Vec::new();
                    let mut tradeable = [false; 12];
                    for t in &mut tradeable {
                        items.push(r.i32()?);
                        r.i32()?;
                        *t = r.u8()? == 1;
                        r.u8()?;
                    }
                    Ok((items, tradeable))
                };
                let (mine, tradeable) = read_items(&mut r)?;
                let peer = r.utf()?;
                let (theirs, _) = read_items(&mut r)?;
                r.done()?;
                self.trade = Some(SessionTrade {
                    binding: [v.session.world, v.session.player, v.session.character],
                    peer,
                    items: [mine, theirs],
                    tradeable,
                    offers: [[false; 12]; 2],
                    ready: v.time_ms + 3000,
                    ready_at: v.clock_at + Duration::from_millis(3000),
                    changed: false,
                });
            }
            23 => {
                let values = read_offer(&mut r)?;
                r.done()?;
                if let Some(t) = &mut self.trade {
                    t.offers[1] = values;
                    t.ready = v.time_ms + 3000;
                    t.ready_at = v.clock_at + Duration::from_millis(3000);
                }
            }
            18 => {
                read_offer(&mut r)?;
                read_offer(&mut r)?;
                r.done()?;
            }
            12 => {
                let result = r.i32()?;
                let reason = r.utf()?;
                r.done()?;
                self.trade = None;
                if self.incoming.is_some() {
                    self.end_incoming(v);
                }
                tracing::info!(
                    account = v.session.account,
                    result,
                    reason,
                    "market native trade outcome (not a settlement receipt)"
                );
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
fn capacity(v: &PlayerView, kind: i32) -> u8 {
    v.character
        .items
        .iter()
        .skip(4)
        .filter(|k| **k < 0 || **k == kind)
        .count()
        .clamp(1, 8) as u8
}
fn parse_ad(s: &str, c: &Content) -> Option<(Goods, i32)> {
    let kind = |name: &str| {
        let mut items = c
            .objects
            .values()
            .filter(|o| !name.is_empty() && o.class == "Equipment" && o.name == name);
        let kind = items.next()?.kind;
        items.next().is_none().then_some(i32::from(kind))
    };
    let (give, want) = s.split_once("; want ")?;
    let (name, quantity) = give.rsplit_once(" x")?;
    let goods = Goods {
        kind: kind(name)?,
        quantity: quantity.parse().ok()?,
    };
    (1..=16)
        .contains(&goods.quantity)
        .then_some((goods, kind(want)?))
}
fn parse_goods(s: &str) -> Option<Goods> {
    let (k, n) = s.split_once('x')?;
    let goods = Goods {
        kind: k.parse().ok()?,
        quantity: n.parse().ok()?,
    };
    ((0..=65535).contains(&goods.kind) && (1..=16).contains(&goods.quantity)).then_some(goods)
}
type ParsedTerms = (u64, [Goods; 2], [i32; 3], u64, u8, u8, u8);
fn parse_terms(s: &str) -> Option<ParsedTerms> {
    let p = s.split(' ').collect::<Vec<_>>();
    if p.len() != 7 {
        return None;
    }
    let (a, b) = p[1].split_once('/')?;
    let meeting = p[2]
        .strip_prefix('@')?
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<i32>, _>>()
        .ok()?
        .try_into()
        .ok()?;
    Some((
        u64::from_str_radix(p[0], 16).ok()?,
        [parse_goods(a)?, parse_goods(b)?],
        meeting,
        p[3].strip_prefix('!')?.parse().ok()?,
        p[4].strip_prefix('b')?.parse().ok()?,
        p[5].strip_prefix('p')?.parse().ok()?,
        p[6].strip_prefix('r')?.parse().ok()?,
    ))
}
pub(in crate::bots) fn read_offer(r: &mut Reader<'_>) -> Result<[bool; 12]> {
    ensure!(r.i16()? == 12, "market offer shape");
    let mut a = [false; 12];
    for b in &mut a {
        let n = r.u8()?;
        ensure!(n <= 1, "market offer boolean");
        *b = n == 1;
    }
    Ok(a)
}
