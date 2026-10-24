//! Parameter descriptions, so the app can build controls for any plugin and
//! plugins can read their parameters with defaults.

use serde::Serialize;
use serde_json::Value;

use crate::{PluginError, Result};

/// One parameter of a plugin operation or mask.
#[derive(Debug, Clone, Serialize)]
pub struct ParamSpec {
    /// Key in the sidecar's `params` object.
    pub key: String,
    /// Label for the UI.
    pub label: String,
    #[serde(flatten)]
    pub kind: ParamKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamKind {
    /// A slider.
    Number {
        min: f64,
        max: f64,
        step: f64,
        default: f64,
    },
    Bool {
        default: bool,
    },
    /// One of a fixed set of strings.
    Choice {
        options: Vec<String>,
        default: String,
    },
    /// A file chosen by the user, e.g. a replacement sky.
    Path,
}

impl ParamSpec {
    pub fn number(key: &str, label: &str, min: f64, max: f64, step: f64, default: f64) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Number {
                min,
                max,
                step,
                default,
            },
        }
    }

    pub fn boolean(key: &str, label: &str, default: bool) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Bool { default },
        }
    }

    pub fn choice(key: &str, label: &str, options: &[&str], default: &str) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Choice {
                options: options.iter().map(|s| s.to_string()).collect(),
                default: default.into(),
            },
        }
    }

    pub fn path(key: &str, label: &str) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Path,
        }
    }

    /// Read this number from `params`, using the default when missing and
    /// clamping to the range.
    pub fn read_number(&self, params: &Value) -> Result<f64> {
        let ParamKind::Number {
            min, max, default, ..
        } = self.kind
        else {
            return Err(self.wrong_kind());
        };
        match params.get(&self.key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => v
                .as_f64()
                .map(|v| v.clamp(min, max))
                .ok_or_else(|| self.invalid("a number")),
        }
    }

    pub fn read_bool(&self, params: &Value) -> Result<bool> {
        let ParamKind::Bool { default } = self.kind else {
            return Err(self.wrong_kind());
        };
        match params.get(&self.key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => v.as_bool().ok_or_else(|| self.invalid("true or false")),
        }
    }

    pub fn read_choice(&self, params: &Value) -> Result<String> {
        let ParamKind::Choice { options, default } = &self.kind else {
            return Err(self.wrong_kind());
        };
        match params.get(&self.key) {
            None | Some(Value::Null) => Ok(default.clone()),
            Some(Value::String(s)) if options.contains(s) => Ok(s.clone()),
            Some(_) => Err(self.invalid(&format!("one of {}", options.join(", ")))),
        }
    }

    /// Read a path; `None` when not set yet.
    pub fn read_path(&self, params: &Value) -> Result<Option<String>> {
        if !matches!(self.kind, ParamKind::Path) {
            return Err(self.wrong_kind());
        }
        match params.get(&self.key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.is_empty() => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(self.invalid("a file path")),
        }
    }

    fn invalid(&self, expected: &str) -> PluginError {
        PluginError::InvalidParams(format!("`{}` should be {expected}", self.key))
    }

    fn wrong_kind(&self) -> PluginError {
        PluginError::Failed(format!("parameter `{}` read as the wrong kind", self.key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_with_defaults_and_clamps() {
        let amount = ParamSpec::number("amount", "Amount", -1.0, 1.0, 0.01, 0.5);
        assert_eq!(amount.read_number(&json!({})).unwrap(), 0.5);
        assert_eq!(amount.read_number(&json!({"amount": 3})).unwrap(), 1.0);
        assert!(amount.read_number(&json!({"amount": "x"})).is_err());

        let class = ParamSpec::choice("class", "Class", &["sky", "subject"], "sky");
        assert_eq!(
            class.read_choice(&json!({"class": "subject"})).unwrap(),
            "subject"
        );
        assert!(class.read_choice(&json!({"class": "dog"})).is_err());

        let file = ParamSpec::path("image", "Image");
        assert_eq!(file.read_path(&json!({"image": ""})).unwrap(), None);
    }

    #[test]
    fn serializes_for_the_ui() {
        let spec = ParamSpec::boolean("invert", "Invert", false);
        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            json!({"key": "invert", "label": "Invert", "kind": "bool", "default": false})
        );
    }
}
