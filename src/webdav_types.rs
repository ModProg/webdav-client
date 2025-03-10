use core::fmt;
use std::collections::HashMap;

use crowd::visit;
use derive_more::{IntoIterator, TryUnwrap};
use intentional::Assert;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, IntoIterator)]
pub struct MultiStatus {
    #[serde(rename = "response")]
    pub responses: Vec<Response>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    pub href: String,
    pub propstat: Vec<PropStat>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PropStat {
    pub status: Status,
    pub prop: HashMap<String, PropValue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Status(pub String);
impl Status {
    #[must_use]
    pub fn is_successful(&self) -> bool {
        self.0.contains(" 2")
    }
}

#[derive(Clone, TryUnwrap)]
#[try_unwrap(ref)]
pub enum PropValue {
    Empty,
    Text(String),
    Xml(HashMap<String, Vec<PropValue>>),
}

impl fmt::Debug for PropValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropValue::Empty => ().fmt(f),
            PropValue::Text(text) => text.fmt(f),
            PropValue::Xml(xml) => xml.fmt(f),
        }
    }
}

impl<'de> Deserialize<'de> for PropValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(visit! {
            PropValue, "anything",
            str(s) {
                Ok(PropValue::Text(s.to_owned()))
            },
            map(mut a) {
                let mut map = HashMap::<_, Vec<_>>::new();
                while let Some((key, value)) = a.next_entry::<String, PropValue>()? {
                    map.entry(key).or_default().push(value);
                }
                Ok(if map.is_empty() {
                    PropValue::Empty
                } else if let Some(mut text) = map.remove("$text"){
                    text.pop().assert("only inserts vecs with at least length 1")
                } else {
                    PropValue::Xml(map)
                })
            }
        })
    }
}
