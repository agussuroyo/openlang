//! Lock-free hand-off from an audio callback to a worker thread.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
};

use crate::domain::AudioFormat;

#[derive(Default)]
struct Shared {
    lost: AtomicU64,
    invalid: AtomicBool,
}

/// Safe to use inside a realtime callback: never allocates, locks or blocks.
pub struct AudioWriter {
    ring: rtrb::Producer<f32>,
    channels: usize,
    shared: Arc<Shared>,
}

impl AudioWriter {
    pub fn write(&mut self, samples: &[f32]) {
        self.write_iter(samples.iter().copied());
    }

    pub fn write_iter(&mut self, samples: impl ExactSizeIterator<Item = f32>) {
        let total = samples.len();
        // Whole frames only, so a nearly full ring never shifts the channels.
        let room = self.ring.slots().min(total) / self.channels * self.channels;
        let mut lost = total - room;
        if room > 0 {
            match self.ring.write_chunk_uninit(room) {
                Ok(chunk) => {
                    chunk.fill_from_iter(samples);
                }
                Err(_) => lost = total,
            }
        }
        if lost > 0 {
            self.shared.lost.fetch_add(lost as u64, Relaxed);
        }
    }

    pub fn invalidator(&self) -> Invalidator {
        Invalidator(self.shared.clone())
    }
}

/// Tells the reader that the device changed and the source must be reopened.
#[derive(Clone)]
pub struct Invalidator(Arc<Shared>);

impl Invalidator {
    pub fn invalidate(&self) {
        self.0.invalid.store(true, Relaxed);
    }
}

pub struct AudioReader {
    pub format: AudioFormat,
    ring: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
}

impl AudioReader {
    /// Moves up to `max` samples, always whole frames.
    pub fn read(&mut self, max: usize) -> Vec<f32> {
        let channels = self.format.channels as usize;
        let n = self.ring.slots().min(max) / channels * channels;
        match self.ring.read_chunk(n) {
            Ok(chunk) => chunk.into_iter().collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn available(&self) -> usize {
        self.ring.slots()
    }

    pub fn take_lost(&self) -> u64 {
        self.shared.lost.swap(0, Relaxed)
    }

    pub fn is_invalid(&self) -> bool {
        self.shared.invalid.load(Relaxed)
    }
}

pub fn channel(format: AudioFormat, seconds: usize) -> (AudioWriter, AudioReader) {
    let size = format.sample_rate as usize * format.channels as usize * seconds;
    let (producer, consumer) = rtrb::RingBuffer::new(size);
    let shared = Arc::new(Shared::default());
    (
        AudioWriter {
            ring: producer,
            channels: usize::from(format.channels.max(1)),
            shared: shared.clone(),
        },
        AudioReader {
            format,
            ring: consumer,
            shared,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lost_samples_instead_of_hiding_them() {
        let (mut w, mut r) = channel(
            AudioFormat {
                sample_rate: 4,
                channels: 1,
            },
            1,
        );
        w.write(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(r.read(16), vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(r.take_lost(), 2);
        assert_eq!(r.take_lost(), 0);
    }

    #[test]
    fn reads_whole_frames_only() {
        let (mut w, mut r) = channel(
            AudioFormat {
                sample_rate: 8,
                channels: 2,
            },
            1,
        );
        w.write(&[1.0, 2.0, 3.0]);
        assert_eq!(r.read(16), vec![1.0, 2.0]);
    }

    #[test]
    fn a_full_ring_keeps_channels_aligned() {
        let (mut w, mut r) = channel(
            AudioFormat {
                sample_rate: 2,
                channels: 2,
            },
            1,
        );
        w.write(&[1.0, 2.0, 3.0]);
        w.write(&[4.0, 5.0, 6.0, 7.0]);
        assert_eq!(r.read(16), vec![1.0, 2.0, 4.0, 5.0]);
        assert_eq!(r.take_lost(), 3);
    }
}
