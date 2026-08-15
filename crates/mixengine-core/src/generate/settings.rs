//! What a user may change about a generated file, and what happens when they misspell it.
//!
//! A user never edits `etc/**` — the file is a projection of the database and the next start
//! overwrites it. What they edit is an **override**, stored in `services.config_overrides_json` and
//! merged over the recipe's defaults on the way into the template. This module is that merge.
//!
//! **Typed, and closed.** A recipe declares every key it understands as a [`Setting`] with a default
//! that fixes its type, and an override naming a key that is not in that list is *refused* rather
//! than ignored. The rule is `config.toml`'s ([`crate::Error::Config`]) one directory down and for
//! the same reason: a silently dropped `max_connectons` is a setting the user believes is in effect,
//! and they find out when the database falls over under load rather than when they typed it.
//!
//! **One key belongs to no recipe**, and every service has it: [`EXTRA`] is the free-form blob
//! `.claude/features/services.md` promises — the directives MixEngine has no opinion about, pasted
//! into the generated file verbatim. It is deliberately not a [`Setting`], because a recipe cannot
//! choose whether to offer it: a config format this build models incompletely is the normal case,
//! and a user who cannot add a line to it is a user editing the generated file by hand.

use std::collections::BTreeMap;

use mixengine_proto::ServiceId;
use serde::Serialize;

use crate::{Error, Result};

/// The override key every recipe carries, whether or not it declares anything else.
///
/// Its value is text and it is rendered by whichever part of the template chooses to — by
/// convention at the end of the file, where a directive overrides an earlier one in most of the
/// formats these templates produce.
pub const EXTRA: &str = "extra";

/// One thing a recipe lets a user change, and what it is when they have not changed it.
///
/// The default is what fixes the type: there is no separate "kind" to keep in step with it, and a
/// recipe that ships `Preset::Number(3306)` has said both that `port` is a number and what it is
/// when nobody said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// What the user writes in `config_overrides_json`, and what the template reads.
    pub key: &'static str,

    /// What it is when they have not overridden it.
    pub default: Preset,
}

/// A [`Setting`]'s default value.
///
/// [`Value`]'s twin, in the shape a `const` can hold: a `&'static [Setting]` is what a recipe
/// declares, and a `String` cannot be built in one. Everything that reads a setting reads the
/// [`Value`] this becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// On or off.
    Flag(bool),
    /// A whole number — a port, a size, a count.
    Number(i64),
    /// A word, a path, an address.
    Text(&'static str),
    /// Several of the above, in the order they were written.
    List(&'static [&'static str]),
}

impl Preset {
    /// This default as the value a template would see.
    #[must_use]
    pub fn value(self) -> Value {
        match self {
            Self::Flag(flag) => Value::Flag(flag),
            Self::Number(number) => Value::Number(number),
            Self::Text(text) => Value::Text(text.to_owned()),
            Self::List(items) => Value::List(items.iter().map(|item| (*item).to_owned()).collect()),
        }
    }
}

/// What a setting is set to.
///
/// `untagged` because this is what reaches a template: `{{ port }}` has to render `3306` and not
/// `{"number": 3306}`. The same shape is what an override is read *from*, so a value that survives
/// [`Settings::merge`] serialises back to the document it came out of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Value {
    /// On or off.
    Flag(bool),
    /// A whole number.
    Number(i64),
    /// Text.
    Text(String),
    /// A list of text.
    List(Vec<String>),
}

impl Value {
    /// What this is, as a noun phrase completing "… has to be".
    ///
    /// For the message a mistyped override produces, which is the only place it is used: naming the
    /// two shapes is the whole of what makes `"port": "3306"` findable.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Flag(_) => "true or false",
            Self::Number(_) => "a whole number",
            Self::Text(_) => "a string",
            Self::List(_) => "a list of strings",
        }
    }

    /// The same, for the JSON value an override arrived as.
    fn kind_of(value: &serde_json::Value) -> &'static str {
        match value {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "true or false",
            serde_json::Value::Number(_) => "a number",
            serde_json::Value::String(_) => "a string",
            serde_json::Value::Array(_) => "a list",
            serde_json::Value::Object(_) => "an object",
        }
    }

    /// Read `offered` as the same kind of value as `self`, or say what the two are.
    fn read_as(&self, offered: &serde_json::Value) -> std::result::Result<Self, &'static str> {
        match (self, offered) {
            (Self::Flag(_), serde_json::Value::Bool(flag)) => Ok(Self::Flag(*flag)),

            // Integers only, and `as_i64` is what enforces it: a JSON `3306.5` is a port that
            // rounds to something the user did not type, and truncating it here is how a
            // configuration file ends up disagreeing with the document it was generated from.
            (Self::Number(_), serde_json::Value::Number(number)) => {
                number.as_i64().map(Self::Number).ok_or("a whole number")
            }
            (Self::Text(_), serde_json::Value::String(text)) => Ok(Self::Text(text.clone())),
            (Self::List(_), serde_json::Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<String>>>()
                .map(Self::List)
                .ok_or("a list of strings"),

            (expected, _) => Err(expected.kind()),
        }
    }
}

/// A service's settings: the recipe's defaults with the user's overrides applied.
///
/// Built by [`merge`](Self::merge) and then only read. The readers are the template — through
/// [`Serialize`], which renders it as a plain object — and the recipe's own [`spec`] method, which
/// needs the same values as Rust to decide a ready check or an argument.
///
/// [`spec`]: super::Recipe::spec
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Settings {
    /// Every declared key, present whether or not it was overridden — a template that reads
    /// `{{ port }}` must not have to know whether anybody set it.
    values: BTreeMap<&'static str, Value>,
}

impl Settings {
    /// Apply the overrides in `document` to `declared`, for `service`.
    ///
    /// `document` is `services.config_overrides_json` as it is stored: an object, `{}` when nothing
    /// has been overridden. [`EXTRA`] is accepted for every service and is not part of `declared`.
    ///
    /// # Errors
    ///
    /// [`Error::UnreadableServiceDocument`] when the column does not hold a JSON object;
    /// [`Error::UnknownSetting`] when it names a key the recipe does not have, listing the ones it
    /// does; and [`Error::SettingType`] when a key it does have was given the wrong shape.
    pub fn merge(
        declared: &'static [Setting],
        document: &str,
        service: &ServiceId,
    ) -> Result<Self> {
        let mut values: BTreeMap<&'static str, Value> = declared
            .iter()
            .map(|setting| (setting.key, setting.default.value()))
            .collect();

        // Not a `Setting`, and present for every service — see the module note.
        values.insert(EXTRA, Value::Text(String::new()));

        let overrides: BTreeMap<String, serde_json::Value> = serde_json::from_str(document)
            .map_err(|source| Error::UnreadableServiceDocument {
                service: service.as_str().to_owned(),
                column: "config_overrides_json",
                source,
            })?;

        for (key, offered) in overrides {
            let Some((declared_key, current)) = values.get_key_value(key.as_str()) else {
                return Err(Error::UnknownSetting {
                    service: service.as_str().to_owned(),
                    key,
                    known: values.keys().map(|known| (*known).to_owned()).collect(),
                });
            };

            let (declared_key, value) = (*declared_key, current.read_as(&offered));

            match value {
                Ok(value) => {
                    values.insert(declared_key, value);
                }
                Err(expected) => {
                    return Err(Error::SettingType {
                        service: service.as_str().to_owned(),
                        key,
                        expected,
                        found: Value::kind_of(&offered),
                    });
                }
            }
        }

        Ok(Self { values })
    }

    /// The free-form directives this service adds to whatever the template renders.
    ///
    /// Empty unless [`EXTRA`] was overridden.
    #[must_use]
    pub fn extra(&self) -> &str {
        self.text(EXTRA)
    }

    /// What `key` is set to, or `false` if the recipe never declared it.
    ///
    /// The four readers below answer for an undeclared key rather than failing, and each answers
    /// with its type's empty value. A recipe reading a key it did not declare is a bug in that
    /// recipe — the same bug in a template renders nothing — and neither is worth a fallible
    /// accessor at every call site: everything a recipe can read, it wrote the declaration for two
    /// screens further up its own file.
    #[must_use]
    pub fn flag(&self, key: &str) -> bool {
        match self.values.get(key) {
            Some(Value::Flag(flag)) => *flag,
            _ => false,
        }
    }

    /// What `key` is set to, or `0`.
    #[must_use]
    pub fn number(&self, key: &str) -> i64 {
        match self.values.get(key) {
            Some(Value::Number(number)) => *number,
            _ => 0,
        }
    }

    /// What `key` is set to, or `""`.
    #[must_use]
    pub fn text(&self, key: &str) -> &str {
        match self.values.get(key) {
            Some(Value::Text(text)) => text,
            _ => "",
        }
    }

    /// What `key` is set to, or nothing.
    #[must_use]
    pub fn list(&self, key: &str) -> &[String] {
        match self.values.get(key) {
            Some(Value::List(items)) => items,
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two settings of different types, which is all a merge can get wrong.
    const DECLARED: &[Setting] = &[
        Setting {
            key: "port",
            default: Preset::Number(3306),
        },
        Setting {
            key: "slow_log",
            default: Preset::Flag(false),
        },
        Setting {
            key: "bind",
            default: Preset::Text("127.0.0.1"),
        },
        Setting {
            key: "modules",
            default: Preset::List(&["rewrite"]),
        },
    ];

    fn service() -> ServiceId {
        ServiceId::parse("mariadb@main").expect("a valid service id")
    }

    #[test]
    fn nothing_overridden_is_every_default() {
        let settings = Settings::merge(DECLARED, "{}", &service()).expect("defaults");

        assert_eq!(settings.number("port"), 3306);
        assert!(!settings.flag("slow_log"));
        assert_eq!(settings.text("bind"), "127.0.0.1");
        assert_eq!(settings.list("modules"), ["rewrite"]);
        assert_eq!(settings.extra(), "");
    }

    #[test]
    fn an_override_replaces_one_default_and_leaves_the_rest() {
        let settings = Settings::merge(DECLARED, r#"{"port": 3307}"#, &service()).expect("a port");

        assert_eq!(settings.number("port"), 3307);
        assert_eq!(settings.text("bind"), "127.0.0.1");
    }

    #[test]
    fn every_service_has_extra_without_declaring_it() {
        let settings = Settings::merge(&[], r#"{"extra": "skip-name-resolve"}"#, &service())
            .expect("the free-form blob");

        assert_eq!(settings.extra(), "skip-name-resolve");
    }

    /// The whole reason the list is closed: a typo is a setting that does nothing, and the user
    /// finds out under load rather than at the moment they wrote it.
    #[test]
    fn a_misspelled_setting_is_refused_and_the_message_names_the_real_ones() {
        let error = Settings::merge(DECLARED, r#"{"prot": 3307}"#, &service())
            .expect_err("a key that is not declared");

        let message = error.to_string();
        assert!(message.contains("prot"), "{message}");
        assert!(message.contains("port"), "{message}");
    }

    #[test]
    fn a_setting_of_the_wrong_shape_names_both_shapes() {
        let error = Settings::merge(DECLARED, r#"{"port": "3307"}"#, &service())
            .expect_err("a number written as a string");

        let message = error.to_string();
        assert!(message.contains("a whole number"), "{message}");
        assert!(message.contains("a string"), "{message}");
    }

    /// A port is not a fraction, and truncating one would leave the file disagreeing with the row
    /// it was generated from.
    #[test]
    fn a_fractional_number_is_not_a_number() {
        Settings::merge(DECLARED, r#"{"port": 3306.5}"#, &service())
            .expect_err("a port that is not whole");
    }

    #[test]
    fn a_column_that_is_not_an_object_says_so_against_the_service() {
        let error =
            Settings::merge(DECLARED, "not json at all", &service()).expect_err("a broken column");

        assert!(error.to_string().contains("mariadb@main"), "{error}");
    }
}
