//! Bounded framing for the untrusted delivery service. Content is MLS ciphertext.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const MAX_FRAME: usize = 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Packet {
    Ping,
    Join {
        package: Vec<u8>,
    },
    Welcome {
        recipient: u64,
        data: Vec<u8>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        key: Vec<u8>,
    },
    Commit {
        data: Vec<u8>,
    },
    Content {
        data: Vec<u8>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<Vec<u8>>,
    },
    Voice {
        #[serde(with = "crate::wire::binary")]
        data: Vec<u8>,
    },
    Pending {
        connection: u64,
        package: Vec<u8>,
    },
    Rejected,
    Disconnect {
        connection: u64,
    },
}

pub fn write_packet(writer: &mut impl Write, packet: &Packet) -> Result<()> {
    let bytes = serde_json::to_vec(packet)?;
    ensure!(bytes.len() <= MAX_FRAME, "Сетевой пакет слишком большой");
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

pub fn read_packet(reader: &mut impl Read) -> Result<Packet> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    ensure!(
        len > 0 && len <= MAX_FRAME,
        "Недопустимый размер сетевого пакета"
    );
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_roundtrip_rejects_oversize_and_truncation() {
        let packet = Packet::Content {
            data: vec![1, 2, 3],
            targets: Vec::new(),
        };
        let mut bytes = Vec::new();
        write_packet(&mut bytes, &packet).unwrap();
        assert_eq!(read_packet(&mut bytes.as_slice()).unwrap(), packet);
        assert!(read_packet(&mut &bytes[..bytes.len() - 1]).is_err());
        assert!(read_packet(&mut &(u32::MAX.to_be_bytes())[..]).is_err());
        assert!(read_packet(&mut &[0, 0, 0, 0][..]).is_err());
    }
}

pub mod binary {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(data: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(data))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        STANDARD
            .decode(String::deserialize(d)?)
            .map_err(serde::de::Error::custom)
    }
}
