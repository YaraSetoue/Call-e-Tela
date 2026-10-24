//! Captura da tela Windows + H.264 (OpenH264 software), com fallback JPEG.
//! 720p 15 fps, canal WebRTC existente. Audio da tela permanece independente.
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, thread, time::{Duration, Instant}};
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, ColorType, DynamicImage};
use openh264::{OpenH264API, encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, UsageType}, formats::{RgbSliceU8, YUVBuffer}};
use tokio::sync::{broadcast, mpsc};
use crate::p2p::MeshEvent;

pub const CAP_WIDTH: u32 = 1280;
pub const CAP_HEIGHT: u32 = 720;
pub const CAP_FPS: u32 = 15;
pub const CHUNK_BYTES: usize = 8 * 1024;
pub const MAX_FRAME_BYTES: usize = 1_000_000;
pub const MAX_CHUNKS: usize = 128;
pub const HEADER_SIZE: usize = 12;

/// Perfil planejado, nao habilitado na interface nesta etapa.
#[allow(dead_code)]
pub const FUTURE_PROFILES: &[(u32,u32)] = &[
    (256, 144), (996, 560), (1280, 720), (1920, 1080), (2560, 1440),
];
#[allow(dead_code)]
pub const FUTURE_FPS: &[u32] = &[15, 30, 60];

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
    ) -> Result<Self, String> {
        if index >= xcap::Monitor::all().map_err(|e| e.to_string())?.len() {
            return Err("Monitor nao encontrado".to_owned());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        thread::spawn(move || {
            // Abrir o monitor dentro da thread evita transferir handles Win32 entre threads.
            let Some(mon) = xcap::Monitor::all().ok().and_then(|items| items.into_iter().nth(index)) else {
                let _ = events.send(MeshEvent::Status("Monitor indisponivel ao iniciar captura".into()));
                return;
            };
            // Um encoder por transmissao. IDR a cada 2s permite que novos
            // participantes comecem a receber mesmo apos a captura iniciar.
            let config = EncoderConfig::new()
                .usage_type(UsageType::ScreenContentRealTime)
                .bitrate(BitRate::from_bps(2_000_000))
                .skip_frames(true)
                .adaptive_quantization(false)
                .background_detection(false)
                .max_frame_rate(FrameRate::from_hz(CAP_FPS as f32))
                .intra_frame_period(IntraFramePeriod::from_num_frames(CAP_FPS * 2));
            let mut h264 = if std::env::var("VOICE_VIDEO_CODEC").as_deref() == Ok("jpeg") {
                let _ = events.send(MeshEvent::Status(
                    "Video em modo JPEG (VOICE_VIDEO_CODEC=jpeg)".into(),
                ));
                None
            } else {
                match Encoder::with_api_config(OpenH264API::from_source(), config) {
                    Ok(value) => {
                        let _ = events.send(MeshEvent::Status(
                            "Video H.264 OpenH264 iniciado: 720p/15 FPS, ~2 Mbps (software)".into(),
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
            let duration = Duration::from_nanos(1_000_000_000 / CAP_FPS as u64);
            let mut failures = 0u32;
            let mut encoded_count = 0u64;
            while !stopped.load(Ordering::Relaxed) {
                let started = Instant::now();
                let result = (|| -> Result<(), String> {
                    let raw = mon.capture_image().map_err(|e| e.to_string())?;
                    let image = DynamicImage::ImageRgba8(raw)
                        .resize(CAP_WIDTH, CAP_HEIGHT, FilterType::Triangle).to_rgb8();
                    let (width, height) = image.dimensions();
                    let bytes = image.as_raw();

                    if let Some(encoder) = h264.as_mut() {
                        let rgb = RgbSliceU8::new(bytes, (width as usize, height as usize));
                        let yuv = YUVBuffer::from_rgb_source(rgb);
                        let bitstream = encoder.encode(&yuv).map_err(|e| e.to_string())?;
                        let compressed = bitstream.to_vec();
                        if !compressed.is_empty() && compressed.len() <= MAX_FRAME_BYTES {
                            let mut message = Vec::with_capacity(4 + compressed.len());
                            // Marcador apenas INTERNO para escolher o tipo de pacote.
                            message.extend_from_slice(b"VSHF");
                            message.extend_from_slice(&compressed);
                            let _ = outgoing.send(message);
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
                        if jpeg.len() <= MAX_FRAME_BYTES {
                            let _ = outgoing.send(jpeg);
                        }
                    }
                    // Previa local nao precisa recodificar: usa RGB da captura.
                    let _ = events.send(MeshEvent::ScreenRgbFrame {
                        peer: "__local__".into(), width, height, rgb: image.into_raw(),
                    });
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
