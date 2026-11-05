//! Shared MQTT 3.1.1 encode + TCP/TLS connect helpers (no MQTT crate).
//!
//! Used by weather subscribe and Home Assistant discovery publish.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub fn encode_remaining_length(mut len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
    out
}

pub fn mqtt_str(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(2 + b.len());
    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    out.extend_from_slice(b);
    out
}

pub fn build_connect(client_id: &str, username: &str, password: &str) -> Vec<u8> {
    let mut flags = 0u8;
    let mut payload = mqtt_str(client_id);
    if !username.is_empty() {
        flags |= 0x80;
        payload.extend(mqtt_str(username));
        if !password.is_empty() {
            flags |= 0x40;
            payload.extend(mqtt_str(password));
        }
    }
    let mut vh = Vec::new();
    vh.extend(mqtt_str("MQTT"));
    vh.push(4); // protocol level 3.1.1
    vh.push(flags);
    vh.extend_from_slice(&30u16.to_be_bytes()); // keep alive
    let mut packet = Vec::new();
    packet.push(0x10); // CONNECT
    let rem = vh.len() + payload.len();
    packet.extend(encode_remaining_length(rem));
    packet.extend(vh);
    packet.extend(payload);
    packet
}

pub fn build_subscribe(packet_id: u16, topic: &str) -> Vec<u8> {
    let mut rem_payload = Vec::new();
    rem_payload.extend_from_slice(&packet_id.to_be_bytes());
    rem_payload.extend(mqtt_str(topic));
    rem_payload.push(0); // QoS 0
    let mut packet = Vec::new();
    packet.push(0x82); // SUBSCRIBE QoS1
    packet.extend(encode_remaining_length(rem_payload.len()));
    packet.extend(rem_payload);
    packet
}

pub fn build_pingreq() -> Vec<u8> {
    vec![0xC0, 0x00]
}

pub fn parse_publish(flags: u8, body: &[u8]) -> Result<(String, Vec<u8>), String> {
    if body.len() < 2 {
        return Err("short publish".into());
    }
    let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
    if body.len() < 2 + tlen {
        return Err("short topic".into());
    }
    let topic = String::from_utf8_lossy(&body[2..2 + tlen]).into_owned();
    let qos = (flags >> 1) & 0x03;
    let mut offset = 2 + tlen;
    if qos > 0 {
        if body.len() < offset + 2 {
            return Err("short packet id".into());
        }
        offset += 2;
    }
    Ok((topic, body[offset..].to_vec()))
}

/// QoS 0 PUBLISH. When `retain` is true, bit 0 of the fixed header flags is set.
pub fn build_publish(topic: &str, payload: &[u8], retain: bool) -> Vec<u8> {
    let mut rem = mqtt_str(topic);
    rem.extend_from_slice(payload);
    let mut packet = Vec::new();
    let mut fixed = 0x30u8; // PUBLISH QoS0
    if retain {
        fixed |= 0x01;
    }
    packet.push(fixed);
    packet.extend(encode_remaining_length(rem.len()));
    packet.extend(rem);
    packet
}

pub async fn read_packet<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<(u8, Vec<u8>), String> {
    let mut fixed = [0u8; 1];
    reader
        .read_exact(&mut fixed)
        .await
        .map_err(|e| e.to_string())?;
    let mut multiplier = 1usize;
    let mut remaining = 0usize;
    loop {
        let mut b = [0u8; 1];
        reader
            .read_exact(&mut b)
            .await
            .map_err(|e| e.to_string())?;
        remaining += (b[0] as usize & 0x7F) * multiplier;
        if b[0] & 0x80 == 0 {
            break;
        }
        multiplier *= 128;
        if multiplier > 128 * 128 * 128 {
            return Err("remaining length overflow".into());
        }
    }
    let mut payload = vec![0u8; remaining];
    if remaining > 0 {
        reader
            .read_exact(&mut payload)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok((fixed[0], payload))
}

pub enum StreamKind {
    Plain(TcpStream),
    Tls(tokio_rustls::client::TlsStream<TcpStream>),
}

impl StreamKind {
    pub async fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        match self {
            Self::Plain(s) => s.write_all(data).await.map_err(|e| e.to_string()),
            Self::Tls(s) => s.write_all(data).await.map_err(|e| e.to_string()),
        }
    }

    pub async fn read_packet(&mut self) -> Result<(u8, Vec<u8>), String> {
        match self {
            Self::Plain(s) => read_packet(s).await,
            Self::Tls(s) => read_packet(s).await,
        }
    }
}

pub async fn connect_stream(broker: &str, port: u16, tls: bool) -> Result<StreamKind, String> {
    let addr = format!("{broker}:{port}");
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("connect {addr}: {e}"))?;
    if !tls {
        return Ok(StreamKind::Plain(tcp));
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let server_name = rustls::pki_types::ServerName::try_from(broker.to_string())
        .map_err(|e| format!("TLS server name: {e}"))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| format!("TLS: {e}"))?;
    Ok(StreamKind::Tls(tls))
}

/// CONNECT + wait for successful CONNACK.
pub async fn mqtt_handshake(
    stream: &mut StreamKind,
    client_id: &str,
    username: &str,
    password: &str,
) -> Result<(), String> {
    stream
        .write_all(&build_connect(client_id, username, password))
        .await?;
    let (fixed, body) = tokio::time::timeout(Duration::from_secs(10), stream.read_packet())
        .await
        .map_err(|_| "CONNACK timeout".to_string())??;
    if fixed & 0xF0 != 0x20 || body.len() < 2 || body[1] != 0 {
        return Err(format!(
            "CONNACK failed (type={fixed:#x} code={})",
            body.get(1).copied().unwrap_or(255)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_sets_retain_bit() {
        let plain = build_publish("a/b", b"1", false);
        assert_eq!(plain[0], 0x30);
        let retained = build_publish("a/b", b"1", true);
        assert_eq!(retained[0], 0x31);
        // topic length + "a/b" + payload
        assert!(retained.len() > 5);
        assert!(retained.windows(3).any(|w| w == b"a/b"));
        assert_eq!(*retained.last().unwrap(), b'1');
    }

    #[test]
    fn remaining_length_encodes_small() {
        assert_eq!(encode_remaining_length(0), vec![0]);
        assert_eq!(encode_remaining_length(127), vec![127]);
        assert_eq!(encode_remaining_length(128), vec![0x80, 0x01]);
    }
}
