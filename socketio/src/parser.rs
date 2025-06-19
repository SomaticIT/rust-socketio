use crate::{
    error::Result,
    packet::{Packet, PacketId},
    Error,
};
use bytes::Bytes;
use rust_engineio::PacketId as EnginePacketId;
use serde::de::IgnoredAny;
use std::fmt::{Debug, Write};
use std::str::from_utf8 as str_from_utf8;

pub trait PacketParser {
    fn is_binary(&self) -> bool;
    fn encode(&self, packet: &Packet) -> Bytes;
    fn decode(&self, payload: &Bytes) -> Result<Packet>;

    fn message_packet_id(&self) -> EnginePacketId {
        if self.is_binary() {
            EnginePacketId::MessageBinary
        } else {
            EnginePacketId::Message
        }
    }
}

impl Debug for dyn PacketParser + Send + Sync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PacketParser").finish()
    }
}

pub struct DefaultPacketParser;

impl PacketParser for DefaultPacketParser {
    fn is_binary(&self) -> bool {
        false
    }

    fn encode(&self, packet: &Packet) -> Bytes {
        // first the packet type
        let mut buffer = String::new();
        buffer.push((packet.packet_type as u8 + b'0') as char);

        // eventually a number of attachments, followed by '-'
        if let PacketId::BinaryAck | PacketId::BinaryEvent = packet.packet_type {
            let _ = write!(buffer, "{}-", packet.attachment_count);
        }

        // if the namespace is different from the default one append it as well,
        // followed by ','
        if packet.nsp != "/" {
            buffer.push_str(&packet.nsp);
            buffer.push(',');
        }

        // if an id is present append it...
        if let Some(id) = packet.id {
            let _ = write!(buffer, "{id}");
        }

        if packet.attachments.is_some() {
            let num = packet.attachment_count - 1;

            // check if an event type is present
            if let Some(event_type) = packet.data.as_ref() {
                let _ = write!(
                    buffer,
                    "[{event_type},{{\"_placeholder\":true,\"num\":{num}}}]",
                );
            } else {
                let _ = write!(buffer, "[{{\"_placeholder\":true,\"num\":{num}}}]");
            }
        } else if let Some(data) = packet.data.as_ref() {
            buffer.push_str(data);
        }

        Bytes::from(buffer)
    }

    fn decode(&self, payload: &Bytes) -> Result<Packet> {
        let mut payload = str_from_utf8(&payload).map_err(Error::InvalidUtf8)?;
        let mut packet = Packet::default();

        // packet_type
        let id_char = payload.chars().next().ok_or(Error::IncompletePacket())?;
        packet.packet_type = PacketId::try_from(id_char)?;
        payload = &payload[id_char.len_utf8()..];

        // attachment_count
        if let PacketId::BinaryAck | PacketId::BinaryEvent = packet.packet_type {
            let (prefix, rest) = payload.split_once('-').ok_or(Error::IncompletePacket())?;
            payload = rest;
            packet.attachment_count = prefix.parse().map_err(|_| Error::InvalidPacket())?;
        }

        // namespace
        if payload.starts_with('/') {
            let (prefix, rest) = payload.split_once(',').ok_or(Error::IncompletePacket())?;
            payload = rest;
            packet.nsp.clear(); // clearing the default
            packet.nsp.push_str(prefix);
        }

        // id
        let Some((non_digit_idx, _)) = payload.char_indices().find(|(_, c)| !c.is_ascii_digit())
        else {
            return Ok(packet);
        };

        if non_digit_idx > 0 {
            let (prefix, rest) = payload.split_at(non_digit_idx);
            payload = rest;
            packet.id = Some(prefix.parse().map_err(|_| Error::InvalidPacket())?);
        }

        // validate json
        serde_json::from_str::<IgnoredAny>(payload).map_err(Error::InvalidJson)?;

        match packet.packet_type {
            PacketId::BinaryAck | PacketId::BinaryEvent => {
                if payload.starts_with('[') && payload.ends_with(']') {
                    payload = &payload[1..payload.len() - 1];
                }

                let mut str = payload.replace("{\"_placeholder\":true,\"num\":0}", "");

                if str.ends_with(',') {
                    str.pop();
                }

                if !str.is_empty() {
                    packet.data = Some(str);
                }
            }
            _ => packet.data = Some(payload.to_string()),
        }

        Ok(packet)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    /// This test suite is taken from the explanation section here:
    /// https://github.com/socketio/socket.io-protocol
    fn test_decode() {
        let parser = DefaultPacketParser;
        let payload = Bytes::from_static(b"0{\"token\":\"123\"}");
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::Connect,
                "/".to_owned(),
                Some(String::from("{\"token\":\"123\"}")),
                None,
                0,
                None,
            ),
            packet.unwrap()
        );

        let utf8_data = "{\"token™\":\"123\"}".to_owned();
        let utf8_payload = format!("0/admin™,{}", utf8_data);
        let payload = Bytes::from(utf8_payload);
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::Connect,
                "/admin™".to_owned(),
                Some(utf8_data),
                None,
                0,
                None,
            ),
            packet.unwrap()
        );

        let payload = Bytes::from_static(b"1/admin,");
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::Disconnect,
                "/admin".to_owned(),
                None,
                None,
                0,
                None,
            ),
            packet.unwrap()
        );

        let payload = Bytes::from_static(b"2[\"hello\",1]");
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::Event,
                "/".to_owned(),
                Some(String::from("[\"hello\",1]")),
                None,
                0,
                None,
            ),
            packet.unwrap()
        );

        let payload = Bytes::from_static(b"2/admin,456[\"project:delete\",123]");
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::Event,
                "/admin".to_owned(),
                Some(String::from("[\"project:delete\",123]")),
                Some(456),
                0,
                None,
            ),
            packet.unwrap()
        );

        let payload = Bytes::from_static(b"3/admin,456[]");
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::Ack,
                "/admin".to_owned(),
                Some(String::from("[]")),
                Some(456),
                0,
                None,
            ),
            packet.unwrap()
        );

        let payload = Bytes::from_static(b"4/admin,{\"message\":\"Not authorized\"}");
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::ConnectError,
                "/admin".to_owned(),
                Some(String::from("{\"message\":\"Not authorized\"}")),
                None,
                0,
                None,
            ),
            packet.unwrap()
        );

        let payload = Bytes::from_static(b"51-[\"hello\",{\"_placeholder\":true,\"num\":0}]");
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::BinaryEvent,
                "/".to_owned(),
                Some(String::from("\"hello\"")),
                None,
                1,
                None,
            ),
            packet.unwrap()
        );

        let payload = Bytes::from_static(
            b"51-/admin,456[\"project:delete\",{\"_placeholder\":true,\"num\":0}]",
        );
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::BinaryEvent,
                "/admin".to_owned(),
                Some(String::from("\"project:delete\"")),
                Some(456),
                1,
                None,
            ),
            packet.unwrap()
        );

        let payload = Bytes::from_static(b"61-/admin,456[{\"_placeholder\":true,\"num\":0}]");
        let packet = parser.decode(&payload);
        assert!(packet.is_ok());

        assert_eq!(
            Packet::new(
                PacketId::BinaryAck,
                "/admin".to_owned(),
                None,
                Some(456),
                1,
                None,
            ),
            packet.unwrap()
        );
    }

    #[test]
    /// This test suites is taken from the explanation section here:
    /// https://github.com/socketio/socket.io-protocol
    fn test_encode() {
        let parser = DefaultPacketParser;
        let packet = Packet::new(
            PacketId::Connect,
            "/".to_owned(),
            Some(String::from("{\"token\":\"123\"}")),
            None,
            0,
            None,
        );

        assert_eq!(
            parser.encode(&packet),
            "0{\"token\":\"123\"}".to_string().into_bytes()
        );

        let packet = Packet::new(
            PacketId::Connect,
            "/admin".to_owned(),
            Some(String::from("{\"token\":\"123\"}")),
            None,
            0,
            None,
        );

        assert_eq!(
            parser.encode(&packet),
            "0/admin,{\"token\":\"123\"}".to_string().into_bytes()
        );

        let packet = Packet::new(
            PacketId::Disconnect,
            "/admin".to_owned(),
            None,
            None,
            0,
            None,
        );

        assert_eq!(parser.encode(&packet), "1/admin,".to_string().into_bytes());

        let packet = Packet::new(
            PacketId::Event,
            "/".to_owned(),
            Some(String::from("[\"hello\",1]")),
            None,
            0,
            None,
        );

        assert_eq!(
            parser.encode(&packet),
            "2[\"hello\",1]".to_string().into_bytes()
        );

        let packet = Packet::new(
            PacketId::Event,
            "/admin".to_owned(),
            Some(String::from("[\"project:delete\",123]")),
            Some(456),
            0,
            None,
        );

        assert_eq!(
            parser.encode(&packet),
            "2/admin,456[\"project:delete\",123]"
                .to_string()
                .into_bytes()
        );

        let packet = Packet::new(
            PacketId::Ack,
            "/admin".to_owned(),
            Some(String::from("[]")),
            Some(456),
            0,
            None,
        );

        assert_eq!(
            parser.encode(&packet),
            "3/admin,456[]".to_string().into_bytes()
        );

        let packet = Packet::new(
            PacketId::ConnectError,
            "/admin".to_owned(),
            Some(String::from("{\"message\":\"Not authorized\"}")),
            None,
            0,
            None,
        );

        assert_eq!(
            parser.encode(&packet),
            "4/admin,{\"message\":\"Not authorized\"}"
                .to_string()
                .into_bytes()
        );

        let packet = Packet::new(
            PacketId::BinaryEvent,
            "/".to_owned(),
            Some(String::from("\"hello\"")),
            None,
            1,
            Some(vec![Bytes::from_static(&[1, 2, 3])]),
        );

        assert_eq!(
            parser.encode(&packet),
            "51-[\"hello\",{\"_placeholder\":true,\"num\":0}]"
                .to_string()
                .into_bytes()
        );

        let packet = Packet::new(
            PacketId::BinaryEvent,
            "/admin".to_owned(),
            Some(String::from("\"project:delete\"")),
            Some(456),
            1,
            Some(vec![Bytes::from_static(&[1, 2, 3])]),
        );

        assert_eq!(
            parser.encode(&packet),
            "51-/admin,456[\"project:delete\",{\"_placeholder\":true,\"num\":0}]"
                .to_string()
                .into_bytes()
        );

        let packet = Packet::new(
            PacketId::BinaryAck,
            "/admin".to_owned(),
            None,
            Some(456),
            1,
            Some(vec![Bytes::from_static(&[3, 2, 1])]),
        );

        assert_eq!(
            parser.encode(&packet),
            "61-/admin,456[{\"_placeholder\":true,\"num\":0}]"
                .to_string()
                .into_bytes()
        );
    }
}
