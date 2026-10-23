//! Teste WebRTC Mesh sem mídia: troca SDP completa (non-trickle ICE).
//! O novo participante inicia uma oferta para cada participante existente.
//! A conexão WSS encaminha APENAS as ofertas e respostas.

use std::{collections::HashMap, error::Error, sync::Arc, time::Duration};

use tokio::sync::{mpsc, watch, Mutex};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
        RTCPeerConnectionState, RTCSessionDescription,
    },
};
use voice_protocol::{ClientMessage, SignalPayload};

type AnyError = Box<dyn Error + Send + Sync>;

#[derive(Debug)]
pub enum MeshEvent {
    Status(String),
}

#[derive(Clone)]
struct Handler {
    peer: String,
    events: mpsc::UnboundedSender<MeshEvent>,
    gathering: watch::Sender<bool>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathering.send_replace(true);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        self.emit(format!("Conexao WebRTC {}: {state:?}", self.short_peer()));
    }

    async fn on_data_channel(&self, data_channel: Arc<dyn DataChannel>) {
        start_channel(self.peer.clone(), data_channel, self.events.clone());
    }
}

impl Handler {
    fn short_peer(&self) -> &str {
        &self.peer[..self.peer.len().min(8)]
    }
    fn emit(&self, msg: String) {
        let _ = self.events.send(MeshEvent::Status(msg));
    }
}

fn start_channel(
    peer: String,
    channel: Arc<dyn DataChannel>,
    events: mpsc::UnboundedSender<MeshEvent>,
) {
    tokio::spawn(async move {
        let short = &peer[..peer.len().min(8)];
        while let Some(event) = channel.poll().await {
            match event {
                DataChannelEvent::OnOpen => {
                    let _ = events.send(MeshEvent::Status(format!(
                        "DataChannel P2P aberto com {short}"
                    )));
                    let greeting = "Ola! Esta mensagem veio diretamente pelo WebRTC.";
                    if let Err(e) = channel.send_text(greeting).await {
                        let _ = events.send(MeshEvent::Status(format!(
                            "Falha no teste P2P para {short}: {e}"
                        )));
                    }
                }
                DataChannelEvent::OnMessage(message) => {
                    match String::from_utf8(message.data.to_vec()) {
                        Ok(text) => {
                            let _ = events.send(MeshEvent::Status(format!(
                                "Recebido P2P de {short}: {text}"
                            )));
                        }
                        Err(_) => {
                            let _ = events.send(MeshEvent::Status(format!(
                                "Mensagem P2P binaria de {short}"
                            )));
                        }
                    }
                }
                DataChannelEvent::OnClose => {
                    let _ = events.send(MeshEvent::Status(format!(
                        "DataChannel com {short} encerrado"
                    )));
                    break;
                }
                _ => {}
            }
        }
    });
}

struct Link {
    connection: Arc<dyn PeerConnection>,
    ice_done: watch::Receiver<bool>,
}

pub struct Mesh {
    links: Mutex<HashMap<String, Link>>,
    signaling: mpsc::UnboundedSender<ClientMessage>,
    events: mpsc::UnboundedSender<MeshEvent>,
    ice_servers: Mutex<Vec<RTCIceServer>>,
}

impl Mesh {
    pub fn new(
        signaling: mpsc::UnboundedSender<ClientMessage>,
        events: mpsc::UnboundedSender<MeshEvent>,
    ) -> Self {
        Self {
            links: Mutex::new(HashMap::new()),
            signaling,
            events,
            ice_servers: Mutex::new(vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }]),
        }
    }

    /// Atualizar ICE antes que qualquer PeerConnection seja criada.
    /// O servidor envia TurnCredentials antes de RoomJoined/RoomCreated.
    pub async fn set_turn_credentials(&self, urls: Vec<String>, username: String, credential: String) {
        let mut servers = self.ice_servers.lock().await;
        // STUN publico como fallback; TURN somente com credenciais temporarias.
        *servers = vec![
            RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            },
            RTCIceServer { urls, username, credential, ..Default::default() },
        ];
        drop(servers);
        self.emit("STUN/TURN configurados para novas conexoes WebRTC.");
    }

    fn emit(&self, message: impl Into<String>) {
        let _ = self.events.send(MeshEvent::Status(message.into()));
    }

    async fn link_for(&self, peer: &str) -> Result<(Arc<dyn PeerConnection>, watch::Receiver<bool>), AnyError> {
        // Obter a configuracao ICE ANTES do mutex de peers evita inversao de locks.
        let ice_servers = self.ice_servers.lock().await.clone();
        let mut map = self.links.lock().await;
        if let Some(link) = map.get(peer) {
            return Ok((link.connection.clone(), link.ice_done.clone()));
        }
        let (gathering, ice_done) = watch::channel(false);
        let config = RTCConfigurationBuilder::default()
            .with_ice_servers(ice_servers)
            .build();
        let handler = Handler {
            peer: peer.to_owned(),
            events: self.events.clone(),
            gathering,
        };
        let pc: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_configuration(config)
                .with_handler(Arc::new(handler))
                .with_udp_addrs(vec!["0.0.0.0:0".to_owned()])
                .build()
                .await?,
        );
        map.insert(peer.to_owned(), Link { connection: pc.clone(), ice_done: ice_done.clone() });
        Ok((pc, ice_done))
    }

    // Non-trickle ICE: evitar enviar SDP antes de reunir os candidatos locais.
    async fn gathered_sdp(
        pc: &Arc<dyn PeerConnection>,
        mut finished: watch::Receiver<bool>,
    ) -> Result<String, AnyError> {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !*finished.borrow() {
                finished.changed().await.map_err(|_| "ICE gathering interrompido")?;
            }
            Ok::<(), AnyError>(())
        })
        .await??;
        let desc = pc.local_description().await.ok_or("SDP local nao disponivel")?;
        Ok(desc.sdp)
    }

    pub async fn offer(&self, peer: &str) -> Result<(), AnyError> {
        let (pc, finished) = self.link_for(peer).await?;
        let channel = pc.create_data_channel("voice-screen-test", None).await?;
        start_channel(peer.to_owned(), channel, self.events.clone());
        let offer = pc.create_offer(None).await?;
        pc.set_local_description(offer).await?;
        self.emit(format!("Reunindo candidatos ICE para {}...", &peer[..peer.len().min(8)]));
        let sdp = Self::gathered_sdp(&pc, finished).await?;
        self.signaling.send(ClientMessage::Signal {
            to: peer.to_owned(),
            payload: SignalPayload::Offer { sdp },
        })?;
        self.emit(format!("Oferta P2P enviada para {}", &peer[..peer.len().min(8)]));
        Ok(())
    }

    pub async fn answer(&self, peer: &str, sdp: String) -> Result<(), AnyError> {
        let (pc, finished) = self.link_for(peer).await?;
        pc.set_remote_description(RTCSessionDescription::offer(sdp)?).await?;
        let answer = pc.create_answer(None).await?;
        pc.set_local_description(answer).await?;
        let sdp = Self::gathered_sdp(&pc, finished).await?;
        self.signaling.send(ClientMessage::Signal {
            to: peer.to_owned(),
            payload: SignalPayload::Answer { sdp },
        })?;
        self.emit(format!("Resposta P2P enviada para {}", &peer[..peer.len().min(8)]));
        Ok(())
    }

    pub async fn apply_answer(&self, peer: &str, sdp: String) -> Result<(), AnyError> {
        let pc = {
            let map = self.links.lock().await;
            map.get(peer).map(|l| l.connection.clone()).ok_or("Resposta de peer desconhecido")?
        };
        pc.set_remote_description(RTCSessionDescription::answer(sdp)?).await?;
        self.emit(format!("Resposta WebRTC recebida de {}", &peer[..peer.len().min(8)]));
        Ok(())
    }

    pub async fn leave(&self, peer: &str) {
        let old = self.links.lock().await.remove(peer);
        if let Some(link) = old {
            let _ = link.connection.close().await;
        }
    }

    pub async fn close_all(&self) {
        let all = {
            let mut map = self.links.lock().await;
            map.drain().map(|(_, link)| link.connection).collect::<Vec<_>>()
        };
        for pc in all { let _ = pc.close().await; }
    }
}
