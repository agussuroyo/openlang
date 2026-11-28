//! Microphone on every platform cpal supports. System audio on Windows (WASAPI loopback of the
//! default output) and Linux (the PulseAudio monitor of the default output, which PipeWire
//! serves too). macOS captures system audio in `audio-coreaudio`.

use std::sync::Arc;

use cpal::{
    BufferSize, Device, ErrorKind, FromSample, I24, SampleFormat, SizedSample, Stream,
    StreamConfig, SupportedStreamConfig, U24,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use openlang_core::{
    Error, Result,
    domain::{AudioFormat, Origin},
    ports::{AudioSource, Capture, Descriptor},
    realtime::{self, AudioReader, AudioWriter},
    registry::Registry,
};

pub fn register(registry: &mut Registry) {
    #[cfg(any(windows, target_os = "linux"))]
    registry.sources.push(Arc::new(Cpal {
        descriptor: Descriptor {
            id: "system",
            name: "System audio",
            summary: "Films, meetings, recordings: anything this computer plays.",
            fields: vec![],
        },
        kind: Kind::System,
    }));
    registry.sources.push(Arc::new(Cpal {
        descriptor: Descriptor {
            id: "microphone",
            name: "Microphone",
            summary: "Your voice and the room around you.",
            fields: vec![],
        },
        kind: Kind::Microphone,
    }));
}

#[derive(Clone, Copy)]
enum Kind {
    Microphone,
    #[cfg(any(windows, target_os = "linux"))]
    System,
}

struct Cpal {
    descriptor: Descriptor,
    kind: Kind,
}

impl Kind {
    /// The default device this capture follows, by id. macOS gets told about changes instead.
    #[cfg(not(target_os = "macos"))]
    fn followed(self, host: &cpal::Host) -> Option<String> {
        let device = match self {
            Kind::Microphone => host.default_input_device(),
            #[cfg(any(windows, target_os = "linux"))]
            Kind::System => host.default_output_device(),
        }?;
        device.id().ok().map(|id| id.id().to_owned())
    }

    fn device(self, host: &cpal::Host) -> Result<(Device, SupportedStreamConfig)> {
        match self {
            Kind::Microphone => {
                let device = host
                    .default_input_device()
                    .ok_or_else(|| Error::Device("no microphone found".into()))?;
                let config = device.default_input_config().map_err(|e| self.failed(e))?;
                Ok((device, config))
            }
            // An input stream on an output device records what it plays.
            #[cfg(windows)]
            Kind::System => {
                let device = host
                    .default_output_device()
                    .ok_or_else(|| Error::Device("no speakers or headphones found".into()))?;
                let config = device.default_output_config().map_err(|e| self.failed(e))?;
                Ok((device, config))
            }
            #[cfg(target_os = "linux")]
            Kind::System => {
                let sink = self
                    .followed(host)
                    .ok_or_else(|| Error::Device("no speakers or headphones found".into()))?;
                let monitor = format!("{sink}.monitor");
                let device = host
                    .input_devices()
                    .map_err(|e| self.failed(e))?
                    .find(|d| d.id().is_ok_and(|id| id.id() == monitor))
                    .ok_or_else(|| {
                        Error::Device("system audio needs PulseAudio or PipeWire".into())
                    })?;
                let config = device.default_input_config().map_err(|e| self.failed(e))?;
                Ok((device, config))
            }
        }
    }

    fn failed(self, e: cpal::Error) -> Error {
        match (denied(&e), self) {
            (true, Kind::Microphone) => Error::Permission("microphone".into()),
            #[cfg(any(windows, target_os = "linux"))]
            (true, Kind::System) => Error::Permission("system audio".into()),
            _ => Error::Device(e.to_string()),
        }
    }

    /// PulseAudio delivers record data in chunks of about 2 s unless asked for less.
    fn buffer(self, rate: u32) -> BufferSize {
        if cfg!(target_os = "linux") {
            BufferSize::Fixed(rate / 50)
        } else {
            BufferSize::Default
        }
    }
}

/// WASAPI never reports `PermissionDenied`: a microphone blocked in the Windows privacy settings
/// fails with `E_ACCESSDENIED`, which cpal keeps only as the `io::Error` text ending its message.
fn denied(e: &cpal::Error) -> bool {
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    e.kind() == ErrorKind::PermissionDenied
        || cfg!(windows)
            && e.to_string()
                .ends_with(&format!("(os error {E_ACCESSDENIED})"))
}

struct Running {
    _watch: follow::Watch,
    _stream: Stream,
    _silence: Option<Stream>,
}
impl Capture for Running {}

/// Reopens the session on a new default device; cpal keeps capturing the old one otherwise.
#[cfg(target_os = "macos")]
mod follow {
    use cidre::{core_audio as ca, os};
    use openlang_core::{Error, Result, realtime::Invalidator};

    extern "C-unwind" fn on_changed(
        _object: ca::Obj,
        _count: u32,
        _addresses: *const ca::PropAddr,
        invalidator: *mut Invalidator,
    ) -> os::Status {
        // SAFETY: the listener is removed before the invalidator is freed.
        if let Some(i) = unsafe { invalidator.as_ref() } {
            i.invalidate();
        }
        Default::default()
    }

    fn address() -> ca::PropAddr {
        ca::PropSelector::HW_DEFAULT_INPUT_DEVICE.global_addr()
    }

    pub struct Watch(*mut Invalidator);

    // SAFETY: the raw pointer is owned by this struct.
    unsafe impl Send for Watch {}

    impl Watch {
        /// macOS registers only the microphone here.
        pub fn new(invalidator: Invalidator) -> Result<Self> {
            let invalidator = Box::into_raw(Box::new(invalidator));
            match ca::System::OBJ.add_prop_listener(&address(), on_changed, invalidator) {
                Ok(()) => Ok(Self(invalidator)),
                Err(e) => {
                    // SAFETY: created above and never handed to Core Audio.
                    drop(unsafe { Box::from_raw(invalidator) });
                    Err(Error::Device(format!("default input listener: {e:?}")))
                }
            }
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            if ca::System::OBJ
                .remove_prop_listener(&address(), on_changed, self.0)
                .is_ok()
            {
                // SAFETY: created by Box::into_raw and no longer reachable by Core Audio.
                drop(unsafe { Box::from_raw(self.0) });
            }
        }
    }
}

/// Reopens the session on a new default device. cpal has no portable change notification, so
/// a small thread compares the default once a second.
#[cfg(not(target_os = "macos"))]
mod follow {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering::Relaxed},
        },
        thread::{self, JoinHandle},
        time::Duration,
    };

    use openlang_core::{Result, realtime::Invalidator};

    use crate::Kind;

    pub struct Watch {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Watch {
        /// `first` is the default when the capture chose its device.
        pub fn new(kind: Kind, first: Option<String>, invalidator: Invalidator) -> Result<Self> {
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let thread = thread::spawn(move || {
                let host = cpal::default_host();
                while !stopped.load(Relaxed) {
                    thread::park_timeout(Duration::from_secs(1));
                    if !stopped.load(Relaxed) && kind.followed(&host) != first {
                        invalidator.invalidate();
                        return;
                    }
                }
            });
            Ok(Self {
                stop,
                thread: Some(thread),
            })
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            self.stop.store(true, Relaxed);
            if let Some(thread) = self.thread.take() {
                thread.thread().unpark();
                let _ = thread.join();
            }
        }
    }
}

/// Windows loopback delivers nothing while no app plays, so the last words would never be
/// followed by the pause that ends them. Playing silence on the same device keeps it delivering.
#[cfg(windows)]
fn keep_delivering(kind: Kind, device: &Device) -> Result<Option<Stream>> {
    let supported = device.default_output_config().map_err(|e| kind.failed(e))?;
    let format = supported.sample_format();
    // Zero bytes are silence only for signed and float samples.
    if !(format.is_float() || format.is_int()) {
        return Ok(None);
    }
    let stream = device
        .build_output_stream_raw(
            supported.config(),
            format,
            |data: &mut cpal::Data, _: &_| data.bytes_mut().fill(0),
            |_| {},
            None,
        )
        .map_err(|e| kind.failed(e))?;
    stream.play().map_err(|e| kind.failed(e))?;
    Ok(Some(stream))
}

fn to_f32<T>(sample: T) -> f32
where
    f32: FromSample<T>,
{
    f32::from_sample_(sample)
}

fn build<T>(
    kind: Kind,
    device: &Device,
    config: StreamConfig,
    mut writer: AudioWriter,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let invalidator = writer.invalidator();
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &_| {
                writer.write_iter(data.iter().map(|s| to_f32(*s)));
            },
            move |e: cpal::Error| match e.kind() {
                ErrorKind::DeviceChanged | ErrorKind::Xrun | ErrorKind::RealtimeDenied => {}
                _ => invalidator.invalidate(),
            },
            None,
        )
        .map_err(|e| kind.failed(e))
}

/// Every PCM format the PulseAudio, ALSA and WASAPI hosts deliver.
fn build_any(
    kind: Kind,
    device: &Device,
    format: SampleFormat,
    config: StreamConfig,
    writer: AudioWriter,
) -> Result<Stream> {
    match format {
        SampleFormat::I8 => build::<i8>(kind, device, config, writer),
        SampleFormat::U8 => build::<u8>(kind, device, config, writer),
        SampleFormat::I16 => build::<i16>(kind, device, config, writer),
        SampleFormat::U16 => build::<u16>(kind, device, config, writer),
        SampleFormat::I24 => build::<I24>(kind, device, config, writer),
        SampleFormat::U24 => build::<U24>(kind, device, config, writer),
        SampleFormat::I32 => build::<i32>(kind, device, config, writer),
        SampleFormat::U32 => build::<u32>(kind, device, config, writer),
        SampleFormat::I64 => build::<i64>(kind, device, config, writer),
        SampleFormat::F32 => build::<f32>(kind, device, config, writer),
        SampleFormat::F64 => build::<f64>(kind, device, config, writer),
        other => Err(Error::Device(format!("unsupported sample format {other}"))),
    }
}

impl AudioSource for Cpal {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn origin(&self) -> Origin {
        match self.kind {
            Kind::Microphone => Origin::Microphone,
            #[cfg(any(windows, target_os = "linux"))]
            Kind::System => Origin::System,
        }
    }

    fn open(&self) -> Result<(AudioReader, Box<dyn Capture>)> {
        // Capture always follows the default device.
        let host = cpal::default_host();
        #[cfg(not(target_os = "macos"))]
        let first = self.kind.followed(&host);
        let (device, supported) = self.kind.device(&host)?;
        let format = AudioFormat {
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
        };
        #[cfg(windows)]
        let silence = match self.kind {
            Kind::System => keep_delivering(self.kind, &device)?,
            Kind::Microphone => None,
        };
        #[cfg(not(windows))]
        let silence = None;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();
        // The stream owns its writer, so a retry needs a new channel.
        let start = |buffer_size| {
            let (writer, reader) = realtime::channel(format, 4);
            let invalidator = writer.invalidator();
            let config = StreamConfig {
                buffer_size,
                ..config
            };
            build_any(self.kind, &device, sample_format, config, writer)
                .map(|stream| (stream, reader, invalidator))
        };
        let buffer = self.kind.buffer(format.sample_rate);
        let (stream, reader, invalidator) = match start(buffer) {
            // ALSA refuses a fixed size outside the device's period range.
            Err(_) if buffer != BufferSize::Default => start(BufferSize::Default),
            started => started,
        }?;
        #[cfg(target_os = "macos")]
        let watch = follow::Watch::new(invalidator)?;
        #[cfg(not(target_os = "macos"))]
        let watch = follow::Watch::new(self.kind, first, invalidator)?;
        stream.play().map_err(|e| self.kind.failed(e))?;
        Ok((
            reader,
            Box::new(Running {
                _watch: watch,
                _stream: stream,
                _silence: silence,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use cpal::Sample;

    use super::*;

    /// Lowest maps to -1, the format's silence to 0, highest to just under or exactly 1.
    fn maps<T: Sample>(min: T, max: T)
    where
        f32: FromSample<T>,
    {
        assert_eq!(to_f32(min), -1.0);
        assert_eq!(to_f32(T::EQUILIBRIUM), 0.0);
        let top = to_f32(max);
        assert!((0.99..=1.0).contains(&top), "max maps to {top}");
    }

    #[test]
    fn every_format_maps_to_unit_range() {
        maps(i8::MIN, i8::MAX);
        maps(u8::MIN, u8::MAX);
        maps(i16::MIN, i16::MAX);
        maps(u16::MIN, u16::MAX);
        let i24 = |v| I24::new(v).unwrap();
        maps(i24(-(1 << 23)), i24((1 << 23) - 1));
        let u24 = |v| U24::new(v).unwrap();
        maps(u24(0), u24((1 << 24) - 1));
        maps(i32::MIN, i32::MAX);
        maps(u32::MIN, u32::MAX);
        maps(i64::MIN, i64::MAX);
        maps(-1.0_f32, 1.0_f32);
        maps(-1.0_f64, 1.0_f64);
    }

    #[test]
    fn permission_denied_is_a_permission_error() {
        let e = cpal::Error::from(ErrorKind::PermissionDenied);
        assert!(matches!(Kind::Microphone.failed(e), Error::Permission(_)));
    }

    #[test]
    fn access_denied_counts_only_on_windows() {
        // What cpal's WASAPI host reports for E_ACCESSDENIED while opening the microphone.
        let e = cpal::Error::with_message(
            ErrorKind::BackendError,
            "Failed to get audio client: Access is denied. (os error -2147024891)",
        );
        assert_eq!(denied(&e), cfg!(windows));
        let other = cpal::Error::with_message(ErrorKind::BackendError, "(os error 5)");
        assert!(!denied(&other));
    }
}
