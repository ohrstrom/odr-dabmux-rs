//! Integer fields that accept either a number or a string such as `"0x4F32"`.
//! JSON has no hexadecimal literals, and YAML 1.2 only reads lowercase `0x`
//! as a number, so IDs written as `0X4F32` arrive as strings.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};

pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u64>,
{
    option(deserializer)?.ok_or_else(|| de::Error::custom("expected an integer, found null"))
}

pub fn option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u64>,
{
    let Some(value) = deserializer.deserialize_any(IntegerVisitor)? else {
        return Ok(None);
    };
    T::try_from(value)
        .map(Some)
        .map_err(|_| de::Error::custom(format!("{value} (0x{value:X}) is out of range")))
}

struct IntegerVisitor;

impl<'de> Visitor<'de> for IntegerVisitor {
    type Value = Option<u64>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a non-negative integer or a string such as \"0x4F32\"")
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Some(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        u64::try_from(value)
            .map(Some)
            .map_err(|_| E::custom(format!("{value} must not be negative")))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        let text = value.trim();
        let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            Some(hex) => u64::from_str_radix(hex, 16),
            None => text.parse(),
        };
        parsed
            .map(Some)
            .map_err(|_| E::custom(format!("{value:?} is not a decimal or 0x-prefixed integer")))
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}
