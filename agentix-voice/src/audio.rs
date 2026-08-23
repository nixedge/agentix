use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub struct AudioRecorder {
    sample_rate: u32,
}

impl AudioRecorder {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    /// Starts recording from the default input device. Sends chunks of f32 samples to `tx`.
    /// Returns a guard that stops recording when dropped.
    pub fn start(&self, tx: mpsc::Sender<Vec<f32>>) -> Result<RecordingGuard> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no input device"))?;

        let desired_rate = self.sample_rate;
        let supported = device
            .supported_input_configs()
            .context("querying input configs")?
            .find(|c| {
                c.channels() == 1
                    && c.sample_format() == SampleFormat::F32
                    && c.min_sample_rate().0 <= desired_rate
                    && c.max_sample_rate().0 >= desired_rate
            });

        let config: StreamConfig = match supported {
            Some(c) => c.with_sample_rate(SampleRate(desired_rate)).into(),
            None => {
                let c = device
                    .default_input_config()
                    .context("getting default input config")?;
                c.into()
            }
        };

        let err_fn = |e| tracing::error!(error = %e, "audio input stream error");
        let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = Arc::clone(&buf);
        let tx_clone = tx.clone();

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let Ok(mut b) = buf_clone.lock() else { return };
                    b.extend_from_slice(data);
                    // send chunks of ~20ms at 16kHz = 320 samples
                    const CHUNK: usize = 320;
                    while b.len() >= CHUNK {
                        let chunk: Vec<f32> = b.drain(..CHUNK).collect();
                        let _ = tx_clone.try_send(chunk);
                    }
                },
                err_fn,
                None,
            )
            .context("building input stream")?;

        stream.play().context("starting input stream")?;
        Ok(RecordingGuard { _stream: stream })
    }
}

pub struct RecordingGuard {
    _stream: cpal::Stream,
}

/// Play f32 PCM samples at the given sample rate on the default output device.
pub fn play_f32(samples: Vec<f32>, sample_rate: u32) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no output device"))?;

    let desired_rate = sample_rate;
    let supported = device
        .supported_output_configs()
        .context("querying output configs")?
        .find(|c| {
            c.channels() == 1
                && c.sample_format() == SampleFormat::F32
                && c.min_sample_rate().0 <= desired_rate
                && c.max_sample_rate().0 >= desired_rate
        });

    let config: StreamConfig = match supported {
        Some(c) => c.with_sample_rate(SampleRate(desired_rate)).into(),
        None => device
            .default_output_config()
            .context("getting default output config")?
            .into(),
    };

    let pos = Arc::new(Mutex::new(0usize));
    let samples = Arc::new(samples);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let pos_clone = Arc::clone(&pos);
    let samples_clone = Arc::clone(&samples);
    let done_clone = Arc::clone(&done);

    let err_fn = |e| tracing::error!(error = %e, "audio output stream error");

    let stream = device
        .build_output_stream(
            &config,
            move |output: &mut [f32], _| {
                let Ok(mut p) = pos_clone.lock() else { return };
                for frame in output.iter_mut() {
                    if *p < samples_clone.len() {
                        *frame = samples_clone[*p];
                        *p += 1;
                    } else {
                        *frame = 0.0;
                        done_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            },
            err_fn,
            None,
        )
        .context("building output stream")?;

    stream.play().context("starting output stream")?;

    // Block until all samples have been played (or timeout)
    let timeout = std::time::Duration::from_secs((samples.len() as u64 / sample_rate as u64) + 2);
    let start = std::time::Instant::now();
    while !done.load(std::sync::atomic::Ordering::Relaxed) {
        if start.elapsed() > timeout {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    Ok(())
}
