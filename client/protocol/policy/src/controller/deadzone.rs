//! Stick and trigger deadzone math, axis normalization, magnitude fusion, and
//! mouse-mode stick/fixed-step curves matching Android's `ControllerHandler`.

use crate::gamepad_constants::button;

/// Fixed 1% center zone kept for 0% and negative anti-deadzone settings so resting sticks read
/// neutral and packet batching keeps working.
pub const MIN_STICK_DEADZONE: f64 = 0.01;

/// Minimum trigger deadzone applied to pads whose triggers false-press at rest (ADT-1, ASUS Gamepad).
pub const QUIRK_TRIGGER_DEADZONE: f32 = 0.30;

/// Signed 16-bit stick scale factor (`0x7FFE = 32766`) matching Moonlight's `Vector2d` scaling.
pub const STICK_SCALE: f32 = 0x7FFE as f32;

/// Minimum and maximum step sizes for fixed-step controller mouse mode.
pub const FIXED_STEP_MIN: i32 = 1;
pub const FIXED_STEP_MAX: i32 = 128;

/// Applies Android's stick deadzone/anti-deadzone policy to `(x, y)` and returns the resulting
/// `(f32, f32)` components matching `com.limelight.utils.Vector2d`.
///
/// Positive `radius` drops inputs with magnitude `<= radius` and passes outer values raw.
/// Zero or negative `radius` drops inputs with magnitude `<= 0.01` and remaps `[0.01, 1]` to
/// `[|radius|, 1]` while preserving direction and full deflection.
pub fn apply_stick_deadzone(x: f32, y: f32, radius: f32) -> (f32, f32) {
    let magnitude = (f64::from(x).powi(2) + f64::from(y).powi(2)).sqrt();
    if radius > 0.0 {
        if magnitude <= f64::from(radius) {
            return (0.0, 0.0);
        }
        return (x, y);
    }
    if magnitude <= MIN_STICK_DEADZONE {
        return (0.0, 0.0);
    }
    let anti = -f64::from(radius);
    let remapped = anti + magnitude * (1.0 - anti);
    if remapped >= 1.0 || magnitude >= 1.0 {
        return (x, y);
    }
    let factor = remapped / magnitude;
    (
        (f64::from(x) * factor) as f32,
        (f64::from(y) * factor) as f32,
    )
}

/// Scales normalized stick coordinates into wire `i16` values (negating Y), clamping
/// out-of-range axes instead of wrapping modulo 65536.
pub fn scale_stick(x: f32, y: f32) -> (i16, i16) {
    let clamp_axis = |v: f32| {
        ((v.clamp(-1.0, 1.0) * STICK_SCALE) as i32)
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
    };
    (clamp_axis(x), clamp_axis(-y))
}

/// Drops trigger travel at or below `deadzone` and passes higher values raw.
pub fn apply_trigger_deadzone(value: f32, deadzone: f32) -> f32 {
    if value <= deadzone { 0.0 } else { value }
}

/// Latches first non-zero trigger activity, rescales `[-1, 1] -> [0, 1]` when `idle_negative`,
/// applies trigger deadzone, and quantizes to `[0, 255]`.
pub fn normalize_triggers(
    lt: f32,
    rt: f32,
    deadzone: f32,
    idle_negative: bool,
    lt_used: &mut bool,
    rt_used: &mut bool,
) -> (u8, u8) {
    if lt != 0.0 {
        *lt_used = true;
    }
    if rt != 0.0 {
        *rt_used = true;
    }
    let mut lt = lt;
    let mut rt = rt;
    if idle_negative {
        if *lt_used {
            lt = (lt + 1.0) / 2.0;
        }
        if *rt_used {
            rt = (rt + 1.0) / 2.0;
        }
    }
    let lt = apply_trigger_deadzone(lt, deadzone);
    let rt = apply_trigger_deadzone(rt, deadzone);
    let quantize = |v: f32| ((v.clamp(0.0, 1.0) * 255.0) as i32).clamp(0, 255) as u8;
    (quantize(lt), quantize(rt))
}

/// Normalizes USB controller stick and trigger axes into `(lx, ly, rx, ry)` and `(lt, rt)`.
#[allow(clippy::too_many_arguments)]
pub fn normalize_usb_axes(
    ls_x: f32,
    ls_y: f32,
    rs_x: f32,
    rs_y: f32,
    lt: f32,
    rt: f32,
    left_deadzone: f32,
    right_deadzone: f32,
    trigger_deadzone: f32,
) -> ([i16; 4], [u8; 2]) {
    let (dlx, dly) = apply_stick_deadzone(ls_x, ls_y, left_deadzone);
    let (lx, ly) = scale_stick(dlx, dly);
    let (drx, dry) = apply_stick_deadzone(rs_x, rs_y, right_deadzone);
    let (rx, ry) = scale_stick(drx, dry);
    let quantize = |v: f32| ((v.clamp(0.0, 1.0) * 255.0) as i32).clamp(0, 255) as u8;
    let lt = quantize(apply_trigger_deadzone(lt, trigger_deadzone));
    let rt = quantize(apply_trigger_deadzone(rt, trigger_deadzone));
    ([lx, ly, rx, ry], [lt, rt])
}

/// Computes hat D-pad button flags from `(hat_x, hat_y)` using `±0.5` thresholds and latches
/// `hat_x_used`/`hat_y_used` when deflected.
pub fn apply_hat_axes(
    input_map: u32,
    hat_x: f32,
    hat_y: f32,
    hat_x_used: &mut bool,
    hat_y_used: &mut bool,
) -> u32 {
    let mut map = input_map & !(button::LEFT | button::RIGHT | button::UP | button::DOWN);
    if f64::from(hat_x) < -0.5 {
        map |= button::LEFT;
        *hat_x_used = true;
    } else if f64::from(hat_x) > 0.5 {
        map |= button::RIGHT;
        *hat_x_used = true;
    }
    if f64::from(hat_y) < -0.5 {
        map |= button::UP;
        *hat_y_used = true;
    } else if f64::from(hat_y) > 0.5 {
        map |= button::DOWN;
        *hat_y_used = true;
    }
    map
}

/// Full Android `handleAxisSet` state container updated in place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AxisSetOutput {
    pub input_map: u32,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub left_stick_x: i16,
    pub left_stick_y: i16,
    pub right_stick_x: i16,
    pub right_stick_y: i16,
    pub left_trigger_used: bool,
    pub right_trigger_used: bool,
    pub hat_x_used: bool,
    pub hat_y_used: bool,
}

/// Normalizes one Android `MotionEvent` axis set matching `ControllerHandler.handleAxisSet`.
#[allow(clippy::too_many_arguments)]
pub fn normalize_axis_set(
    ls_x: f32,
    ls_y: f32,
    rs_x: f32,
    rs_y: f32,
    lt: f32,
    rt: f32,
    hat_x: f32,
    hat_y: f32,
    has_left_stick: bool,
    has_right_stick: bool,
    has_triggers: bool,
    has_hat: bool,
    left_deadzone: f32,
    right_deadzone: f32,
    trigger_deadzone: f32,
    triggers_idle_negative: bool,
    mut state: AxisSetOutput,
) -> AxisSetOutput {
    if has_left_stick {
        let (x, y) = apply_stick_deadzone(ls_x, ls_y, left_deadzone);
        let (sx, sy) = scale_stick(x, y);
        state.left_stick_x = sx;
        state.left_stick_y = sy;
    }
    if has_right_stick {
        let (x, y) = apply_stick_deadzone(rs_x, rs_y, right_deadzone);
        let (sx, sy) = scale_stick(x, y);
        state.right_stick_x = sx;
        state.right_stick_y = sy;
    }
    if has_triggers {
        let (lt_u8, rt_u8) = normalize_triggers(
            lt,
            rt,
            trigger_deadzone,
            triggers_idle_negative,
            &mut state.left_trigger_used,
            &mut state.right_trigger_used,
        );
        state.left_trigger = lt_u8;
        state.right_trigger = rt_u8;
    }
    if has_hat {
        state.input_map = apply_hat_axes(
            state.input_map,
            hat_x,
            hat_y,
            &mut state.hat_x_used,
            &mut state.hat_y_used,
        );
    }
    state
}

/// Unsigned trigger fusion (`maxByMagnitude(byte, byte)`).
pub fn max_by_magnitude_u8(a: u8, b: u8) -> u8 {
    if a > b { a } else { b }
}

/// Signed stick fusion preserving the sign of the larger magnitude (`maxByMagnitude(short, short)`).
pub fn max_by_magnitude_i16(a: i16, b: i16) -> i16 {
    if i32::from(a).abs() > i32::from(b).abs() {
        a
    } else {
        b
    }
}

/// Cubic mouse-emulation curve matching `ControllerHandler.convertRawStickAxisToPixelMovement`.
pub fn mouse_stick_curve(stick_x: i16, stick_y: i16) -> (f32, f32) {
    let inv_scale = f64::from(1.0f32 / STICK_SCALE);
    let x = (f64::from(stick_x) * inv_scale) as f32;
    let y = (f64::from(stick_y) * inv_scale) as f32;
    let x = (f64::from(x) * 4.0) as f32;
    let y = (f64::from(y) * 4.0) as f32;
    let mag = (f64::from(x).powi(2) + f64::from(y).powi(2)).sqrt();
    if mag > 0.0 {
        let factor = mag.powi(2);
        (
            (f64::from(x) * factor) as f32,
            (f64::from(y) * factor) as f32,
        )
    } else {
        (x, y)
    }
}

/// Next step size for fixed-step controller mouse mode (`1, 2, 4, ..., 128, 1`).
pub fn fixed_step_next(step: i32) -> i32 {
    if !(FIXED_STEP_MIN..FIXED_STEP_MAX).contains(&step) {
        return FIXED_STEP_MIN;
    }
    let highest_bit = 1i32 << (31 - (step as u32).leading_zeros());
    (highest_bit * 2).min(FIXED_STEP_MAX)
}

/// Fixed-step movement delta along one axis (`Integer.signum((int) component) * step`).
pub fn fixed_step_delta(component: f64, step: i32) -> i32 {
    (component as i32).signum().saturating_mul(step)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mag(v: (f32, f32)) -> f64 {
        (f64::from(v.0).powi(2) + f64::from(v.1).powi(2)).sqrt()
    }

    #[test]
    fn positive_deadzone_drops_inside_and_keeps_outer_raw() {
        assert_eq!(apply_stick_deadzone(0.05, 0.05, 0.1), (0.0, 0.0));
        assert_eq!(apply_stick_deadzone(0.06, 0.08, 0.1), (0.0, 0.0));
        let outside = apply_stick_deadzone(0.3, 0.4, 0.1);
        assert!((outside.0 - 0.3).abs() < 1e-6);
        assert!((outside.1 - 0.4).abs() < 1e-6);
    }

    #[test]
    fn negative_anti_deadzone_remaps_and_preserves_direction_and_center() {
        let v = apply_stick_deadzone(0.1, 0.0, -0.2);
        assert!((v.0 - 0.28).abs() < 1e-6);
        assert_eq!(v.1, 0.0);

        let diag = apply_stick_deadzone(0.06, -0.08, -0.1);
        let m = mag(diag);
        assert!((m - (0.1 + 0.1 * 0.9)).abs() < 1e-6);
        assert!((f64::from(diag.0) / m - 0.6).abs() < 1e-6);
        assert!((f64::from(diag.1) / m + 0.8).abs() < 1e-6);

        assert_eq!(apply_stick_deadzone(0.0, 1.0, -0.2), (0.0, 1.0));
        assert_eq!(apply_stick_deadzone(-1.0, 0.0, -0.2), (-1.0, 0.0));

        for r in [0.0, -0.05, -0.2] {
            assert_eq!(apply_stick_deadzone(0.005, -0.005, r), (0.0, 0.0));
            assert_eq!(apply_stick_deadzone(0.01, 0.0, r), (0.0, 0.0));
        }
        let pass = apply_stick_deadzone(0.02, 0.5, 0.0);
        assert_eq!(pass, (0.02, 0.5));
    }

    #[test]
    fn trigger_deadzone_and_idle_negative_latch() {
        assert_eq!(apply_trigger_deadzone(0.02, 0.02), 0.0);
        assert_eq!(apply_trigger_deadzone(0.05, 0.02), 0.05);
        assert_eq!(apply_trigger_deadzone(-0.01, 0.0), 0.0);
        assert_eq!(apply_trigger_deadzone(0.29, QUIRK_TRIGGER_DEADZONE), 0.0);
        assert_eq!(apply_trigger_deadzone(0.31, QUIRK_TRIGGER_DEADZONE), 0.31);

        let mut lt_used = false;
        let mut rt_used = false;
        // Initial zero stays idle even with idle_negative = true.
        assert_eq!(
            normalize_triggers(0.0, 0.0, 0.02, true, &mut lt_used, &mut rt_used),
            (0, 0)
        );
        assert!(!lt_used && !rt_used);
        // Idle negative -1.0 after first touch becomes 0.0 -> 0.
        assert_eq!(
            normalize_triggers(-1.0, 1.0, 0.02, true, &mut lt_used, &mut rt_used),
            (0, 255)
        );
        assert!(lt_used && rt_used);
    }

    #[test]
    fn magnitude_fusion_handles_unsigned_bytes_and_min_short() {
        assert_eq!(max_by_magnitude_u8(127, 255), 255);
        assert_eq!(max_by_magnitude_u8(200, 10), 200);
        assert_eq!(max_by_magnitude_i16(32767, -32768), -32768);
        assert_eq!(max_by_magnitude_i16(-12345, 12000), -12345);
        assert_eq!(max_by_magnitude_i16(100, -100), -100);
    }

    #[test]
    fn hat_axes_and_mouse_fixed_step_match_java() {
        let mut hx = false;
        let mut hy = false;
        let m = apply_hat_axes(button::A | button::UP, -0.8, 0.9, &mut hx, &mut hy);
        assert_eq!(m, button::A | button::LEFT | button::DOWN);
        assert!(hx && hy);

        let mut step = FIXED_STEP_MIN;
        for expected in [2, 4, 8, 16, 32, 64, 128, 1, 2] {
            step = fixed_step_next(step);
            assert_eq!(step, expected);
        }
        assert_eq!(fixed_step_next(0), 1);
        assert_eq!(fixed_step_next(5), 8);
        assert_eq!(fixed_step_delta(3.7, 16), 16);
        assert_eq!(fixed_step_delta(-1.0, 16), -16);
        assert_eq!(fixed_step_delta(0.99, 64), 0);
        assert_eq!(fixed_step_delta(-1.0, i32::MIN), i32::MAX);
        assert_eq!(fixed_step_delta(1.0, i32::MIN), i32::MIN);

        assert_eq!(mouse_stick_curve(0, 0), (0.0, 0.0));
        let (mx, my) = mouse_stick_curve(0x7FFE, 0);
        assert!((mx - 64.0).abs() < 1e-4);
        assert_eq!(my, 0.0);
    }

    #[test]
    fn out_of_range_axes_clamp_without_wraparound() {
        assert_eq!(scale_stick(1.05, -1.05), (32766, 32766));
        assert_eq!(scale_stick(-1.5, 1.5), (-32766, -32766));

        let (mut lt_used, mut rt_used) = (false, false);
        assert_eq!(
            normalize_triggers(1.05, -0.5, 0.0, false, &mut lt_used, &mut rt_used),
            (255, 0)
        );
        let (sticks, triggers) =
            normalize_usb_axes(1.05, -1.05, -2.0, 2.0, 1.05, -0.5, 0.0, 0.0, 0.0);
        assert_eq!(sticks, [32766, 32766, -32766, -32766]);
        assert_eq!(triggers, [255, 0]);
    }
}
