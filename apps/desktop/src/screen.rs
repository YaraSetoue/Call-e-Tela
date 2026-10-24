//! Captura da tela Windows + H.264 (OpenH264 software), com fallback JPEG.
//! Perfil de qualidade selecionavel, canal WebRTC existente. Audio permanece independente.
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, thread, time::{Duration, Instant}};
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, ColorType, DynamicImage};
use openh264::{OpenH264API, encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, UsageType}, formats::{RgbSliceU8, YUVBuffer}};
use tokio::sync::{broadcast, mpsc};
use crate::p2p::MeshEvent;

#[cfg(target_os = "windows")]
#[path = "native_video.rs"]
mod native_video;

pub const CHUNK_BYTES: usize = 8 * 1024;
pub const MAX_FRAME_BYTES: usize = 1_000_000;
pub const MAX_CHUNKS: usize = 128;
pub const HEADER_SIZE: usize = 12;

// Todas as resolucoes sao pares, exigencia pratica dos encoders YUV 4:2:0.
// FPS alto / 1440p com codificacao por CPU e experimental: acompanhe o uso de CPU.
const RESOLUTIONS: &[(u32, u32)] = &[
    (256, 144), (996, 560), (1280, 720), (1920, 1080), (2560, 1440),
];
const FRAME_RATES: &[u32] = &[15, 30, 60];

#[derive(Clone, Copy, Debug)]
pub struct VideoProfile {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

impl VideoProfile {
    pub fn from_indices(resolution: usize, frame_rate: usize) -> Self {
        let (width, height) = RESOLUTIONS.get(resolution).copied().unwrap_or((1280, 720));
        let fps = FRAME_RATES.get(frame_rate).copied().unwrap_or(15);
        let relative = (width as f64 * height as f64 * fps as f64) / (1280.0 * 720.0 * 15.0);
        let bitrate_bps = (2_000_000.0 * relative.sqrt()).round().clamp(150_000.0, 8_000_000.0) as u32;
        Self { width, height, fps, bitrate_bps }
    }
    pub fn label(self) -> String {
        format!("{}p / {} FPS", self.height, self.fps)
    }
}

#[cfg(test)]
mod video_profile_tests {
    use super::*;
    #[test]
    fn profile_defaults_and_ranges() {
        let standard = VideoProfile::from_indices(2, 0);
        assert_eq!((standard.width, standard.height, standard.fps, standard.bitrate_bps), (1280, 720, 15, 2_000_000));
        for resolution in 0..5 {
            for fps in 0..3 {
                let value = VideoProfile::from_indices(resolution, fps);
                assert_eq!(value.width % 2, 0);
                assert_eq!(value.height % 2, 0);
                assert!(value.bitrate_bps <= 8_000_000);
            }
        }
    }
}

pub fn monitor_names() -> Vec<String> {
    xcap::Monitor::all().map(|monitors| monitors.into_iter().enumerate().map(|(i, mon)| {
        format!("Monitor {} - {}", i + 1,
            mon.friendly_name().unwrap_or_else(|_| "Tela".to_owned()))
    }).collect()).unwrap_or_default()
}

pub struct CaptureHandle { stop: Arc<AtomicBool> }
impl CaptureHandle {
    pub fn start(
        index: usize,
        outgoing: broadcast::Sender<Vec<u8>>,
        events: mpsc::UnboundedSender<MeshEvent>,
        profile: VideoProfile,
    ) -> Result<Self, String> {
        if index >= xcap::Monitor::all().map_err(|e| e.to_string())?.len() {
            return Err("Monitor nao encontrado".to_owned());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        thread::spawn(move || {
            // Backend Windows Graphics Capture + Media Foundation H.264 por hardware.
            // Nenhum ffmpeg.exe ou processo externo. OpenH264 CPU e fallback.
            // VOICE_VIDEO_BACKEND=software desativa explicitamente a via GPU.
            let backend = std::env::var("VOICE_VIDEO_BACKEND")
                .unwrap_or_else(|_| "auto".into());
            let jpeg_forced = std::env::var("VOICE_VIDEO_CODEC").as_deref() == Ok("jpeg");
            if !jpeg_forced && backend != "software" {
                #[cfg(target_os = "windows")]
                {
                    match native_video::capture(
                        index, profile, outgoing.clone(), events.clone(), stopped.clone(),
                    ) {
                        Ok(()) => return, // Parou normalmente.
                        Err(error) => {
                            if stopped.load(Ordering::Relaxed) { return; }
                            let _ = events.send(MeshEvent::Status(format!(
                                "Captura H.264 nativa por GPU indisponivel: {error}. Usando OpenH264 (CPU)."
                            )));
                        }
                    }
                }
            }
            if stopped.load(Ordering::Relaxed) { return; }
            // Abrir o monitor dentro da thread evita transferir handles Win32 entre threads.
            let Some(mon) = xcap::Monitor::all().ok().and_then(|items| items.into_iter().nth(index)) else {
                let _ = events.send(MeshEvent::Status("Monitor indisponivel ao iniciar captura".into()));
                return;
            };
            // Um encoder por transmissao. IDR a cada 2s permite que novos
            // participantes comecem a receber mesmo apos a captura iniciar.
            let config = EncoderConfig::new()
                .usage_type(UsageType::ScreenContentRealTime)
                .bitrate(BitRate::from_bps(profile.bitrate_bps))
                // Para SCREEN_CONTENT_REAL_TIME, estes recursos sao desativados
                // pelo OpenH264; explicitar evita avisos na inicializacao.
                .adaptive_quantization(false)
                .background_detection(false)
                // Permite controle efetivo do bitrate (ate 2 Mbps alvo).
                .skip_frames(true)
                .max_frame_rate(FrameRate::from_hz(profile.fps as f32))
                .intra_frame_period(IntraFramePeriod::from_num_frames(profile.fps * 2));
            let mut h264 = if std::env::var("VOICE_VIDEO_CODEC").as_deref() == Ok("jpeg") {
                let _ = events.send(MeshEvent::Status(
                    "Video em modo JPEG (VOICE_VIDEO_CODEC=jpeg)".into(),
                ));
                None
            } else {
                match Encoder::with_api_config(OpenH264API::from_source(), config) {
                    Ok(value) => {
                        let _ = events.send(MeshEvent::Status(
                            format!("Video H.264 OpenH264: {}, alvo {:.2} Mbps (software)",
                                profile.label(), profile.bitrate_bps as f32 / 1_000_000.0),
                        ));
                        Some(value)
                    }
                    Err(error) => {
                        let _ = events.send(MeshEvent::Status(format!(
                            "H.264 indisponivel ({error}); usando JPEG como fallback"
                        )));
                        None
                    }
                }
            };
            let duration = Duration::from_nanos(1_000_000_000 / profile.fps as u64);
            let mut failures = 0u32;
            let mut encoded_count = 0u64;
            // O encoder usa a taxa escolhida. Somente a previa LOCAL e limitada
            // a 15 FPS, evitando filas de RGB na UI ao transmitir a 30/60 FPS.
            let preview_interval = Duration::from_nanos(1_000_000_000 / profile.fps.min(15).max(1) as u64);
            let mut last_preview = Instant::now() - preview_interval;
            // Telemetria em janelas de 5 segundos para identificar gargalos reais.
            let mut window_at = Instant::now();
            let mut capture_ok = 0u64;
            let mut produced = 0u64;
            let mut capture_time = Duration::ZERO;
            let mut resize_time = Duration::ZERO;
            let mut encode_time = Duration::ZERO;
            let mut encoded_bytes = 0u64;
            while !stopped.load(Ordering::Relaxed) {
                let started = Instant::now();
                let result = (|| -> Result<(), String> {
                    let capture_at = Instant::now();
                    let raw = mon.capture_image().map_err(|e| e.to_string())?;
                    capture_time += capture_at.elapsed();
                    capture_ok += 1;
                    let resize_at = Instant::now();
                    let resized = DynamicImage::ImageRgba8(raw)
                        .resize(profile.width, profile.height, FilterType::Triangle);
                    // H.264 4:2:0 exige dimensoes pares. Cortar no maximo 1px
                    // evita falhas em monitores com proporcoes incomuns.
                    let even_width = resized.width() & !1;
                    let even_height = resized.height() & !1;
                    if even_width < 2 || even_height < 2 {
                        return Err("Dimensoes da captura invalidas".to_string());
                    }
                    let image = resized.crop_imm(0, 0, even_width, even_height).to_rgb8();
                    let (width, height) = image.dimensions();
                    let bytes = image.as_raw();
                    resize_time += resize_at.elapsed();
                    let encode_at = Instant::now();

                    if let Some(encoder) = h264.as_mut() {
                        let rgb = RgbSliceU8::new(bytes, (width as usize, height as usize));
                        let yuv = YUVBuffer::from_rgb_source(rgb);
                        let bitstream = encoder.encode(&yuv).map_err(|e| e.to_string())?;
                        let compressed = bitstream.to_vec();
                        encode_time += encode_at.elapsed();
                        if !compressed.is_empty() && compressed.len() <= MAX_FRAME_BYTES {
                            let mut message = Vec::with_capacity(4 + compressed.len());
                            // Marcador apenas INTERNO para escolher o tipo de pacote.
                            message.extend_from_slice(b"VSHF");
                            message.extend_from_slice(&compressed);
                            let _ = outgoing.send(message);
                            produced += 1;
                            encoded_bytes += compressed.len() as u64;
                            encoded_count += 1;
                            if encoded_count == 1 {
                                let _ = events.send(MeshEvent::Status(format!(
                                    "Primeiro quadro H.264 enviado ({} bytes)", compressed.len(),
                                )));
                            }
                        } else if compressed.len() > MAX_FRAME_BYTES {
                            return Err(format!("H.264 quadro excede limite: {} bytes", compressed.len()));
                        }
                    } else {
                        let mut jpeg = Vec::with_capacity(65_536);
                        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 48);
                        encoder.encode(bytes, width, height, ColorType::Rgb8.into())
                            .map_err(|e| e.to_string())?;
                        encode_time += encode_at.elapsed();
                        if jpeg.len() <= MAX_FRAME_BYTES {
                            encoded_bytes += jpeg.len() as u64;
                            produced += 1;
                            let _ = outgoing.send(jpeg);
                        }
                    }
                    // Previa local limitada a 15 FPS, mas rede/encoder continuam
                    // com o FPS do perfil (ate 60). Economiza copias RGB e UI.
                    if last_preview.elapsed() >= preview_interval {
                        last_preview = Instant::now();
                        let _ = events.send(MeshEvent::ScreenRgbFrame {
                            peer: "__local__".into(), width, height, rgb: image.into_raw(),
                        });
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => failures = 0,
                    Err(e) => {
                        failures += 1;
                        if failures == 1 || failures % 150 == 0 {
                            let _ = events.send(MeshEvent::Status(format!(
                                "Falha capturando/codificando video: {e}"
                            )));
                        }
                        if failures == 3 && h264.is_some() {
                            h264 = None;
                            let _ = events.send(MeshEvent::Status(
                                "H.264 falhou repetidamente; alternando para JPEG".into(),
                            ));
                        }
                    }
                }
                if window_at.elapsed() >= Duration::from_secs(5) {
                    let secs = window_at.elapsed().as_secs_f64();
                    let denom = capture_ok.max(1) as f64;
                    let mode = if h264.is_some() { "H264" } else { "JPEG" };
                    let _ = events.send(MeshEvent::Status(format!(
                        "VIDEO ORIGEM {} {}: capturado {:.1} FPS / codificado {:.1} FPS; media captura {:.1}ms, resize+RGB {:.1}ms, encoder+YUV {:.1}ms; saida {:.2} Mbps (antes de replicar por espectador)",
                        mode, profile.label(), capture_ok as f64 / secs, produced as f64 / secs,
                        capture_time.as_secs_f64() * 1000.0 / denom,
                        resize_time.as_secs_f64() * 1000.0 / denom,
                        encode_time.as_secs_f64() * 1000.0 / denom,
                        encoded_bytes as f64 * 8.0 / secs / 1_000_000.0,
                    )));
                    capture_ok = 0;
                    produced = 0;
                    capture_time = Duration::ZERO;
                    resize_time = Duration::ZERO;
                    encode_time = Duration::ZERO;
                    encoded_bytes = 0;
                    window_at = Instant::now();
                }
                if let Some(wait) = duration.checked_sub(started.elapsed()) { thread::sleep(wait); }
            }

        });
        Ok(Self { stop })
    }
}
impl Drop for CaptureHandle {
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); }
}

/// Pacote de JPEG: VSS1 + frame_id(u32 BE) + index(u16 BE) + count(u16 BE) + dados.
pub fn chunks(frame_id: u32, jpeg: &[u8]) -> Option<Vec<Vec<u8>>> {
    chunks_with_magic(*b"VSS1", frame_id, jpeg)
}

/// Quadros H.264 Annex-B sao fragmentados com VSH1 (12 bytes de cabecalho).
pub fn chunks_h264(frame_id: u32, h264: &[u8]) -> Option<Vec<Vec<u8>>> {
    chunks_with_magic(*b"VSH1", frame_id, h264)
}

fn chunks_with_magic(magic: [u8; 4], frame_id: u32, jpeg: &[u8]) -> Option<Vec<Vec<u8>>> {
    if jpeg.len() > MAX_FRAME_BYTES || jpeg.is_empty() { return None; }
    let total = jpeg.len().div_ceil(CHUNK_BYTES);
    if total > MAX_CHUNKS { return None; }
    Some(jpeg.chunks(CHUNK_BYTES).enumerate().map(|(i, payload)| {
        let mut result = Vec::with_capacity(HEADER_SIZE + payload.len());
        result.extend_from_slice(&magic);
        result.extend_from_slice(&frame_id.to_be_bytes());
        result.extend_from_slice(&(i as u16).to_be_bytes());
        result.extend_from_slice(&(total as u16).to_be_bytes());
        result.extend_from_slice(payload);
        result
    }).collect())
}

pub struct Reassembler {
    magic: [u8; 4],
    id: Option<u32>,
    parts: Vec<Option<Vec<u8>>>,
    received: usize,
    bytes: usize,
    since: Instant,
}
impl Reassembler {
    pub fn new() -> Self {
        Self::new_with_magic(*b"VSS1")
    }
    pub fn new_h264() -> Self {
        Self::new_with_magic(*b"VSH1")
    }
    fn new_with_magic(magic: [u8;4]) -> Self {
        Self { magic, id: None, parts: Vec::new(), received: 0, bytes: 0, since: Instant::now() }
    }
    pub fn push(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        if packet.len() < HEADER_SIZE || packet[..4] != self.magic { return None; }
        let id = u32::from_be_bytes(packet[4..8].try_into().ok()?);
        let index = u16::from_be_bytes(packet[8..10].try_into().ok()?) as usize;
        let total = u16::from_be_bytes(packet[10..12].try_into().ok()?) as usize;
        if total == 0 || total > MAX_CHUNKS || index >= total { return None; }
        if self.id != Some(id) || self.since.elapsed() > Duration::from_millis(350) {
            self.id = Some(id);
            self.parts = vec![None; total];
            self.received = 0;
            self.bytes = 0;
            self.since = Instant::now();
        }
        if self.parts.len() != total || self.parts[index].is_some() { return None; }
        self.bytes += packet.len() - HEADER_SIZE;
        if self.bytes > MAX_FRAME_BYTES {
            self.id = None;
            self.parts.clear();
            return None;
        }
        self.parts[index] = Some(packet[HEADER_SIZE..].to_vec());
        self.received += 1;
        if self.received != total { return None; }
        let mut jpeg = Vec::with_capacity(self.bytes);
        for part in &mut self.parts { jpeg.extend_from_slice(part.take()?.as_slice()); }
        self.id = None;
        self.parts.clear();
        Some(jpeg)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reassemble_h264_and_jpeg_with_distinct_magics() {
        let h264 = vec![0x7bu8; CHUNK_BYTES * 2 + 23];
        let pieces = chunks_h264(42, &h264).unwrap();
        assert_eq!(pieces.len(), 3);
        let mut decoder = Reassembler::new_h264();
        let mut jpeg_decoder = Reassembler::new();
        assert!(jpeg_decoder.push(&pieces[0]).is_none());
        for part in pieces.iter().take(2) {
            assert!(decoder.push(part).is_none());
        }
        assert_eq!(decoder.push(&pieces[2]), Some(h264));
    }

    #[test]
    fn oversized_frame_and_bad_chunks_are_rejected() {
        assert!(chunks_h264(1, &vec![0x01; MAX_FRAME_BYTES + 1]).is_none());
        assert!(chunks_h264(1, &[]).is_none());
        let mut assembler = Reassembler::new_h264();
        assert!(assembler.push(b"VSH1").is_none());
        let mut wrong = chunks_h264(2, &[1, 2, 3]).unwrap().remove(0);
        wrong[10] = 0xff;
        wrong[11] = 0xff;
        assert!(assembler.push(&wrong).is_none());
    }
}
