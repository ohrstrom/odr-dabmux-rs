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

/// A list of integers, each written as for [`deserialize`].
pub fn list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u64>,
{
    struct Item(u64);
    impl<'de> serde::Deserialize<'de> for Item {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer
                .deserialize_any(IntegerVisitor)?
                .map(Item)
                .ok_or_else(|| de::Error::custom("expected an integer, found null"))
        }
    }
    let items: Vec<Item> = serde::Deserialize::deserialize(deserializer)?;
    items
        .into_iter()
        .map(|Item(value)| {
            T::try_from(value)
                .map_err(|_| de::Error::custom(format!("{value} (0x{value:X}) is out of range")))
        })
        .collect()
}

/// A frequency written in MHz (number or string) and kept in whole kHz, so
/// that `87.6` is exactly 87 600 kHz rather than a float below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frequency {
    pub khz: u32,
}

impl<'de> serde::Deserialize<'de> for Frequency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MhzVisitor;
        impl<'de> Visitor<'de> for MhzVisitor {
            type Value = Frequency;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a frequency in MHz, such as 234.208")
            }

            fn visit_f64<E: de::Error>(self, mhz: f64) -> Result<Frequency, E> {
                let khz = (mhz * 1000.0).round();
                if !(1.0..=u32::MAX as f64).contains(&khz) || (khz - mhz * 1000.0).abs() > 1e-3 {
                    return Err(E::custom(format!(
                        "{mhz} MHz is not a positive whole number of kHz"
                    )));
                }
                Ok(Frequency { khz: khz as u32 })
            }

            fn visit_u64<E: de::Error>(self, mhz: u64) -> Result<Frequency, E> {
                self.visit_f64(mhz as f64)
            }

            fn visit_i64<E: de::Error>(self, mhz: i64) -> Result<Frequency, E> {
                self.visit_f64(mhz as f64)
            }

            fn visit_str<E: de::Error>(self, mhz: &str) -> Result<Frequency, E> {
                let value = mhz
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| E::custom(format!("{mhz:?} is not a frequency in MHz")))?;
                self.visit_f64(value)
            }
        }
        deserializer.deserialize_any(MhzVisitor)
    }
}

impl serde::Serialize for Frequency {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(f64::from(self.khz) / 1000.0)
    }
}
