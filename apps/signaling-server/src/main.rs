use std::{collections::HashMap, sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};

use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State},
    response::Response,
    routing::{any, get},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use tokio::{net::TcpListener, sync::{mpsc, Mutex}};
use tracing::{info, warn};
use uuid::Uuid;
use voice_protocol::{ClientMessage, PeerInfo, ServerMessage, ROOM_CAPACITY};

#[derive(Clone, Default)]
struct AppState {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    turn: Option<Arc<TurnConfig>>,
}

/// O segredo vem SOMENTE do arquivo privado do Coturn, montado read-only no container.
struct TurnConfig {
    host: String,
    secret: String,
}

impl TurnConfig {
    fn load() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let path = match std::env::var("TURN_SECRET_FILE") {
            Ok(path) => path,
            Err(_) => return Ok(None),
        };
        // Arquivo dedicado 0400 (UID 10001), derivado do segredo do Coturn.
        let file = std::fs::read_to_string(path)?;
        let secret = file.trim();
        if secret.is_empty() { return Err("TURN_SHARED_SECRET vazio".into()); }
        let host = std::env::var("TURN_HOST")?;
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            return Err("TURN_HOST invalido".into());
        }
        Ok(Some(Self { host, secret: secret.to_owned() }))
    }

    fn credentials(&self, peer_id: &str) -> ServerMessage {
        // Coturn REST: usuario = timestamp de expiracao : id; senha = base64(HMAC-SHA1).
        let expires_at = (SystemTime::now() + Duration::from_secs(3600))
            .duration_since(UNIX_EPOCH)
            .expect("Relogio do sistema invalido")
            .as_secs();
        let username = format!("{expires_at}:{peer_id}");
        let mut mac = Hmac::<Sha1>::new_from_slice(self.secret.as_bytes())
            .expect("HMAC aceita chave de qualquer tamanho");
        mac.update(username.as_bytes());
        let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        ServerMessage::TurnCredentials {
            urls: vec![
                format!("stun:{}:3478", self.host),
                format!("turn:{}:3478?transport=udp", self.host),
                format!("turn:{}:3478?transport=tcp", self.host),
            ],
            username,
            credential,
            expires_at,
        }
    }
}


#[derive(Default)]
struct Room {
    peers: HashMap<String, Peer>,
}

struct Peer {
    name: String,
    tx: mpsc::Sender<ServerMessage>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "signaling_server=info".into()))
        .init();

    let turn = TurnConfig::load()?.map(Arc::new);
    if turn.is_some() { info!("Credenciais TURN temporarias habilitadas"); }
    else { warn!("TURN nao configurado: ICE usara apenas STUN publico"); }
    let state = AppState { turn, ..Default::default() };
    let app = Router::new()
        .route("/health", get(|| async { Json(serde_json::json!({"status": "ok"})) }))
        .route("/ws", any(ws_handler))
        .with_state(state);

    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = TcpListener::bind(&addr).await?;
    info!(%addr, "Servidor de sinalizacao iniciado");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| client_session(socket, state))
}

fn valid_name(name: &str) -> bool {
    let n = name.trim();
    (2..=32).contains(&n.chars().count()) && !n.chars().any(char::is_control)
}

fn send_error(tx: &mpsc::Sender<ServerMessage>, code: &str, message: &str) {
    let _ = tx.try_send(ServerMessage::Error {
        code: code.into(), message: message.into(),
    });
}

async fn client_session(socket: WebSocket, state: AppState) {
    let peer_id = Uuid::new_v4().simple().to_string();
    let (mut writer, mut reader) = socket.split();
    let (tx, mut out_rx) = mpsc::channel::<ServerMessage>(64);

    let writer_task = tokio::spawn(async move {
        while let Some(outgoing) = out_rx.recv().await {
            let Ok(json) = serde_json::to_string(&outgoing) else { continue };
            if writer.send(Message::Text(json.into())).await.is_err() { break; }
        }
    });

    let mut membership: Option<String> = None;
    while let Some(message) = reader.next().await {
        let text = match message {
            Ok(Message::Text(s)) => s,
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };
        let command = match serde_json::from_str::<ClientMessage>(&text) {
            Ok(cmd) => cmd,
            Err(_) => {
                send_error(&tx, "invalid_message", "Mensagem JSON invalida");
                continue;
            }
        };
        match command {
            ClientMessage::CreateRoom { name } => {
                if membership.is_some() {
                    send_error(&tx, "already_joined", "Ja existe uma sala ativa nesta conexao");
                    continue;
                }
                if !valid_name(&name) {
                    send_error(&tx, "invalid_name", "Nome deve ter entre 2 e 32 caracteres");
                    continue;
                }
                let room_id = Uuid::new_v4().simple().to_string();
                let peer = Peer { name: name.trim().into(), tx: tx.clone() };
                let mut rooms = state.rooms.lock().await;
                let mut room = Room::default();
                room.peers.insert(peer_id.clone(), peer);
                rooms.insert(room_id.clone(), room);
                drop(rooms);
                membership = Some(room_id.clone());
                // Enviar as credenciais ANTES de confirmar a sala, para o cliente montar
                // a PeerConnection com TURN antes de gerar a oferta SDP.
                if let Some(turn) = &state.turn {
                    let _ = tx.try_send(turn.credentials(&peer_id));
                }
                let _ = tx.try_send(ServerMessage::RoomCreated { room_id, peer_id: peer_id.clone() });
            }
            ClientMessage::JoinRoom { room_id, name } => {
                if membership.is_some() {
                    send_error(&tx, "already_joined", "Ja existe uma sala ativa nesta conexao");
                    continue;
                }
                if !valid_name(&name) {
                    send_error(&tx, "invalid_name", "Nome deve ter entre 2 e 32 caracteres");
                    continue;
                }
                let room_id = match Uuid::parse_str(room_id.trim()) {
                    Ok(uuid) => uuid.simple().to_string(),
                    Err(_) => {
                        send_error(&tx, "invalid_room_id", "Codigo de sala invalido");
                        continue;
                    }
                };
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    send_error(&tx, "room_not_found", "Sala inexistente ou encerrada");
                    continue;
                };
                if room.peers.len() >= ROOM_CAPACITY {
                    send_error(&tx, "room_full", "Sala atingiu o limite de participantes");
                    continue;
                }
                let peers = room.peers.iter().map(|(id, peer)| PeerInfo {
                    id: id.clone(), name: peer.name.clone(),
                }).collect::<Vec<_>>();
                let new_peer = PeerInfo { id: peer_id.clone(), name: name.trim().into() };
                // Enviar credenciais ANTES do evento que inicia as ofertas P2P.
                if let Some(turn) = &state.turn {
                    let _ = tx.try_send(turn.credentials(&peer_id));
                }
                let _ = tx.try_send(ServerMessage::RoomJoined {
                    room_id: room_id.clone(), peer_id: peer_id.clone(), peers,
                });
                for peer in room.peers.values() {
                    let _ = peer.tx.try_send(ServerMessage::PeerJoined { peer: new_peer.clone() });
                }
                room.peers.insert(peer_id.clone(), Peer { name: name.trim().into(), tx: tx.clone() });
                membership = Some(room_id);
            }
            ClientMessage::Signal { to, payload } => {
                let Some(room_id) = membership.as_ref() else {
                    send_error(&tx, "not_joined", "Entre em uma sala antes de sinalizar");
                    continue;
                };
                let rooms = state.rooms.lock().await;
                let target = rooms.get(room_id).and_then(|room| room.peers.get(&to));
                if to == peer_id || target.is_none() {
                    send_error(&tx, "peer_not_found", "Destinatario nao pertence a esta sala");
                } else if let Some(target) = target {
                    if target.tx.try_send(ServerMessage::Signal { from: peer_id.clone(), payload }).is_err() {
                        send_error(&tx, "peer_busy", "Destinatario indisponivel");
                    }
                }
            }
        }
    }

    if let Some(room_id) = membership {
        let mut rooms = state.rooms.lock().await;
        if let Some(room) = rooms.get_mut(&room_id) {
            room.peers.remove(&peer_id);
            for peer in room.peers.values() {
                let _ = peer.tx.try_send(ServerMessage::PeerLeft { peer_id: peer_id.clone() });
            }
            if room.peers.is_empty() { rooms.remove(&room_id); }
        }
        info!(%peer_id, %room_id, "Participante desconectou");
    } else {
        warn!(%peer_id, "Socket terminou sem entrar em sala");
    }
    writer_task.abort();
}

#[cfg(test)]
mod tests {
    use super::{valid_name, TurnConfig};
    use voice_protocol::ServerMessage;
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha1::Sha1;

    #[test]
    fn turn_credentials_match_shared_secret() {
        let cfg = TurnConfig { host: "voz.virtuabil.cloud".into(), secret: "chave-de-teste".into() };
        if let ServerMessage::TurnCredentials { username, credential, urls, expires_at } = cfg.credentials("abc") {
            assert!(username.ends_with(":abc"));
            assert_eq!(expires_at.to_string(), username.split(':').next().unwrap());
            assert_eq!(urls.len(), 3);
            let mut mac = Hmac::<Sha1>::new_from_slice(cfg.secret.as_bytes()).unwrap();
            mac.update(username.as_bytes());
            let expected = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
            assert_eq!(credential, expected);
        } else { panic!("evento inesperado") }
    }
    #[test]
    fn validate_names() {
        assert!(valid_name("Usuario A"));
        assert!(!valid_name(" "));
        assert!(!valid_name(&"x".repeat(33)));
        assert!(!valid_name("a\nb"));
    }
}
