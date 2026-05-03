//! Audio format conversion for STT consumption.
//!
//! ScreenCaptureKit delivers Float32 LPCM at 48 kHz stereo. Soniox (and most
//! streaming STT services) want S16LE PCM at 16 kHz mono. This module does
//! the three-step conversion: channel mix → resample → format convert.
//!
//! The resampler is a simple linear interpolator. For human speech (≤ 8 kHz)
//! aliasing from skipping the anti-alias low-pass is inaudible to STT models.
//!
//! See `docs/specs/phase-2-step-15-live-pipeline.md` §6.4.

/// Convert interleaved Float32 PCM at `src_sample_rate` to mono S16LE PCM
/// at 16 kHz. Returns the converted byte buffer.
pub fn convert_to_stt_pcm(src: &[f32], src_sample_rate: u32, src_channels: u16) -> Vec<u8> {
    // 1. Mix to mono
    let mono: Vec<f32> = if src_channels == 1 {
        src.to_vec()
    } else {
        src.chunks_exact(src_channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect()
    };

    // 2. Resample to 16 kHz (linear interpolation)
    let target_rate = 16000_u32;
    if src_sample_rate == target_rate {
        // No resample needed
        let mut out = Vec::with_capacity(mono.len() * 2);
        for s in mono {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        return out;
    }

    let ratio = src_sample_rate as f32 / target_rate as f32;
    let target_len = (mono.len() as f32 / ratio) as usize;
    let mut resampled = Vec::with_capacity(target_len);
    for i in 0..target_len {
        let src_idx_f = i as f32 * ratio;
        let lo = src_idx_f.floor() as usize;
        let hi = (lo + 1).min(mono.len().saturating_sub(1));
        let frac = src_idx_f - lo as f32;
        let s = mono[lo] * (1.0 - frac) + mono[hi] * frac;
        resampled.push(s);
    }

    // 3. Convert Float32 → S16LE
    let mut out = Vec::with_capacity(resampled.len() * 2);
    for s in resampled {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_mono_48k_to_mono_16k_is_3_to_1() {
        let src = vec![0.5_f32; 48000]; // 1 second of mono at 48k
        let out = convert_to_stt_pcm(&src, 48000, 1);
        // 16 kHz mono S16LE = 16000 samples × 2 bytes = 32000 bytes
        assert!(
            (out.len() as i32 - 32000_i32).abs() <= 4,
            "expected ≈32000 bytes, got {}",
            out.len()
        );
    }

    #[test]
    fn convert_stereo_48k_mixes_channels_to_zero() {
        // Interleaved stereo: L=1.0, R=-1.0 — mono mix is zero
        let src = vec![1.0_f32, -1.0_f32].repeat(48000);
        let out = convert_to_stt_pcm(&src, 48000, 2);
        let all_zero = out
            .chunks_exact(2)
            .all(|b| i16::from_le_bytes([b[0], b[1]]) == 0);
        assert!(
            all_zero,
            "expected all-zero S16LE samples after mixing L+R/2"
        );
    }

    #[test]
    fn convert_clamps_out_of_range_to_max() {
        // A sample value > 1.0 should clamp to i16::MAX, not wrap
        let src = vec![2.5_f32; 16000]; // already at target rate
        let out = convert_to_stt_pcm(&src, 16000, 1);
        let first = i16::from_le_bytes([out[0], out[1]]);
        assert_eq!(first, i16::MAX);
    }

    #[test]
    fn convert_empty_input_returns_empty() {
        let out = convert_to_stt_pcm(&[], 48000, 2);
        assert!(out.is_empty());
    }
}
