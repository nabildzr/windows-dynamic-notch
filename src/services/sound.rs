//! Soft two-tone chime for timer and reminder notifications, synthesized
//! once into an in-memory WAV and played asynchronously with `PlaySoundW`.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::core::PCWSTR;

const SAMPLE_RATE: u32 = 44_100;

pub fn play_chime() {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    let wav = WAV.get_or_init(build_chime);
    // SND_MEMORY interprets the pointer as the WAV image, which stays alive
    // for the whole program in the `OnceLock`.
    let played = unsafe { PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
    if !played.as_bool() {
        log::debug!("PlaySoundW failed");
    }
}

/// Two bell-like notes (A5 then E6) with soft attack and exponential decay.
fn build_chime() -> Vec<u8> {
    let duration = 1.25f32;
    let count = (SAMPLE_RATE as f32 * duration) as usize;
    let notes = [(880.0f32, 0.0f32), (1318.5, 0.17)];
    let mut samples = Vec::with_capacity(count);
    for n in 0..count {
        let t = n as f32 / SAMPLE_RATE as f32;
        let mut value = 0.0;
        for (frequency, start) in notes {
            let local = t - start;
            if local < 0.0 {
                continue;
            }
            let attack = (local / 0.008).min(1.0);
            let envelope = attack * (-local / 0.32).exp();
            // Fundamental plus a quiet octave partial for a bell timbre.
            value += envelope * ((TAU * frequency * local).sin() + 0.18 * (TAU * frequency * 2.0 * local).sin());
        }
        samples.push((value * 0.16 * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16);
    }
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}
