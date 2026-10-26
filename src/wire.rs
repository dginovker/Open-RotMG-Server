use anyhow::{Result, bail, ensure};

pub const HELLO: u8 = 17;
pub const CREATE: u8 = 36;
pub const LOAD: u8 = 45;
pub const MAP_INFO: u8 = 60;
pub const CREATE_SUCCESS: u8 = 31;
pub const UPDATE: u8 = 26;
pub const UPDATE_ACK: u8 = 11;
pub const NEW_TICK: u8 = 62;
pub const MOVE: u8 = 7;
pub const PLAYER_SHOOT: u8 = 38;
pub const PING: u8 = 6;
pub const PONG: u8 = 16;
pub const PLAYER_TEXT: u8 = 69;
pub const TEXT: u8 = 25;
pub const CREATE_GUILD: u8 = 15;
pub const GUILD_RESULT: u8 = 58;
pub const GUILD_INVITE: u8 = 8;
pub const INVITED_TO_GUILD: u8 = 77;
pub const JOIN_GUILD: u8 = 5;
pub const CHANGE_GUILD_RANK: u8 = 40;
pub const GUILD_REMOVE: u8 = 78;
pub const ACCOUNT_LIST: u8 = 46;
pub const EDIT_ACCOUNT_LIST: u8 = 53;
pub const TELEPORT: u8 = 49;
pub const BUY: u8 = 50;
pub const BUY_RESULT: u8 = 27;
pub const GOTO: u8 = 52;
pub const SHOW_EFFECT: u8 = 56;
pub const PLAY_SOUND: u8 = 44;
pub const NOTIFICATION: u8 = 63;
pub(crate) const MAX_FRAME: usize = 1_048_576;
const CLIENT_OUT: &[u8] = &[
    0x31, 0x1f, 0x80, 0x69, 0x14, 0x51, 0xc7, 0x1b, 0x09, 0xa1, 0x3a, 0x2a, 0x6e,
];
const CLIENT_IN: &[u8] = &[
    0x72, 0xc5, 0x58, 0x3c, 0xaf, 0xb6, 0x81, 0x89, 0x95, 0xcb, 0xd7, 0x4b, 0x80,
];

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(
            n <= self.data.len() - self.pos,
            "truncated packet at byte {}: need {n}, have {}",
            self.pos,
            self.data.len() - self.pos
        );
        let bytes = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(bytes)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into()?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        let v = f32::from_be_bytes(self.take(4)?.try_into()?);
        ensure!(v.is_finite(), "non-finite coordinate");
        Ok(v)
    }
    pub fn utf(&mut self) -> Result<String> {
        let n = u16::from_be_bytes(self.take(2)?.try_into()?) as usize;
        Ok(std::str::from_utf8(self.take(n)?)?.to_owned())
    }
    pub(crate) fn remaining(&self) -> &[u8] {
        &self.data[self.pos..]
    }
    pub fn done(&self) -> Result<()> {
        ensure!(
            self.pos == self.data.len(),
            "{} trailing packet bytes",
            self.data.len() - self.pos
        );
        Ok(())
    }
}

#[derive(Default)]
pub struct Writer(pub Vec<u8>);
impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        assert!(v.is_finite());
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn utf(&mut self, v: &str) -> &mut Self {
        let n = u16::try_from(v.len()).expect("server string exceeds protocol limit");
        self.0.extend(n.to_be_bytes());
        self.0.extend(v.as_bytes());
        self
    }
}

struct Rc4 {
    s: [u8; 256],
    i: usize,
    j: usize,
}
impl Rc4 {
    fn new(key: &[u8]) -> Self {
        assert!(!key.is_empty());
        let mut s = [0; 256];
        for (i, v) in s.iter_mut().enumerate() {
            *v = i as u8;
        }
        let mut j = 0;
        for i in 0..256 {
            j = (j + s[i] as usize + key[i % key.len()] as usize) & 255;
            s.swap(i, j);
        }
        Self { s, i: 0, j: 0 }
    }
    fn apply(&mut self, data: &mut [u8]) {
        for v in data {
            self.i = (self.i + 1) & 255;
            self.j = (self.j + self.s[self.i] as usize) & 255;
            self.s.swap(self.i, self.j);
            *v ^= self.s[(self.s[self.i] as usize + self.s[self.j] as usize) & 255];
        }
    }
}

#[derive(Clone)]
pub struct Packet {
    pub id: u8,
    pub body: Vec<u8>,
}
pub struct Codec {
    incoming: Rc4,
    outgoing: Rc4,
    buffer: Vec<u8>,
}
impl Default for Codec {
    fn default() -> Self {
        Self::server()
    }
}
impl Codec {
    pub fn server() -> Self {
        Self {
            incoming: Rc4::new(CLIENT_OUT),
            outgoing: Rc4::new(CLIENT_IN),
            buffer: Vec::new(),
        }
    }
    #[cfg(test)]
    pub(crate) fn client() -> Self {
        Self {
            incoming: Rc4::new(CLIENT_IN),
            outgoing: Rc4::new(CLIENT_OUT),
            buffer: Vec::new(),
        }
    }
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<Packet>> {
        ensure!(
            self.buffer.len() + data.len() <= MAX_FRAME * 2,
            "receive buffer limit"
        );
        self.buffer.extend(data);
        let mut out = Vec::new();
        let mut pos = 0;
        while self.buffer.len() - pos >= 5 {
            let n = i32::from_be_bytes(self.buffer[pos..pos + 4].try_into()?) as usize;
            if !(5..=MAX_FRAME).contains(&n) {
                bail!("invalid frame length {n}");
            }
            if self.buffer.len() - pos < n {
                break;
            }
            let id = self.buffer[pos + 4];
            let mut body = self.buffer[pos + 5..pos + n].to_vec();
            self.incoming.apply(&mut body);
            out.push(Packet { id, body });
            pos += n;
        }
        self.buffer.drain(..pos);
        Ok(out)
    }
    pub fn frame(&mut self, id: u8, mut body: Vec<u8>) -> Vec<u8> {
        assert!(body.len() + 5 <= MAX_FRAME);
        self.outgoing.apply(&mut body);
        let mut out = Vec::with_capacity(body.len() + 5);
        out.extend(((body.len() + 5) as i32).to_be_bytes());
        out.push(id);
        out.extend(body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rc4_known_vector() {
        let mut b = b"Plaintext".to_vec();
        Rc4::new(b"Key").apply(&mut b);
        assert_eq!(
            b,
            vec![0xbb, 0xf3, 0x16, 0xe8, 0xd9, 0x40, 0xaf, 0x0a, 0xd3]
        );
    }
    #[test]
    fn fragmented_stream_and_empty_ack() {
        let mut client = Codec::client();
        let mut server = Codec::server();
        let mut bytes = client.frame(HELLO, b"first".to_vec());
        bytes.extend(client.frame(UPDATE_ACK, vec![]));
        bytes.extend(client.frame(MOVE, b"second".to_vec()));
        let mut packets = Vec::new();
        for chunk in bytes.chunks(3) {
            packets.extend(server.feed(chunk).unwrap());
        }
        assert_eq!(packets.len(), 3);
        assert_eq!(packets[0].body, b"first");
        assert_eq!(packets[1].id, UPDATE_ACK);
        assert!(packets[1].body.is_empty());
        assert_eq!(packets[2].body, b"second");
        let reply = server.frame(PING, vec![0, 0, 0, 1]);
        assert_eq!(client.feed(&reply).unwrap()[0].body, vec![0, 0, 0, 1]);
    }
    #[test]
    fn framing_is_not_encrypted() {
        let mut c = Codec::server();
        let f = c.frame(CREATE_SUCCESS, vec![0; 8]);
        assert_eq!(&f[..5], &[0, 0, 0, 13, 31]);
    }
    #[test]
    fn invalid_frames_fail() {
        assert!(Codec::server().feed(&[0, 0, 0, 4, 17]).is_err());
        assert!(Reader::new(&[0, 5, b'x']).utf().is_err());
    }
    #[test]
    fn create_has_only_class_type() {
        let mut r = Reader::new(&[3, 14]);
        assert_eq!(r.i16().unwrap(), 0x030e);
        r.done().unwrap();
    }
    #[test]
    fn selected_client_source_vectors() {
        // These fixtures derive from selected-client ABC; they are not captured gameplay or valid RSA login.
        fn hex(s: &str) -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        }
        let streams: serde_json::Value =
            serde_json::from_str(include_str!("packets.json")).unwrap();
        for stream in streams.as_array().unwrap() {
            let direction = stream["direction"].as_str().unwrap();
            assert!(["client_to_server", "server_to_client"].contains(&direction));
            let c2s = direction == "client_to_server";
            let mut encoder = if c2s {
                Codec::client()
            } else {
                Codec::server()
            };
            let frames = stream["frames"].as_array().unwrap();
            let mut bytes = Vec::new();
            for f in frames {
                let plain = hex(f["plain"].as_str().unwrap());
                let wire = hex(f["wire"].as_str().unwrap());
                assert_eq!(
                    encoder.frame(plain[4], plain[5..].to_vec()),
                    wire,
                    "{}",
                    f["name"]
                );
                bytes.extend(wire);
            }
            for n in [1, 3, 5, 17, bytes.len()] {
                let mut decoder = if c2s {
                    Codec::server()
                } else {
                    Codec::client()
                };
                let mut packets = Vec::new();
                for chunk in bytes.chunks(n) {
                    packets.extend(decoder.feed(chunk).unwrap());
                }
                assert_eq!(packets.len(), frames.len());
                for (p, f) in packets.iter().zip(frames) {
                    let plain = hex(f["plain"].as_str().unwrap());
                    assert_eq!(p.id, plain[4]);
                    assert_eq!(p.body, plain[5..]);
                }
            }
        }
    }
}
