use std::time::Instant;

#[derive(Debug, PartialEq)]
pub enum VadResult {
    Speaking,
    Silence,
    EndOfUtterance,
}

pub struct Vad {
    threshold: f32,
    silence_ms: u64,
    last_speech_at: Option<Instant>,
}

impl Vad {
    pub fn new(threshold: f32, silence_ms: u64) -> Self {
        Vad {
            threshold,
            silence_ms,
            last_speech_at: None,
        }
    }

    pub fn process(&mut self, samples: &[f32]) -> VadResult {
        let rms = if samples.is_empty() {
            0.0
        } else {
            let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
            (sum_sq / samples.len() as f32).sqrt()
        };

        if rms > self.threshold {
            self.last_speech_at = Some(Instant::now());
            return VadResult::Speaking;
        }

        match self.last_speech_at {
            None => VadResult::Silence,
            Some(t) => {
                let elapsed_ms = t.elapsed().as_millis() as u64;
                if elapsed_ms >= self.silence_ms {
                    VadResult::EndOfUtterance
                } else {
                    VadResult::Silence
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.last_speech_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn silence_samples_not_speech() {
        let mut vad = Vad::new(0.02, 800);
        let samples = vec![0.0f32; 512];
        assert_eq!(vad.process(&samples), VadResult::Silence);
    }

    #[test]
    fn loud_samples_are_speech() {
        let mut vad = Vad::new(0.02, 800);
        let samples = vec![0.5f32; 512];
        assert_eq!(vad.process(&samples), VadResult::Speaking);
        assert!(vad.last_speech_at.is_some());
    }

    #[test]
    fn end_of_utterance_after_silence_window() {
        let mut vad = Vad::new(0.02, 50);
        // first trigger speech
        let loud = vec![0.5f32; 512];
        assert_eq!(vad.process(&loud), VadResult::Speaking);
        // wait longer than silence_ms
        thread::sleep(Duration::from_millis(100));
        let silent = vec![0.0f32; 512];
        assert_eq!(vad.process(&silent), VadResult::EndOfUtterance);
    }
}
