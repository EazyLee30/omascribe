//! Bounded, local input leveling. Never modifies recorded PCM or invents samples.
#[derive(Default)]
pub struct SpeechLevel {
    peak_hold: f32,
}
impl SpeechLevel {
    pub fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        let peak = samples.iter().map(|v| v.abs()).fold(0f32, f32::max);
        let rms = (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt();
        // Retain recent speech peaks so ordinary pauses/consonants do not pump the gain.
        let decay = (-(samples.len() as f32) / 32000.).exp();
        self.peak_hold = peak.max(self.peak_hold * decay);
        if rms < 0.00015 || peak < 0.0005 || self.peak_hold >= 0.02 {
            return;
        }
        let gain = (0.25 / self.peak_hold).clamp(1., 128.).min(0.90 / peak);
        for sample in samples {
            *sample *= gain;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_voice_gains_headroom_without_amplifying_silence_or_clipping() {
        let mut level = SpeechLevel::default();
        let mut silence = vec![0.; 3200];
        level.process(&mut silence);
        assert!(silence.iter().all(|s| *s == 0.));
        let mut quiet = vec![0.001; 3200];
        level.process(&mut quiet);
        assert!((quiet[0] - 0.128).abs() < 0.00001);
        let mut loud = vec![0.5; 3200];
        level.process(&mut loud);
        assert!(loud.iter().all(|s| *s == 0.5));
        let mut consonant = vec![0.001; 3200];
        level.process(&mut consonant);
        assert_eq!(consonant[0], 0.001);
        let mut noise = vec![1. / 32768.; 3200];
        SpeechLevel::default().process(&mut noise);
        assert_eq!(noise[0], 1. / 32768.);
    }
}
