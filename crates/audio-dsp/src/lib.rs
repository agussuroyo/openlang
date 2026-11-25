//! Converts captured audio to the mono format speech recognizers expect.

use std::{sync::Arc, time::Duration};

use openlang_core::{
    Error, Result,
    domain::{AudioChunk, AudioFormat},
    ports::{AudioTransform, TransformFactory},
    registry::Registry,
};
use rubato::{Fft, FixedSync, Indexing, Resampler, audioadapter_buffers::direct::SequentialSlice};

pub fn register(registry: &mut Registry) {
    registry.resampler = Some(Arc::new(ToMono));
}

struct ToMono;

impl TransformFactory for ToMono {
    fn create(&self, output: AudioFormat) -> Box<dyn AudioTransform> {
        Box::new(MonoResampler {
            output,
            input_rate: 0,
            fft: None,
            pending: Vec::new(),
            next_at: None,
        })
    }
}

const BLOCK: usize = 1024;

struct MonoResampler {
    output: AudioFormat,
    input_rate: u32,
    fft: Option<Fft<f32>>,
    pending: Vec<f32>,
    next_at: Option<Duration>,
}

fn failed(e: impl std::fmt::Display) -> Error {
    Error::Device(format!("resampler: {e}"))
}

impl MonoResampler {
    fn mix(chunk: &AudioChunk) -> impl Iterator<Item = f32> + '_ {
        let channels = chunk.format.channels.max(1) as usize;
        chunk
            .samples
            .chunks_exact(channels)
            .map(move |f| f.iter().sum::<f32>() / channels as f32)
    }

    fn seconds(samples: usize, rate: u32) -> Duration {
        Duration::from_secs_f64(samples as f64 / f64::from(rate))
    }

    fn convert(&mut self, flush: bool) -> Result<Vec<f32>> {
        let Some(fft) = &mut self.fft else {
            return Ok(std::mem::take(&mut self.pending));
        };
        let mut all = Vec::new();
        let mut used = 0;
        loop {
            let need = fft.input_frames_next();
            let have = self.pending.len() - used;
            if have < need && !(flush && have > 0) {
                break;
            }
            let take = have.min(need);
            let mut out = vec![0.0; fft.output_frames_next()];
            let size = out.len();
            let block = &self.pending[used..used + take];
            let input = SequentialSlice::new(block, 1, take).map_err(failed)?;
            let mut output = SequentialSlice::new_mut(&mut out, 1, size).map_err(failed)?;
            let partial = (take < need).then_some(Indexing {
                input_offset: 0,
                output_offset: 0,
                partial_len: Some(take),
                active_channels_mask: None,
            });
            let (_, written) = fft
                .process_into_buffer(&input, &mut output, partial.as_ref())
                .map_err(failed)?;
            all.extend_from_slice(&out[..written]);
            used += take;
        }
        self.pending.drain(..used);
        Ok(all)
    }

    fn chunk(&mut self, samples: Vec<f32>) -> Option<AudioChunk> {
        if samples.is_empty() {
            return None;
        }
        let at = self.next_at.unwrap_or_default();
        self.next_at = Some(at + Self::seconds(samples.len(), self.output.sample_rate));
        Some(AudioChunk {
            format: self.output,
            samples,
            at,
            segment: None,
        })
    }
}

impl AudioTransform for MonoResampler {
    fn process(&mut self, chunk: &AudioChunk) -> Result<Option<AudioChunk>> {
        let rate = chunk.format.sample_rate;
        if rate != self.input_rate {
            // The device changed; what is left of the old rate is under one block.
            self.pending.clear();
            self.input_rate = rate;
            self.fft = if rate == self.output.sample_rate {
                None
            } else {
                let fixed = FixedSync::Input;
                Some(
                    Fft::new(
                        rate as usize,
                        self.output.sample_rate as usize,
                        BLOCK,
                        1,
                        fixed,
                    )
                    .map_err(failed)?,
                )
            };
        }
        self.next_at.get_or_insert(chunk.at);
        self.pending.extend(Self::mix(chunk));
        let samples = self.convert(false)?;
        Ok(self.chunk(samples))
    }

    fn flush(&mut self) -> Result<Option<AudioChunk>> {
        let samples = self.convert(true)?;
        Ok(self.chunk(samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, channels: u16, hz: f64) -> AudioChunk {
        let samples = (0..rate)
            .flat_map(|i| {
                let v = (f64::from(i) / f64::from(rate) * hz * std::f64::consts::TAU).sin() as f32;
                std::iter::repeat_n(v, channels as usize)
            })
            .collect();
        AudioChunk {
            format: AudioFormat {
                sample_rate: rate,
                channels,
            },
            samples,
            at: Duration::from_secs(5),
            segment: None,
        }
    }

    fn convert(chunk: &AudioChunk) -> AudioChunk {
        let mut t = ToMono.create(AudioFormat::SPEECH);
        let mut first = t.process(chunk).unwrap().unwrap();
        if let Some(rest) = t.flush().unwrap() {
            first.samples.extend(rest.samples);
        }
        first
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn keeps_speech_and_length_at_common_rates() {
        for (rate, channels) in [(48_000, 2), (44_100, 2), (24_000, 1), (16_000, 1)] {
            let out = convert(&tone(rate, channels, 440.0));
            assert_eq!(out.format, AudioFormat::SPEECH);
            assert_eq!(out.at, Duration::from_secs(5));
            assert!(
                (out.samples.len() as i64 - 16_000).abs() < 1_200,
                "{rate}: {}",
                out.samples.len()
            );
            let middle = &out.samples[4_000..12_000];
            assert!(
                (rms(middle) - 0.707).abs() < 0.02,
                "{rate}: {}",
                rms(middle)
            );
        }
    }

    #[test]
    fn removes_what_would_alias() {
        let out = convert(&tone(48_000, 1, 15_000.0));
        assert!(rms(&out.samples[4_000..12_000]) < 0.01);
    }
}
