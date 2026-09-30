//! Classic FIC carousel for programme audio ensembles.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};

use crate::config::{encode_label, short_label_mask, SubchannelKind, ValidatedConfig};
use dabmux::frame::{cifs_per_transmission_frame, FrameClock};

/// FIG 0/13 user application type for MOT Slideshow (TS 101 756 Table 16).
const USER_APPLICATION_SLIDESHOW: u16 = 0x002;
/// X-PAD application type "MOT, start of X-PAD data group" (EN 300 401 Table 11).
const XPAD_APPTY_MOT_START: u8 = 12;
/// Data service component type MOT (TS 101 756 Table 2).
const DSCTY_MOT: u8 = 60;

pub struct FicCarousel {
    subchannel_cursor: usize,
    service_cursor: usize,
    label_cursor: usize,
    metadata_stage: u8,
    language_cursor: usize,
    pty_cursor: usize,
    application_cursor: usize,
    reconfiguration_counter: Option<u16>,
}

impl FicCarousel {
    pub fn new() -> Self {
        Self {
            subchannel_cursor: 0,
            service_cursor: 0,
            label_cursor: 0,
            metadata_stage: 0,
            language_cursor: 0,
            pty_cursor: 0,
            application_cursor: 0,
            reconfiguration_counter: None,
        }
    }

    /// Signal this FIG 0/7 count instead of the configured one. The runtime
    /// increments it on structural changes.
    pub fn with_reconfiguration_counter(mut self, counter: Option<u16>) -> Self {
        self.reconfiguration_counter = counter;
        self
    }

    pub fn write(&mut self, config: &ValidatedConfig, clock: FrameClock) -> Result<Vec<u8>> {
        let fib_count = if config.source.ensemble.mode == 3 {
            4
        } else {
            3
        };
        let mut fibs = vec![[0u8; 30]; fib_count];
        let mut lengths = vec![0usize; fib_count];

        // FIG 0/0, followed by FIG 0/7, opens FIB 0 of each transmission frame.
        let mode = config.source.ensemble.mode;
        if clock
            .count
            .is_multiple_of(cifs_per_transmission_frame(mode))
        {
            push(
                &mut fibs[0],
                &mut lengths[0],
                &fig0_0(config.source.ensemble.id, clock.count),
            )?;
            let counter = self
                .reconfiguration_counter
                .or(config.source.ensemble.reconfiguration_counter);
            if let Some(counter) = counter {
                push(
                    &mut fibs[0],
                    &mut lengths[0],
                    &fig0_7(counter, config.source.services.len() as u8),
                )?;
            }
        }
        let mut subch_fig = vec![0, 1];
        let start = self.subchannel_cursor;
        while self.subchannel_cursor < config.subchannels.len() {
            let sub = &config.subchannels[self.subchannel_cursor];
            let available = 30 - lengths[0];
            if subch_fig.len() + 4 > available {
                break;
            }
            let sad = sub.start_address_cu;
            let size = sub.size_cu;
            let option = (sub.tpl >> 2) & 1;
            let level = sub.tpl & 3;
            subch_fig.extend_from_slice(&[
                (sub.id << 2) | ((sad >> 8) as u8 & 3),
                sad as u8,
                0x80 | (option << 4) | (level << 2) | ((size >> 8) as u8 & 3),
                size as u8,
            ]);
            self.subchannel_cursor += 1;
        }
        if self.subchannel_cursor == start {
            bail!("FIG 0/1 made no progress");
        }
        if self.subchannel_cursor == config.subchannels.len() {
            self.subchannel_cursor = 0;
        }
        subch_fig[0] = (subch_fig.len() - 1) as u8;
        push(&mut fibs[0], &mut lengths[0], &subch_fig)?;

        // Pack whole service entries until FIB 1 fills; resume next frame.
        let mut fig = vec![0, 2];
        let first_service = self.service_cursor;
        loop {
            let service = &config.source.services[self.service_cursor];
            let components: Vec<_> = config.service_components(self.service_cursor).collect();
            let entry_len = 3 + 2 * components.len();
            if fig.len() + entry_len > 30 {
                break;
            }
            fig.extend_from_slice(&(service.id as u16).to_be_bytes());
            fig.push(components.len() as u8);
            for component in components {
                let sub = &config.subchannels[component.subchannel];
                let ascty = match config.source.subchannels[component.subchannel].kind {
                    SubchannelKind::DabPlus => 0x3f,
                    SubchannelKind::MpegAudio => 0,
                };
                let primary = u8::from(component.scids == 0);
                fig.extend_from_slice(&[ascty, (sub.id << 2) | (primary << 1)]);
            }
            self.service_cursor = (self.service_cursor + 1) % config.source.services.len();
            if self.service_cursor == first_service {
                break;
            }
        }
        if fig.len() == 2 {
            bail!("FIG 0/2 service entry does not fit in a FIB");
        }
        fig[0] = (fig.len() - 1) as u8;
        push(&mut fibs[1], &mut lengths[1], &fig)?;

        // Rotate labels while reserving a periodic slot for UTC time/date.
        if clock.count.is_multiple_of(4) {
            push(&mut fibs[2], &mut lengths[2], &fig0_10(clock))?;
            push(&mut fibs[2], &mut lengths[2], &fig0_9(config, clock))?;
            if let Some(fig) = self.next_metadata(config) {
                push(&mut fibs[2], &mut lengths[2], &fig)?;
            }
        } else {
            let label_fig = if self.label_cursor == 0 {
                fig1_label(
                    0,
                    config.source.ensemble.id,
                    &config.source.ensemble.label,
                    config.source.ensemble.short_label.as_deref(),
                )
            } else {
                let service = &config.source.services[self.label_cursor - 1];
                fig1_label(
                    1,
                    service.id as u16,
                    &service.label,
                    service.short_label.as_deref(),
                )
            };
            push(&mut fibs[2], &mut lengths[2], &label_fig)?;
            self.label_cursor = (self.label_cursor + 1) % (config.source.services.len() + 1);
            push(&mut fibs[2], &mut lengths[2], &fig0_9(config, clock))?;
        }

        let mut output = Vec::with_capacity(fib_count * 32);
        for (mut fib, used) in fibs.into_iter().zip(lengths) {
            if used < 30 {
                fib[used] = 0xff;
            }
            let crc = dabmux::edi::crc16(&fib);
            output.extend_from_slice(&fib);
            output.extend_from_slice(&crc.to_be_bytes());
        }
        Ok(output)
    }

    fn next_metadata(&mut self, config: &ValidatedConfig) -> Option<Vec<u8>> {
        for _ in 0..3 {
            let stage = self.metadata_stage;
            let (entries, cursor, limit, extension): (Vec<Vec<u8>>, &mut usize, usize, u8) =
                match stage {
                    0 => {
                        // One entry per subchannel; validation rejects conflicts.
                        let mut signalled = HashSet::new();
                        let entries = config
                            .components
                            .iter()
                            .filter_map(|component| {
                                let language = config.source.services[component.service].language;
                                (language != 0 && signalled.insert(component.subchannel)).then(
                                    || vec![config.subchannels[component.subchannel].id, language],
                                )
                            })
                            .collect();
                        (entries, &mut self.language_cursor, 7, 5)
                    }
                    1 => {
                        let entries = config
                            .source
                            .services
                            .iter()
                            .filter(|service| service.pty != 0)
                            .map(|service| {
                                let [high, low] = (service.id as u16).to_be_bytes();
                                vec![high, low, 0, service.pty]
                            })
                            .collect();
                        (entries, &mut self.pty_cursor, 3, 17)
                    }
                    _ => {
                        let entries = config
                            .components
                            .iter()
                            .zip(&config.source.components)
                            .filter(|(_, raw)| !raw.user_applications.is_empty())
                            .map(|(component, _)| {
                                let service = &config.source.services[component.service];
                                let [high, low] = (service.id as u16).to_be_bytes();
                                // One application, type in 11 bits, 2 bytes of X-PAD data.
                                let [ua_high, ua_low] =
                                    (USER_APPLICATION_SLIDESHOW << 5 | 2).to_be_bytes();
                                vec![
                                    high,
                                    low,
                                    component.scids << 4 | 1,
                                    ua_high,
                                    ua_low,
                                    XPAD_APPTY_MOT_START,
                                    DSCTY_MOT,
                                ]
                            })
                            .collect();
                        (entries, &mut self.application_cursor, 2, 13)
                    }
                };
            if entries.is_empty() {
                self.metadata_stage = (stage + 1) % 3;
                continue;
            }
            let mut fig = vec![0, extension];
            let end = (*cursor + limit).min(entries.len());
            for entry in &entries[*cursor..end] {
                fig.extend_from_slice(entry);
            }
            *cursor = end;
            if *cursor == entries.len() {
                *cursor = 0;
                self.metadata_stage = (stage + 1) % 3;
            }
            fig[0] = (fig.len() - 1) as u8;
            return Some(fig);
        }
        None
    }
}

fn fig0_0(eid: u16, count: u64) -> [u8; 6] {
    let high = ((count / 250) % 20) as u8;
    let low = (count % 250) as u8;
    let [e0, e1] = eid.to_be_bytes();
    [5, 0, e0, e1, high, low]
}

fn fig0_9(config: &ValidatedConfig, clock: FrameClock) -> [u8; 5] {
    static LOOKUP_FAILED: AtomicBool = AtomicBool::new(false);
    let lto = if config.source.ensemble.local_time_offset_auto {
        match crate::timing::local_offset_half_hours(clock.unix_seconds) {
            Ok(lto) => {
                LOOKUP_FAILED.store(false, Ordering::Relaxed);
                lto
            }
            Err(err) => {
                if !LOOKUP_FAILED.swap(true, Ordering::Relaxed) {
                    tracing::warn!(%err, "local time offset lookup failed; signalling UTC");
                }
                0
            }
        }
    } else {
        config.source.ensemble.local_time_offset_half_hours
    };
    let lto_field = if lto < 0 {
        ((-lto) as u8) | 0x20
    } else {
        lto as u8
    };
    [
        4,
        9,
        lto_field,
        config.source.ensemble.ecc,
        config.source.ensemble.international_table,
    ]
}

fn fig0_7(counter: u16, service_count: u8) -> [u8; 4] {
    [
        3,
        7,
        (service_count << 2) | ((counter >> 8) as u8 & 3),
        counter as u8,
    ]
}

fn fig0_10(clock: FrameClock) -> [u8; 8] {
    let mjd = (clock.unix_seconds.div_euclid(86_400) + 40_587) as u32;
    let sod = clock.unix_seconds.rem_euclid(86_400) as u32;
    let hour = sod / 3600;
    let minute = (sod / 60) % 60;
    let second = sod % 60;
    let millis = u32::from(clock.millisecond);
    [
        7,
        10,
        ((mjd >> 10) & 0x7f) as u8,
        ((mjd >> 2) & 0xff) as u8,
        (((mjd & 3) << 6) | ((hour >> 2) & 7) | 0x18) as u8,
        ((minute & 0x3f) | ((hour & 3) << 6)) as u8,
        (((second & 0x3f) << 2) | ((millis >> 8) & 3)) as u8,
        millis as u8,
    ]
}

fn fig1_label(extension: u8, id: u16, label: &str, short_label: Option<&str>) -> [u8; 22] {
    let mut fig = [0u8; 22];
    fig[0] = (1 << 5) | 21;
    fig[1] = extension;
    fig[2..4].copy_from_slice(&id.to_be_bytes());
    fig[4..20].fill(b' ');
    let encoded = encode_label("label", label).expect("validated label");
    fig[4..4 + encoded.len()].copy_from_slice(&encoded);
    let mask = short_label_mask("short_label", label, short_label).expect("validated short label");
    fig[20..22].copy_from_slice(&mask.to_be_bytes());
    fig
}

fn push(fib: &mut [u8; 30], used: &mut usize, fig: &[u8]) -> Result<()> {
    if *used + fig.len() > fib.len() {
        bail!("FIB payload overflow");
    }
    fib[*used..*used + fig.len()].copy_from_slice(fig);
    *used += fig.len();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn writes_three_crc_protected_fibs() {
        let config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        let valid = config.validate().unwrap();
        let mut carousel = FicCarousel::new();
        let fibs = carousel
            .write(&valid, FrameClock::new(0, 0, 0).unwrap())
            .unwrap();
        assert_eq!(fibs.len(), 96);
        assert_eq!(&fibs[..4], &[5, 0, 0x4f, 0xff]);
        assert_eq!(&fibs[6..12], &[5, 1, 4, 0, 0x88, 0x48]);
        assert_eq!(&fibs[64..72], &[7, 10, 0x27, 0xa2, 0xd8, 0, 0, 0]);
        assert_eq!(&fibs[72..77], &[4, 9, 0, 0xe1, 0]);
        for fib in fibs.as_chunks::<32>().0 {
            assert_eq!(&fib[30..], &dabmux::edi::crc16(&fib[..30]).to_be_bytes());
        }
    }

    #[test]
    fn packs_multiple_services_into_fig_zero_two() {
        let mut config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        config.services.push(crate::config::ServiceConfig {
            uid: "radio_two".into(),
            id: 0x4da5,
            label: "Radio Two".into(),
            short_label: None,
            pty: 0,
            language: 0,
        });
        config.components.push(crate::config::ComponentConfig {
            uid: "component_two".into(),
            service: "radio_two".into(),
            subchannel: "audio_one".into(),
            user_applications: Vec::new(),
        });
        let mut carousel = FicCarousel::new();
        let fibs = carousel
            .write(
                &config.validate().unwrap(),
                FrameClock::new(0, 0, 0).unwrap(),
            )
            .unwrap();
        assert_eq!(
            &fibs[32..45],
            &[11, 2, 0x4d, 0xa4, 1, 0x3f, 6, 0x4d, 0xa5, 1, 0x3f, 6, 0xff]
        );
    }

    #[test]
    fn reconfiguration_counter_follows_ensemble_identity() {
        let mut config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        config.ensemble.reconfiguration_counter = Some(0x123);
        let mut carousel = FicCarousel::new();
        let fibs = carousel
            .write(
                &config.validate().unwrap(),
                FrameClock::new(0, 0, 0).unwrap(),
            )
            .unwrap();
        assert_eq!(&fibs[6..10], &[3, 7, 5, 0x23]);
        assert_eq!(&fibs[10..16], &[5, 1, 4, 0, 0x88, 0x48]);
    }

    #[test]
    fn ensemble_information_opens_each_transmission_frame_only() {
        let mut config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        config.ensemble.reconfiguration_counter = Some(0x123);
        let valid = config.clone().validate().unwrap();
        let mut carousel = FicCarousel::new().with_reconfiguration_counter(Some(0x124));
        let first = carousel
            .write(&valid, FrameClock::new(8, 0, 0).unwrap())
            .unwrap();
        assert_eq!(&first[..10], &[5, 0, 0x4f, 0xff, 0, 8, 3, 7, 5, 0x24]);
        let second = carousel
            .write(&valid, FrameClock::new(9, 0, 0).unwrap())
            .unwrap();
        assert_eq!(&second[..2], &[5, 1], "FIB 0 starts with FIG 0/1");

        // Mode IV has 48 ms transmission frames; modes II and III 24 ms.
        for (mode, count, expected) in [(4, 2, true), (4, 3, false), (2, 1, true), (3, 1, true)] {
            config.ensemble.mode = mode;
            let valid = config.clone().validate().unwrap();
            let fic = FicCarousel::new()
                .write(&valid, FrameClock::new(count, 0, 0).unwrap())
                .unwrap();
            assert_eq!(fic[1] == 0, expected, "mode {mode} frame {count}");
        }
    }

    #[test]
    fn shared_subchannel_language_is_signalled_once() {
        let mut config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        config.services[0].language = 8;
        config.services.push(crate::config::ServiceConfig {
            uid: "radio_two".into(),
            id: 0x4da5,
            label: "Radio Two".into(),
            short_label: None,
            pty: 0,
            language: 8,
        });
        config.components.push(crate::config::ComponentConfig {
            uid: "component_two".into(),
            service: "radio_two".into(),
            subchannel: "audio_one".into(),
            user_applications: Vec::new(),
        });
        let valid = config.validate().unwrap();
        let fibs = FicCarousel::new()
            .write(&valid, FrameClock::new(0, 0, 0).unwrap())
            .unwrap();
        assert_eq!(&fibs[64 + 13..64 + 17], &[3, 5, 1, 8]);
    }

    #[test]
    fn explicit_short_label_mask_is_emitted_in_fig_one() {
        let mut config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        config.ensemble.label = "RND D00 - XX".into();
        config.ensemble.short_label = Some("RND D00".into());
        config.services[0].label = "105 DJ HRND-001".into();
        config.services[0].short_label = Some("HRND-001".into());
        let valid = config.validate().unwrap();
        let mut carousel = FicCarousel::new();
        let first = carousel
            .write(&valid, FrameClock::new(1, 0, 0).unwrap())
            .unwrap();
        assert_eq!(&first[64 + 20..64 + 22], &0xfe00u16.to_be_bytes());
        let second = carousel
            .write(&valid, FrameClock::new(2, 0, 24).unwrap())
            .unwrap();
        assert_eq!(&second[64 + 20..64 + 22], &0x01feu16.to_be_bytes());
    }

    #[test]
    fn production_programme_metadata_rotates_through_time_slots() {
        let mut config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        config.services[0].pty = 15;
        config.services[0].language = 8;
        config.components[0]
            .user_applications
            .push(crate::config::UserApplication::Slideshow);
        let valid = config.validate().unwrap();
        let mut carousel = FicCarousel::new();
        let mut observed = Vec::new();
        for count in 0..9 {
            let fibs = carousel
                .write(
                    &valid,
                    FrameClock::new(count, 0, count as u16 * 24).unwrap(),
                )
                .unwrap();
            if count.is_multiple_of(4) {
                observed.push(fibs[64 + 13..64 + 22].to_vec());
            }
        }
        assert_eq!(&observed[0][..4], &[3, 5, 1, 8]);
        assert_eq!(&observed[1][..6], &[5, 17, 0x4d, 0xa4, 0, 15]);
        assert_eq!(&observed[2], &[8, 13, 0x4d, 0xa4, 1, 0, 0x42, 12, 60]);
    }

    #[test]
    fn twelve_service_metadata_completes_within_one_carousel_cycle() {
        use crate::config::{
            ComponentConfig, InputConfig, ProtectionConfig, ServiceConfig, SubchannelConfig,
            UserApplication,
        };
        let mut config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        config.services.clear();
        config.subchannels.clear();
        config.components.clear();
        for index in 0..12 {
            let uid = format!("service_{index}");
            let sub_uid = format!("sub_{index}");
            config.services.push(ServiceConfig {
                uid: uid.clone(),
                id: 0x4001 + index,
                label: format!("Station {index}"),
                short_label: None,
                pty: 15,
                language: 8,
            });
            config.subchannels.push(SubchannelConfig {
                uid: sub_uid.clone(),
                id: index as u8 + 1,
                bitrate: if index < 4 {
                    72
                } else if index < 8 {
                    64
                } else {
                    48
                },
                kind: SubchannelKind::DabPlus,
                protection: ProtectionConfig::EepA { level: 3 },
                input: InputConfig::Edi {
                    uri: format!("tcp://127.0.0.1:{}", 9001 + index),
                    stream_index: 1,
                    buffer_frames: 40,
                    prebuffer_frames: 4,
                    timing: crate::config::InputTiming::Prebuffering,
                    backpressure: None,
                },
            });
            config.components.push(ComponentConfig {
                uid: format!("component_{index}"),
                service: uid,
                subchannel: sub_uid,
                user_applications: vec![UserApplication::Slideshow],
            });
        }
        let valid = config.validate().unwrap();
        assert_eq!(
            valid.subchannels.iter().map(|sub| sub.size_cu).sum::<u16>(),
            552
        );
        let mut carousel = FicCarousel::new();
        let mut totals = [0usize; 3];
        for count in 0..48 {
            let fibs = carousel
                .write(&valid, FrameClock::new(count, 0, 0).unwrap())
                .unwrap();
            if !count.is_multiple_of(4) {
                continue;
            }
            let fig = &fibs[64 + 13..];
            match fig[1] {
                5 => totals[0] += (usize::from(fig[0]) - 1) / 2,
                17 => totals[1] += (usize::from(fig[0]) - 1) / 4,
                13 => totals[2] += (usize::from(fig[0]) - 1) / 7,
                _ => panic!("unexpected metadata extension"),
            }
        }
        assert_eq!(totals, [12, 12, 12]);
    }
}

/// Round trips through the vendored EDInburgh decoder, an independent reading of
/// EN 300 401, so that bit-layout mistakes in the writers cannot hide behind
/// tests derived from the writers themselves.
#[cfg(test)]
mod oracle_tests {
    use super::*;
    use crate::config::{
        ComponentConfig, Config, InputConfig, InputTiming, ProtectionConfig, ServiceConfig,
        SubchannelConfig, UserApplication,
    };
    use crate::testsupport::edinburgh::fic::{DateTimeUTC, FicDecoder, Fig};
    use crate::testsupport::edinburgh::tables;
    use std::collections::{HashMap, HashSet};

    fn decode(fic: &[u8]) -> Vec<Fig> {
        for fib in fic.chunks(32) {
            assert_eq!(&fib[30..], &dabmux::edi::crc16(&fib[..30]).to_be_bytes());
        }
        FicDecoder::from_bytes(fic).unwrap()
    }

    fn twelve_service_config() -> ValidatedConfig {
        let mut config: Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        config.ensemble.label = "Grüezi Mux".into();
        config.ensemble.short_label = Some("Grüezi".into());
        config.ensemble.international_table = 1;
        config.ensemble.local_time_offset_half_hours = 2;
        config.services.clear();
        config.subchannels.clear();
        config.components.clear();
        for index in 0..12u8 {
            let (label, short_label) = if index == 0 {
                ("Radio Zürich 1".to_string(), Some("Zürich".to_string()))
            } else {
                (format!("Station {index}"), None)
            };
            config.services.push(ServiceConfig {
                uid: format!("service_{index}"),
                id: 0x4001 + u32::from(index),
                label,
                short_label,
                pty: 15,
                language: 8,
            });
            config.subchannels.push(SubchannelConfig {
                uid: format!("sub_{index}"),
                id: index + 1,
                bitrate: [72, 64, 48][usize::from(index / 4)],
                kind: SubchannelKind::DabPlus,
                protection: if index == 4 {
                    ProtectionConfig::EepB { level: 2 }
                } else {
                    ProtectionConfig::EepA { level: 3 }
                },
                input: InputConfig::Edi {
                    uri: format!("tcp://127.0.0.1:{}", 9001 + u16::from(index)),
                    stream_index: 1,
                    buffer_frames: 40,
                    prebuffer_frames: 4,
                    timing: InputTiming::Prebuffering,
                    backpressure: None,
                },
            });
            config.components.push(ComponentConfig {
                uid: format!("component_{index}"),
                service: format!("service_{index}"),
                subchannel: format!("sub_{index}"),
                user_applications: vec![UserApplication::Slideshow],
            });
        }
        config.validate().unwrap()
    }

    #[test]
    fn decoder_sees_complete_and_correct_ensemble_description() {
        let valid = twelve_service_config();
        let source = &valid.source;
        let mut carousel = FicCarousel::new();
        let mut subchannels = HashMap::new();
        let mut components = HashMap::new();
        let mut languages = HashMap::new();
        let mut applications = HashMap::new();
        let mut labels = HashMap::new();
        let mut ensemble_label = None;
        let mut fig0_9_seen = false;
        for count in 0..100 {
            let fic = carousel
                .write(&valid, FrameClock::new(count, 0, 0).unwrap())
                .unwrap();
            let figs = decode(&fic);
            // Mode I: FIG 0/0 once per 96 ms transmission frame, first in FIB 0.
            let fig0_0 = figs
                .iter()
                .position(|fig| matches!(fig, Fig::F0_0(f) if f.eid == source.ensemble.id));
            assert_eq!(
                fig0_0,
                count.is_multiple_of(4).then_some(0),
                "FIG 0/0 placement in frame {count}"
            );
            for fig in figs {
                match fig {
                    Fig::F0_1(f) => {
                        for sub in f.subchannels {
                            subchannels.insert(sub.id, (sub.start, sub.size, sub.pl, sub.bitrate));
                        }
                    }
                    Fig::F0_2(f) => {
                        for c in f.services {
                            components.insert(c.sid, (c.tmid, c.scid, c.primary, c.ca));
                        }
                    }
                    Fig::F0_5(f) => {
                        for l in f.services {
                            languages.insert(l.scid, l.language);
                        }
                    }
                    Fig::F0_9(f) => {
                        assert_eq!((f.ecc, f.int_table_id, f.lto), (source.ensemble.ecc, 1, 1));
                        fig0_9_seen = true;
                    }
                    Fig::F0_13(f) => {
                        for s in f.services {
                            applications.insert(s.sid, (s.scids, s.uas));
                        }
                    }
                    Fig::F1_0(f) => ensemble_label = Some((f.eid, f.label, f.short_label)),
                    Fig::F1_1(f) => {
                        labels.insert(f.sid, (f.label, f.short_label));
                    }
                    _ => {}
                }
            }
        }

        assert!(fig0_9_seen);
        assert_eq!(
            ensemble_label,
            Some((source.ensemble.id, "Grüezi Mux".into(), "Grüezi".into()))
        );
        for (sub, raw) in valid.subchannels.iter().zip(&source.subchannels) {
            let expected_pl = match raw.protection {
                ProtectionConfig::EepA { level } => format!("EEP {level}-A"),
                ProtectionConfig::EepB { level } => format!("EEP {level}-B"),
            };
            assert_eq!(
                subchannels.get(&sub.id),
                Some(&(
                    usize::from(sub.start_address_cu),
                    Some(usize::from(sub.size_cu)),
                    Some(expected_pl),
                    Some(usize::from(sub.bitrate)),
                )),
                "FIG 0/1 for {}",
                sub.uid
            );
        }
        let sids: HashSet<_> = source.services.iter().map(|s| s.id as u16).collect();
        for (service, sub) in source.services.iter().zip(&valid.subchannels) {
            let sid = service.id as u16;
            assert_eq!(components.get(&sid), Some(&(0, sub.id, true, false)));
            assert_eq!(languages.get(&sub.id), Some(&tables::Language::Deu));
            assert_eq!(
                applications.get(&sid),
                Some(&(0, vec![tables::UserApplication::Sls]))
            );
            let short: String = match &service.short_label {
                Some(short) => short.clone(),
                None => service.label.chars().take(8).collect(),
            };
            assert_eq!(labels.get(&sid), Some(&(service.label.clone(), short)));
        }
        assert_eq!(components.len(), sids.len());
    }

    #[test]
    fn decoder_reads_fig0_10_date_and_time() {
        let valid = twelve_service_config();
        let mut carousel = FicCarousel::new();
        // 2024-01-02 03:04:05.678 UTC, on a frame that carries FIG 0/10.
        let fic = carousel
            .write(&valid, FrameClock::new(4, 1_704_164_645, 678).unwrap())
            .unwrap();
        let time = decode(&fic)
            .into_iter()
            .find_map(|fig| match fig {
                Fig::F0_10(f) => Some(f),
                _ => None,
            })
            .expect("FIG 0/10 on frame phase 0");
        assert!(time.utc_flag && !time.lsi);
        match time.utc {
            DateTimeUTC::Long {
                year,
                month,
                day,
                hours,
                minutes,
                seconds,
                milliseconds,
            } => assert_eq!(
                (year, month, day, hours, minutes, seconds, milliseconds),
                (2024, 1, 2, 3, 4, 5, 678)
            ),
            DateTimeUTC::Short { .. } => panic!("expected long-form UTC"),
        }
    }
}
