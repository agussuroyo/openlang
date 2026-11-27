//! Microphone on every platform cpal supports.

use std::sync::Arc;

use cpal::{
    ErrorKind, FromSample, SampleFormat, SizedSample, Stream,
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
    registry.sources.push(Arc::new(Microphone(Descriptor {
        id: "microphone",
        name: "Microphone",
        summary: "Your voice and the room around you.",
        fields: vec![],
    })));
}

struct Microphone(Descriptor);

struct Running {
    #[cfg(target_os = "macos")]
    _default: default_input::Watch,
    _stream: Stream,
}
impl Capture for Running {}

/// cpal on macOS follows a new default output device but not a new default input: switching the
/// system mic while the old one stays connected would keep capturing the old one. The session
/// reopens on the new one instead.
#[cfg(target_os = "macos")]
mod default_input {
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

fn failed(e: cpal::Error) -> Error {
    match e.kind() {
        ErrorKind::PermissionDenied => Error::Permission("microphone".into()),
        _ => Error::Device(e.to_string()),
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
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
                writer.write_iter(data.iter().map(|s| f32::from_sample_(*s)));
            },
            move |e: cpal::Error| match e.kind() {
                ErrorKind::DeviceChanged | ErrorKind::Xrun | ErrorKind::RealtimeDenied => {}
                _ => invalidator.invalidate(),
            },
            None,
        )
        .map_err(failed)
}

impl AudioSource for Microphone {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn origin(&self) -> Origin {
        Origin::Microphone
    }

    fn open(&self) -> Result<(AudioReader, Box<dyn Capture>)> {
        // Capture always follows the default input.
        let device = cpal::default_host()
            .default_input_device()
            .ok_or_else(|| Error::Device("no microphone found".into()))?;
        let supported = device.default_input_config().map_err(failed)?;
        let format = AudioFormat {
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
        };
        let (writer, reader) = realtime::channel(format, 4);
        #[cfg(target_os = "macos")]
        let default = default_input::Watch::new(writer.invalidator())?;
        let sample_format = supported.sample_format();
        let config = supported.into();
        let stream = match sample_format {
            SampleFormat::F32 => build::<f32>(&device, config, writer),
            SampleFormat::I16 => build::<i16>(&device, config, writer),
            SampleFormat::I32 => build::<i32>(&device, config, writer),
            SampleFormat::U16 => build::<u16>(&device, config, writer),
            other => Err(Error::Device(format!("unsupported sample format {other}"))),
        }?;
        stream.play().map_err(failed)?;
        Ok((
            reader,
            Box::new(Running {
                #[cfg(target_os = "macos")]
                _default: default,
                _stream: stream,
            }),
        ))
    }
}
