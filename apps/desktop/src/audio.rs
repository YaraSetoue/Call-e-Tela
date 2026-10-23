//! Audio MVP: CPAL (Windows), Opus mono 48k/20ms, WebRTC DataChannel.
//! Supported device sample formats: F32, I16 and U16 (48 kHz).
//! Input/output selection, master volume and per-participant gain.
//! Device enumeration uses an ephemeral session index; refresh before choosing.

use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    f32::consts::PI,
    sync::{atomic::{AtomicBool, Ordering}, mpsc as std_mpsc, Arc, Mutex},
    thread,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, FromSample, Sample, SampleFormat, SampleRate, SizedSample, Stream, StreamConfig,
};
use opus::{Application, Channels, Decoder, Encoder};
use tokio::sync::{broadcast, mpsc};

use crate::p2p::MeshEvent;

pub const FRAME: usize = 960;
const MAX_SAMPLES: usize = FRAME * 10;
const HEADER: &[u8; 4] = b"VSA1";
type AnyError = Box<dyn Error + Send + Sync>;

struct PeerPlayback {
    queue: VecDeque<f32>,
    started: bool,
    gain: f32,
}

impl Default for PeerPlayback {
    fn default() -> Self { Self { queue: VecDeque::new(), started: false, gain: 1.0 } }
}

struct PlaybackState {
    peers: HashMap<String, PeerPlayback>,
    master_gain: f32,
}

impl Default for PlaybackState {
    fn default() -> Self { Self { peers: HashMap::new(), master_gain: 1.0 } }
}

#[derive(Clone, Default)]
pub struct PlaybackBus {
    inner: Arc<Mutex<PlaybackState>>,
}

impl PlaybackBus {
    pub fn push(&self, peer: &str, samples: &[f32]) {
        if let Ok(mut state) = self.inner.lock() {
            let p = state.peers.entry(peer.to_owned()).or_default();
            for &sample in samples {
                if p.queue.len() >= MAX_SAMPLES { p.queue.pop_front(); }
                p.queue.push_back(sample);
            }
        }
    }

    /// Volume percentage. 0 mutes, 100 is normal, 200 amplifies (may clip).
    pub fn set_master_percent(&self, percent: f32) {
        if let Ok(mut state) = self.inner.lock() {
            state.master_gain = percent.clamp(0.0, 200.0) / 100.0;
        }
    }

    pub fn set_peer_percent(&self, peer: &str, percent: f32) {
        if let Ok(mut state) = self.inner.lock() {
            state.peers.entry(peer.to_owned()).or_default().gain = percent.clamp(0.0, 200.0) / 100.0;
        }
    }

    pub fn peer_percent(&self, peer: &str) -> f32 {
        self.inner.lock().ok()
            .and_then(|state| state.peers.get(peer).map(|p| p.gain * 100.0))
            .unwrap_or(100.0)
    }

    pub fn remove(&self, peer: &str) {
        if let Ok(mut state) = self.inner.lock() { state.peers.remove(peer); }
    }

    /// Local speaker test: no network, no microphone, ~200 ms tone.
    pub fn test_speaker(&self) {
        let samples: Vec<f32> = (0..FRAME * 10)
            .map(|n| 0.18 * (2.0 * PI * 440.0 * (n as f32) / 48_000.0).sin())
            .collect();
        self.push("__speaker_test", &samples);
    }
}

pub fn microphone_names() -> Vec<String> {
    let mut names = vec!["Padrao do Windows".to_owned()];
    if let Ok(devices) = cpal::default_host().input_devices() {
        for (i, device) in devices.enumerate() {
            let name = device.name().unwrap_or_else(|_| "Dispositivo sem nome".into());
            names.push(format!("{} - {}", i + 1, name));
        }
    }
    names
}

pub fn output_device_names() -> Vec<String> {
    let mut names = vec!["Padrao do Windows".to_owned()];
    if let Ok(devices) = cpal::default_host().output_devices() {
        for (i, device) in devices.enumerate() {
            let name = device.name().unwrap_or_else(|_| "Dispositivo sem nome".into());
            names.push(format!("{} - {}", i + 1, name));
        }
    }
    names
}

fn output_device(index: usize) -> Result<Device, AnyError> {
    let host = cpal::default_host();
    if index == 0 {
        return host.default_output_device().ok_or_else(|| "Saida padrao nao encontrada".into());
    }
    host.output_devices()?.nth(index - 1)
        .ok_or_else(|| format!("Saida de indice {index} indisponivel. Atualize a lista.").into())
}

fn input_device(index: usize) -> Result<Device, AnyError> {
    let host = cpal::default_host();
    if index == 0 {
        return host.default_input_device().ok_or_else(|| "Microfone padrao nao encontrado".into());
    }
    host.input_devices()?.nth(index - 1)
        .ok_or_else(|| format!("Microfone de indice {index} indisponivel. Atualize a lista.").into())
}

fn config_for(device: &Device, input: bool) -> Result<(StreamConfig, SampleFormat), AnyError> {
    let ranges: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };
    let mut options: Vec<_> = ranges.into_iter().filter(|r| {
        r.min_sample_rate().0 <= 48_000 && 48_000 <= r.max_sample_rate().0
            && matches!(r.sample_format(), SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16)
    }).collect();
    // Prefer float, then signed 16-bit, and avoid unnecessary multichannel streams.
    options.sort_by_key(|r| {
        let priority = match r.sample_format() { SampleFormat::F32 => 0, SampleFormat::I16 => 1, _ => 2 };
        (priority, r.channels())
    });
    let selected = options.into_iter().next()
        .ok_or("Dispositivo nao oferece F32/I16/U16 a 48000 Hz; escolha outro dispositivo.")?;
    let format = selected.sample_format();
    Ok((selected.with_sample_rate(SampleRate(48_000)).into(), format))
}

fn mix_next(state: &mut PlaybackState) -> f32 {
    let mut mixed = 0.0f32;
    for peer in state.peers.values_mut() {
        // Start with 40 ms buffered. If the input underflows, accumulate again
        // rather than playing rapidly interrupted 20 ms bursts.
        if !peer.started && peer.queue.len() >= FRAME * 2 { peer.started = true; }
        if peer.started {
            match peer.queue.pop_front() {
                Some(sample) => mixed += sample * peer.gain,
                None => peer.started = false,
            }
        }
    }
    (mixed * state.master_gain * 0.80).clamp(-1.0, 1.0)
}

fn write_output<T: Sample + FromSample<f32>>(data: &mut [T], channels: usize, bus: &PlaybackBus) {
    if let Ok(mut state) = bus.inner.try_lock() {
        for frame in data.chunks_mut(channels) {
            let sample = T::from_sample(mix_next(&mut state));
            for output in frame { *output = sample; }
        }
    } else {
        for value in data.iter_mut() { *value = T::EQUILIBRIUM; }
    }
}

fn output_stream(bus: PlaybackBus, index: usize) -> Result<(Stream, String), AnyError> {
    let device = output_device(index)?;
    let device_name = device.name().unwrap_or_else(|_| "Alto-falante".into());
    let (config, sample_format) = config_for(&device, false)?;
    let channels = config.channels as usize;
    let stream = match sample_format {
        SampleFormat::F32 => device.build_output_stream(&config,
            move |data: &mut [f32], _| write_output(data, channels, &bus),
            |e| eprintln!("Falha na saida de audio: {e}"), None)?,
        SampleFormat::I16 => device.build_output_stream(&config,
            move |data: &mut [i16], _| write_output(data, channels, &bus),
            |e| eprintln!("Falha na saida de audio: {e}"), None)?,
        SampleFormat::U16 => device.build_output_stream(&config,
            move |data: &mut [u16], _| write_output(data, channels, &bus),
            |e| eprintln!("Falha na saida de audio: {e}"), None)?,
        _ => return Err("Formato de saida nao suportado".into()),
    };
    stream.play()?;
    Ok((stream, format!("Saida ativa: {device_name} (48 kHz, {sample_format:?})")))
}

fn convert_input<T: Sample>(
    data: &[T], channels: usize, pending: &mut Vec<f32>,
    frames_tx: &std_mpsc::SyncSender<Vec<f32>>,
) where f32: FromSample<T> {
    for frame in data.chunks(channels) {
        if frame.is_empty() { continue; }
        let mono = frame.iter().copied().map(f32::from_sample).sum::<f32>() / frame.len() as f32;
        pending.push(mono.clamp(-1.0, 1.0));
        if pending.len() == FRAME {
            let next = std::mem::replace(pending, Vec::with_capacity(FRAME));
            let _ = frames_tx.try_send(next); // realtime: never block WASAPI callback
        }
    }
}

fn build_input<T>(device: &Device, config: &StreamConfig,
    frames_tx: std_mpsc::SyncSender<Vec<f32>>,
) -> Result<Stream, AnyError>
where T: SizedSample, f32: FromSample<T> {
    let channels = config.channels as usize;
    let mut pending = Vec::with_capacity(FRAME);
    let stream = device.build_input_stream(config,
        move |data: &[T], _| convert_input(data, channels, &mut pending, &frames_tx),
        |e| eprintln!("Falha na captura de audio: {e}"), None)?;
    Ok(stream)
}

fn input_stream(
    sender: broadcast::Sender<Vec<u8>>,
    enabled: Arc<AtomicBool>,
    device_index: usize,
    events: mpsc::UnboundedSender<MeshEvent>,
) -> Result<(Stream, String), AnyError> {
    let device = input_device(device_index)?;
    let name = device.name().unwrap_or_else(|_| "Microfone".to_owned());
    let (config, sample_format) = config_for(&device, true)?;
    let (frames_tx, frames_rx) = std_mpsc::sync_channel::<Vec<f32>>(8);
    let stream = match sample_format {
        SampleFormat::F32 => build_input::<f32>(&device, &config, frames_tx)?,
        SampleFormat::I16 => build_input::<i16>(&device, &config, frames_tx)?,
        SampleFormat::U16 => build_input::<u16>(&device, &config, frames_tx)?,
        _ => return Err("Formato de microfone nao suportado".into()),
    };
    // Start encoder worker only after opening the input device successfully.
    thread::spawn(move || {
        let mut encoder = match Encoder::new(48_000, Channels::Mono, Application::Voip) {
            Ok(encoder) => encoder,
            Err(e) => { let _ = events.send(MeshEvent::Status(format!("Erro no Opus encoder: {e}"))); return; }
        };
        let mut seq = 0u32;
        let mut peak = 0.0f32;
        while let Ok(frame) = frames_rx.recv() {
            if !enabled.load(Ordering::Acquire) { continue; }
            peak = peak.max(frame.iter().fold(0.0f32, |m, s| m.max(s.abs())));
            let mut encoded = [0u8; 1500];
            match encoder.encode_float(&frame, &mut encoded) {
                Ok(len) => {
                    let mut packet = Vec::with_capacity(8 + len);
                    packet.extend_from_slice(HEADER);
                    packet.extend_from_slice(&seq.to_be_bytes());
                    packet.extend_from_slice(&encoded[..len]);
                    seq = seq.wrapping_add(1);
                    let _ = sender.send(packet);
                    if seq % 50 == 0 { // update every ~1 second, not every audio packet
                        let _ = events.send(MeshEvent::MicLevel(peak));
                        peak = 0.0;
                    }
                }
                Err(e) => { let _ = events.send(MeshEvent::Status(format!("Erro codificando Opus: {e}"))); }
            }
        }
    });
    stream.play()?;
    Ok((stream, format!("Microfone aberto: {name} (48 kHz, {sample_format:?})")))
}

enum AudioCommand {
    Capture { enabled: bool, device_index: usize },
    Output { device_index: usize },
}

pub struct AudioSession {
    commands: std_mpsc::Sender<AudioCommand>,
    pub frames: broadcast::Sender<Vec<u8>>,
    pub playback: PlaybackBus,
}

impl AudioSession {
    pub fn new(events: mpsc::UnboundedSender<MeshEvent>, playback: PlaybackBus) -> Self {
        let (frames, _) = broadcast::channel(64);
        let (commands, receiver) = std_mpsc::channel();
        let tx = frames.clone();
        let output_bus = playback.clone();
        thread::spawn(move || {
            // Output stays alive on this thread. Replace only after the new
            // device has been opened and started successfully.
            let mut speaker: Option<Stream> = None;
            let mut output_index: Option<usize> = None;
            let mut mic: Option<Stream> = None;
            let mut mic_enabled: Option<Arc<AtomicBool>> = None;
            let mut mic_index: Option<usize> = None;
            while let Ok(command) = receiver.recv() {
                match command {
                    AudioCommand::Output { device_index } => {
                        if speaker.is_some() && output_index == Some(device_index) { continue; }
                        match output_stream(output_bus.clone(), device_index) {
                            Ok((new_stream, message)) => {
                                speaker = Some(new_stream);
                                output_index = Some(device_index);
                                let _ = events.send(MeshEvent::Status(message));
                            }
                            Err(e) => {
                                let _ = events.send(MeshEvent::Status(format!(
                                    "Falha ao mudar dispositivo de saida: {e}"
                                )));
                            }
                        }
                    }
                    AudioCommand::Capture { enabled: true, device_index } => {
                        if mic.is_some() && mic_index == Some(device_index) { continue; }
                        if let Some(flag) = mic_enabled.take() { flag.store(false, Ordering::Release); }
                        mic.take();
                        mic_index = None;
                        let enabled = Arc::new(AtomicBool::new(true));
                        match input_stream(tx.clone(), enabled.clone(), device_index, events.clone()) {
                            Ok((stream, message)) => {
                                mic = Some(stream);
                                mic_enabled = Some(enabled);
                                mic_index = Some(device_index);
                                let _ = events.send(MeshEvent::Status(message));
                            }
                            Err(e) => {
                                let _ = events.send(MeshEvent::Status(format!("Falha ao abrir microfone: {e}")));
                            }
                        }
                    }
                    AudioCommand::Capture { enabled: false, .. } => {
                        if let Some(flag) = mic_enabled.take() { flag.store(false, Ordering::Release); }
                        mic.take();
                        mic_index = None;
                        let _ = events.send(MeshEvent::MicLevel(0.0));
                        let _ = events.send(MeshEvent::Status("Microfone desligado".into()));
                    }
                }
            }
            if let Some(flag) = mic_enabled { flag.store(false, Ordering::Release); }
        });
        Self { commands, frames, playback }
    }

    pub fn capture(&self, enable: bool, device_index: usize) {
        let _ = self.commands.send(AudioCommand::Capture { enabled: enable, device_index });
    }

    pub fn output(&self, device_index: usize) {
        let _ = self.commands.send(AudioCommand::Output { device_index });
    }
}

pub struct PeerDecoder {
    decoder: Decoder,
    last_seq: Option<u32>,
}

impl PeerDecoder {
    pub fn new() -> Result<Self, opus::Error> {
        Ok(Self { decoder: Decoder::new(48_000, Channels::Mono)?, last_seq: None })
    }

    /// Returns Some(decoded sample count), None for an invalid/old message, Err on codec error.
    pub fn decode(&mut self, packet: &[u8], playback: &PlaybackBus, peer: &str)
        -> Result<Option<usize>, opus::Error> {
        if packet.len() <= 8 || &packet[..4] != HEADER { return Ok(None); }
        let seq = u32::from_be_bytes(packet[4..8].try_into().unwrap());
        if let Some(prev) = self.last_seq {
            let advance = seq.wrapping_sub(prev);
            if advance == 0 || advance >= 0x8000_0000 { return Ok(None); }
            for _ in 0..advance.saturating_sub(1).min(2) {
                let mut lost = [0.0f32; FRAME];
                if let Ok(n) = self.decoder.decode_float(&[], &mut lost, false) {
                    playback.push(peer, &lost[..n]);
                }
            }
        }
        self.last_seq = Some(seq);
        let mut samples = [0.0f32; 5760];
        let n = self.decoder.decode_float(&packet[8..], &mut samples, false)?;
        playback.push(peer, &samples[..n]);
        Ok(Some(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn per_peer_and_master_gain() {
        let bus = PlaybackBus::default();
        bus.set_peer_percent("a", 50.0);
        bus.set_master_percent(100.0);
        let mut state = bus.inner.lock().unwrap();
        let p = state.peers.get_mut("a").unwrap();
        p.started = true;
        p.queue.push_back(1.0);
        let sample = mix_next(&mut state);
        assert!((sample - 0.4).abs() < 0.001);
    }

    #[test]
    fn opus_roundtrip_mono() {
        let mut encoder = Encoder::new(48_000, Channels::Mono, Application::Voip).unwrap();
        let mut decoder = PeerDecoder::new().unwrap();
        let input = [0.1f32; FRAME];
        let mut data = [0u8; 1500];
        let len = encoder.encode_float(&input, &mut data).unwrap();
        let mut packet = vec![0u8; 8];
        packet[..4].copy_from_slice(HEADER);
        packet.extend_from_slice(&data[..len]);
        let bus = PlaybackBus::default();
        assert_eq!(decoder.decode(&packet, &bus, "peer").unwrap(), Some(FRAME));
    }
}
