//! Safe wrapper over Apple's on-device Speech and Translation frameworks.
//!
//! Every call blocks and must run off the main thread, while the main run loop is alive.
#![cfg(target_os = "macos")]

use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr::null_mut,
    time::Duration,
};

use openlang_core::{
    Error, Result,
    ports::{SttOutput, SttUpdate},
};

type SpeechCallback = extern "C" fn(*mut c_void, *const c_char, f64, f64, bool, *const c_char);
type ProgressCallback = extern "C" fn(*mut c_void, f64);

unsafe extern "C" {
    fn ol_free(p: *mut c_char);
    fn ol_stt_languages() -> *mut c_char;
    fn ol_stt_status(tag: *const c_char) -> i32;
    fn ol_stt_install(tag: *const c_char, ctx: *mut c_void, cb: ProgressCallback) -> *mut c_char;
    fn ol_stt_open(
        tag: *const c_char,
        ctx: *mut c_void,
        cb: SpeechCallback,
        error: *mut *mut c_char,
    ) -> *mut c_void;
    fn ol_stt_push(session: *mut c_void, samples: *const f32, count: isize) -> *mut c_char;
    fn ol_stt_finish(session: *mut c_void) -> *mut c_char;
    fn ol_tr_languages() -> *mut c_char;
    fn ol_tr_status(source: *const c_char, target: *const c_char) -> i32;
    fn ol_tr_open(source: *const c_char, target: *const c_char) -> *mut c_void;
    fn ol_tr_translate(
        session: *mut c_void,
        text: *const c_char,
        error: *mut *mut c_char,
    ) -> *mut c_char;
    fn ol_tr_close(session: *mut c_void);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    Downloadable,
    Unsupported,
}

impl Status {
    fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Ready,
            1 => Self::Downloadable,
            _ => Self::Unsupported,
        }
    }
}

fn c(text: &str) -> CString {
    // Interior NULs cannot cross the boundary and carry no meaning in speech text.
    CString::new(text.replace('\0', "")).unwrap_or_default()
}

/// Takes ownership of a string allocated by the bridge.
unsafe fn take(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { ol_free(p) };
    Some(s)
}

unsafe fn check(error: *mut c_char) -> Result<()> {
    unsafe { take(error) }.map_or(Ok(()), |e| Err(Error::Provider(e)))
}

fn lines(p: *mut c_char) -> Vec<String> {
    unsafe { take(p) }
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

pub mod speech {
    use super::*;

    pub fn languages() -> Vec<String> {
        lines(unsafe { ol_stt_languages() })
    }

    pub fn status(tag: &str) -> Status {
        Status::from_code(unsafe { ol_stt_status(c(tag).as_ptr()) })
    }

    pub fn install(tag: &str, progress: &dyn Fn(f32)) -> Result<()> {
        extern "C" fn tick(ctx: *mut c_void, value: f64) {
            // SAFETY: `ctx` points to `progress`, which outlives the blocking install call.
            let f = unsafe { &*ctx.cast::<&dyn Fn(f32)>() };
            f(value as f32);
        }
        let mut progress = progress;
        unsafe {
            check(ol_stt_install(
                c(tag).as_ptr(),
                (&raw mut progress).cast(),
                tick,
            ))
        }
    }

    pub struct Session {
        raw: *mut c_void,
        /// Read by the bridge until `ol_stt_finish` returns.
        output: *mut SttOutput,
    }

    // SAFETY: the Swift session is only used through `&mut self` or consumed by `finish`.
    unsafe impl Send for Session {}

    extern "C" fn on_update(
        ctx: *mut c_void,
        text: *const c_char,
        start: f64,
        end: f64,
        is_final: bool,
        error: *const c_char,
    ) {
        // SAFETY: `ctx` is the boxed output, freed only after the session has finished.
        let output = unsafe { &*ctx.cast::<SttOutput>() };
        let read = |p| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        let seconds = |s: f64| Duration::try_from_secs_f64(s).unwrap_or_default();
        if !error.is_null() {
            output(Err(Error::Provider(read(error))));
        } else if !text.is_null() {
            output(Ok(SttUpdate {
                text: read(text),
                language: None,
                start: seconds(start),
                end: seconds(end),
                is_final,
            }));
        }
    }

    impl Session {
        pub fn open(tag: &str, output: SttOutput) -> Result<Self> {
            let output = Box::into_raw(Box::new(output));
            let mut error = null_mut();
            let raw = unsafe { ol_stt_open(c(tag).as_ptr(), output.cast(), on_update, &mut error) };
            if raw.is_null() {
                // `output` is leaked on purpose: a listener that already started may still hold it.
                let message = unsafe { take(error) }.unwrap_or_else(|| "unknown error".into());
                return Err(Error::Provider(message));
            }
            Ok(Self { raw, output })
        }

        /// 16 kHz mono samples.
        pub fn push(&mut self, samples: &[f32]) -> Result<()> {
            if self.raw.is_null() {
                return Ok(());
            }
            unsafe {
                check(ol_stt_push(
                    self.raw,
                    samples.as_ptr(),
                    samples.len() as isize,
                ))
            }
        }

        pub fn finish(&mut self) -> Result<()> {
            let raw = std::mem::replace(&mut self.raw, null_mut());
            if raw.is_null() {
                return Ok(());
            }
            let result = unsafe { check(ol_stt_finish(raw)) };
            // SAFETY: the bridge makes no callback after `ol_stt_finish` returns.
            drop(unsafe { Box::from_raw(self.output) });
            result
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self.finish();
        }
    }
}

pub mod translation {
    use super::*;

    pub fn languages() -> Vec<String> {
        lines(unsafe { ol_tr_languages() })
    }

    pub fn status(source: &str, target: &str) -> Status {
        Status::from_code(unsafe { ol_tr_status(c(source).as_ptr(), c(target).as_ptr()) })
    }

    pub struct Session(*mut c_void);

    // SAFETY: the Swift session is only used through `&mut self`.
    unsafe impl Send for Session {}

    impl Session {
        pub fn open(source: &str, target: &str) -> Self {
            Self(unsafe { ol_tr_open(c(source).as_ptr(), c(target).as_ptr()) })
        }

        pub fn translate(&mut self, text: &str) -> Result<String> {
            let mut error = null_mut();
            let out = unsafe { take(ol_tr_translate(self.0, c(text).as_ptr(), &mut error)) };
            out.ok_or_else(|| {
                let message = unsafe { take(error) }.unwrap_or_else(|| "unknown error".into());
                Error::Provider(message)
            })
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            unsafe { ol_tr_close(self.0) };
        }
    }
}
