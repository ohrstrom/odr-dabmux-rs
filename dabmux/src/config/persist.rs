//! Writing the operator configuration back to its YAML file.
//!
//! Identifiers are written in hex, as operators write them (`id: 0x4d01`);
//! SubChIds, packet addresses and other counts stay decimal. Comments of the
//! previous file cannot be kept; it is copied to `<file>.bak` first.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::Value;

use super::Config;

/// Fields holding an identifier, written in hex. `id` is one too, except in
/// the shared `subchannels` map, where it is the SubChId.
const HEX_FIELDS: &[&str] = &[
    "id",
    "ecc",
    "eid",
    "pi",
    "lsn",
    "transfer_sid",
    "transfer_eid",
    "other_ensembles",
    "ensembles",
];

const HEX_TOKEN: &str = "__dabmux_hex__";

pub fn to_yaml(config: &Config) -> anyhow::Result<String> {
    let mut value = serde_json::to_value(config)?;
    drop_nulls(&mut value);
    let mut tokens = Vec::new();
    mark_hex(&mut value, None, false, &mut tokens);
    let mut yaml = serde_norway::to_string(&value)?;
    // Longest first, so that one token is never a prefix of a replaced one.
    tokens.sort_by_key(|(token, _): &(String, String)| std::cmp::Reverse(token.len()));
    for (token, hex) in &tokens {
        for quoted in [format!("'{token}'"), format!("\"{token}\""), token.clone()] {
            yaml = yaml.replace(&quoted, hex);
        }
    }
    Ok(yaml)
}

/// Unset optional fields read back as unset, so they are left out.
fn drop_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, child| !child.is_null());
            map.values_mut().for_each(drop_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(drop_nulls),
        _ => {}
    }
}

/// Replace identifier numbers with unique string tokens, swapped for hex
/// literals once serialized.
fn mark_hex(
    value: &mut Value,
    key: Option<&str>,
    in_subchannels: bool,
    tokens: &mut Vec<(String, String)>,
) {
    let hex_field =
        key.is_some_and(|key| HEX_FIELDS.contains(&key) && !(key == "id" && in_subchannels));
    match value {
        Value::Object(map) => {
            for (child_key, child) in map.iter_mut() {
                let subchannels = in_subchannels || (key.is_none() && child_key == "subchannels");
                mark_hex(child, Some(child_key), subchannels, tokens);
            }
        }
        Value::Array(items) => {
            for item in items {
                // List items inherit their field, e.g. `other_ensembles`.
                let item_key = if matches!(item, Value::Number(_)) {
                    key
                } else {
                    None
                };
                mark_hex(item, item_key, in_subchannels, tokens);
            }
        }
        Value::Number(number) if hex_field => {
            if let Some(n) = number.as_u64() {
                let token = format!("{HEX_TOKEN}{}", tokens.len());
                tokens.push((token.clone(), hex_literal(key.unwrap_or_default(), n)));
                *value = Value::String(token);
            }
        }
        _ => {}
    }
}

fn hex_literal(key: &str, n: u64) -> String {
    let digits = match key {
        "ecc" => 2,
        "lsn" => 3,
        _ if n > 0xff_ffff => 8,
        _ if n > 0xffff => 6,
        _ => 4,
    };
    format!("0x{n:0digits$x}")
}

/// Write `yaml` to `path` atomically, after copying the current file to
/// `<path>.bak`.
pub fn write_file(path: &Path, yaml: &str) -> anyhow::Result<PathBuf> {
    let backup = with_suffix(path, ".bak");
    if path.exists() {
        std::fs::copy(path, &backup)
            .with_context(|| format!("backing up {} to {}", path.display(), backup.display()))?;
    }
    let temporary = with_suffix(path, ".tmp");
    std::fs::write(&temporary, yaml).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(backup)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{parse_yaml, read_file};

    #[test]
    fn written_yaml_reads_back_identically() {
        for path in [
            "config.example.yaml",
            "config.production.example.yaml",
            "config.mux-zh.example.yaml",
            "config.service-linking.example.yaml",
            "tests/fixtures/minimal.yaml",
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
            let config = read_file(&path).unwrap();
            let yaml = to_yaml(&config).unwrap();
            assert!(!yaml.contains(HEX_TOKEN), "{}", path.display());
            assert_eq!(
                parse_yaml(&yaml).unwrap(),
                config,
                "{}\n{yaml}",
                path.display()
            );
        }
    }

    #[test]
    fn identifiers_are_written_in_hex() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("config.service-linking.example.yaml");
        let yaml = to_yaml(&read_file(&path).unwrap()).unwrap();
        for expected in [
            "id: 0x4fff",
            "ecc: 0xec",
            "id: 0x8daa",
            "lsn: 0xabc",
            "pi: 0x1234",
            "- 0x4ffe",
        ] {
            assert!(yaml.contains(expected), "{expected} missing in\n{yaml}");
        }
        assert!(yaml.contains("subchannel_id: 1"), "{yaml}");
    }
}
