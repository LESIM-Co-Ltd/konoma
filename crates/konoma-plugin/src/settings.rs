//! The `[plugins.<id>]` values a plugin receives.

use std::collections::BTreeMap;

/// One settings value (the TOML data model).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Table(BTreeMap<String, Value>),
}

/// The settings of one plugin: the table under `[plugins.<id>]`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Settings(BTreeMap<String, Value>);

static EMPTY: Settings = Settings(BTreeMap::new());

impl Settings {
    /// No settings.
    pub fn new() -> Settings {
        Settings::default()
    }

    /// A shared empty value, for contexts that borrow settings.
    pub fn empty() -> &'static Settings {
        &EMPTY
    }

    /// Set a value (builder style).
    pub fn with(mut self, key: impl Into<String>, value: Value) -> Settings {
        self.0.insert(key.into(), value);
        self
    }

    /// Set a value.
    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        self.0.insert(key.into(), value);
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// A boolean, or `default` when missing or of another type.
    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some(Value::Bool(b)) => *b,
            _ => default,
        }
    }

    /// An integer, or `default` when missing or of another type.
    pub fn int(&self, key: &str, default: i64) -> i64 {
        match self.get(key) {
            Some(Value::Int(i)) => *i,
            _ => default,
        }
    }

    /// A float (an integer is accepted), or `default`.
    pub fn float(&self, key: &str, default: f64) -> f64 {
        match self.get(key) {
            Some(Value::Float(f)) => *f,
            Some(Value::Int(i)) => *i as f64,
            _ => default,
        }
    }

    /// A string, or `default` when missing or of another type.
    pub fn str<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        match self.get(key) {
            Some(Value::Str(s)) => s,
            _ => default,
        }
    }

    /// A list of strings. Entries of another type are skipped; missing gives an empty list.
    pub fn list_str(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Some(Value::List(v)) => v
                .iter()
                .filter_map(|x| match x {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.0.iter()
    }
}

impl From<BTreeMap<String, Value>> for Settings {
    fn from(m: BTreeMap<String, Value>) -> Settings {
        Settings(m)
    }
}
