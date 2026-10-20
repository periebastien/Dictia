//! Encodage FLAC en mémoire : compression sans perte, upload réduit.

use anyhow::{anyhow, Result};
use flacenc::component::BitRepr;
use flacenc::error::Verify;

/// Encode un signal mono `f32` (-1..1) en FLAC 16 bits.
pub fn encode_mono(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let pcm: Vec<i32> = samples
        .iter()
        .map(|&x| (x.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i32)
        .collect();
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| anyhow!("configuration FLAC invalide : {e:?}"))?;
    let source = flacenc::source::MemSource::from_samples(&pcm, 1, 16, sample_rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| anyhow!("encodage FLAC : {e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| anyhow!("écriture FLAC : {e:?}"))?;
    Ok(sink.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_valid_flac_header_and_compresses() {
        let samples: Vec<f32> = (0..16_000).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
        let bytes = encode_mono(&samples, 16_000).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        // Plus petit que le PCM 16 bits brut (32 000 octets).
        assert!(bytes.len() < 32_000, "{}", bytes.len());
    }

    #[test]
    fn stream_info_declares_16khz_mono_16bit() {
        let bytes = encode_mono(&[0.0; 1_000], 16_000).unwrap();
        // STREAMINFO : en-tête de bloc (4 octets) après « fLaC » ; la
        // fréquence (20 bits), les canaux - 1 (3 bits) et les bits - 1
        // (5 bits) commencent à l'octet 18.
        let info = &bytes[8..];
        let rate =
            (u32::from(info[10]) << 12) | (u32::from(info[11]) << 4) | (u32::from(info[12]) >> 4);
        let channels = ((info[12] >> 1) & 0b111) + 1;
        let bits = (((info[12] & 1) << 4) | (info[13] >> 4)) + 1;
        assert_eq!((rate, channels, bits), (16_000, 1, 16));
    }
}
