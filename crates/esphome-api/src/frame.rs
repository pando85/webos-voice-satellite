//! ESPHome native API framing: plaintext and Noise.
//!
//! **Plaintext** frame: `0x00`, varint payload length, varint message type,
//! protobuf payload.
//!
//! **Noise** frame: `0x01`, 16-bit big-endian length, frame body. The first
//! client frame is an empty "client hello"; the device answers with a server
//! hello body `[0x01 (chosen protocol), name, 0x00, mac, 0x00]`. The next client
//! frame is `[0x00, noise handshake message]` and the device answers
//! `[0x00, noise handshake message]` (or `[0x01, reason]` on failure). After the
//! handshake each frame body is ciphertext of `type (u16 BE), length (u16 BE),
//! payload`. Protocol: `Noise_NNpsk0_25519_ChaChaPoly_SHA256`, prologue
//! `b"NoiseAPIInit\0\0"`, PSK = 32-byte key from base64.
//!
//! Sources: `api.proto` header comment (ESPHome 2026.10.1) and aioesphomeapi
//! 46.6.0 `_frame_helper/noise.py` / `plain_text.py` (MIT); see
//! `docs/research/esphome-rust-satellite.md` A1.

use std::fmt;

use crate::varint;

/// Plaintext frame preamble byte.
pub const PLAINTEXT_PREAMBLE: u8 = 0x00;
/// Noise frame preamble byte.
pub const NOISE_PREAMBLE: u8 = 0x01;
/// Noise protocol name (the only one ESPHome defines, selector byte `0x01`).
pub const NOISE_PROTOCOL: &str = "Noise_NNpsk0_25519_ChaChaPoly_SHA256";
/// Noise prologue used by ESPHome.
pub const NOISE_PROLOGUE: &[u8] = b"NoiseAPIInit\0\0";
/// Largest protobuf payload accepted or sent. Noise limits a whole frame to
/// 65535 bytes (16-byte tag and 4-byte inner header included); plaintext uses
/// the same cap so a peer cannot make us buffer more.
pub const MAX_PAYLOAD: usize = 65_535 - 16 - 4;
/// Reason string that aioesphomeapi maps to "invalid encryption key".
pub const HANDSHAKE_MAC_FAILURE: &str = "Handshake MAC failure";

const NOISE_HEADER_LEN: usize = 3;
const NOISE_TAG_LEN: usize = 16;

/// A decoded API message: type id and protobuf payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    /// Message type id (`option (id)` in `api.proto`).
    pub type_id: u16,
    /// Encoded protobuf message.
    pub payload: Vec<u8>,
}

/// Framing or encryption error. Any error ends the connection.
#[derive(Debug)]
pub enum FrameError {
    /// Socket error.
    Io(std::io::Error),
    /// The peer closed the connection.
    Closed,
    /// The peer used the wrong framing (plaintext vs Noise) or a bad preamble.
    BadPreamble(u8),
    /// Frame or payload larger than [`MAX_PAYLOAD`].
    TooLarge(usize),
    /// Malformed frame.
    Protocol(&'static str),
    /// Noise handshake or decryption failure.
    Noise(snow::Error),
    /// No traffic within the allowed time.
    Timeout,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "i/o error: {err}"),
            Self::Closed => f.write_str("connection closed by peer"),
            Self::BadPreamble(b) => write!(f, "unexpected frame preamble 0x{b:02x}"),
            Self::TooLarge(n) => write!(f, "frame too large ({n} bytes)"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::Noise(err) => write!(f, "noise error: {err}"),
            Self::Timeout => f.write_str("timeout"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<std::io::Error> for FrameError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<snow::Error> for FrameError {
    fn from(err: snow::Error) -> Self {
        Self::Noise(err)
    }
}

/// Appends a plaintext frame to `out`.
///
/// # Errors
///
/// [`FrameError::TooLarge`] if `payload` exceeds [`MAX_PAYLOAD`].
pub fn encode_plaintext(type_id: u16, payload: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|_| payload.len() <= MAX_PAYLOAD)
        .ok_or(FrameError::TooLarge(payload.len()))?;
    out.push(PLAINTEXT_PREAMBLE);
    varint::encode(len, out);
    varint::encode(u32::from(type_id), out);
    out.extend_from_slice(payload);
    Ok(())
}

/// Decodes one plaintext frame from the start of `buf`.
///
/// Returns `Ok(None)` if more bytes are needed, otherwise the message and the
/// number of bytes consumed.
///
/// # Errors
///
/// Bad preamble, oversized frame or malformed varints.
pub fn decode_plaintext(buf: &[u8]) -> Result<Option<(RawMessage, usize)>, FrameError> {
    let Some(&preamble) = buf.first() else {
        return Ok(None);
    };
    if preamble != PLAINTEXT_PREAMBLE {
        return Err(FrameError::BadPreamble(preamble));
    }
    let mut pos = 1;
    let Some((len, n)) =
        varint::decode(&buf[pos..]).map_err(|_| FrameError::Protocol("length varint"))?
    else {
        return Ok(None);
    };
    pos += n;
    let len = len as usize;
    if len > MAX_PAYLOAD {
        return Err(FrameError::TooLarge(len));
    }
    let Some((type_id, n)) =
        varint::decode(&buf[pos..]).map_err(|_| FrameError::Protocol("type varint"))?
    else {
        return Ok(None);
    };
    pos += n;
    let type_id =
        u16::try_from(type_id).map_err(|_| FrameError::Protocol("message type out of range"))?;
    if buf.len() < pos + len {
        return Ok(None);
    }
    let payload = buf[pos..pos + len].to_vec();
    Ok(Some((RawMessage { type_id, payload }, pos + len)))
}

/// Appends a Noise outer frame (`0x01`, u16 BE length, body) to `out`.
///
/// # Errors
///
/// [`FrameError::TooLarge`] if `body` is longer than 65535 bytes.
pub fn encode_noise_frame(body: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    let len = u16::try_from(body.len()).map_err(|_| FrameError::TooLarge(body.len()))?;
    out.push(NOISE_PREAMBLE);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(body);
    Ok(())
}

/// Decodes one Noise outer frame from the start of `buf`.
///
/// Returns `Ok(None)` if more bytes are needed, otherwise the frame body and
/// the number of bytes consumed.
///
/// # Errors
///
/// [`FrameError::BadPreamble`] if the frame does not start with `0x01`.
pub fn decode_noise_frame(buf: &[u8]) -> Result<Option<(&[u8], usize)>, FrameError> {
    let Some(&preamble) = buf.first() else {
        return Ok(None);
    };
    if preamble != NOISE_PREAMBLE {
        return Err(FrameError::BadPreamble(preamble));
    }
    if buf.len() < NOISE_HEADER_LEN {
        return Ok(None);
    }
    let len = usize::from(u16::from_be_bytes([buf[1], buf[2]]));
    let end = NOISE_HEADER_LEN + len;
    if buf.len() < end {
        return Ok(None);
    }
    Ok(Some((&buf[NOISE_HEADER_LEN..end], end)))
}

/// Decodes a base64 Noise PSK (32 bytes, standard alphabet, as in ESPHome
/// `api: encryption: key:`).
///
/// # Errors
///
/// Returns a message if the string is not base64 or not 32 bytes long.
pub fn decode_psk(psk: &str) -> Result<[u8; 32], &'static str> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(psk.trim())
        .map_err(|_| "noise_psk is not valid base64")?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| "noise_psk must decode to 32 bytes")
}

/// Server hello frame body: protocol selector, NUL-terminated name and MAC.
#[must_use]
pub fn server_hello_body(name: &str, mac_compact: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(3 + name.len() + mac_compact.len());
    body.push(0x01);
    body.extend_from_slice(name.as_bytes());
    body.push(0x00);
    body.extend_from_slice(mac_compact.as_bytes());
    body.push(0x00);
    body
}

/// Handshake rejection frame body: `0x01` followed by a reason string.
#[must_use]
pub fn handshake_reject_body(reason: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(1 + reason.len());
    body.push(0x01);
    body.extend_from_slice(reason.as_bytes());
    body
}

/// Device (responder) side of the Noise handshake.
pub struct NoiseResponder {
    hs: snow::HandshakeState,
}

impl NoiseResponder {
    /// Creates a responder for the given PSK.
    ///
    /// # Errors
    ///
    /// Only if the snow resolver lacks a primitive (a build configuration bug).
    pub fn new(psk: &[u8; 32]) -> Result<Self, FrameError> {
        let params = NOISE_PROTOCOL.parse().map_err(FrameError::Noise)?;
        let hs = snow::Builder::new(params)
            .psk(0, psk)?
            .prologue(NOISE_PROLOGUE)?
            .build_responder()?;
        Ok(Self { hs })
    }

    /// Processes the client's handshake frame body (`[0x00, message]`) and
    /// returns the response frame body (`[0x00, message]`).
    ///
    /// # Errors
    ///
    /// [`FrameError::Noise`] on a wrong PSK or corrupt message; the caller should
    /// send [`handshake_reject_body`] with [`HANDSHAKE_MAC_FAILURE`] and close.
    pub fn respond(&mut self, body: &[u8]) -> Result<Vec<u8>, FrameError> {
        let (&indicator, message) = body
            .split_first()
            .ok_or(FrameError::Protocol("empty handshake frame"))?;
        if indicator != 0x00 {
            return Err(FrameError::Protocol("bad handshake indicator"));
        }
        let mut scratch = vec![0_u8; message.len()];
        self.hs.read_message(message, &mut scratch)?;
        let mut out = vec![0_u8; 1 + 64 + NOISE_TAG_LEN];
        let n = self.hs.write_message(&[], &mut out[1..])?;
        out.truncate(1 + n);
        Ok(out)
    }

    /// Finishes the handshake.
    ///
    /// # Errors
    ///
    /// If [`Self::respond`] has not completed the handshake.
    pub fn into_transport(self) -> Result<NoiseTransport, FrameError> {
        Ok(NoiseTransport::from_state(self.hs.into_transport_mode()?))
    }
}

/// Established Noise session: encrypts and decrypts API messages.
pub struct NoiseTransport {
    state: snow::TransportState,
    scratch: Vec<u8>,
}

impl NoiseTransport {
    /// Wraps an established snow transport (either side; used by tests that
    /// play the HA role).
    #[must_use]
    pub fn from_state(state: snow::TransportState) -> Self {
        Self {
            state,
            scratch: Vec::new(),
        }
    }

    /// Encrypts one API message and appends the complete outer frame to `out`.
    ///
    /// # Errors
    ///
    /// Oversized payload or a cipher error.
    pub fn encode(
        &mut self,
        type_id: u16,
        payload: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), FrameError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(FrameError::TooLarge(payload.len()));
        }
        let len = u16::try_from(payload.len()).map_err(|_| FrameError::TooLarge(payload.len()))?;
        self.scratch.clear();
        self.scratch.extend_from_slice(&type_id.to_be_bytes());
        self.scratch.extend_from_slice(&len.to_be_bytes());
        self.scratch.extend_from_slice(payload);
        let mut cipher = vec![0_u8; self.scratch.len() + NOISE_TAG_LEN];
        let n = self.state.write_message(&self.scratch, &mut cipher)?;
        cipher.truncate(n);
        encode_noise_frame(&cipher, out)
    }

    /// Decrypts one frame body into an API message.
    ///
    /// # Errors
    ///
    /// Authentication failure or a malformed inner header.
    pub fn decode(&mut self, body: &[u8]) -> Result<RawMessage, FrameError> {
        let mut plain = vec![0_u8; body.len()];
        let n = self.state.read_message(body, &mut plain)?;
        plain.truncate(n);
        if plain.len() < 4 {
            return Err(FrameError::Protocol("decrypted message too short"));
        }
        let type_id = u16::from_be_bytes([plain[0], plain[1]]);
        let len = usize::from(u16::from_be_bytes([plain[2], plain[3]]));
        if plain.len() - 4 < len {
            return Err(FrameError::Protocol("decrypted length mismatch"));
        }
        plain.truncate(4 + len);
        plain.drain(..4);
        Ok(RawMessage {
            type_id,
            payload: plain,
        })
    }
}

/// Message codec of an established connection.
pub enum Codec {
    /// Plaintext framing.
    Plaintext,
    /// Noise-encrypted framing.
    Noise(Box<NoiseTransport>),
}

impl Codec {
    /// Appends one encoded message to `out`.
    ///
    /// # Errors
    ///
    /// Oversized payload or a cipher error.
    pub fn encode(
        &mut self,
        type_id: u16,
        payload: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), FrameError> {
        match self {
            Self::Plaintext => encode_plaintext(type_id, payload, out),
            Self::Noise(t) => t.encode(type_id, payload, out),
        }
    }

    /// Decodes and removes one message from the front of `buf`, if complete.
    ///
    /// # Errors
    ///
    /// Any framing or decryption error.
    pub fn decode(&mut self, buf: &mut Vec<u8>) -> Result<Option<RawMessage>, FrameError> {
        match self {
            Self::Plaintext => match decode_plaintext(buf)? {
                Some((msg, used)) => {
                    buf.drain(..used);
                    Ok(Some(msg))
                }
                None => Ok(None),
            },
            Self::Noise(t) => {
                let Some((body, used)) = decode_noise_frame(buf)? else {
                    return Ok(None);
                };
                let msg = t.decode(body)?;
                buf.drain(..used);
                Ok(Some(msg))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PSK: [u8; 32] = [7; 32];

    /// Client (HA) side of the handshake, as aioesphomeapi performs it.
    fn client_handshake(psk: &[u8; 32]) -> snow::HandshakeState {
        snow::Builder::new(NOISE_PROTOCOL.parse().unwrap())
            .psk(0, psk)
            .unwrap()
            .prologue(NOISE_PROLOGUE)
            .unwrap()
            .build_initiator()
            .unwrap()
    }

    fn established(
        psk_client: &[u8; 32],
    ) -> Result<(snow::TransportState, NoiseTransport), FrameError> {
        let mut client = client_handshake(psk_client);
        let mut msg = vec![0_u8; 128];
        let n = client.write_message(&[], &mut msg[1..]).unwrap();
        msg.truncate(1 + n);
        let mut server = NoiseResponder::new(&PSK)?;
        let reply = server.respond(&msg)?;
        assert_eq!(reply[0], 0x00);
        let mut scratch = vec![0_u8; 128];
        client.read_message(&reply[1..], &mut scratch).unwrap();
        Ok((
            client.into_transport_mode().unwrap(),
            server.into_transport()?,
        ))
    }

    #[test]
    fn plaintext_round_trip_and_partial_reads() {
        let payload: Vec<u8> = (0..=255).collect();
        let mut wire = Vec::new();
        encode_plaintext(106, &payload, &mut wire).unwrap();
        encode_plaintext(7, &[], &mut wire).unwrap();
        // 0x00, len 256 = [0x80, 0x02], type 106 = [0x6a]
        assert_eq!(&wire[..4], &[0x00, 0x80, 0x02, 0x6a]);

        let mut codec = Codec::Plaintext;
        let mut buf = Vec::new();
        let mut out = Vec::new();
        // Feed one byte at a time.
        for &b in &wire {
            buf.push(b);
            while let Some(msg) = codec.decode(&mut buf).unwrap() {
                out.push(msg);
            }
        }
        assert!(buf.is_empty());
        assert_eq!(
            out,
            [
                RawMessage {
                    type_id: 106,
                    payload
                },
                RawMessage {
                    type_id: 7,
                    payload: vec![]
                }
            ]
        );
    }

    #[test]
    fn plaintext_rejects_noise_and_oversize() {
        assert!(matches!(
            decode_plaintext(&[0x01, 0x00, 0x00]),
            Err(FrameError::BadPreamble(1))
        ));
        let mut big = vec![0x00];
        varint::encode(u32::try_from(MAX_PAYLOAD + 1).unwrap(), &mut big);
        assert!(matches!(
            decode_plaintext(&big),
            Err(FrameError::TooLarge(_))
        ));
        let mut out = Vec::new();
        assert!(encode_plaintext(1, &vec![0; MAX_PAYLOAD + 1], &mut out).is_err());
    }

    #[test]
    fn noise_outer_frame() {
        let mut wire = Vec::new();
        encode_noise_frame(b"abc", &mut wire).unwrap();
        assert_eq!(wire, [0x01, 0x00, 0x03, b'a', b'b', b'c']);
        assert_eq!(decode_noise_frame(&wire[..4]).unwrap(), None);
        assert_eq!(decode_noise_frame(&wire).unwrap(), Some((&b"abc"[..], 6)));
        // Client hello is an empty frame.
        assert_eq!(
            decode_noise_frame(&[0x01, 0x00, 0x00]).unwrap(),
            Some((&b""[..], 3))
        );
        assert!(matches!(
            decode_noise_frame(&[0x00, 0x01]),
            Err(FrameError::BadPreamble(0))
        ));
    }

    #[test]
    fn server_hello_layout() {
        assert_eq!(
            server_hello_body("tv", "02aabbccddee"),
            b"\x01tv\x0002aabbccddee\x00".to_vec()
        );
        assert_eq!(handshake_reject_body("x"), b"\x01x".to_vec());
    }

    #[test]
    fn noise_round_trip_both_directions_with_partial_reads() {
        let (mut client, server) = established(&PSK).unwrap();
        let mut codec = Codec::Noise(Box::new(server));

        // Client -> device: inner header type/len BE, as aioesphomeapi builds it.
        let payload = b"hello voice".to_vec();
        let mut inner = vec![0x00, 106, 0x00, u8::try_from(payload.len()).unwrap()];
        inner.extend_from_slice(&payload);
        let mut cipher = vec![0_u8; inner.len() + 16];
        let n = client.write_message(&inner, &mut cipher).unwrap();
        let mut wire = Vec::new();
        encode_noise_frame(&cipher[..n], &mut wire).unwrap();

        let mut buf = Vec::new();
        let mut got = None;
        for &b in &wire {
            buf.push(b);
            if let Some(msg) = codec.decode(&mut buf).unwrap() {
                got = Some(msg);
            }
        }
        assert_eq!(
            got,
            Some(RawMessage {
                type_id: 106,
                payload
            })
        );
        assert!(buf.is_empty());

        // Device -> client.
        let mut out = Vec::new();
        codec.encode(0x0102, b"xyz", &mut out).unwrap();
        let (body, used) = decode_noise_frame(&out).unwrap().unwrap();
        assert_eq!(used, out.len());
        let mut plain = vec![0_u8; body.len()];
        let n = client.read_message(body, &mut plain).unwrap();
        assert_eq!(&plain[..n], &[0x01, 0x02, 0x00, 0x03, b'x', b'y', b'z']);
    }

    #[test]
    fn noise_wrong_psk_fails_handshake() {
        assert!(matches!(established(&[8; 32]), Err(FrameError::Noise(_))));
    }

    #[test]
    fn noise_tampered_frame_fails() {
        let (mut client, server) = established(&PSK).unwrap();
        let mut codec = Codec::Noise(Box::new(server));
        let mut cipher = vec![0_u8; 4 + 16];
        let n = client.write_message(&[0, 7, 0, 0], &mut cipher).unwrap();
        cipher[0] ^= 0xff;
        let mut buf = Vec::new();
        encode_noise_frame(&cipher[..n], &mut buf).unwrap();
        assert!(matches!(codec.decode(&mut buf), Err(FrameError::Noise(_))));
    }

    #[test]
    fn psk_decoding() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(PSK);
        assert_eq!(decode_psk(&b64), Ok(PSK));
        assert!(decode_psk("not base64!").is_err());
        assert!(decode_psk("AAAA").is_err());
    }
}
