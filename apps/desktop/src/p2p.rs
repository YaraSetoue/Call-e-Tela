//! WebRTC Mesh: sinalizacao Trickle ICE e um unico DataChannel para teste e Opus.
//! Diagnostico: usar o canal que ja foi comprovado funcional para isolar o fluxo de voz.
//! Esta versao usa SCTP confiavel para teste; migrar voz para RTP na etapa de qualidade.

use std::{collections::HashMap, error::Error, sync::Arc, time::Duration};
use crate::screen::{self, Reassembler};

use bytes::BytesMut;
use tokio::sync::{broadcast, mpsc, Mutex};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer, RTCIceTransportPolicy,
        RTCPeerConnectionIceEvent, RTCIceCandidateInit,
        RTCPeerConnectionState, RTCSessionDescription,
    },
};
use voice_protocol::{ClientMessage, SignalPayload};
use crate::audio::{PeerDecoder, PlaybackBus};

type AnyError = Box<dyn Error + Send + Sync>;

#[derive(Debug)]
pub enum MeshEvent {
    Status(String),
    MicLevel(f32),
    VoiceStats { peer: String, sent: u64, received: u64 },
    ScreenFrame { peer: String, jpeg: Vec<u8> },
    ScreenStopped { peer: String },
}

#[derive(Clone)]
struct Handler {
    peer: String,
    events: mpsc::UnboundedSender<MeshEvent>,
    signaling: mpsc::UnboundedSender<ClientMessage>,
    voice: broadcast::Sender<Vec<u8>>,
    screen_frames: broadcast::Sender<Vec<u8>>,
    playback: PlaybackBus,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        match event.candidate.to_json() {
            Ok(candidate) => {
                let kind = if candidate.candidate.contains(" typ relay ") {
                    "relay"
                } else if candidate.candidate.contains(" typ srflx ") {
                    "srflx"
                } else {
                    "host/outro"
                };
                self.emit(format!("ICE local {}: candidato {kind}", self.short_peer()));
                if let Err(err) = self.signaling.send(ClientMessage::Signal {
                    to: self.peer.clone(),
                    payload: SignalPayload::IceCandidate {
                        candidate: candidate.candidate,
                        sdp_mid: candidate.sdp_mid,
                        sdp_mline_index: candidate.sdp_mline_index,
                    },
                }) {
                    self.emit(format!("ICE local {} nao enviado para a sinalizacao: {err}", self.short_peer()));
                }
            }
            Err(err) => self.emit(format!("Falha serializando ICE de {}: {err}", self.short_peer())),
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        self.emit(format!("ICE gathering {}: {state:?}", self.short_peer()));
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
                Ok(name) if name == "voice-screen-jpeg-v1" => start_screen_channel(
                    me.peer, data_channel, me.events, me.screen_frames.subscribe(),
                ),
                Ok(name) if name == "voice-screen-test" => start_media_channel(
                    me.peer, data_channel, me.events,
                    me.voice.subscribe(), me.playback,
                ),
                Ok(name) => {
                    let _ = me.events.send(MeshEvent::Status(format!(
                        "Canal P2P inesperado recebido: {name}"
                    )));
                }
                Err(e) => {
                    let _ = me.events.send(MeshEvent::Status(format!(
                        "Falha lendo identificador do canal P2P: {e}"
                    )));
                }
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

// Nesta etapa de diagnostico, teste P2P e voz compartilham o MESMO canal.
// Ele ja funcionou entre redes diferentes; um segundo DataChannel nao sera necessario.
fn start_media_channel(
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
        let mut warned_not_open = false;
        let mut packets_sent = 0u64;
        let mut packets_received = 0u64;
        let mut reported_send_error = false;
        let mut warned_no_audio = false;
        let mut watchdog = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(5),
            Duration::from_secs(5),
        );
        watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let _ = events.send(MeshEvent::Status(format!(
            "Canal P2P criado com {short}; aguardando OnOpen"
        )));

        loop {
            tokio::select! {
                received = channel.poll() => {
                    match received {
                        Some(DataChannelEvent::OnOpen) => {
                            opened = true;
                            let _ = events.send(MeshEvent::Status(format!(
                                "Canal P2P ABERTO com {short} (teste e voz)"
                            )));
                            let _ = events.send(MeshEvent::VoiceStats {
                                peer: peer.clone(), sent: packets_sent, received: packets_received,
                            });
                            if let Err(e) = channel.send_text("P2P aberto: teste e voz Opus no mesmo canal").await {
                                let _ = events.send(MeshEvent::Status(format!(
                                    "Falha no teste P2P para {short}: {e}"
                                )));
                            }
                        }
                        Some(DataChannelEvent::OnMessage(packet)) => {
                            let data = packet.data.as_ref();
                            if data.starts_with(b"VSA1") {
                                match decoder.decode(data, &playback, &peer) {
                                    Ok(Some(_)) => {
                                        packets_received += 1;
                                        if packets_received == 1 || packets_received % 50 == 0 {
                                            let _ = events.send(MeshEvent::VoiceStats {
                                                peer: peer.clone(),
                                                sent: packets_sent,
                                                received: packets_received,
                                            });
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        let _ = events.send(MeshEvent::Status(format!(
                                            "Erro decodificando Opus de {short}: {e}"
                                        )));
                                    }
                                }
                            } else if let Ok(text) = std::str::from_utf8(data) {
                                let _ = events.send(MeshEvent::Status(format!(
                                    "Mensagem P2P de {short}: {text}"
                                )));
                            }
                        }
                        Some(DataChannelEvent::OnError) => {
                            let _ = events.send(MeshEvent::Status(format!(
                                "Erro no DataChannel P2P de {short}"
                            )));
                        }
                        Some(DataChannelEvent::OnClose) | None => break,
                        _ => {}
                    }
                }
                packet = outgoing.recv(), if opened => {
                    match packet {
                        Ok(data) => {
                            match channel.try_send(BytesMut::from(data.as_slice())).await {
                                Ok(()) => {
                                    packets_sent += 1;
                                    if packets_sent == 1 || packets_sent % 50 == 0 {
                                        let _ = events.send(MeshEvent::VoiceStats {
                                            peer: peer.clone(),
                                            sent: packets_sent,
                                            received: packets_received,
                                        });
                                    }
                                }
                                Err(e) => {
                                    if !reported_send_error {
                                        let _ = events.send(MeshEvent::Status(format!(
                                            "Falha enviando Opus para {short}: {e}"
                                        )));
                                        reported_send_error = true;
                                    }
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            let _ = events.send(MeshEvent::Status(format!(
                                "Audio atrasado em {short}: {skipped} quadros descartados"
                            )));
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = watchdog.tick() => {
                    if !opened && !warned_not_open {
                        warned_not_open = true;
                        let _ = events.send(MeshEvent::Status(format!(
                            "Canal com {short} nao abriu apos alguns segundos; verifique ICE/SDP nos dois PCs"
                        )));
                    }

                    if opened && packets_sent == 0 && !warned_no_audio {
                        warned_no_audio = true;
                        let _ = events.send(MeshEvent::Status(format!(
                            "Canal P2P aberto com {short}, mas ainda sem Opus enviado: verifique se o microfone esta LIGADO"
                        )));
                    }
                }
            }
        }
        playback.remove(&peer);
        let _ = events.send(MeshEvent::Status(format!("Canal P2P encerrado: {short}")));
    });
}

// Canal de tela separado do canal Opus: imagens atrasadas sao descartadas.
fn start_screen_channel(
    peer: String,
    channel: Arc<dyn DataChannel>,
    events: mpsc::UnboundedSender<MeshEvent>,
    mut outgoing: broadcast::Receiver<Vec<u8>>,
) {
    tokio::spawn(async move {
        let short = peer[..peer.len().min(8)].to_owned();
        let mut ready = false;
        let mut assembler = Reassembler::new();
        let mut frame_id: u32 = 0;
        let mut errors = 0u32;
        loop {
            tokio::select! {
                event = channel.poll() => {
                    match event {
                        Some(DataChannelEvent::OnOpen) => {
                            ready = true;
                            let _ = events.send(MeshEvent::Status(format!("Tela P2P disponivel com {short}")));
                        }
                        Some(DataChannelEvent::OnMessage(packet)) => {
                            let data = packet.data.as_ref();
                            if data == b"VSS0" {
                                let _ = events.send(MeshEvent::ScreenStopped { peer: peer.clone() });
                            } else if let Some(jpeg) = assembler.push(data) {
                                let _ = events.send(MeshEvent::ScreenFrame { peer: peer.clone(), jpeg });
                            }
                        }
                        Some(DataChannelEvent::OnClose) | None => break,
                        _ => {}
                    }
                }
                packet = outgoing.recv(), if ready => {
                    match packet {
                        Ok(mut data) => {
                            // Nunca acumular quadros antigos se a rede estiver lenta.
                            while let Ok(next) = outgoing.try_recv() { data = next; }
                            if data.as_slice() == b"VSS0" {
                                let _ = channel.send(BytesMut::from(data.as_slice())).await;
                                continue;
                            }
                            frame_id = frame_id.wrapping_add(1);
                            if let Some(parts) = screen::chunks(frame_id, &data) {
                                for part in parts {
                                    // Limitar o tempo de espera e manter a voz responsiva.
                                    let result = tokio::time::timeout(
                                        Duration::from_millis(35),
                                        channel.send(BytesMut::from(part.as_slice())),
                                    ).await;
                                    if !matches!(result, Ok(Ok(()))) {
                                        errors += 1;
                                        if errors == 1 {
                                            let _ = events.send(MeshEvent::Status(format!(
                                                "Tela atrasada para {short}; quadro descartado para preservar audio"
                                            )));
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
        let _ = events.send(MeshEvent::ScreenStopped { peer });
    });
}

struct Link {
    connection: Arc<dyn PeerConnection>,
    remote_ready: bool,
    pending_ice: Vec<RTCIceCandidateInit>,
}

pub struct Mesh {
    links: Mutex<HashMap<String, Link>>,
    early_ice: Mutex<HashMap<String, Vec<RTCIceCandidateInit>>>,
    signaling: mpsc::UnboundedSender<ClientMessage>,
    events: mpsc::UnboundedSender<MeshEvent>,
    ice_servers: Mutex<Vec<RTCIceServer>>,
    voice: broadcast::Sender<Vec<u8>>,
    screen_frames: broadcast::Sender<Vec<u8>>,
    playback: PlaybackBus,
}

impl Mesh {
    pub fn new(
        signaling: mpsc::UnboundedSender<ClientMessage>,
        events: mpsc::UnboundedSender<MeshEvent>,
        voice: broadcast::Sender<Vec<u8>>,
        screen_frames: broadcast::Sender<Vec<u8>>,
        playback: PlaybackBus,
    ) -> Self {
        Self {
            links: Mutex::new(HashMap::new()),
            early_ice: Mutex::new(HashMap::new()), signaling, events, voice, screen_frames, playback,
            ice_servers: Mutex::new(vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }]),
        }
    }

    pub async fn set_turn_credentials(&self, urls: Vec<String>, username: String, credential: String) {
        self.emit(format!("Credenciais TURN recebidas: {} URLs; usuario presente: {}; senha presente: {}",
            urls.len(), !username.is_empty(), !credential.is_empty()));
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

    async fn link_for(&self, peer: &str) -> Result<Arc<dyn PeerConnection>, AnyError> {
        let ice_servers = self.ice_servers.lock().await.clone();
        self.emit(format!("Montando PeerConnection para {} ({} servidores ICE)", &peer[..peer.len().min(8)], ice_servers.len()));
        let mut map = self.links.lock().await;
        if let Some(link) = map.get(peer) {
            return Ok(link.connection.clone());
        }
        let relay_only = std::env::var("VOICE_FORCE_RELAY").as_deref() == Ok("1");
        let mut builder = RTCConfigurationBuilder::default().with_ice_servers(ice_servers);
        if relay_only {
            self.emit("MODO TESTE: ICE somente TURN/relay (VOICE_FORCE_RELAY=1)");
            builder = builder.with_ice_transport_policy(RTCIceTransportPolicy::Relay);
        }
        let config = builder.build();
        let handler = Handler {
            peer: peer.to_owned(), events: self.events.clone(), signaling: self.signaling.clone(),
            voice: self.voice.clone(), screen_frames: self.screen_frames.clone(), playback: self.playback.clone(),
        };
        let pc: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_configuration(config)
                .with_data_channel_send_buffer_limit(16 * 1024)
                .with_handler(Arc::new(handler))
                .with_udp_addrs(vec!["0.0.0.0:0".to_owned()])
                .build().await?,
        );
        let pending_ice = self.early_ice.lock().await.remove(peer).unwrap_or_default();
        map.insert(peer.to_owned(), Link { connection: pc.clone(), remote_ready: false, pending_ice });
        self.emit(format!("PeerConnection criada para {}", &peer[..peer.len().min(8)]));
        Ok(pc)
    }

    // Candidatos podem chegar antes da oferta/resposta SDP. Guarda-los e aplicar
    // somente depois que a descricao remota for instalada evita corridas.
    pub async fn apply_candidate(
        &self,
        peer: &str,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) -> Result<(), AnyError> {
        let kind = if candidate.contains(" typ relay ") { "relay" }
            else if candidate.contains(" typ srflx ") { "srflx" }
            else { "host/outro" };
        let short_peer = &peer[..peer.len().min(8)];
        self.emit(format!("ICE remoto recebido de {short_peer}: {kind}"));
        let init = RTCIceCandidateInit {
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..Default::default()
        };
        let ready_connection = {
            let mut links = self.links.lock().await;
            if let Some(link) = links.get_mut(peer) {
                if link.remote_ready {
                    Some(link.connection.clone())
                } else {
                    link.pending_ice.push(init.clone());
                    None
                }
            } else {
                self.early_ice.lock().await.entry(peer.to_owned()).or_default().push(init.clone());
                None
            }
        };
        if let Some(pc) = ready_connection {
            pc.add_ice_candidate(init).await?;
            self.emit(format!("ICE remoto aplicado de {short_peer}: {kind}"));
        } else {
            self.emit(format!("ICE remoto aguardando SDP de {short_peer}: {kind}"));
        }
        Ok(())
    }

    async fn remote_ready(&self, peer: &str) -> Result<(), AnyError> {
        let (pc, pending) = {
            let mut map = self.links.lock().await;
            let link = map.get_mut(peer).ok_or("PeerConnection ausente ao aplicar ICE")?;
            link.remote_ready = true;
            (link.connection.clone(), std::mem::take(&mut link.pending_ice))
        };
        let mut applied = 0usize;
        for candidate in pending {
            match pc.add_ice_candidate(candidate).await {
                Ok(()) => applied += 1,
                Err(err) => self.emit(format!("Candidato ICE remoto de {} rejeitado: {err}", &peer[..peer.len().min(8)])),
            }
        }
        self.emit(format!("SDP remoto pronto para {}; {} candidatos ICE pendentes aplicados",
            &peer[..peer.len().min(8)], applied));
        Ok(())
    }

    pub async fn offer(&self, peer: &str) -> Result<(), AnyError> {
        self.emit(format!("Comecando oferta P2P para {}", &peer[..peer.len().min(8)]));
        let pc = self.link_for(peer).await?;
        // O canal de teste P2P existente passa a carregar mensagens e quadros Opus.
        // Usa o modo confiavel padrao apenas para diagnosticar o primeiro audio.
        let media_channel = pc.create_data_channel("voice-screen-test", None).await?;
        start_media_channel(peer.to_owned(), media_channel, self.events.clone(),
            self.voice.subscribe(), self.playback.clone());
        let screen_channel = pc.create_data_channel("voice-screen-jpeg-v1", Some(RTCDataChannelInit {
            ordered: false,
            max_packet_life_time: Some(220),
            ..Default::default()
        })).await?;
        start_screen_channel(peer.to_owned(), screen_channel, self.events.clone(), self.screen_frames.subscribe());
        self.emit(format!("DataChannel P2P criado para {}; gerando oferta", &peer[..peer.len().min(8)]));
        let offer = pc.create_offer(None).await?;
        pc.set_local_description(offer).await?;
        self.emit(format!("Enviando oferta SDP imediatamente; candidatos ICE seguem depois para {}", &peer[..peer.len().min(8)]));
        let sdp = pc.local_description().await.ok_or("SDP local nao disponivel")?.sdp;
        self.signaling.send(ClientMessage::Signal {
            to: peer.to_owned(), payload: SignalPayload::Offer { sdp },
        })?;
        self.emit(format!("Oferta P2P enviada para {}", &peer[..peer.len().min(8)]));
        Ok(())
    }

    pub async fn answer(&self, peer: &str, sdp: String) -> Result<(), AnyError> {
        self.emit(format!("Comecando resposta P2P para {}", &peer[..peer.len().min(8)]));
        let pc = self.link_for(peer).await?;
        self.emit(format!("Aplicando oferta remota de {}", &peer[..peer.len().min(8)]));
        pc.set_remote_description(RTCSessionDescription::offer(sdp)?).await?;
        self.remote_ready(peer).await?;
        let answer = pc.create_answer(None).await?;
        pc.set_local_description(answer).await?;
        self.emit(format!("Resposta SDP imediata; ICE segue em mensagens separadas para {}", &peer[..peer.len().min(8)]));
        let sdp = pc.local_description().await.ok_or("SDP local nao disponivel")?.sdp;
        self.signaling.send(ClientMessage::Signal {
            to: peer.to_owned(), payload: SignalPayload::Answer { sdp },
        })?;
        self.emit(format!("Resposta P2P enviada para {}", &peer[..peer.len().min(8)]));
        Ok(())
    }

    pub async fn apply_answer(&self, peer: &str, sdp: String) -> Result<(), AnyError> {
        self.emit(format!("Aplicando resposta SDP de {}", &peer[..peer.len().min(8)]));
        let pc = {
            let map = self.links.lock().await;
            map.get(peer).map(|l| l.connection.clone()).ok_or("Resposta de peer desconhecido")?
        };
        pc.set_remote_description(RTCSessionDescription::answer(sdp)?).await?;
        self.remote_ready(peer).await?;
        self.emit(format!("Resposta WebRTC recebida de {}", &peer[..peer.len().min(8)]));
        Ok(())
    }

    pub async fn leave(&self, peer: &str) {
        let old = self.links.lock().await.remove(peer);
        if let Some(link) = old { let _ = link.connection.close().await; }
        self.early_ice.lock().await.remove(peer);
        self.playback.remove(peer);
    }

    pub async fn close_all(&self) {
        let all = {
            let mut map = self.links.lock().await;
            map.drain().map(|(_, link)| link.connection).collect::<Vec<_>>()
        };
        for pc in all { let _ = pc.close().await; }
        self.early_ice.lock().await.clear();
    }
}
