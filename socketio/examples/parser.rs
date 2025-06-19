use std::time::Duration;

use bytes::Bytes;
use ciborium::Value;
use rust_socketio::{ClientBuilder, Packet, PacketId, PacketParser, Payload, RawClient};
use serde::{Deserialize, Serialize};
use serde_json::json;

fn main() {
    // define a callback which is called when a payload is received
    // this callback gets the payload as well as an instance of the
    // socket to communicate with the server
    let callback = |payload: Payload, socket: RawClient| {
        match payload {
            #[allow(deprecated)]
            Payload::String(str) => println!("Received: {}", str),
            Payload::Text(text) => println!("Received json: {:#?}", text),
            Payload::Binary(bin_data) => println!("Received bytes: {:#?}", bin_data),
        }
        socket
            .emit("test", json!({"got ack": true}))
            .expect("Server unreachable")
    };

    // get a socket that is connected to the admin namespace
    let socket = ClientBuilder::new("http://localhost:4200")
        .namespace("/admin")
        .parser(CborPacketParser)
        .on("test", callback)
        .on("error", |err, _| eprintln!("Error: {:#?}", err))
        .connect()
        .expect("Connection failed");

    // emit to the "foo" event
    let json_payload = json!({"token": 123});
    socket
        .emit("foo", json_payload)
        .expect("Server unreachable");

    // define a callback, that's executed when the ack got acked
    let ack_callback = |message: Payload, _| {
        println!("Yehaa! My ack got acked?");
        println!("Ack data: {:#?}", message);
    };

    let json_payload = json!({"myAckData": 123});
    // emit with an ack

    socket
        .emit_with_ack("test", json_payload, Duration::from_secs(2), ack_callback)
        .expect("Server unreachable");

    socket.disconnect().expect("Disconnect failed")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CborPacket {
    r#type: u8,
    nsp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<ciborium::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<i32>,
    #[serde(default, skip_serializing_if = "is_zero")]
    attachments: u8,
}

fn is_zero(val: &u8) -> bool {
    *val == 0
}

impl From<&Packet> for CborPacket {
    fn from(value: &Packet) -> Self {
        Self {
            r#type: u8::from(value.packet_type),
            nsp: value.nsp.clone(),
            data: if let Some(data) = value.data.clone() {
                json_to_cbor(data)
            } else {
                value.attachments.as_ref().map(|attachments| {
                    Value::Array(
                        attachments
                            .iter()
                            .map(|attachment| Value::from(&attachment[..]))
                            .collect(),
                    )
                })
            },
            id: value.id,
            attachments: value.attachment_count,
        }
    }
}

impl TryFrom<CborPacket> for Packet {
    type Error = rust_socketio::Error;

    fn try_from(
        CborPacket {
            r#type,
            nsp,
            data,
            id,
            attachments,
        }: CborPacket,
    ) -> Result<Self, Self::Error> {
        println!("try_from: type={type}");
        let packet_type = PacketId::try_from(r#type)?;
        println!("try_from: nsp={nsp}");
        if !nsp.starts_with("/") {
            return Err(rust_socketio::Error::InvalidPacket());
        }

        let mut packet = Packet::default();
        packet.packet_type = packet_type;
        packet.nsp = nsp;
        packet.id = id;
        packet.attachment_count = attachments;

        println!("try_from: data={:?}", data);
        if let Some(Value::Map(_)) = data {
            let json: serde_json::Value = data
                .unwrap()
                .deserialized()
                .map_err(|e| rust_socketio::Error::PacketParserError(e.to_string()))?;

            packet.data = Some(serde_json::to_string(&json)?);
        } else if let Some(Value::Array(data)) = data {
            let mut packet_data: Vec<Value> = Vec::new();
            let mut attachments: Vec<Bytes> = Vec::new();

            for value in data {
                if let Value::Bytes(bytes) = value {
                    attachments.push(bytes.into());
                } else {
                    packet_data.push(value);
                }
            }

            let data_json: serde_json::Value = Value::Array(packet_data)
                .deserialized()
                .map_err(|e| rust_socketio::Error::PacketParserError(e.to_string()))?;

            packet.data = Some(serde_json::to_string(&data_json)?);
            packet.attachment_count = attachments.len() as u8;
            packet.attachments = Some(attachments);
        } else {
            return Err(rust_socketio::Error::InvalidPacket());
        }

        println!("decode success {:?}", packet);
        Ok(packet)
    }
}

pub struct CborPacketParser;

impl PacketParser for CborPacketParser {
    fn is_binary(&self) -> bool {
        true
    }

    fn encode(&self, packet: &Packet) -> Bytes {
        println!("encode {:?}", packet);
        let cbor_packet = CborPacket::from(packet);
        let mut buffer: Vec<u8> = Vec::new();
        ciborium::into_writer(&cbor_packet, &mut buffer).expect("can't encode cbor packet");
        Bytes::from(buffer)
    }

    fn decode(&self, payload: &Bytes) -> Result<Packet, rust_socketio::Error> {
        println!("decode {:?}", payload);
        println!(
            "decode {:?}",
            payload
                .iter()
                .map(|b| format!("{:02x}", b).to_string())
                .collect::<Vec<String>>()
                .join("")
        );
        let cbor_packet: CborPacket = ciborium::from_reader(&payload[..]).map_err(|e| {
            println!("decode err: {:?}", e);
            rust_socketio::Error::InvalidPacket()
        })?;

        println!("decoded {:?}", cbor_packet);
        Packet::try_from(cbor_packet)
    }
}

fn json_to_cbor(value: String) -> Option<Value> {
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&value) {
        let mut buffer: Vec<u8> = Vec::new();
        ciborium::into_writer(&data, &mut buffer).expect("can't encode cbor packet");
        ciborium::from_reader(&buffer[..]).ok()
    } else {
        Some(Value::from(value))
    }
}
