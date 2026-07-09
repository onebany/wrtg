use aes::Aes256;
use ctr::cipher::StreamCipher;
use ctr::Ctr128BE;

use crate::mtproto::{
    new_relay_decrypt_stream, MAX_MTPROTO_PACKET, PROTO_ABRIDGED_INT, PROTO_INTERMEDIATE_INT,
    PROTO_PADDED_INTERMEDIATE_INT,
};

type Aes256Ctr = Ctr128BE<Aes256>;

pub struct MsgSplitter {
    dec: Aes256Ctr,
    proto: u32,
    cipher_buf: Vec<u8>,
    plain_buf: Vec<u8>,
    disabled: bool,
}

impl MsgSplitter {
    pub fn new(relay_init: &[u8], proto_int: u32) -> Result<Self, String> {
        let dec = new_relay_decrypt_stream(relay_init)?;
        Ok(Self {
            dec,
            proto: proto_int,
            cipher_buf: Vec::new(),
            plain_buf: Vec::new(),
            disabled: false,
        })
    }

    pub fn split(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        if chunk.is_empty() {
            return Vec::new();
        }
        if self.disabled {
            return vec![chunk.to_vec()];
        }

        self.cipher_buf.extend_from_slice(chunk);
        let mut plain_chunk = chunk.to_vec();
        self.dec.apply_keystream(&mut plain_chunk);
        self.plain_buf.extend_from_slice(&plain_chunk);

        let mut parts = Vec::new();
        let mut offset = 0;
        let buf_len = self.cipher_buf.len();

        while offset < buf_len {
            let packet_len = self.next_packet_len(offset, buf_len - offset);
            match packet_len {
                -1 => break,
                0 => {
                    parts.push(self.cipher_buf[offset..].to_vec());
                    offset = buf_len;
                    self.disabled = true;
                    break;
                }
                n => {
                    let n = n as usize;
                    parts.push(self.cipher_buf[offset..offset + n].to_vec());
                    offset += n;
                }
            }
        }

        if offset > 0 {
            self.cipher_buf = self.cipher_buf[offset..].to_vec();
            self.plain_buf = self.plain_buf[offset..].to_vec();
        }
        parts
    }

    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        if self.cipher_buf.is_empty() {
            return Vec::new();
        }
        let tail = std::mem::take(&mut self.cipher_buf);
        self.plain_buf.clear();
        vec![tail]
    }

    fn next_packet_len(&self, offset: usize, avail: usize) -> i32 {
        if avail == 0 {
            return -1;
        }
        match self.proto {
            PROTO_ABRIDGED_INT => self.next_abridged_len(offset, avail),
            PROTO_INTERMEDIATE_INT | PROTO_PADDED_INTERMEDIATE_INT => {
                self.next_intermediate_len(offset, avail)
            }
            _ => 0,
        }
    }

    fn next_abridged_len(&self, offset: usize, avail: usize) -> i32 {
        let first = self.plain_buf[offset];
        if first == 0x7F || first == 0xFF {
            if avail < 4 {
                return -1;
            }
            let b = &self.plain_buf[offset + 1..offset + 4];
            let payload_len = (b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16) * 4;
            let packet_len = 4 + payload_len;
            if packet_len > MAX_MTPROTO_PACKET {
                return 0;
            }
            if avail < packet_len {
                return -1;
            }
            return packet_len as i32;
        }
        let payload_len = (first & 0x7F) as usize * 4;
        if payload_len == 0 {
            return 0;
        }
        let packet_len = 1 + payload_len;
        if packet_len > MAX_MTPROTO_PACKET {
            return 0;
        }
        if avail < packet_len {
            return -1;
        }
        packet_len as i32
    }

    fn next_intermediate_len(&self, offset: usize, avail: usize) -> i32 {
        if avail < 4 {
            return -1;
        }
        let payload_len =
            u32::from_le_bytes(self.plain_buf[offset..offset + 4].try_into().unwrap()) as usize
                & 0x7FFF_FFFF;
        if payload_len == 0 {
            return 0;
        }
        let packet_len = 4 + payload_len;
        if packet_len > MAX_MTPROTO_PACKET {
            return 0;
        }
        if avail < packet_len {
            return -1;
        }
        packet_len as i32
    }
}
