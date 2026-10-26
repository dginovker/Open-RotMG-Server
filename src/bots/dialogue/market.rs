use super::*;
use crate::bots::pursuits::market::{Message, goods_text, parse_quote};

pub(super) struct Wording {
    created: Instant,
    result: Arc<Mutex<Option<std::result::Result<String, Failure>>>>,
}

pub(super) const PROMPT: &str = r#"Write a reusable 2012 Realm trade-chat TEMPLATE using reply_to_player once with the sole allowed intent.
The brace tokens are LITERAL text: {give}, {want}, @{name}. Never replace them with items, numbers or a real name.
For market_advertise, examples of valid reply strings are "wts {give} for {want}", "SELLING {give} = {want}", "wtb {want} for {give}" and "s> {give} for {want} @{name}".
Choose a seller opening (wts, selling, sell, s>, trading) followed by {give}, then for or =, then {want}; or a buyer opening (wtb, buying, buy) followed by {want}, then for or =, then {give}. Optionally append literal @{name}.
For market_refuse, reply exactly "nty need {want}", "nah need {want}" or "no thx need {want}", with your own capitalization.
Vary opening and capitalization for the supplied personality/style seed. Keep every required brace token exactly once. No other words, quotes, punctuation, promises or acceptance. Real current inventory facts are inserted by the program only after generation. Native trade windows alone authorize acceptance."#;

fn validated_template_words(
    template: &str,
    intent: Intent,
) -> std::result::Result<Vec<&str>, Failure> {
    let words = template.split_whitespace().collect::<Vec<_>>();
    let lower = words
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let valid = match intent {
        Intent::MarketAdvertise => {
            (words.len() == 4 || (words.len() == 5 && lower[4] == "@{name}"))
                && matches!(lower[2].as_str(), "for" | "=")
                && ((matches!(
                    lower[0].as_str(),
                    "wts" | "selling" | "sell" | "s>" | "trading"
                ) && lower[1] == "{give}"
                    && lower[3] == "{want}")
                    || (matches!(lower[0].as_str(), "wtb" | "buying" | "buy")
                        && lower[1] == "{want}"
                        && lower[3] == "{give}"))
        }
        Intent::MarketRefuse => {
            matches!(lower.as_slice(), [a,b,c] if matches!(a.as_str(), "nty" | "nah") && b == "need" && c == "{want}")
                || matches!(lower.as_slice(), [a,b,c,d] if a == "no" && b == "thx" && c == "need" && d == "{want}")
        }
        _ => false,
    };
    if !valid {
        return Err(Failure("invalid_market_wording"));
    }
    Ok(words)
}

fn render(
    template: &str,
    intent: Intent,
    give: &str,
    want: &str,
    name: &str,
) -> std::result::Result<String, Failure> {
    let mut result = Vec::new();
    for word in validated_template_words(template, intent)? {
        result.push(match word.to_ascii_lowercase().as_str() {
            "{give}" => give.to_owned(),
            "{want}" => want.to_owned(),
            "@{name}" => format!("@{name}"),
            _ => word.to_owned(),
        });
    }
    let text = result.join(" ");
    crate::world::chat::validate(&text).map_err(|_| Failure("invalid_market_wording"))?;
    if text.encode_utf16().count() > 128 {
        return Err(Failure("invalid_market_wording"));
    }
    Ok(text)
}

impl Service {
    pub(in crate::bots) fn market_wording(
        self: &Arc<Self>,
        name: &str,
        message: &Message,
        c: &Content,
        personality: Value,
    ) -> Result<Option<String>> {
        let (intent, give, want) = if let Some(q) = parse_quote(&message.text, c) {
            (
                Intent::MarketAdvertise,
                goods_text(&q[0], c)?,
                goods_text(&q[1], c)?,
            )
        } else {
            (
                Intent::MarketRefuse,
                String::new(),
                message
                    .text
                    .strip_prefix("nty need ")
                    .context("invalid market speech intent")?
                    .to_owned(),
            )
        };
        let key = format!("{name}:{intent:?}");
        let mut cache = self.wordings.lock().expect("market speech cache poisoned");
        if let Some(entry) = cache.get(&key) {
            let result = entry.result.lock().expect("market speech result poisoned");
            match result.as_ref() {
                Some(Ok(template)) => {
                    return Ok(Some(render(template, intent, &give, &want, name)?));
                }
                Some(Err(error)) if entry.created.elapsed() < Duration::from_secs(60) => {
                    return Err((*error).into());
                }
                Some(Err(_)) => {}
                None => return Ok(None),
            }
        }
        if !self.automatic_available() {
            return Ok(None);
        }
        ensure!(
            cache.contains_key(&key) || cache.len() < 256,
            "market speech speaker cache full"
        );
        let result = Arc::new(Mutex::new(None));
        cache.insert(
            key,
            Wording {
                created: Instant::now(),
                result: result.clone(),
            },
        );
        let service = self.clone();
        let name = name.to_owned();
        let payload = json!({"allowed_intents":[intent],"automatic":true,"market_generation":true,
            "bot":{"personality":personality,"style_seed":name.bytes().fold(0u32, |h,b| h.wrapping_mul(31).wrapping_add(u32::from(b)))},
            "message":"Generate a trade speech template in my own voice, using only the given grammar."});
        tokio::spawn(async move {
            let start = Instant::now();
            let id = format!("market-{:016x}", rand::random::<u64>());
            let reply = service.ask(&payload, &id).await.and_then(|reply| {
                if reply.intent != intent {
                    return Err(Failure("invalid_market_wording"));
                }
                if let Err(error) = validated_template_words(&reply.reply, intent) {
                    tracing::error!(request=id, bot=name, ?intent, template=?reply.reply, "provider returned invalid market template");
                    return Err(error);
                }
                Ok(reply)
            });
            service.completed(&reply, start.elapsed());
            if let Err(error) = &reply {
                tracing::error!(request=id, bot=name, %error, "market generation failed; no substitute speech");
            }
            *result.lock().expect("market speech result poisoned") = Some(reply.map(|r| r.reply));
        });
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_market_templates_cannot_change_inventory_prices_or_consent() {
        for text in [
            "WTS {give} FOR {want}",
            "selling {give} = {want} @{name}",
            "WTB {want} FOR {give}",
        ] {
            let generated =
                render(text, Intent::MarketAdvertise, "4 spd", "1 def", "Trader").unwrap();
            assert!(generated.contains("4 spd") && generated.contains("1 def"));
        }
        assert_eq!(
            render(
                "nah need {want}",
                Intent::MarketRefuse,
                "",
                "1 def",
                "Trader"
            )
            .unwrap(),
            "nah need 1 def"
        );
        for text in [
            "wts 4 spd for 1 def",
            "wts {give} for 6 life",
            "wts {give} for {want} free",
            "agreed {give} for {want}",
            "wts {give} for {give}",
        ] {
            assert!(
                render(text, Intent::MarketAdvertise, "4 spd", "1 def", "Trader").is_err(),
                "{text}"
            );
        }
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    use crate::bots::pursuits::market::{Channel, Message};
    async fn generated(
        service: &Arc<Service>,
        name: &str,
        message: &Message,
        c: &Content,
    ) -> Result<String> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(text) =
                    service.market_wording(name, message, c, json!({"voice":"terse"}))?
                {
                    return Ok(text);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("generation timed out")
    }
    fn envelope(intent: Intent, text: &str) -> Value {
        json!({"model":MODEL,"choices":[{"finish_reason":"tool_calls","message":{
            "provider_metadata":{"gateway":{"routing":{"canonicalSlug":MODEL,"finalProvider":"nvidia"},"cost":"0"}},
            "tool_calls":[{"type":"function","function":{"name":"reply_to_player","arguments":json!({"intent":intent,"reply":text}).to_string()}}]}}]})
    }
    #[tokio::test]
    async fn market_template_reuses_generated_voice_for_current_goods_and_observes_provider_failure()
     {
        let c = Content::load(std::path::Path::new("data")).unwrap();
        let message = Message {
            text: "wts 4 spd for 1 def".into(),
            channel: Channel::Local,
        };
        let (service, server) = super::super::tests::provider(
            axum::http::StatusCode::OK,
            envelope(Intent::MarketAdvertise, "SELLING {give} FOR {want} @{name}"),
        )
        .await;
        assert_eq!(
            generated(&service, "Trader", &message, &c).await.unwrap(),
            "SELLING 4 spd FOR 1 def @Trader"
        );
        assert_eq!(
            generated(&service, "Trader", &message, &c).await.unwrap(),
            "SELLING 4 spd FOR 1 def @Trader"
        );
        assert_eq!(service.health()["daily_requests"], 1);
        let changed = Message {
            text: "wts 6 def for 1 life".into(),
            ..message.clone()
        };
        assert_eq!(
            generated(&service, "Trader", &changed, &c).await.unwrap(),
            "SELLING 6 def FOR 1 life @Trader"
        );
        assert_eq!(service.health()["daily_requests"], 1);
        assert_eq!(
            generated(&service, "Other", &message, &c).await.unwrap(),
            "SELLING 4 spd FOR 1 def @Other"
        );
        assert_eq!(service.health()["daily_requests"], 2);
        server.abort();
        let (service, server) =
            super::super::tests::provider(axum::http::StatusCode::SERVICE_UNAVAILABLE, json!({}))
                .await;
        assert!(generated(&service, "Trader", &message, &c).await.is_err());
        assert_eq!(service.health()["failures"], 1);
        assert_eq!(service.health()["successes"], 0);
        assert_eq!(service.health()["last_error"], "provider_rejected");
        assert!(generated(&service, "Trader", &changed, &c).await.is_err());
        assert_eq!(service.health()["daily_requests"], 1);

        assert!(
            service
                .market_wording("Trader", &message, &c, json!({"voice":"terse"}))
                .is_err()
        );
        service
            .wordings
            .lock()
            .unwrap()
            .get_mut("Trader:MarketAdvertise")
            .unwrap()
            .created = Instant::now() - Duration::from_secs(61);
        assert!(generated(&service, "Trader", &message, &c).await.is_err());
        assert_eq!(service.health()["daily_requests"], 2);
        assert_eq!(service.health()["successes"], 0);
        server.abort();
    }
    #[tokio::test]
    async fn market_template_pending_generation_uses_current_terms_and_rejects_invented_prices_or_intent()
     {
        let c = Content::load(std::path::Path::new("data")).unwrap();
        let (service, server) = super::super::tests::provider(
            axum::http::StatusCode::OK,
            envelope(Intent::MarketAdvertise, "wts {give} for {want}"),
        )
        .await;
        let old = Message {
            text: "wts 4 spd for 1 def".into(),
            channel: Channel::Local,
        };
        let changed = Message {
            text: "wts 5 spd for 1 def".into(),
            ..old.clone()
        };
        assert!(
            service
                .market_wording("Trader", &old, &c, json!({"voice":"terse"}))
                .unwrap()
                .is_none()
        );
        assert!(
            service
                .market_wording("Trader", &changed, &c, json!({"voice":"terse"}))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            generated(&service, "Trader", &changed, &c).await.unwrap(),
            "wts 5 spd for 1 def"
        );
        assert_eq!(service.health()["daily_requests"], 1);
        let current = Message {
            text: "wts 6 def for 1 life".into(),
            ..old.clone()
        };
        assert_eq!(
            generated(&service, "Trader", &current, &c).await.unwrap(),
            "wts 6 def for 1 life"
        );
        assert_eq!(service.health()["daily_requests"], 1);
        {
            let cache = service.wordings.lock().unwrap();
            let template = cache["Trader:MarketAdvertise"].result.lock().unwrap();
            assert_eq!(
                template.as_ref().unwrap().as_ref().unwrap(),
                "wts {give} for {want}"
            );
        }
        server.abort();
        for invalid in [
            envelope(Intent::MarketAdvertise, "wts {give} for 9 life"),
            envelope(Intent::MarketRefuse, "nah need {want}"),
        ] {
            let (service, server) =
                super::super::tests::provider(axum::http::StatusCode::OK, invalid).await;
            assert!(generated(&service, "Trader", &old, &c).await.is_err());
            assert_eq!(service.health()["last_error"], "invalid_market_wording");
            assert_eq!(service.health()["successes"], 0);
            assert_eq!(service.health()["failures"], 1);
            assert!(generated(&service, "Trader", &current, &c).await.is_err());
            assert_eq!(service.health()["daily_requests"], 1);
            server.abort();
        }
    }
    #[tokio::test]
    async fn market_refusal_template_renders_current_wants_without_another_provider_request() {
        let c = Content::load(std::path::Path::new("data")).unwrap();
        let (service, server) = super::super::tests::provider(
            axum::http::StatusCode::OK,
            envelope(Intent::MarketRefuse, "nah need {want}"),
        )
        .await;
        let old = Message {
            text: "nty need 1 def".into(),
            channel: Channel::Tell("Peer".into()),
        };
        assert!(
            service
                .market_wording("Trader", &old, &c, json!({}))
                .unwrap()
                .is_none()
        );
        let changed = Message {
            text: "nty need 4 spd".into(),
            ..old.clone()
        };
        assert_eq!(
            generated(&service, "Trader", &changed, &c).await.unwrap(),
            "nah need 4 spd"
        );
        assert_eq!(
            generated(&service, "Trader", &old, &c).await.unwrap(),
            "nah need 1 def"
        );
        assert_eq!(service.health()["daily_requests"], 1);
        server.abort();
    }
    #[tokio::test]
    async fn market_pending_generation_loses_publication_authority_when_real_stock_is_lost() {
        use crate::bots::pursuits::market::Market;
        let (c, mut v) = crate::bots::quest_tests::fixture();
        v.map.name = "Nexus".into();
        v.session.world = -2;
        v.character.items[4..8].fill(0xa21);
        let mut market = Market::default();
        market.name = "Trader".into();
        let grid = Grid::terrain(&v);
        for _ in 0..32 {
            let d = market
                .decide(
                    &v,
                    &c,
                    None,
                    &grid,
                    &mut rand::rngs::mock::StepRng::new(0, 1),
                )
                .unwrap()
                .unwrap();
            if market.outgoing.is_some() {
                break;
            }
            v.position = d.movement.expect("market position should be reachable");
        }
        let ad = market.outgoing.clone().unwrap();
        assert!(market.message_current(&ad, &v, &c, None).unwrap());
        let (service, server) = super::super::tests::provider(
            axum::http::StatusCode::OK,
            envelope(Intent::MarketAdvertise, "wts {give} for {want}"),
        )
        .await;
        assert!(
            service
                .market_wording("Trader", &ad, &c, json!({}))
                .unwrap()
                .is_none()
        );
        v.character.items[4] = -1;
        v.character.inventory_revision += 1;
        assert!(!market.message_current(&ad, &v, &c, None).unwrap());
        assert_eq!(
            generated(&service, "Trader", &ad, &c).await.unwrap(),
            "wts 4 spd for 1 def"
        );
        assert!(
            !market.message_current(&ad, &v, &c, None).unwrap(),
            "a valid model reply cannot restore lost stock or publication authority"
        );
        server.abort();
    }
    #[tokio::test]
    #[ignore = "requires approved server-only free-provider environment"]
    async fn real_provider_market_template_and_talk_smoke() {
        tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::INFO)
            .try_init()
            .expect("provider smoke tracing unavailable");
        async fn wording(service: &Arc<Service>, message: &Message, c: &Content) -> Result<String> {
            loop {
                if let Some(text) =
                    service.market_wording("SmokeTrader", message, c, json!({"voice":"terse"}))?
                {
                    return Ok(text);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        tokio::time::timeout(Duration::from_secs(25), async {
            let service = Service::configured(true)?.context("canonical provider service disabled")?;
            let content = Content::load(std::path::Path::new("data"))?;
            use crate::bots::pursuits::market::Goods;
            let goods = [Goods {kind:0xa21,quantity:4}, Goods {kind:0xa20,quantity:1}];
            let ad = Message {text:format!("wts {} for {}", goods_text(&goods[0],&content)?,goods_text(&goods[1],&content)?),channel:Channel::Local};
            let first = wording(&service,&ad,&content).await.context("advertisement template")?;
            ensure!(parse_quote(&first,&content) == Some(goods), "generated advertisement facts changed");
            let current = [Goods {kind:0xa20,quantity:6}, Goods {kind:0xae9,quantity:1}];
            let changed = Message {text:format!("wts {} for {}",goods_text(&current[0],&content)?,goods_text(&current[1],&content)?),..ad};
            let updated = wording(&service,&changed,&content).await?;
            ensure!(parse_quote(&updated,&content) == Some(current), "reused template rendered stale facts");
            ensure!(service.health()["daily_requests"] == 1, "advertisement exceeded its one-request smoke budget");
            let refusal = Message {text:"nty need 1 def".into(),channel:Channel::Local};
            wording(&service,&refusal,&content).await.context("refusal template")?;
            let changed_refusal = Message {text:"nty need 4 spd".into(),..refusal};
            let refusal_text = wording(&service,&changed_refusal,&content).await?;
            ensure!(refusal_text.to_ascii_lowercase().ends_with("need 4 spd"), "refusal template rendered stale wants");
            ensure!(service.health()["daily_requests"] == 2, "refusal exceeded its one-request smoke budget");
            let payload = json!({"allowed_intents":["talk"],"bot":{"name":"SmokeTrader","world":"Nexus","hp":500,"max_hp":500,"mp":200,"level":20,"equipment":[],"priorities":{"recovery":0.7,"resources":4,"companions":3}},
                "speaker":{"name":"SmokeHuman","hp":500,"max_hp":500,"level":20},"crew":[],"agreement":null,"message":"SmokeTrader, hello!"});
            let started=Instant::now();
            let reply=service.ask(&payload,"real-market-smoke-talk").await;
            service.completed(&reply,started.elapsed());
            let reply=reply?;
            ensure!(reply.intent == Intent::Talk && !reply.reply.trim().is_empty(), "generic speech did not produce the sole allowed Talk intent");
            let health=service.health();
            ensure!(health["daily_requests"] == 3 && health["successes"] == 3 && health["failures"] == 0 && health["contract_failed"] == false, "three-call free-provider smoke contract failed");
            println!("{}",json!({"smoke":"market-template-and-talk","advertisement_current_facts":true,"refusal_current_facts":true,"health":health}));
            Ok::<(),anyhow::Error>(())
        }).await.expect("free-provider smoke exceeded its 25-second deadline").expect("free-provider smoke failed without retry or substitute speech");
    }
}
