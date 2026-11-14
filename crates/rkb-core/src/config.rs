//! Minimal, deterministic configuration parser and typed engine settings.
//!
//! The format is intentionally tiny so it works with Android assets without a
//! serialization dependency:
//!
//! ```text
//! # comments are ignored
//! app_name = RKB RUST
//! [graphics]
//! target_fps = 60
//! vsync = true
//! ```
//!
//! Dotted keys such as `graphics.target_fps = 60` are also accepted.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// Error raised while parsing a configuration file or a typed value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub line: usize,
    pub message: String,
}

impl ConfigError {
    #[must_use]
    pub fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for ConfigError {}

/// Parsed key/value configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    values: BTreeMap<String, String>,
}

impl Config {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            values: BTreeMap::new(),
        }
    }

    /// Parses `key = value` pairs and `[section]` headers.
    pub fn parse(input: &str) -> Result<Self, ConfigError> {
        let mut config = Self::new();
        let mut section = String::new();

        for (index, raw_line) in input.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if let Some(header) = line.strip_prefix('[') {
                let Some(name) = header.strip_suffix(']') else {
                    return Err(ConfigError::new(line_number, "unterminated section header"));
                };
                let name = name.trim();
                if name.is_empty() {
                    return Err(ConfigError::new(line_number, "empty section name"));
                }
                if !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
                {
                    return Err(ConfigError::new(line_number, "invalid section name"));
                }
                section = name.to_owned();
                continue;
            }

            let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':')) else {
                return Err(ConfigError::new(
                    line_number,
                    "expected 'key = value' or '[section]'",
                ));
            };
            let key = key.trim();
            if key.is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
            {
                return Err(ConfigError::new(line_number, "invalid key"));
            }

            let full_key = if key.contains('.') || section.is_empty() {
                key.to_owned()
            } else {
                format!("{section}.{key}")
            };

            let value = value.trim();
            let value = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            config.values.insert(full_key, value.to_owned());
        }

        Ok(config)
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    #[must_use]
    pub fn get_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }

    /// Parses a value of any [`FromStr`] type, reporting a useful line-less error.
    pub fn get_parsed<T>(&self, key: &str) -> Result<Option<T>, ConfigError>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        self.get(key)
            .map(|value| {
                value.parse::<T>().map_err(|error| {
                    ConfigError::new(0, format!("invalid value for '{key}': {error}"))
                })
            })
            .transpose()
    }

    pub fn get_bool(&self, key: &str) -> Result<Option<bool>, ConfigError> {
        let Some(value) = self.get(key) else {
            return Ok(None);
        };
        match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(Some(true)),
            "false" | "0" | "no" | "off" => Ok(Some(false)),
            _ => Err(ConfigError::new(
                0,
                format!("invalid boolean for '{key}': {value}"),
            )),
        }
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// Log verbosity used by the host and Android bridge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "warn" | "warning" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            other => Err(format!("unknown log level '{other}'")),
        }
    }
}

/// Validated high-level engine settings.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineConfig {
    pub app_name: String,
    pub target_fps: u32,
    pub fixed_hz: u32,
    pub max_delta_ms: u32,
    pub vsync: bool,
    pub touch_scale: f32,
    pub log_level: LogLevel,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            app_name: "RKB RUST".to_owned(),
            target_fps: 60,
            fixed_hz: 60,
            max_delta_ms: 250,
            vsync: true,
            touch_scale: 1.0,
            log_level: LogLevel::Info,
        }
    }
}

impl EngineConfig {
    /// Loads and validates engine settings, falling back to defaults per key.
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let defaults = Self::default();
        let app_name = config
            .get("app.name")
            .or_else(|| config.get("app_name"))
            .unwrap_or(&defaults.app_name)
            .to_owned();
        if app_name.trim().is_empty() {
            return Err(ConfigError::new(0, "app_name must not be empty"));
        }

        let target_fps = config
            .get_parsed::<u32>("graphics.target_fps")?
            .unwrap_or(defaults.target_fps);
        let fixed_hz = config
            .get_parsed::<u32>("simulation.fixed_hz")?
            .unwrap_or(defaults.fixed_hz);
        let max_delta_ms = config
            .get_parsed::<u32>("simulation.max_delta_ms")?
            .unwrap_or(defaults.max_delta_ms);
        let vsync = config.get_bool("graphics.vsync")?.unwrap_or(defaults.vsync);
        let touch_scale = config
            .get_parsed::<f32>("input.touch_scale")?
            .unwrap_or(defaults.touch_scale);
        let log_level = config
            .get_parsed::<LogLevel>("logging.level")?
            .unwrap_or(defaults.log_level);

        if !(1..=1000).contains(&target_fps) {
            return Err(ConfigError::new(
                0,
                "graphics.target_fps must be in 1..=1000",
            ));
        }
        if !(1..=500).contains(&fixed_hz) {
            return Err(ConfigError::new(
                0,
                "simulation.fixed_hz must be in 1..=500",
            ));
        }
        if !(16..=1000).contains(&max_delta_ms) {
            return Err(ConfigError::new(
                0,
                "simulation.max_delta_ms must be in 16..=1000",
            ));
        }
        if !touch_scale.is_finite() || touch_scale <= 0.0 || touch_scale > 4.0 {
            return Err(ConfigError::new(0, "input.touch_scale must be in (0, 4]"));
        }

        Ok(Self {
            app_name,
            target_fps,
            fixed_hz,
            max_delta_ms,
            vsync,
            touch_scale,
            log_level,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_comments_and_quotes() {
        let config = Config::parse(
            r#"
            # engine
            app_name = "RKB RUST"
            [graphics]
            target_fps = 120
            vsync = no
            [input]
            touch_scale = 1.25
            "#,
        )
        .unwrap();

        assert_eq!(config.len(), 4);
        assert_eq!(config.get("app_name"), Some("RKB RUST"));
        assert_eq!(config.get("graphics.target_fps"), Some("120"));
        assert_eq!(config.get_bool("graphics.vsync"), Ok(Some(false)));
    }

    #[test]
    fn dotted_keys_work_without_sections() {
        let config = Config::parse("graphics.target_fps = 90\n").unwrap();
        assert_eq!(config.get("graphics.target_fps"), Some("90"));
    }

    #[test]
    fn rejects_malformed_lines() {
        let error = Config::parse("just some text").unwrap_err();
        assert_eq!(error.line, 1);
        assert!(error.to_string().contains("line 1"));
        assert!(Config::parse("[broken").is_err());
    }

    #[test]
    fn engine_config_applies_defaults_and_values() {
        let config = Config::parse(
            r#"
            app_name = Demo
            [graphics]
            target_fps = 90
            vsync = false
            [simulation]
            fixed_hz = 30
            [input]
            touch_scale = 2.0
            [logging]
            level = debug
            "#,
        )
        .unwrap();
        let engine = EngineConfig::from_config(&config).unwrap();
        assert_eq!(engine.app_name, "Demo");
        assert_eq!(engine.target_fps, 90);
        assert_eq!(engine.fixed_hz, 30);
        assert!(!engine.vsync);
        assert_eq!(engine.touch_scale, 2.0);
        assert_eq!(engine.log_level, LogLevel::Debug);
        assert_eq!(engine.max_delta_ms, 250);
    }

    #[test]
    fn engine_config_rejects_out_of_range_values() {
        let config = Config::parse("[graphics]\ntarget_fps = 0\n").unwrap();
        assert!(EngineConfig::from_config(&config).is_err());

        let config = Config::parse("[input]\ntouch_scale = 0\n").unwrap();
        assert!(EngineConfig::from_config(&config).is_err());
    }

    #[test]
    fn empty_config_uses_defaults() {
        assert_eq!(
            EngineConfig::from_config(&Config::new()).unwrap(),
            EngineConfig::default()
        );
    }
}
