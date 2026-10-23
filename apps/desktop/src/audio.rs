//! Prototipo de audio em tempo real (Windows): CPAL -> Opus -> DataChannel.
//! Codec mono a 48 kHz, quadros de 20 ms (960 amostras).
//! A operacao de audio/CPAL fica numa thread dedicada para nao bloquear a UI.

use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    sync::{atomic::{AtomicBool, Ordering}, mpsc as std_mpsc, Arc, Mutex},
    thread,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, SampleFormat, SampleRate, Stream, StreamConfig,
};
use opus::{Application, Channels, Decoder, Encoder};
use tokio::sync::{broadcast, mpsc};

use crate::p2p::MeshEvent;

pub const FRAME: usize = 960;
const MAX_SAMPLES: usize = 960 * 10; // 200 ms; descartamos antigos para reduzir latencia.
const HEADER: &[u8; 4] = b"VSA1";

#[derive(Default)]
struct PeerPlayback {
    queue: VecDeque<f32>,
    started: bool,
}

#[derive(Clone, Default)]
pub struct PlaybackBus {
    peers: Arc<Mutex<HashMap<String, PeerPlayback>>>,
}

impl PlaybackBus {
    pub fn push(&self, peer: &str, samples: &[f32]) {
        if let Ok(mut all) = self.peers.lock() {
            let p = all.entry(peer.to_owned()).or_default();
            // Ao congestionar, manter as amostras mais recentes.
            for sample in samples {
                if p.queue.len() >= MAX_SAMPLES {
                    p.queue.pop_front();
                }
                p.queue.push_back(*sample);
            }
        }
    }

    pub fn remove(&self, peer: &str) {
        if let Ok(mut all) = self.peers.lock() {
            all.remove(peer);
        }
    }
}

fn config_for(device: &Device, input: bool) -> Result<StreamConfig, Box<dyn Error + Send + Sync>> {
    let supported: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };
    let mut matching: Vec<_> = supported.into_iter()
        .filter(|r| {
            r.sample_format() == SampleFormat::F32
                && r.min_sample_rate().0 <= 48_000
                && r.max_sample_rate().0 >= 48_000
        })
        .collect();
    matching.sort_by_key(|r| r.channels());
    let choice = matching.into_iter().next().ok_or(
        "Dispositivo nao suporta audio f32 a 48000 Hz. Ajuste o formato do dispositivo no Windows."
    )?;
    Ok(choice.with_sample_rate(SampleRate(48_000)).into())
}

fn output_stream(bus: PlaybackBus) -> Result<Stream, Box<dyn Error + Send + Sync>> {
    let device = cpal::default_host().default_output_device().ok_or("Alto-falante nao encontrado")?;
    let config = config_for(&device, false)?;
    let channels = config.channels as usize;
    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _| {
            // Nunca bloquear a callback de WASAPI em espera de mutex.
            if let Ok(mut peers) = bus.peers.try_lock() {
                for frame in data.chunks_mut(channels) {
                    let mut mixed = 0.0f32;
                    for peer in peers.values_mut() {
                        if !peer.started && peer.queue.len() >= FRAME * 2 {
                            peer.started = true; // 40 ms de pre-buffer
                        }
                        if peer.started {
                            if let Some(sample) = peer.queue.pop_front() {
                                mixed += sample;
                            } else {
                                peer.started = false;
                            }
                        }
                    }
                    // Mixagem simples, limiter para evitar clipping.
                    let sample = (mixed * 0.75).clamp(-1.0, 1.0);
                    for output in frame { *output = sample; }
                }
            } else {
                data.fill(0.0);
            }
        },
        |e| eprintln!("Erro na saida de audio: {e}"),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

fn input_stream(sender: broadcast::Sender<Vec<u8>>, enabled: Arc<AtomicBool>) -> Result<Stream, Box<dyn Error + Send + Sync>> {
    let device = cpal::default_host().default_input_device().ok_or("Microfone nao encontrado")?;
    let config = config_for(&device, true)?;
    let channels = config.channels as usize;
    let (frames_tx, frames_rx) = std_mpsc::sync_channel::<Vec<f32>>(8);
    thread::spawn(move || {
        let mut encoder = match Encoder::new(48_000, Channels::Mono, Application::Voip) {
            Ok(value) => value,
            Err(e) => { eprintln!("Erro criando encoder Opus: {e}"); return; }
        };
        let mut seq = 0u32;
        while let Ok(frame) = frames_rx.recv() {
            if !enabled.load(Ordering::Acquire) { continue; }
            let mut encoded = [0u8; 1500];
            match encoder.encode_float(&frame, &mut encoded) {
                Ok(len) => {
                    let mut packet = Vec::with_capacity(8 + len);
                    packet.extend_from_slice(HEADER);
                    packet.extend_from_slice(&seq.to_be_bytes());
                    packet.extend_from_slice(&encoded[..len]);
                    seq = seq.wrapping_add(1);
                    let _ = sender.send(packet); // se ninguem estiver ouvindo, descartar
                }
                Err(e) => eprintln!("Erro codificando Opus: {e}"),
            }
        }
    });
    let mut pending = Vec::with_capacity(FRAME);
    let stream = device.build_input_stream(
        &config,
        move |data: &[f32], _| {
            for frame in data.chunks(channels) {
                let mono = frame.iter().copied().sum::<f32>() / channels as f32;
                pending.push(mono);
                if pending.len() == FRAME {
                    let next = std::mem::replace(&mut pending, Vec::with_capacity(FRAME));
                    // A callback nao deve esperar caso o codificador fique lento.
                    let _ = frames_tx.try_send(next);
                }
            }
        },
        |e| eprintln!("Erro na captura de audio: {e}"),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

enum AudioCommand {
    Capture(bool),
}

pub struct AudioSession {
    commands: std_mpsc::Sender<AudioCommand>,
    pub frames: broadcast::Sender<Vec<u8>>,
    pub playback: PlaybackBus,
}

impl AudioSession {
    pub fn new(events: mpsc::UnboundedSender<MeshEvent>) -> Self {
        let (frames, _) = broadcast::channel(32);
        let playback = PlaybackBus::default();
        let (commands, receiver) = std_mpsc::channel();
        let tx = frames.clone();
        let output_bus = playback.clone();
        thread::spawn(move || {
            let _speaker = match output_stream(output_bus) {
                Ok(stream) => {
                    let _ = events.send(MeshEvent::Status("Reproducao de voz preparada".into()));
                    Some(stream)
                }
                Err(e) => {
                    let _ = events.send(MeshEvent::Status(format!("Falha na reproducao: {e}")));
                    None
                }
            };
            let mut mic: Option<Stream> = None;
            let mut mic_enabled: Option<Arc<AtomicBool>> = None;
            while let Ok(command) = receiver.recv() {
                match command {
                    AudioCommand::Capture(true) if mic.is_none() => {
                        let enabled = Arc::new(AtomicBool::new(true));
                        match input_stream(tx.clone(), enabled.clone()) {
                            Ok(stream) => {
                                mic = Some(stream);
                                mic_enabled = Some(enabled);
                                let _ = events.send(MeshEvent::Status("Microfone ativado".into()));
                            }
                            Err(e) => {
                                let _ = events.send(MeshEvent::Status(format!("Falha no microfone: {e}")));
                            }
                        }
                    }
                    AudioCommand::Capture(false) => {
                        if let Some(flag) = mic_enabled.take() { flag.store(false, Ordering::Release); }
                        mic.take(); // solta o dispositivo e interrompe a captura
                        let _ = events.send(MeshEvent::Status("Microfone desativado".into()));
                    }
                    _ => {}
                }
            }
            if let Some(flag) = mic_enabled { flag.store(false, Ordering::Release); }
        });
        Self { commands, frames, playback }
    }

    pub fn capture(&self, enable: bool) {
        let _ = self.commands.send(AudioCommand::Capture(enable));
    }
}

/// Cada participante tem seu proprio decoder (Opus guarda estado entre pacotes).
pub struct PeerDecoder {
    decoder: Decoder,
    last_seq: Option<u32>,
}

impl PeerDecoder {
    pub fn new() -> Result<Self, opus::Error> {
        Ok(Self { decoder: Decoder::new(48_000, Channels::Mono)?, last_seq: None })
    }

    pub fn decode(&mut self, packet: &[u8], playback: &PlaybackBus, peer: &str) {
        if packet.len() <= 8 || &packet[..4] != HEADER { return; }
        let seq = u32::from_be_bytes(packet[4..8].try_into().unwrap());
        if let Some(prev) = self.last_seq {
            let advance = seq.wrapping_sub(prev);
            if advance == 0 || advance >= 0x8000_0000 { return; }
            // PLC simples para ate dois quadros perdidos.
            for _ in 0..advance.saturating_sub(1).min(2) {
                let mut lost = [0.0f32; FRAME];
                if let Ok(n) = self.decoder.decode_float(&[], &mut lost, false) {
                    playback.push(peer, &lost[..n]);
                }
            }
        }
        self.last_seq = Some(seq);
        let mut samples = [0.0f32; 5760];
        if let Ok(n) = self.decoder.decode_float(&packet[8..], &mut samples, false) {
            playback.push(peer, &samples[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opus_roundtrip_mono() {
        let mut encoder = Encoder::new(48_000, Channels::Mono, Application::Voip).unwrap();
        let mut decoder = PeerDecoder::new().unwrap();
        let mut packet = vec![0u8; 8];
        packet[..4].copy_from_slice(HEADER);
        let input = [0.0f32; FRAME];
        let mut data = [0u8; 1500];
        let len = encoder.encode_float(&input, &mut data).unwrap();
        packet.extend_from_slice(&data[..len]);
        let bus = PlaybackBus::default();
        decoder.decode(&packet, &bus, "peer");
        assert!(bus.peers.lock().unwrap().get("peer").unwrap().queue.len() > 0);
    }
}
