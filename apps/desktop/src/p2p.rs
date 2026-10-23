//! WebRTC Mesh: SDP completo, DataChannel de teste e DataChannel de voz Opus.
//! Esta etapa usa SCTP nao ordenado para validar voz. Migrar para RTP na etapa de qualidade.

use std::{collections::HashMap, error::Error, sync::Arc, time::Duration};

use bytes::BytesMut;
use tokio::sync::{broadcast, mpsc, watch, Mutex};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
        RTCPeerConnectionState, RTCSessionDescription,
    },
};
use voice_protocol::{ClientMessage, SignalPayload};
use crate::audio::{PeerDecoder, PlaybackBus};

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
    voice: broadcast::Sender<Vec<u8>>,
    playback: PlaybackBus,
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
        // Nunca aguardar um metodo do DataChannel dentro do callback do driver.
        // A consulta do label usa o mesmo driver, entao fazemos isso noutra task.
        let me = self.clone();
        tokio::spawn(async move {
            match data_channel.label().await {
                Ok(name) if name == "voice-opus-v1" => start_voice_channel(
                    me.peer, data_channel, me.events,
                    me.voice.subscribe(), me.playback,
                ),
                _ => start_test_channel(me.peer, data_channel, me.events),
            }
        });
    }
}

impl Handler {
    fn short_peer(&self) -> &str { &self.peer[..self.peer.len().min(8)] }
    fn emit(&self, message: String) {
        let _ = self.events.send(MeshEvent::Status(message));
    }
}

fn start_test_channel(peer: String, channel: Arc<dyn DataChannel>, events: mpsc::UnboundedSender<MeshEvent>) {
    tokio::spawn(async move {
        let short = &peer[..peer.len().min(8)];
        while let Some(event) = channel.poll().await {
            match event {
                DataChannelEvent::OnOpen => {
                    let _ = events.send(MeshEvent::Status(format!("DataChannel P2P aberto com {short}")));
                    let _ = channel.send_text("Ola! Esta mensagem veio diretamente pelo WebRTC.").await;
                }
                DataChannelEvent::OnMessage(message) => {
                    if let Ok(text) = String::from_utf8(message.data.to_vec()) {
                        let _ = events.send(MeshEvent::Status(format!("Recebido P2P de {short}: {text}")));
                    }
                }
                DataChannelEvent::OnClose => break,
                _ => {}
            }
        }
    });
}

fn start_voice_channel(
    peer: String,
    channel: Arc<dyn DataChannel>,
    events: mpsc::UnboundedSender<MeshEvent>,
    mut outgoing: broadcast::Receiver<Vec<u8>>,
    playback: PlaybackBus,
) {
    tokio::spawn(async move {
        let short = peer[..peer.len().min(8)].to_owned();
        let mut decoder = match PeerDecoder::new() {
            Ok(value) => value,
            Err(e) => {
                let _ = events.send(MeshEvent::Status(format!("Erro no decoder Opus: {e}")));
                return;
            }
        };
        let mut opened = false;
        loop {
            tokio::select! {
                received = channel.poll() => {
                    match received {
                        Some(DataChannelEvent::OnOpen) => {
                            opened = true;
                            let _ = events.send(MeshEvent::Status(format!("Voz P2P disponivel com {short}")));
                        }
                        Some(DataChannelEvent::OnMessage(packet)) => {
                            // Somente mensagens binarias do canal dedicado a voz.
                            decoder.decode(packet.data.as_ref(), &playback, &peer);
                        }
                        Some(DataChannelEvent::OnClose) | None => break,
                        _ => {}
                    }
                }
                packet = outgoing.recv(), if opened => {
                    match packet {
                        Ok(data) => {
                            // Evitar bloquear a transmissao se o buffer SCTP estiver cheio.
                            let _ = channel.try_send(BytesMut::from(data.as_slice())).await;
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
        playback.remove(&peer);
        let _ = events.send(MeshEvent::Status(format!("Canal de voz encerrado: {short}")));
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
    voice: broadcast::Sender<Vec<u8>>,
    playback: PlaybackBus,
}

impl Mesh {
    pub fn new(
        signaling: mpsc::UnboundedSender<ClientMessage>,
        events: mpsc::UnboundedSender<MeshEvent>,
        voice: broadcast::Sender<Vec<u8>>,
        playback: PlaybackBus,
    ) -> Self {
        Self {
            links: Mutex::new(HashMap::new()), signaling, events, voice, playback,
            ice_servers: Mutex::new(vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }]),
        }
    }

    pub async fn set_turn_credentials(&self, urls: Vec<String>, username: String, credential: String) {
        let mut servers = self.ice_servers.lock().await;
        *servers = vec![
            RTCIceServer { urls: vec!["stun:stun.l.google.com:19302".to_owned()], ..Default::default() },
            RTCIceServer { urls, username, credential, ..Default::default() },
        ];
        drop(servers);
        self.emit("STUN/TURN configurados para novas conexoes WebRTC.");
    }

    fn emit(&self, message: impl Into<String>) {
        let _ = self.events.send(MeshEvent::Status(message.into()));
    }

    async fn link_for(&self, peer: &str) -> Result<(Arc<dyn PeerConnection>, watch::Receiver<bool>), AnyError> {
        let ice_servers = self.ice_servers.lock().await.clone();
        let mut map = self.links.lock().await;
        if let Some(link) = map.get(peer) {
            return Ok((link.connection.clone(), link.ice_done.clone()));
        }
        let (gathering, ice_done) = watch::channel(false);
        let config = RTCConfigurationBuilder::default().with_ice_servers(ice_servers).build();
        let handler = Handler {
            peer: peer.to_owned(), events: self.events.clone(), gathering,
            voice: self.voice.clone(), playback: self.playback.clone(),
        };
        let pc: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_configuration(config)
                .with_data_channel_send_buffer_limit(16 * 1024)
                .with_handler(Arc::new(handler))
                .with_udp_addrs(vec!["0.0.0.0:0".to_owned()])
                .build().await?,
        );
        map.insert(peer.to_owned(), Link { connection: pc.clone(), ice_done: ice_done.clone() });
        Ok((pc, ice_done))
    }

    async fn gathered_sdp(
        pc: &Arc<dyn PeerConnection>,
        mut finished: watch::Receiver<bool>,
    ) -> Result<String, AnyError> {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !*finished.borrow() {
                finished.changed().await.map_err(|_| "ICE gathering interrompido")?;
            }
            Ok::<(), AnyError>(())
        }).await??;
        let desc = pc.local_description().await.ok_or("SDP local nao disponivel")?;
        Ok(desc.sdp)
    }

    pub async fn offer(&self, peer: &str) -> Result<(), AnyError> {
        let (pc, finished) = self.link_for(peer).await?;
        let test_channel = pc.create_data_channel("voice-screen-test", None).await?;
        start_test_channel(peer.to_owned(), test_channel, self.events.clone());
        let voice_channel = pc.create_data_channel("voice-opus-v1", Some(RTCDataChannelInit {
            ordered: false,
            max_retransmits: Some(0),
            ..Default::default()
        })).await?;
        start_voice_channel(peer.to_owned(), voice_channel, self.events.clone(),
            self.voice.subscribe(), self.playback.clone());
        let offer = pc.create_offer(None).await?;
        pc.set_local_description(offer).await?;
        self.emit(format!("Reunindo candidatos ICE para {}...", &peer[..peer.len().min(8)]));
        let sdp = Self::gathered_sdp(&pc, finished).await?;
        self.signaling.send(ClientMessage::Signal {
            to: peer.to_owned(), payload: SignalPayload::Offer { sdp },
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
            to: peer.to_owned(), payload: SignalPayload::Answer { sdp },
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
        if let Some(link) = old { let _ = link.connection.close().await; }
        self.playback.remove(peer);
    }

    pub async fn close_all(&self) {
        let all = {
            let mut map = self.links.lock().await;
            map.drain().map(|(_, link)| link.connection).collect::<Vec<_>>()
        };
        for pc in all { let _ = pc.close().await; }
    }
}
