//! Force feedback of a uinput pad. A game uploads a rumble effect to the pad's event node
//! (`EVIOCSFF`) and plays it with `EV_FF` events; for a uinput device the kernel hands both to
//! the process that created it, which must answer each upload and erase request. [`Effects`] is
//! the state of those effects (what is stored, what plays now, for how long), [`serve`] is the
//! thread that speaks the protocol on the uinput file and reports the motors' strengths to the
//! client as the Windows pads' rumble: a low-frequency and a high-frequency motor, each 0 to
//! 65535 (Moonlight's rumble packet; `strong_magnitude` is the low-frequency motor).

use super::{PadReport, feedback::Sink};
use std::{
    collections::BTreeMap,
    fs::File,
    mem::size_of,
    os::fd::AsRawFd,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(super) const EV_FF: u16 = 0x15;
/// The event type with which uinput asks its process to answer a request.
const EV_UINPUT: u16 = 0x0101;
const UI_FF_UPLOAD: u16 = 1;
const UI_FF_ERASE: u16 = 2;
pub(super) const FF_RUMBLE: u16 = 0x50;
pub(super) const FF_GAIN: u16 = 0x60;
/// How many effects a game may have uploaded at once.
pub(super) const MAX_EFFECTS: u32 = 16;
/// The rumble packet kind of the control stream (`RUMBLE_DATA`).
const RUMBLE_DATA: u16 = 0x010b;
/// How often the thread looks at running effects that have an end.
const TICK: Duration = Duration::from_millis(10);

/// The kernel's `struct ff_effect` for a rumble effect: the union's first four bytes are the
/// strong and weak magnitudes. The union is 32 bytes (a periodic effect's size), 8-aligned.
#[repr(C)]
#[derive(Clone, Copy)]
struct FfEffect {
    kind: u16,
    id: i16,
    direction: u16,
    trigger_button: u16,
    trigger_interval: u16,
    replay_length: u16,
    replay_delay: u16,
    _pad: u16,
    union: [u64; 4],
}
/// `struct uinput_ff_upload`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Upload {
    request_id: u32,
    retval: i32,
    effect: FfEffect,
    old: FfEffect,
}
/// `struct uinput_ff_erase`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Erase {
    request_id: u32,
    retval: i32,
    effect_id: u32,
}

const fn ioc(dir: u32, nr: u8, size: usize) -> libc::c_ulong {
    ((dir << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr as u32) as libc::c_ulong
}
const UI_BEGIN_FF_UPLOAD: libc::c_ulong = ioc(3, 200, size_of::<Upload>());
const UI_END_FF_UPLOAD: libc::c_ulong = ioc(1, 201, size_of::<Upload>());
const UI_BEGIN_FF_ERASE: libc::c_ulong = ioc(3, 202, size_of::<Erase>());
const UI_END_FF_ERASE: libc::c_ulong = ioc(1, 203, size_of::<Erase>());

/// One stored rumble effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Effect {
    /// The low-frequency (heavy) motor, 0 to 65535.
    pub(super) strong: u16,
    /// The high-frequency (light) motor.
    pub(super) weak: u16,
    /// How long one play lasts in milliseconds; 0 plays until stopped.
    pub(super) length: u16,
    /// How long after the start the effect begins, in milliseconds.
    pub(super) delay: u16,
}

#[derive(Clone, Copy, Debug)]
struct Play {
    started: Instant,
    /// How many times the effect runs, back to back, each with its delay.
    count: u32,
}

/// The effects a pad stores and which of them play.
#[derive(Debug)]
pub(super) struct Effects {
    stored: BTreeMap<i16, Effect>,
    playing: BTreeMap<i16, Play>,
    /// `FF_GAIN`, 0 to 0xffff.
    gain: u32,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            stored: BTreeMap::new(),
            playing: BTreeMap::new(),
            gain: 0xffff,
        }
    }
}

impl Effects {
    /// Stores `effect` as `id`, replacing the effect of that id. A playing effect goes on
    /// with its new magnitudes.
    pub(super) fn upload(&mut self, id: i16, effect: Effect) {
        self.stored.insert(id, effect);
    }
    /// Removes effect `id`, stopping it.
    pub(super) fn erase(&mut self, id: i16) {
        self.stored.remove(&id);
        self.playing.remove(&id);
    }
    /// Plays effect `id` `count` times from `now`; a count of 0 stops it.
    pub(super) fn play(&mut self, id: i16, count: u32, now: Instant) {
        if count == 0 {
            self.playing.remove(&id);
        } else if self.stored.contains_key(&id) {
            self.playing.insert(
                id,
                Play {
                    started: now,
                    count,
                },
            );
        }
    }
    pub(super) fn set_gain(&mut self, gain: u32) {
        self.gain = gain.min(0xffff);
    }
    /// Whether `play` is past its last run at `now`.
    fn finished(effect: Effect, play: Play, now: Instant) -> bool {
        if effect.length == 0 {
            return false;
        }
        let run = u128::from(effect.length) + u128::from(effect.delay);
        now.saturating_duration_since(play.started).as_millis() >= run * u128::from(play.count)
    }
    /// Whether `play` is inside a run's length, after its delay, at `now`.
    fn running(effect: Effect, play: Play, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(play.started).as_millis();
        let delay = u128::from(effect.delay);
        if effect.length == 0 {
            return elapsed >= delay;
        }
        elapsed % (u128::from(effect.length) + delay) >= delay
    }
    /// The `[strong, weak]` motors at `now`: the playing effects added, scaled by the gain.
    /// Effects past their end are dropped.
    pub(super) fn motors(&mut self, now: Instant) -> [u16; 2] {
        let stored = &self.stored;
        self.playing.retain(|id, play| {
            stored
                .get(id)
                .is_some_and(|e| !Self::finished(*e, *play, now))
        });
        let mut sum = [0u32; 2];
        for (id, play) in &self.playing {
            let effect = self.stored[id];
            if Self::running(effect, *play, now) {
                sum[0] += u32::from(effect.strong);
                sum[1] += u32::from(effect.weak);
            }
        }
        sum.map(|m| (u64::from(m.min(0xffff)) * u64::from(self.gain) / 0xffff) as u16)
    }
    /// Whether an effect with an end is playing, so the motors may change without an event.
    pub(super) fn timed(&self) -> bool {
        self.playing.keys().any(|id| {
            let e = self.stored[id];
            e.length != 0 || e.delay != 0
        })
    }
}

/// The rumble packet's payload for `motors`: the low-frequency then the high-frequency motor,
/// 16 bits each.
pub(super) fn rumble_payload(motors: [u16; 2]) -> Vec<u8> {
    [motors[0].to_le_bytes(), motors[1].to_le_bytes()].concat()
}

fn read_effect(effect: &FfEffect) -> Option<Effect> {
    if effect.kind != FF_RUMBLE {
        return None;
    }
    let bytes = effect.union[0].to_ne_bytes();
    Some(Effect {
        strong: u16::from_ne_bytes([bytes[0], bytes[1]]),
        weak: u16::from_ne_bytes([bytes[2], bytes[3]]),
        length: effect.replay_length,
        delay: effect.replay_delay,
    })
}

fn ioctl<T>(fd: i32, request: libc::c_ulong, arg: &mut T) -> std::io::Result<()> {
    // SAFETY: `fd` is an open uinput file and `arg` is the struct `request` reads and writes.
    if unsafe { libc::ioctl(fd, request, std::ptr::from_mut(arg)) } < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Answers one upload or erase request of the kernel.
fn answer(fd: i32, effects: &mut Effects, code: u16, request: u32) -> std::io::Result<()> {
    match code {
        UI_FF_UPLOAD => {
            // SAFETY: `Upload` is plain data for which all zeros is valid.
            let mut upload: Upload = unsafe { std::mem::zeroed() };
            upload.request_id = request;
            ioctl(fd, UI_BEGIN_FF_UPLOAD, &mut upload)?;
            upload.retval = match read_effect(&upload.effect) {
                Some(effect) => {
                    effects.upload(upload.effect.id, effect);
                    0
                }
                None => -libc::EINVAL,
            };
            ioctl(fd, UI_END_FF_UPLOAD, &mut upload)
        }
        UI_FF_ERASE => {
            let mut erase = Erase {
                request_id: request,
                retval: 0,
                effect_id: 0,
            };
            ioctl(fd, UI_BEGIN_FF_ERASE, &mut erase)?;
            effects.erase(erase.effect_id as i16);
            ioctl(fd, UI_END_FF_ERASE, &mut erase)
        }
        _ => Ok(()),
    }
}

/// Handles one `input_event` the kernel queued for the pad's creator.
fn handle(
    fd: i32,
    effects: &mut Effects,
    (kind, code, value): (u16, u16, i32),
    now: Instant,
) -> std::io::Result<()> {
    match (kind, code) {
        (EV_UINPUT, _) => answer(fd, effects, code, value as u32)?,
        (EV_FF, FF_GAIN) => effects.set_gain(value.max(0) as u32),
        (EV_FF, id) if id < FF_GAIN => effects.play(id as i16, value.max(0) as u32, now),
        _ => {}
    }
    Ok(())
}

/// Serves pad `id`'s force feedback on `file` until `stop` is set or the device is gone, telling
/// `sink` of each change of the motors.
pub(super) fn serve(
    file: Arc<File>,
    id: u16,
    sink: Arc<Sink>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    let spawned = std::thread::Builder::new()
        .name(format!("pad {id} force feedback"))
        .spawn(move || {
            let fd = file.as_raw_fd();
            let mut effects = Effects::default();
            let mut motors = [0u16; 2];
            let mut buffer = [0u8; size_of::<libc::input_event>() * 16];
            while !stop.load(Ordering::Acquire) {
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let wait = if effects.timed() {
                    TICK
                } else {
                    Duration::from_millis(100)
                };
                // SAFETY: one valid pollfd.
                let ready = unsafe { libc::poll(&raw mut poll, 1, wait.as_millis() as i32) };
                if ready > 0 {
                    if poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                        break;
                    }
                    // SAFETY: reads into `buffer`, whose length is passed.
                    let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
                    if n < 0 {
                        let error = std::io::Error::last_os_error();
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                        ) {
                            continue;
                        }
                        break;
                    }
                    let size = size_of::<libc::input_event>();
                    for chunk in buffer[..n as usize].chunks_exact(size) {
                        // SAFETY: `chunk` holds one `input_event` of plain data.
                        let event: libc::input_event =
                            unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
                        if let Err(error) = handle(
                            fd,
                            &mut effects,
                            (event.type_, event.code, event.value),
                            Instant::now(),
                        ) {
                            tracing::warn!(%error, id, "force feedback request failed");
                        }
                    }
                }
                let now = effects.motors(Instant::now());
                if now != motors {
                    motors = now;
                    sink.push(PadReport::Feedback {
                        id,
                        kind: RUMBLE_DATA,
                        data: rumble_payload(motors),
                    });
                }
            }
        });
    match spawned {
        Ok(handle) => handle,
        Err(error) => {
            tracing::warn!(%error, "the force feedback thread did not start");
            std::thread::spawn(|| {})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rumble(strong: u16, weak: u16, length: u16, delay: u16) -> Effect {
        Effect {
            strong,
            weak,
            length,
            delay,
        }
    }

    #[test]
    fn kernel_structs_have_the_kernels_size() {
        assert_eq!(size_of::<FfEffect>(), 48);
        assert_eq!(size_of::<Upload>(), 104);
        assert_eq!(size_of::<Erase>(), 12);
        assert_eq!(UI_BEGIN_FF_UPLOAD, 0xc068_55c8);
        assert_eq!(UI_END_FF_UPLOAD, 0x4068_55c9);
        assert_eq!(UI_BEGIN_FF_ERASE, 0xc00c_55ca);
        assert_eq!(UI_END_FF_ERASE, 0x400c_55cb);
    }

    #[test]
    fn a_rumble_effect_is_read_from_the_kernels_layout() {
        // SAFETY: `FfEffect` is plain data for which all zeros is valid.
        let mut effect: FfEffect = unsafe { std::mem::zeroed() };
        effect.kind = FF_RUMBLE;
        effect.id = 3;
        effect.replay_length = 500;
        effect.replay_delay = 20;
        let mut union = [0u8; 8];
        union[..2].copy_from_slice(&0x1234u16.to_ne_bytes());
        union[2..4].copy_from_slice(&0xabcdu16.to_ne_bytes());
        effect.union[0] = u64::from_ne_bytes(union);
        assert_eq!(read_effect(&effect), Some(rumble(0x1234, 0xabcd, 500, 20)));
        effect.kind = 0x51;
        assert_eq!(read_effect(&effect), None);
    }

    #[test]
    fn playing_stopping_and_erasing() {
        let t = Instant::now();
        let mut effects = Effects::default();
        effects.upload(0, rumble(1000, 2000, 0, 0));
        assert_eq!(effects.motors(t), [0, 0]);
        effects.play(0, 1, t);
        assert_eq!(effects.motors(t), [1000, 2000]);
        // Length 0 plays until stopped.
        assert_eq!(effects.motors(t + Duration::from_secs(60)), [1000, 2000]);
        effects.play(0, 0, t);
        assert_eq!(effects.motors(t), [0, 0]);
        effects.play(0, 1, t);
        effects.erase(0);
        assert_eq!(effects.motors(t), [0, 0]);
        // An effect that was never uploaded does not play.
        effects.play(7, 1, t);
        assert_eq!(effects.motors(t), [0, 0]);
    }

    #[test]
    fn length_delay_and_count() {
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        let mut effects = Effects::default();
        effects.upload(1, rumble(500, 0, 100, 50));
        effects.play(1, 2, t);
        assert_eq!(effects.motors(at(10)), [0, 0], "delayed");
        assert_eq!(effects.motors(at(60)), [500, 0]);
        assert_eq!(effects.motors(at(160)), [0, 0], "the second run's delay");
        assert_eq!(effects.motors(at(210)), [500, 0], "the second run");
        assert_eq!(effects.motors(at(300)), [0, 0], "finished");
        assert!(!effects.timed(), "a finished effect is dropped");
    }

    #[test]
    fn effects_add_and_the_gain_scales() {
        let t = Instant::now();
        let mut effects = Effects::default();
        effects.upload(0, rumble(40000, 100, 0, 0));
        effects.upload(1, rumble(40000, 200, 0, 0));
        effects.play(0, 1, t);
        effects.play(1, 1, t);
        assert_eq!(effects.motors(t), [65535, 300]);
        effects.set_gain(0x8000);
        assert_eq!(effects.motors(t), [32768, 150]);
        effects.set_gain(0);
        assert_eq!(effects.motors(t), [0, 0]);
    }

    #[test]
    fn the_payload_is_low_then_high_frequency() {
        assert_eq!(rumble_payload([0x0102, 0x0304]), vec![2, 1, 4, 3]);
    }
}
