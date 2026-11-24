// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/sc26_usb.h and
// driver/src/steam_controller.h / steam_controller.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Steam Controller (2026) feature reports, hardware attribute updates,
//! settings chunks, and haptic output decoding.

use super::{
    ATTR_BOARD_REVISION, ATTR_BOOTLOADER_BUILD_TIME, ATTR_CAPABILITIES,
    ATTR_CONNECTION_INTERVAL_US, ATTR_FIRMWARE_BUILD_TIME, ATTR_PRODUCT_ID,
    CMD_CLEAR_DIGITAL_MAPPINGS, CMD_GET_ATTRIBUTES_VALUES, CMD_GET_DEVICE_INFO,
    CMD_GET_KEYED_VALUE, CMD_GET_SETTINGS_DEFAULTS, CMD_GET_SETTINGS_MAXS, CMD_GET_SETTINGS_VALUES,
    CMD_GET_STRING_ATTRIBUTE, CMD_LOAD_DEFAULT_SETTINGS, CMD_SET_DEFAULT_DIGITAL_MAPPINGS,
    CMD_SET_SETTINGS_VALUES, CMD_TRIGGER_HAPTIC_PULSE, CMD_WRITE_C1, CMD_WRITE_DC, CMD_WRITE_E2,
    FEATURE_REPORT_SIZE, FEATURES_REPORT_ID, HAPTIC_ACK_REPORT_ID, HAPTIC_PULSE_ID,
    HAPTIC_PULSE_REPORT_SIZE, HAPTIC_REPORT_SIZE, HAPTIC_RUMBLE_ID, HAPTIC_SCRIPT_ID,
    SETTING_COUNT, SETTING_IMU_MODE, SETTING_LIZARD_MODE, STRING_ATTR_BOARD_SERIAL,
    STRING_ATTR_REPLY_LENGTH, STRING_ATTR_UNIT_SERIAL, Sc26State, default_setting,
    default_settings, is_output_report, report_size,
};
use crate::protocol::{
    FeedbackEvent, FeedbackType, STEAM_SETTINGS_TRIPLES_MAX, SteamControllerAttributesRequest,
    SteamHapticFeedback, SteamSettingsFeedback, sc_attr_flags,
};

pub fn printable_attribute_string(text: &[u8; 16]) -> bool {
    let mut n = 0usize;
    while n < 16 && text[n] != 0 {
        if !(0x20..=0x7E).contains(&text[n]) {
            return false;
        }
        n += 1;
    }
    n != 0
}

fn store_attribute_string(out: &mut [u8; 16], text: &[u8; 16]) {
    out.fill(0);
    for n in 0..15 {
        if text[n] == 0 {
            break;
        }
        out[n] = text[n];
    }
}

pub fn apply_attributes(req: &SteamControllerAttributesRequest, state: &mut Sc26State) -> u32 {
    let fs = &mut state.features;
    let mut applied = 0u32;
    if (req.flags & sc_attr_flags::FIRMWARE_BUILD_TIME) != 0 {
        fs.attrs.firmware_build_time = req.firmware_build_time;
        applied |= sc_attr_flags::FIRMWARE_BUILD_TIME;
    }
    if (req.flags & sc_attr_flags::BOOTLOADER_BUILD_TIME) != 0 {
        fs.attrs.bootloader_build_time = req.bootloader_build_time;
        applied |= sc_attr_flags::BOOTLOADER_BUILD_TIME;
    }
    if (req.flags & sc_attr_flags::BOARD_REVISION) != 0 {
        fs.attrs.board_revision = req.board_revision;
        applied |= sc_attr_flags::BOARD_REVISION;
    }
    if (req.flags & sc_attr_flags::CAPABILITIES) != 0 {
        fs.attrs.capabilities = req.capabilities;
        applied |= sc_attr_flags::CAPABILITIES;
    }
    if (req.flags & sc_attr_flags::BUILD_ID) != 0 && printable_attribute_string(&req.build_id) {
        store_attribute_string(&mut fs.build_id, &req.build_id);
        applied |= sc_attr_flags::BUILD_ID;
    }
    if (req.flags & sc_attr_flags::SERIALS) != 0 {
        if printable_attribute_string(&req.unit_serial) {
            store_attribute_string(&mut fs.unit_serial, &req.unit_serial);
            applied |= sc_attr_flags::SERIALS;
        }
        if printable_attribute_string(&req.board_serial) {
            store_attribute_string(&mut fs.board_serial, &req.board_serial);
            applied |= sc_attr_flags::SERIALS;
        }
    }
    applied
}

pub fn apply_output(data: &[u8], controller_id: u32) -> Option<FeedbackEvent> {
    if data.len() < 2 || !is_output_report(data[0]) || data[0] > HAPTIC_SCRIPT_ID {
        return None;
    }
    let declared = report_size(data[0]);
    if data.len() < declared || declared == 0 || declared > 12 {
        return None;
    }
    let mut payload = SteamHapticFeedback {
        length: declared as u8,
        ..Default::default()
    };
    payload.report[..declared].copy_from_slice(&data[..declared]);
    Some(FeedbackEvent::new(
        controller_id,
        FeedbackType::SteamHaptic,
        &payload.to_bytes(),
    ))
}

pub fn apply_settings(data: &[u8], controller_id: u32, events: &mut [FeedbackEvent]) -> usize {
    if data.len() < 3 || data[0] != FEATURES_REPORT_ID {
        return 0;
    }
    let command = data[1];
    let length = usize::from(data[2]);
    if length > data.len() - 3 {
        return 0;
    }
    let payload = &data[3..];
    let triples = if command == CMD_SET_SETTINGS_VALUES {
        let count = length / 3;
        if count == 0 {
            return 0;
        }
        count
    } else if command == CMD_LOAD_DEFAULT_SETTINGS {
        0
    } else {
        return 0;
    };

    let needed = if command == CMD_LOAD_DEFAULT_SETTINGS {
        1
    } else {
        triples.div_ceil(STEAM_SETTINGS_TRIPLES_MAX)
    };
    if needed > events.len() {
        return 0;
    }

    for (n, slot) in events[..needed].iter_mut().enumerate() {
        let first = n * STEAM_SETTINGS_TRIPLES_MAX;
        let count = (triples - first).min(STEAM_SETTINGS_TRIPLES_MAX);
        let mut chunk = SteamSettingsFeedback {
            command,
            count: count as u8,
            ..Default::default()
        };
        if count != 0 {
            chunk.triples[..count * 3].copy_from_slice(&payload[first * 3..first * 3 + count * 3]);
        }
        *slot = FeedbackEvent::new(
            controller_id,
            FeedbackType::SteamSettings,
            &chunk.to_bytes(),
        );
    }
    needed
}

pub fn set_feature(id: u8, data: &[u8], state: &mut Sc26State) -> bool {
    if id != FEATURES_REPORT_ID || data.len() < 3 || data[0] != FEATURES_REPORT_ID {
        return false;
    }
    let kind = data[1];
    let length = usize::from(data[2]);
    if length > data.len() - 3 {
        return false;
    }
    let payload = &data[3..3 + length];
    let fs = &mut state.features;
    fs.reply.fill(0);
    fs.reply[0] = FEATURES_REPORT_ID;
    fs.reply[1] = kind;
    fs.reply[2] = 0;
    fs.have_reply = true;
    let out_cap = FEATURE_REPORT_SIZE - 3;

    match kind {
        CMD_CLEAR_DIGITAL_MAPPINGS | CMD_SET_DEFAULT_DIGITAL_MAPPINGS => true,
        CMD_LOAD_DEFAULT_SETTINGS => {
            fs.settings = default_settings();
            fs.lizard_mode = true;
            fs.imu_mode = 0;
            true
        }
        CMD_SET_SETTINGS_VALUES => {
            for chunk in payload.as_chunks::<3>().0 {
                let id = chunk[0];
                let val = u16::from_le_bytes([chunk[1], chunk[2]]);
                if usize::from(id) < SETTING_COUNT {
                    fs.settings[usize::from(id)] = val;
                }
                if id == SETTING_LIZARD_MODE {
                    fs.lizard_mode = val != 0;
                }
                if id == SETTING_IMU_MODE {
                    fs.imu_mode = val as u8;
                }
            }
            true
        }
        CMD_GET_SETTINGS_VALUES | CMD_GET_SETTINGS_DEFAULTS | CMD_GET_SETTINGS_MAXS => {
            let max_triples = out_cap / 3;
            let mut n = 0usize;
            if length == 0 {
                for id in 0..SETTING_COUNT {
                    if n >= max_triples {
                        break;
                    }
                    let v = match kind {
                        CMD_GET_SETTINGS_VALUES => fs.settings[id],
                        CMD_GET_SETTINGS_MAXS => 0xFFFF,
                        _ => default_setting(id as u8),
                    };
                    fs.reply[3 + n * 3] = id as u8;
                    fs.reply[4 + n * 3..6 + n * 3].copy_from_slice(&v.to_le_bytes());
                    n += 1;
                }
            } else {
                for &id in payload {
                    if n >= max_triples {
                        break;
                    }
                    let v = match kind {
                        CMD_GET_SETTINGS_VALUES if usize::from(id) < SETTING_COUNT => {
                            fs.settings[usize::from(id)]
                        }
                        CMD_GET_SETTINGS_MAXS => 0xFFFF,
                        _ if usize::from(id) < SETTING_COUNT => default_setting(id),
                        _ => 0,
                    };
                    fs.reply[3 + n * 3] = id;
                    fs.reply[4 + n * 3..6 + n * 3].copy_from_slice(&v.to_le_bytes());
                    n += 1;
                }
            }
            fs.reply[2] = (n * 3) as u8;
            true
        }
        CMD_GET_ATTRIBUTES_VALUES => {
            let list = [
                (ATTR_PRODUCT_ID, fs.attrs.product_id),
                (ATTR_CAPABILITIES, fs.attrs.capabilities),
                (ATTR_BOOTLOADER_BUILD_TIME, fs.attrs.bootloader_build_time),
                (ATTR_FIRMWARE_BUILD_TIME, fs.attrs.firmware_build_time),
                (ATTR_BOARD_REVISION, fs.attrs.board_revision),
                (ATTR_CONNECTION_INTERVAL_US, fs.attrs.connection_interval_us),
            ];
            let mut n = 0usize;
            for (tag, val) in list {
                if (n + 1) * 5 > out_cap {
                    break;
                }
                fs.reply[3 + n * 5] = tag;
                fs.reply[4 + n * 5..8 + n * 5].copy_from_slice(&val.to_le_bytes());
                n += 1;
            }
            fs.reply[2] = (n * 5) as u8;
            true
        }
        CMD_GET_STRING_ATTRIBUTE => {
            let tag = payload.first().copied().unwrap_or(STRING_ATTR_UNIT_SERIAL);
            fs.reply[3] = tag;
            let text = if tag == STRING_ATTR_BOARD_SERIAL {
                &fs.board_serial
            } else {
                &fs.unit_serial
            };
            let max_copy = usize::from(STRING_ATTR_REPLY_LENGTH - 1);
            for (n, &b) in text.iter().take(max_copy).enumerate() {
                if b == 0 {
                    break;
                }
                fs.reply[4 + n] = b;
            }
            fs.reply[2] = STRING_ATTR_REPLY_LENGTH;
            true
        }
        CMD_WRITE_C1 | CMD_WRITE_DC | CMD_WRITE_E2 => true,
        CMD_GET_KEYED_VALUE => {
            if payload == b"user/wireless_transport\0" {
                fs.reply[3] = 0;
                fs.reply[2] = 1;
            }
            true
        }
        CMD_GET_DEVICE_INFO => {
            let sub = payload.first().copied().unwrap_or(0);
            fs.reply[3] = sub;
            if sub == 0 {
                fs.reply[4..8].copy_from_slice(&fs.attrs.firmware_build_time.to_le_bytes());
                fs.reply[8..12].copy_from_slice(&fs.attrs.board_revision.to_le_bytes());
                for (i, &b) in fs.build_id.iter().enumerate() {
                    if b == 0 {
                        break;
                    }
                    fs.reply[12 + i] = b;
                }
                for (i, &b) in fs.unit_serial.iter().enumerate() {
                    if b == 0 {
                        break;
                    }
                    fs.reply[28 + i] = b;
                }
                fs.reply[2] = 41;
            } else if sub == 1 {
                fs.reply[2] = 34;
            } else {
                fs.reply[6] = 1;
                fs.reply[2] = 9;
            }
            true
        }
        CMD_TRIGGER_HAPTIC_PULSE => true,
        _ => {
            fs.last_unknown_command = kind;
            fs.unknown_commands += 1;
            true
        }
    }
}

pub fn get_feature(id: u8, buffer: &mut [u8], state: &Sc26State) -> usize {
    if id != FEATURES_REPORT_ID || buffer.len() < FEATURE_REPORT_SIZE {
        return 0;
    }
    buffer.fill(0);
    if state.features.have_reply {
        buffer[..FEATURE_REPORT_SIZE].copy_from_slice(&state.features.reply);
    } else {
        buffer[0] = FEATURES_REPORT_ID;
    }
    FEATURE_REPORT_SIZE
}

pub fn decode_haptic_output(data: &[u8], left: &mut u16, right: &mut u16) -> bool {
    if data.len() < 2 {
        return false;
    }
    if data[0] == HAPTIC_RUMBLE_ID {
        if data.len() < HAPTIC_REPORT_SIZE {
            return false;
        }
        *left = u16::from_le_bytes([data[4], data[5]]);
        *right = u16::from_le_bytes([data[7], data[8]]);
        return true;
    }
    if data[0] == HAPTIC_PULSE_ID {
        if data.len() < HAPTIC_PULSE_REPORT_SIZE {
            return false;
        }
        let on_us = u16::from_le_bytes([data[2], data[3]]);
        let off_us = u16::from_le_bytes([data[4], data[5]]);
        let repeat = u16::from_le_bytes([data[6], data[7]]);
        let mut magnitude = 0u16;
        if repeat != 0 && on_us != 0 {
            let period = (u32::from(on_us) + u32::from(off_us)).max(1);
            magnitude = ((u32::from(on_us) * 65535) / period) as u16;
        }
        if data[1] == 1 {
            *left = magnitude;
        } else {
            *right = magnitude;
        }
        return true;
    }
    false
}

pub fn encode_haptic_ack(data: &[u8], ack: &mut [u8]) -> usize {
    if ack.len() < 6 || data.len() < HAPTIC_PULSE_REPORT_SIZE || data[0] != HAPTIC_PULSE_ID {
        return 0;
    }
    if data[2..8].iter().any(|&b| b != 0) {
        return 0;
    }
    ack[0] = HAPTIC_ACK_REPORT_ID;
    ack[1] = if data[1] == 0 { 0x04 } else { 0x03 };
    ack[2] = 0x02;
    ack[3..6].fill(0);
    6
}
