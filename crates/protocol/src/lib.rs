//! JSON protocol shared between the Windows desktop client and the signaling server.
//! Media is NOT transported over WebSocket; only signaling/control messages are.
use serde::{Deserialize, Serialize};

pub const ROOM_CAPACITY: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalPayload {
    Offer { sdp: String },
    Answer { sdp: String },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    CreateRoom { name: String },
    JoinRoom { room_id: String, name: String },
    Signal { to: String, payload: SignalPayload },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    RoomCreated { room_id: String, peer_id: String },
    RoomJoined { room_id: String, peer_id: String, peers: Vec<PeerInfo> },
    PeerJoined { peer: PeerInfo },
    PeerLeft { peer_id: String },
    /// Credenciais temporarias para STUN/TURN. Nunca contem o segredo compartilhado.
    TurnCredentials {
        urls: Vec<String>,
        username: String,
        credential: String,
        expires_at: u64,
    },
    Signal { from: String, payload: SignalPayload },
    Error { code: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_client_command() {
        let cmd = ClientMessage::CreateRoom { name: "Pessoa A".into() };
        assert_eq!(serde_json::to_string(&cmd).unwrap(), r#"{"type":"create_room","name":"Pessoa A"}"#);
    }

    #[test]
    fn serialize_ice_candidate() {
        let signal = SignalPayload::IceCandidate {
            candidate: "candidate:1 1 udp ...".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        };
        let json = serde_json::to_string(&signal).unwrap();
        assert_eq!(serde_json::from_str::<SignalPayload>(&json).unwrap(), signal);
    }
}
