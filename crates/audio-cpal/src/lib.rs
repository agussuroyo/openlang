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

struct Running(#[allow(dead_code)] Stream);
impl Capture for Running {}

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
        let device = cpal::default_host()
            .default_input_device()
            .ok_or_else(|| Error::Device("no microphone found".into()))?;
        let supported = device.default_input_config().map_err(failed)?;
        let format = AudioFormat {
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
        };
        let (writer, reader) = realtime::channel(format, 4);
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
        Ok((reader, Box::new(Running(stream))))
    }
}
