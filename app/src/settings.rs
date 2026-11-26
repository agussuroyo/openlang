use std::{collections::BTreeMap, fs, path::PathBuf};

use openlang_core::{
    Error, Result,
    ports::{Descriptor, FieldKind, Options, SecretStore},
    registry::Registry,
    session::{Choice, Plan},
};
use serde::{Deserialize, Serialize};

use crate::placement::{Edge, Place};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub sources: Vec<String>,
    /// `None` asks the recognizer to detect the language.
    pub source_language: Option<String>,
    pub target_language: String,
    /// In order of preference; the rest are fallbacks.
    pub stt: Vec<String>,
    pub translators: Vec<String>,
    pub options: BTreeMap<String, Options>,
    pub allow_network: bool,
    pub subtitle: SubtitleStyle,
    /// The caption ignores the mouse; the controls never do.
    pub click_through: bool,
    pub caption_place: Place,
    pub caption_size: (f64, f64),
    pub island_place: Place,
    pub island_mode: IslandMode,
}

/// How subtitles look. The caption window turns each field into CSS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleStyle {
    pub show: SubtitleShow,
    pub text_size: u32,
    /// `None` uses the system font.
    pub font: Option<String>,
    pub color: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleShow {
    Both,
    Translation,
    Original,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            show: SubtitleShow::Both,
            text_size: 14,
            font: None,
            color: "#ffffff".into(),
        }
    }
}

impl SubtitleStyle {
    /// Keeps hand-edited values from breaking the caption CSS.
    fn fit(self) -> Self {
        let is_hex = |c: &str| {
            c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit())
        };
        let is_font_name = |f: &String| {
            !f.trim().is_empty()
                && f.chars()
                    .all(|x| x.is_alphanumeric() || x == ' ' || x == '-')
        };
        Self {
            text_size: self.text_size.clamp(11, 40),
            font: self.font.filter(is_font_name),
            color: if is_hex(&self.color) {
                self.color
            } else {
                Self::default().color
            },
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandMode {
    Always,
    HideListening,
    /// Only for downloads, permission requests and errors.
    Problems,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sources: vec![],
            source_language: Some("en-US".into()),
            target_language: "id".into(),
            stt: vec![],
            translators: vec![],
            options: BTreeMap::new(),
            allow_network: false,
            subtitle: SubtitleStyle::default(),
            click_through: false,
            caption_place: Place::preset(Edge::Middle, Edge::End),
            caption_size: (480.0, 120.0),
            // A corner, because the middle of the screen is where people look.
            island_place: Place::preset(Edge::End, Edge::Start),
            island_mode: IslandMode::Always,
        }
    }
}

impl Settings {
    /// Fills gaps and drops modules that are no longer installed.
    fn fit(mut self, r: &Registry) -> Self {
        fn keep(chosen: &mut Vec<String>, installed: &[&str]) {
            chosen.retain(|id| installed.contains(&id.as_str()));
            if chosen.is_empty()
                && let Some(first) = installed.first()
            {
                chosen.push((*first).to_owned());
            }
        }
        let ids = |d: &Descriptor| d.id;
        keep(
            &mut self.sources,
            &r.sources
                .iter()
                .map(|x| ids(x.descriptor()))
                .collect::<Vec<_>>(),
        );
        keep(
            &mut self.stt,
            &r.stt
                .iter()
                .map(|x| ids(x.descriptor()))
                .collect::<Vec<_>>(),
        );
        keep(
            &mut self.translators,
            &r.translators
                .iter()
                .map(|x| ids(x.descriptor()))
                .collect::<Vec<_>>(),
        );
        self.subtitle = self.subtitle.fit();
        self
    }

    /// True when a running session has to restart to honour the change.
    pub fn pipeline_differs(&self, other: &Self) -> bool {
        let look = |s: &Self| {
            let d = Self::default();
            Self {
                subtitle: d.subtitle,
                click_through: d.click_through,
                caption_place: d.caption_place,
                caption_size: d.caption_size,
                island_place: d.island_place,
                island_mode: d.island_mode,
                ..s.clone()
            }
        };
        look(self) != look(other)
    }

    pub fn plan(&self, r: &Registry, secrets: &dyn SecretStore) -> Result<Plan> {
        let choose = |id: &String, d: &Descriptor| -> Result<Choice> {
            Ok(Choice {
                id: id.clone(),
                options: self.options_for(d, secrets)?,
            })
        };
        Ok(Plan {
            sources: self.sources.clone(),
            stt: self
                .stt
                .iter()
                .map(|id| choose(id, r.stt(id)?.descriptor()))
                .collect::<Result<_>>()?,
            translators: self
                .translators
                .iter()
                .map(|id| choose(id, r.translator(id)?.descriptor()))
                .collect::<Result<_>>()?,
            source_language: self.source_language.clone(),
            target_language: self.target_language.clone(),
            allow_network: self.allow_network,
            partial_interval: Duration::from_millis(250),
        })
    }

    pub fn options_for(&self, d: &Descriptor, secrets: &dyn SecretStore) -> Result<Options> {
        let mut options = self.options.get(d.id).cloned().unwrap_or_default();
        for f in d.fields.iter().filter(|f| f.kind == FieldKind::Secret) {
            if let Some(v) = secrets.get(&secret_key(d.id, f.key))? {
                options.insert(f.key.to_owned(), v);
            }
        }
        Ok(options)
    }
}

pub fn secret_key(provider: &str, field: &str) -> String {
    format!("{provider}/{field}")
}

pub struct SettingsFile(pub PathBuf);

impl SettingsFile {
    pub fn load(&self, r: &Registry) -> Settings {
        fs::read(&self.0)
            .ok()
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
            .unwrap_or_default()
            .fit(r)
    }

    pub fn save(&self, s: &Settings) -> Result<()> {
        let fail = |e: std::io::Error| Error::Config(e.to_string());
        if let Some(dir) = self.0.parent() {
            fs::create_dir_all(dir).map_err(fail)?;
        }
        let json = serde_json::to_vec_pretty(s).map_err(|e| Error::Config(e.to_string()))?;
        // Write beside, then rename, so a crash never leaves half a file.
        let temporary = self.0.with_extension("tmp");
        fs::write(&temporary, json).map_err(fail)?;
        fs::rename(&temporary, &self.0).map_err(fail)
    }
}

/// The operating system's own password store.
pub struct Keychain(pub &'static str);

impl SecretStore for Keychain {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(self.0, key).map_err(failed)?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(failed(e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(self.0, key).map_err(failed)?;
        if !value.is_empty() {
            return entry.set_password(value).map_err(failed);
        }
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(failed(e)),
        }
    }
}

fn failed(e: keyring::Error) -> Error {
    Error::Config(format!("keychain: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip_and_drop_missing_modules() {
        let path = std::env::temp_dir().join(format!("openlang-{}.json", std::process::id()));
        let file = SettingsFile(path.clone());
        let registry = crate::modules::registry(&std::env::temp_dir());
        let saved = Settings {
            target_language: "ja".into(),
            stt: vec!["gone".into()],
            ..file.load(&registry)
        };
        file.save(&saved).unwrap();
        let loaded = file.load(&registry);
        fs::remove_file(path).unwrap();
        assert_eq!(loaded.target_language, "ja");
        assert!(!loaded.stt.contains(&"gone".into()));
    }

    #[test]
    fn appearance_changes_do_not_restart_the_session() {
        let a = Settings::default();
        assert!(!a.pipeline_differs(&Settings {
            subtitle: SubtitleStyle {
                text_size: 40,
                ..SubtitleStyle::default()
            },
            ..a.clone()
        }));
        assert!(a.pipeline_differs(&Settings {
            target_language: "ja".into(),
            ..a.clone()
        }));
    }

    #[test]
    fn bad_subtitle_values_fall_back() {
        let fitted = SubtitleStyle {
            text_size: 400,
            font: Some("x;}body{".into()),
            color: "red;}".into(),
            ..SubtitleStyle::default()
        }
        .fit();
        assert_eq!(fitted.text_size, 40);
        assert_eq!(fitted.font, None);
        assert_eq!(fitted.color, "#ffffff");

        let kept = SubtitleStyle {
            font: Some("Helvetica Neue".into()),
            color: "#FFCC00".into(),
            ..SubtitleStyle::default()
        }
        .fit();
        assert_eq!(kept.font.as_deref(), Some("Helvetica Neue"));
        assert_eq!(kept.color, "#FFCC00");
    }
}
