//! Everything the Mac is playing, through a Core Audio process tap. No virtual driver needed.
#![cfg(target_os = "macos")]

use std::{any::Any, sync::Arc};

use cidre::{
    arc, cat, cf,
    core_audio::{self as ca, aggregate_device_keys as agg, sub_device_keys as sub},
    ns, os,
};
use openlang_core::{
    Error, Result,
    domain::{AudioFormat, Origin},
    ports::{AudioSource, Capture, Descriptor},
    realtime::{self, AudioReader, AudioWriter, Invalidator},
    registry::Registry,
};

pub fn register(registry: &mut Registry) {
    registry.sources.push(Arc::new(SystemAudio(Descriptor {
        id: "system",
        name: "System audio",
        summary: "Films, meetings, recordings: anything this Mac plays.",
        fields: vec![],
    })));
}

struct SystemAudio(Descriptor);

fn failed(step: &'static str) -> impl Fn(os::Error) -> Error {
    move |e| Error::Device(format!("{step}: {e:?}"))
}

extern "C" fn on_audio(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input: &cat::AudioBufList<1>,
    _input_time: &cat::AudioTimeStamp,
    _output: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    writer: Option<&mut AudioWriter>,
) -> os::Status {
    let buffer = &input.buffers[0];
    if let Some(writer) = writer
        && !buffer.data.is_null()
    {
        let count = buffer.data_bytes_size as usize / size_of::<f32>();
        // SAFETY: the tap format was checked to be mono f32, and Core Audio owns the buffer for
        // the duration of this call.
        writer.write(unsafe { std::slice::from_raw_parts(buffer.data.cast::<f32>(), count) });
    }
    Default::default()
}

extern "C-unwind" fn on_output_changed(
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

fn output_address() -> ca::PropAddr {
    ca::PropSelector::HW_DEFAULT_OUTPUT_DEVICE.global_addr()
}

/// Fields drop in order: stop the device, remove it, destroy the tap, then free the callback data.
struct Running {
    /// The started aggregate device; cidre does not export its type.
    started: Option<Box<dyn Any>>,
    _tap: ca::TapGuard,
    _writer: Box<AudioWriter>,
    invalidator: *mut Invalidator,
}

// SAFETY: Core Audio objects are plain handles and the raw pointer is owned by this struct.
unsafe impl Send for Running {}
impl Capture for Running {}

impl Drop for Running {
    fn drop(&mut self) {
        self.started = None;
        let removed = ca::System::OBJ.remove_prop_listener(
            &output_address(),
            on_output_changed,
            self.invalidator,
        );
        if removed.is_ok() {
            // SAFETY: created by Box::into_raw and no longer reachable by Core Audio.
            drop(unsafe { Box::from_raw(self.invalidator) });
        }
    }
}

fn aggregate(output: &cf::String, tap: &cf::String) -> Result<ca::AggregateDevice> {
    let device = cf::DictionaryOf::with_keys_values(&[sub::uid()], &[output.as_type_ref()]);
    let tap = cf::DictionaryOf::with_keys_values(&[sub::uid()], &[tap.as_type_ref()]);
    let uid: arc::R<cf::String> = cf::Uuid::new().to_cf_string();
    let description = cf::DictionaryOf::with_keys_values(
        &[
            agg::is_private(),
            agg::is_stacked(),
            agg::tap_auto_start(),
            agg::name(),
            agg::main_sub_device(),
            agg::uid(),
            agg::sub_device_list(),
            agg::tap_list(),
        ],
        &[
            cf::Boolean::value_true().as_type_ref(),
            cf::Boolean::value_false(),
            cf::Boolean::value_true(),
            cf::str!(c"OpenLang"),
            output,
            &uid,
            &cf::ArrayOf::from_slice(&[device.as_ref()]),
            &cf::ArrayOf::from_slice(&[tap.as_ref()]),
        ],
    );
    ca::AggregateDevice::with_desc(&description).map_err(failed("aggregate device"))
}

impl AudioSource for SystemAudio {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn origin(&self) -> Origin {
        Origin::System
    }

    fn open(&self) -> Result<(AudioReader, Box<dyn Capture>)> {
        let output = ca::System::default_output_device().map_err(failed("output device"))?;
        let output_uid = output.uid().map_err(failed("output device"))?;

        // Leaving this app out means spoken translations can never be translated again.
        let own = ca::Process::with_pid(std::process::id() as _).map_err(failed("own process"))?;
        let excluded = ns::Array::from_slice(&[ns::Number::with_u32(own.0.0).as_ref()]);
        let mut description = ca::TapDesc::with_mono_global_tap_excluding_processes(&excluded);
        description.set_private(true);
        let tap = description
            .create_process_tap()
            .map_err(|e| Error::Permission(format!("system audio recording ({e:?})")))?;

        let asbd = tap.asbd().map_err(failed("tap format"))?;
        if asbd.channels_per_frame != 1 || asbd.bits_per_channel != 32 {
            return Err(Error::Device(format!("unexpected tap format {asbd:?}")));
        }
        let format = AudioFormat {
            sample_rate: asbd.sample_rate as u32,
            channels: 1,
        };
        let (writer, reader) = realtime::channel(format, 4);
        let invalidator = writer.invalidator();
        let mut writer = Box::new(writer);

        let tap_uid = tap.uid().map_err(failed("tap"))?;
        let device = aggregate(&output_uid, &tap_uid)?;
        let proc_id = device
            .create_io_proc_id(on_audio, Some(writer.as_mut()))
            .map_err(failed("audio callback"))?;
        let invalidator = Box::into_raw(Box::new(invalidator));
        ca::System::OBJ
            .add_prop_listener(&output_address(), on_output_changed, invalidator)
            .map_err(failed("output listener"))?;
        let mut running = Running {
            started: None,
            _tap: tap,
            _writer: writer,
            invalidator,
        };
        let started = ca::device_start(device, Some(proc_id)).map_err(failed("start capture"))?;
        running.started = Some(Box::new(started));
        Ok((reader, Box::new(running)))
    }
}
