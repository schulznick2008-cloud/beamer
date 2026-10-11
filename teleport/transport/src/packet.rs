//! Paketformat nach SCHNITTSTELLEN.md 4.1: 16-Byte-Header (zugleich AAD), ChaCha20-Poly1305-Ciphertext.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 16;
pub const TAG_LEN: usize = 16;
/// Groesse eines Datagramms inklusive Header und Tag.
pub const MAX_PACKET: usize = 1200;
/// Groesster Klartext eines Pakets.
pub const MAX_PLAINTEXT: usize = MAX_PACKET - HEADER_LEN - TAG_LEN;

pub const T_VIDEO: u8 = 1;
pub const T_AUDIO: u8 = 2;
pub const T_FEC: u8 = 3;
pub const T_NACK: u8 = 4;
pub const T_KEYFRAME_REQ: u8 = 5;
pub const T_FEEDBACK: u8 = 6;
pub const T_PING: u8 = 7;
pub const T_PONG: u8 = 8;

/// Klartext-Kopf von video/audio: frame_id, frag_idx, frag_cnt, pts_us, fflags.
pub const MEDIA_HEAD: usize = 4 + 2 + 2 + 8 + 1;
/// Klartext-Kopf von fec: frame_id, group_start, group_len, XOR der Quelllaengen.
pub const FEC_HEAD: usize = 4 + 2 + 2 + 2;
/// Groesste Nutzlast je Fragment. So gewaehlt, dass auch das FEC-Paket (Kopf + Paritaet ueber das
/// groesste Quellpaket) noch in MAX_PLAINTEXT passt.
pub const MAX_FRAG_PAYLOAD: usize = MAX_PLAINTEXT - FEC_HEAD - MEDIA_HEAD;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub ptype: u8,
    pub flags: u16,
    pub stream_id: u32,
    pub seq: u64,
}

pub fn encode_header(h: &Header) -> [u8; HEADER_LEN] {
    let mut b = [0u8; HEADER_LEN];
    b[0] = VERSION;
    b[1] = h.ptype;
    b[2..4].copy_from_slice(&h.flags.to_be_bytes());
    b[4..8].copy_from_slice(&h.stream_id.to_be_bytes());
    b[8..16].copy_from_slice(&h.seq.to_be_bytes());
    b
}

/// Liest den Header. `None` bei falscher Laenge, Version oder unbekanntem Typ.
pub fn decode_header(d: &[u8]) -> Option<Header> {
    if d.len() < HEADER_LEN + TAG_LEN || d.len() > MAX_PACKET || d[0] != VERSION {
        return None;
    }
    let ptype = d[1];
    if !(T_VIDEO..=T_PONG).contains(&ptype) {
        return None;
    }
    Some(Header {
        ptype,
        flags: u16::from_be_bytes([d[2], d[3]]),
        stream_id: u32::from_be_bytes([d[4], d[5], d[6], d[7]]),
        seq: u64::from_be_bytes(d[8..16].try_into().ok()?),
    })
}

/// Eine Richtung: Schluessel plus Nonce-Praefix.
pub struct Cipher {
    aead: ChaCha20Poly1305,
    prefix: [u8; 4],
}

impl Cipher {
    pub fn new(key: &[u8; 32], prefix: &[u8; 4]) -> Self {
        Cipher {
            aead: ChaCha20Poly1305::new(Key::from_slice(key)),
            prefix: *prefix,
        }
    }

    fn nonce(&self, seq: u64) -> Nonce {
        let mut n = [0u8; 12];
        n[..4].copy_from_slice(&self.prefix);
        n[4..].copy_from_slice(&seq.to_be_bytes());
        *Nonce::from_slice(&n)
    }

    /// Baut ein komplettes Datagramm. `None`, wenn es groesser als MAX_PACKET wuerde.
    pub fn seal(&self, h: &Header, plain: &[u8]) -> Option<Vec<u8>> {
        if plain.len() > MAX_PLAINTEXT {
            return None;
        }
        let head = encode_header(h);
        let ct = self
            .aead
            .encrypt(
                &self.nonce(h.seq),
                Payload {
                    msg: plain,
                    aad: &head,
                },
            )
            .ok()?;
        let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
        out.extend_from_slice(&head);
        out.extend_from_slice(&ct);
        Some(out)
    }

    /// Prueft Tag und AAD und liefert den Klartext. Der Replay-Schutz liegt beim Aufrufer.
    pub fn open(&self, datagram: &[u8], h: &Header) -> Option<Vec<u8>> {
        self.aead
            .decrypt(
                &self.nonce(h.seq),
                Payload {
                    msg: &datagram[HEADER_LEN..],
                    aad: &datagram[..HEADER_LEN],
                },
            )
            .ok()
    }
}

/// Klartext eines video/audio-Pakets.
pub struct MediaHead {
    pub frame_id: u32,
    pub frag_idx: u16,
    pub frag_cnt: u16,
    pub pts_us: u64,
    pub fflags: u8,
}

pub fn encode_media(h: &MediaHead, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(MEDIA_HEAD + payload.len());
    v.extend_from_slice(&h.frame_id.to_be_bytes());
    v.extend_from_slice(&h.frag_idx.to_be_bytes());
    v.extend_from_slice(&h.frag_cnt.to_be_bytes());
    v.extend_from_slice(&h.pts_us.to_be_bytes());
    v.push(h.fflags);
    v.extend_from_slice(payload);
    v
}

pub fn decode_media(p: &[u8]) -> Option<MediaHead> {
    if p.len() < MEDIA_HEAD {
        return None;
    }
    Some(MediaHead {
        frame_id: u32::from_be_bytes(p[0..4].try_into().ok()?),
        frag_idx: u16::from_be_bytes(p[4..6].try_into().ok()?),
        frag_cnt: u16::from_be_bytes(p[6..8].try_into().ok()?),
        pts_us: u64::from_be_bytes(p[8..16].try_into().ok()?),
        fflags: p[16],
    })
}

/// Klartext eines fec-Pakets.
pub struct FecHead {
    pub frame_id: u32,
    pub group_start: u16,
    pub group_len: u16,
    /// XOR der Klartextlaengen aller Quellpakete der Gruppe.
    pub len_xor: u16,
}

pub fn encode_fec(h: &FecHead, parity: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(FEC_HEAD + parity.len());
    v.extend_from_slice(&h.frame_id.to_be_bytes());
    v.extend_from_slice(&h.group_start.to_be_bytes());
    v.extend_from_slice(&h.group_len.to_be_bytes());
    v.extend_from_slice(&h.len_xor.to_be_bytes());
    v.extend_from_slice(parity);
    v
}

pub fn decode_fec(p: &[u8]) -> Option<(FecHead, &[u8])> {
    if p.len() < FEC_HEAD {
        return None;
    }
    Some((
        FecHead {
            frame_id: u32::from_be_bytes(p[0..4].try_into().ok()?),
            group_start: u16::from_be_bytes(p[4..6].try_into().ok()?),
            group_len: u16::from_be_bytes(p[6..8].try_into().ok()?),
            len_xor: u16::from_be_bytes(p[8..10].try_into().ok()?),
        },
        &p[FEC_HEAD..],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frag_payload_fits_fec() {
        let src = MEDIA_HEAD + MAX_FRAG_PAYLOAD;
        assert!(FEC_HEAD + src <= MAX_PLAINTEXT);
        assert_eq!(HEADER_LEN + FEC_HEAD + src + TAG_LEN, MAX_PACKET);
    }

    #[test]
    fn seal_open_roundtrip_and_tamper() {
        let c = Cipher::new(&[7; 32], &[1, 2, 3, 4]);
        let h = Header {
            ptype: T_PING,
            flags: 0,
            stream_id: 9,
            seq: 5,
        };
        let d = c.seal(&h, b"hallo").unwrap();
        assert_eq!(decode_header(&d), Some(h));
        assert_eq!(c.open(&d, &h).unwrap(), b"hallo");
        for i in 0..d.len() {
            let mut bad = d.clone();
            bad[i] ^= 1;
            if let Some(bh) = decode_header(&bad) {
                assert!(c.open(&bad, &bh).is_none(), "Byte {i} unbemerkt geaendert");
            }
        }
    }

    #[test]
    fn too_big_rejected() {
        let c = Cipher::new(&[7; 32], &[0; 4]);
        let h = Header {
            ptype: T_VIDEO,
            flags: 0,
            stream_id: 1,
            seq: 1,
        };
        assert!(c.seal(&h, &vec![0; MAX_PLAINTEXT]).unwrap().len() == MAX_PACKET);
        assert!(c.seal(&h, &vec![0; MAX_PLAINTEXT + 1]).is_none());
    }
}
