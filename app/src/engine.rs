//! Owns the running session and tells the windows what it is doing.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use openlang_core::{
    Error, Result,
    auto_language::to_prepare,
    domain::{EventBody, SessionEvent},
    ports::{EventSink, Job, Provider, Readiness, SecretStore},
    registry::{Registry, check_consent},
    session::Session,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{
    settings::{Settings, SettingsFile},
    windows::{self, Floating},
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Idle,
    Starting,
    Preparing { name: String, progress: f32 },
    Listening,
    Stopping,
    Error { error: Error },
}

pub struct Engine {
    pub registry: Registry,
    pub secrets: Box<dyn SecretStore>,
    file: SettingsFile,
    settings: Mutex<Settings>,
    status: Mutex<Status>,
    /// Held while starting or stopping, so the two never overlap.
    session: Mutex<Option<Session>>,
    app: AppHandle,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Windows(AppHandle);

impl EventSink for Windows {
    fn accept(&self, event: &SessionEvent) -> Result<()> {
        if matches!(event.body, EventBody::Translation(_)) {
            return Ok(()); // The caption carries the same text.
        }
        let target = match event.body {
            EventBody::Transcript(_) | EventBody::Caption(_) => Floating::Caption,
            _ => Floating::Island,
        };
        self.0
            .emit_to(target.label(), "session", event)
            .map_err(|e| Error::Provider(format!("window: {e}")))
    }
}

impl Engine {
    pub fn new(
        app: AppHandle,
        registry: Registry,
        file: SettingsFile,
        secrets: Box<dyn SecretStore>,
    ) -> Self {
        let settings = Mutex::new(file.load(&registry));
        Self {
            registry,
            secrets,
            file,
            settings,
            status: Mutex::new(Status::Idle),
            session: Mutex::new(None),
            app,
        }
    }

    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    pub fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    fn set_status(&self, status: Status) {
        *lock(&self.status) = status.clone();
        let _ = self.app.emit("status", status);
    }

    pub fn start(&self) -> Result<()> {
        let mut session = lock(&self.session);
        if session.is_some() {
            return Ok(());
        }
        self.set_status(Status::Starting);
        match self.open() {
            Ok(s) => {
                *session = Some(s);
                self.set_status(Status::Listening);
                Ok(())
            }
            Err(error) => {
                self.set_status(Status::Error {
                    error: error.clone(),
                });
                Err(error)
            }
        }
    }

    fn open(&self) -> Result<Session> {
        let settings = self.settings();
        let plan = settings.plan(&self.registry, self.secrets.as_ref())?;
        let job = |options: &openlang_core::ports::Options| Job {
            source: plan.source_language.clone(),
            target: plan.target_language.clone(),
            options: options.clone(),
        };
        // Only the preferred providers are prepared; fallbacks must already be usable.
        if let Some(c) = plan.stt.first() {
            let p = self.registry.stt(&c.id)?;
            check_consent(p.as_ref(), &c.options, plan.allow_network)?;
            let job = job(&c.options);
            self.prepare(to_prepare(p.as_ref(), self.detector(), &job), &job)?;
        }
        if let Some(c) = plan.translators.first() {
            let p = self.registry.translator(&c.id)?;
            check_consent(p.as_ref(), &c.options, plan.allow_network)?;
            self.prepare(p.as_ref(), &job(&c.options))?;
        }
        Session::start(
            &self.registry,
            &plan,
            vec![Arc::new(Windows(self.app.clone()))],
            &[],
        )
    }

    pub fn detector(&self) -> Option<&dyn Provider> {
        self.registry
            .detector
            .as_deref()
            .map(|d| d as &dyn Provider)
    }

    fn prepare(&self, provider: &dyn Provider, job: &Job) -> Result<()> {
        if !matches!(provider.readiness(job), Readiness::NeedsDownload { .. }) {
            return provider.readiness(job).into_result();
        }
        let name = provider.descriptor().name.to_owned();
        provider.prepare(job, &|progress| {
            self.set_status(Status::Preparing {
                name: name.clone(),
                progress,
            });
        })?;
        self.set_status(Status::Starting);
        provider.readiness(job).into_result()
    }

    pub fn stop(&self) {
        let mut session = lock(&self.session);
        if let Some(s) = session.take() {
            self.set_status(Status::Stopping);
            s.stop();
        }
        self.set_status(Status::Idle);
    }

    pub fn toggle(&self) {
        // Starting can wait on a permission dialog; presses made meanwhile must not pile up.
        let running = match self.session.try_lock() {
            Ok(s) => s.is_some(),
            Err(TryLockError::Poisoned(e)) => e.into_inner().is_some(),
            Err(TryLockError::WouldBlock) => return,
        };
        if running {
            self.stop();
        } else {
            // The failure is already visible as a status.
            let _ = self.start();
        }
    }

    pub fn save(&self, new: Settings) -> Result<()> {
        self.file.save(&new)?;
        let old = std::mem::replace(&mut *lock(&self.settings), new.clone());
        let _ = self.app.emit("settings", &new);
        windows::arrange(&self.app, &new);
        let running = lock(&self.session).is_some();
        if running && old.pipeline_differs(&new) {
            self.stop();
            self.start()?;
        }
        Ok(())
    }
}
