//! Initial classic FIC carousel for programme audio ensembles.

use anyhow::{bail, Result};

use crate::config::{short_label_mask, SubchannelKind, ValidatedConfig};
use dabmux::frame::FrameClock;

pub struct FicCarousel {
    subchannel_cursor: usize,
    service_cursor: usize,
    label_cursor: usize,
}

impl FicCarousel {
    pub fn new() -> Self {
        Self {
            subchannel_cursor: 0,
            service_cursor: 0,
            label_cursor: 0,
        }
    }

    pub fn write(&mut self, config: &ValidatedConfig, clock: FrameClock) -> Result<Vec<u8>> {
        let fib_count = if config.source.ensemble.mode == 3 {
            4
        } else {
            3
        };
        let mut fibs = vec![[0u8; 30]; fib_count];
        let mut lengths = vec![0usize; fib_count];

        // Ensemble identity always starts FIB 0.
        push(
            &mut fibs[0],
            &mut lengths[0],
            &fig0_0(config.source.ensemble.id, clock.count),
        )?;
        if let Some(counter) = config.source.ensemble.reconfiguration_counter {
            push(
                &mut fibs[0],
                &mut lengths[0],
                &fig0_7(counter, config.source.services.len() as u8),
            )?;
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
            let components: Vec<_> = config
                .source
                .components
                .iter()
                .filter(|c| c.service == service.uid)
                .collect();
            let entry_len = 3 + 2 * components.len();
            if fig.len() + entry_len > 30 {
                break;
            }
            fig.extend_from_slice(&(service.id as u16).to_be_bytes());
            fig.push(components.len() as u8);
            for (ix, component) in components.iter().enumerate() {
                let (sub, raw) = config
                    .subchannels
                    .iter()
                    .zip(&config.source.subchannels)
                    .find(|(_, raw)| raw.uid == component.subchannel)
                    .ok_or_else(|| anyhow::anyhow!("unresolved component subchannel"))?;
                let ascty = match raw.kind {
                    SubchannelKind::DabPlus => 0x3f,
                    SubchannelKind::MpegAudio => 0,
                };
                fig.extend_from_slice(&[ascty, (sub.id << 2) | (u8::from(ix == 0) << 1)]);
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
        }
        push(&mut fibs[2], &mut lengths[2], &fig0_9(config))?;

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
}

fn fig0_0(eid: u16, count: u64) -> [u8; 6] {
    let high = ((count / 250) % 20) as u8;
    let low = (count % 250) as u8;
    let [e0, e1] = eid.to_be_bytes();
    [5, 0, e0, e1, high, low]
}

fn fig0_9(config: &ValidatedConfig) -> [u8; 5] {
    let lto = config.source.ensemble.local_time_offset_half_hours;
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
        ((second & 0x3f) | (((millis >> 8) & 3) << 6)) as u8,
        millis as u8,
    ]
}

fn fig1_label(extension: u8, id: u16, label: &str, short_label: Option<&str>) -> [u8; 22] {
    let mut fig = [0u8; 22];
    fig[0] = (1 << 5) | 21;
    fig[1] = extension;
    fig[2..4].copy_from_slice(&id.to_be_bytes());
    fig[4..20].fill(b' ');
    fig[4..4 + label.len()].copy_from_slice(label.as_bytes());
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
        });
        config.components.push(crate::config::ComponentConfig {
            uid: "component_two".into(),
            service: "radio_two".into(),
            subchannel: "audio_one".into(),
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
}
