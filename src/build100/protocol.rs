use crate::{
    guid,
    wire::{Codec, Packet, Reader},
};
use anyhow::{Result, ensure};

pub const HELLO: u8 = 17;
pub const PONG: u8 = 23;
const CLIENT_OUT: &[u8] = b"zCV2tsY0]s;|]";
const CLIENT_IN: &[u8] = b"hPvJ(R}Mp$-cg";

pub fn server_codec() -> Codec {
    Codec::server_with_keys(CLIENT_OUT, CLIENT_IN)
}

pub struct Hello {
    pub game_id: i32,
    pub guid: String,
}
impl Hello {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut r = Reader::new(body);
        ensure!(r.i32()? == 100, "client must be build100");
        let hello = Self {
            game_id: r.i32()?,
            guid: r.utf()?,
        };
        r.done()?;
        guid(&hello.guid)?;
        Ok(hello)
    }
}

#[derive(Debug, PartialEq)]
pub enum Selection {
    Create { kind: i16, name: String },
    Load { character_id: i32 },
}
impl Selection {
    pub fn parse(packet: &Packet) -> Result<Self> {
        let mut r = Reader::new(&packet.body);
        let selection = match packet.id {
            3 => Self::Create {
                kind: r.i16()?,
                name: r.utf()?,
            },
            25 => {
                let character_id = r.i32()?;
                r.utf()?; // The saved character, not an advisory Load name, owns its identity.
                Self::Load { character_id }
            }
            id => anyhow::bail!("expected build100 Create/Load, got {id}"),
        };
        r.done()?;
        Ok(selection)
    }
}

pub fn teleport(body: &[u8]) -> Result<i32> {
    let mut r = Reader::new(body);
    let target = r.i32()?;
    r.done()?;
    Ok(target)
}

pub fn pong(body: &[u8]) -> Result<i32> {
    let mut r = Reader::new(body);
    let serial = r.i32()?;
    r.done()?;
    Ok(serial)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Writer;
    const PING: u8 = 22;
    const RECONNECT: u8 = 21;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn historical_contracts_reencoded_streams() {
        // Independently RC4-reencoded synthetic fields; not native capture bytes (see phase-1.md).
        let client_stream = hex(concat!(
            "0000003711",
            "3ae580e8e8bfe7f0956b8ec0c0e70bd68f18336557d752a88e257cbf60328f2c793254f3039e1785e813a48d26aae006a857",
            "0000001019",
            "c5cc26df7e1bef67f805c5",
            "0000000917",
            "153ba297"
        ));
        let expected = [
            (
                HELLO,
                hex(concat!(
                    "00000064000000000028",
                    "30313233343536373839616263646566303132333435363738396162636465663031323334353637"
                )),
            ),
            (25, hex("0000000000055068617365")),
            (PONG, hex("01020304")),
        ];
        for size in [1, 3, 5, client_stream.len()] {
            let mut codec = server_codec();
            let packets: Vec<_> = client_stream
                .chunks(size)
                .flat_map(|chunk| codec.feed(chunk).unwrap())
                .collect();
            assert_eq!(packets.len(), expected.len());
            for (p, (id, body)) in packets.iter().zip(&expected) {
                assert_eq!(p.id, *id);
                assert_eq!(&p.body, body);
            }
            let hello = Hello::parse(&packets[0].body).unwrap();
            assert_eq!(hello.game_id, 0);
            assert_eq!(hello.guid, "0123456789abcdef0123456789abcdef01234567");
            assert_eq!(
                Selection::parse(&packets[1]).unwrap(),
                Selection::Load { character_id: 0 }
            );
            assert_eq!(pong(&packets[2].body).unwrap(), 0x01020304);
        }
        let mut codec = server_codec();
        assert_eq!(
            codec.frame(PING, hex("01020304")),
            hex("0000000916f29f147b")
        );
        assert_eq!(
            codec.frame(RECONNECT, hex("0000002a")),
            hex("000000091562d17cf1")
        );
    }

    #[test]
    fn historical_contracts_create_is_class_and_name_not_later_class_only() {
        let mut packet = Packet {
            id: 3,
            body: hex("030e00055068617365"),
        };
        assert_eq!(
            Selection::parse(&packet).unwrap(),
            Selection::Create {
                kind: 0x30e,
                name: "Phase".into()
            }
        );
        for n in 0..packet.body.len() {
            assert!(
                Selection::parse(&Packet {
                    id: 3,
                    body: packet.body[..n].to_vec()
                })
                .is_err()
            );
        }
        packet.body.push(0);
        assert!(Selection::parse(&packet).is_err());
        assert!(
            Selection::parse(&Packet {
                id: 36,
                body: hex("030e")
            })
            .is_err()
        );
    }

    #[test]
    fn historical_contracts_hello_entry_and_portal_game_ids() {
        for game in [-1, 0, 42] {
            let mut w = Writer::new();
            w.i32(100)
                .i32(game)
                .utf("0123456789abcdef0123456789abcdef01234567");
            assert_eq!(Hello::parse(&w.0).unwrap().game_id, game);
            for n in 0..w.0.len() {
                assert!(Hello::parse(&w.0[..n]).is_err());
            }
            w.u8(0);
            assert!(Hello::parse(&w.0).is_err());
        }
        let mut w = Writer::new();
        w.i32(104)
            .i32(0)
            .utf("0123456789abcdef0123456789abcdef01234567");
        assert!(Hello::parse(&w.0).is_err());
        assert!(pong(&[]).is_err());
        assert!(pong(&[0; 5]).is_err());
        // Parsing a destination is not transfer authorization; Room does not reinterpret Pong as travel.
        assert_ne!(PONG, RECONNECT);
    }
}
