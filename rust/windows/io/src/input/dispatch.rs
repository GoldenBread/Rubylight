//! Input event batching, staging, and per-event dispatch for [`Injector`].

use super::*;

impl Injector {
    pub fn apply(&mut self, e: &Event) -> Result<()> {
        let mut errors = self.apply_all(std::slice::from_ref(e));
        errors.pop().map_or(Ok(()), Err)
    }
    /// Apply a pass of events in order, with one SendInput call where the
    /// order allows; returns why events failed.
    pub fn apply_all(&mut self, events: &[Event]) -> Vec<anyhow::Error> {
        if tracing::enabled!(target: "input_timing", tracing::Level::TRACE) {
            for e in events {
                if let Event::Relative { x, y } = e {
                    tracing::trace!(
                        target: "input_timing",
                        qpc = virtual_hid::qpc(),
                        x,
                        y,
                        "arrived"
                    );
                }
            }
        }
        for e in events {
            if let Err(error) = self.stage(e) {
                self.pending.errors.push(error);
            }
        }
        if let Err(error) = self.flush_touch_moves() {
            self.pending.errors.push(error);
        }
        self.flush_and_sync();
        std::mem::take(&mut self.pending.errors)
    }
    /// Add an event's keyboard and mouse input to the pass, or apply it.
    fn stage(&mut self, e: &Event) -> Result<()> {
        if !self.policy.allows(e) {
            return Ok(());
        }
        use Event::*;
        // Touch moves held back go out before anything that follows them.
        if !matches!(e, Touch { event: 3, .. })
            && let Err(error) = self.flush_touch_moves()
        {
            self.pending.errors.push(error);
        }
        if let Touch {
            event, id, x, y, ..
        } = *e
            && self.policy.touch.translates()
        {
            // Relative movement and scrolling are measured in stream pixels; the stream sets
            // its size before input, so 1080p stands in only for a touch that comes earlier.
            // Under a host resolution cap those are the coded picture's pixels, not the
            // larger display's, so tap slop and scroll distance feel slightly finer (absolute
            // positions still map onto the whole display).
            let size = self.stream.or_else(|| {
                let rect = self.rect?;
                Some((
                    u32::try_from(rect.right - rect.left).ok()?,
                    u32::try_from(rect.bottom - rect.top).ok()?,
                ))
            });
            let inputs = self.touch_profile.translate(
                &self.policy.touch,
                event,
                id,
                [x, y],
                size.unwrap_or((1920, 1080)),
                std::time::Instant::now(),
            );
            for input in &inputs {
                if let Err(error) = self.stage(input) {
                    self.pending.errors.push(error);
                }
            }
            return Ok(());
        }
        match e {
            // How a key or button is sent depends on the keys or buttons held,
            // including those pressed earlier in the pass.
            Keyboard { .. } if self.press_pending(true) => self.flush_and_sync(),
            MouseButton { .. } if self.press_pending(false) => self.flush_and_sync(),
            // Finish the absolute-mode click before relative input can press again.
            Relative { .. } if self.left_release.is_some() => {
                self.flush_and_sync();
                if let Err(error) = self.release_deferred_left(false) {
                    self.pending.errors.push(error);
                }
            }
            // Touch and pen go through another call: keep them in order.
            Touch { .. } | Pen { .. } => self.flush_and_sync(),
            _ => {}
        }
        let start = self.pending.inputs.len();
        let press = self.stage_event(e)?;
        let end = self.pending.inputs.len();
        if end > start {
            self.pending.events.push((end, press));
        } else if let Some(press) = press {
            // Nothing to send, such as a key another client holds.
            self.hold(press);
        }
        Ok(())
    }
    fn stage_event(&mut self, e: &Event) -> Result<Option<Press>> {
        use Event::*;
        match e {
            Relative { x, y } => {
                self.absolute = false;
                self.pending.push_auto(Self::mouse(
                    i32::from(*x),
                    i32::from(*y),
                    0,
                    MOUSEEVENTF_MOVE,
                ));
            }
            Absolute {
                x,
                y,
                width,
                height,
                // SAFETY: GetSystemMetrics uses valid metric constants and does not borrow any pointers.
            } => unsafe {
                self.absolute = true;
                let Some(rect) = self.display() else {
                    return Ok(None);
                };
                let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
                let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
                let w = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
                let h = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
                // The client's far edge is the display's last pixel, not the
                // first pixel of its neighbour.
                let (width, height) = (i64::from(*width).max(1), i64::from(*height).max(1));
                let (span_x, span_y) = (
                    i64::from((rect.right - rect.left - 1).max(0)),
                    i64::from((rect.bottom - rect.top - 1).max(0)),
                );
                let (px, py) = match self.picture() {
                    // The client's size is the stream's picture with its bars.
                    Some((stream, picture)) => {
                        let (fx, fy) = butterpollo_media_policy::display_policy::stream_to_picture(
                            (
                                i64::from(*x).clamp(0, width) as f64 / width as f64,
                                i64::from(*y).clamp(0, height) as f64 / height as f64,
                            ),
                            stream,
                            picture,
                        );
                        (
                            rect.left + (fx * span_x as f64) as i32,
                            rect.top + (fy * span_y as f64) as i32,
                        )
                    }
                    None => (
                        rect.left + (i64::from(*x).clamp(0, width) * span_x / width) as i32,
                        rect.top + (i64::from(*y).clamp(0, height) * span_y / height) as i32,
                    ),
                };
                self.pending.push_auto(Self::mouse(
                    ((i64::from(px - left) * 65535) / i64::from((w - 1).max(1))) as i32,
                    ((i64::from(py - top) * 65535) / i64::from((h - 1).max(1))) as i32,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                ));
            },
            MouseButton { button, down } => {
                // With absolute input (touch, pen, a tablet), Moonlight sends a
                // right press right after a left release for a long press: the
                // left click would land first and open a link. Hold the left
                // release back briefly and click right meanwhile, as Sunshine does.
                // `absolute_left_release_delay` = 0 sends it at once.
                if *button == 1 && self.absolute && !self.policy.left_release_delay.is_zero() {
                    if !*down && self.buttons.contains(button) {
                        self.buttons.remove(button);
                        self.left_release_virtual
                            .set(self.virtual_buttons.remove(button));
                        self.left_release =
                            Some(std::time::Instant::now() + self.policy.left_release_delay);
                        return Ok(None);
                    }
                    if *down && self.left_release.take().is_some() {
                        // Still down in Windows: its release never went out.
                        self.buttons.insert(*button);
                        if self.left_release_virtual.replace(false) {
                            self.virtual_buttons.insert(*button);
                        }
                        return Ok(None);
                    }
                }
                if *button == 3 && *down && self.left_release.is_some() {
                    let route = if self.virtual_mouse && virtual_hid::MOUSE.ready() {
                        MouseRoute::Virtual
                    } else {
                        MouseRoute::Injection
                    };
                    self.pending.push_routed(Self::button(3, true), route);
                    self.pending.push_routed(Self::button(3, false), route);
                    return Ok(None);
                }
                let owned = self.buttons.contains(button);
                let virtual_route = if *down {
                    !owned && self.virtual_mouse && virtual_hid::MOUSE.ready()
                } else {
                    owned && self.virtual_buttons.remove(button)
                };
                let held_id = u32::from(*button)
                    | if virtual_route {
                        VIRTUAL_BUTTON_FLAG
                    } else {
                        0
                    };
                let route = if virtual_route {
                    MouseRoute::Virtual
                } else {
                    MouseRoute::Injection
                };
                let pending = &mut self.pending;
                Self::update_held(
                    &mut HELD.lock().unwrap(),
                    (false, held_id),
                    *down,
                    owned,
                    Self::button(*button, *down),
                    |input| {
                        for &item in input {
                            pending.push_routed(item, route);
                        }
                        Ok(())
                    },
                )?;
                if *down {
                    if virtual_route {
                        self.virtual_buttons.insert(*button);
                    }
                    return Ok(Some(Press {
                        keyboard: false,
                        id: held_id,
                        source: 0,
                        flags: 0,
                        counted: !owned,
                        repeat: None,
                    }));
                }
                // A release is recorded even when Windows refuses it.
                self.buttons.remove(button);
            }
            Scroll { amount, horizontal } => {
                let index = usize::from(*horizontal);
                let amount = if self.policy.high_resolution_scrolling {
                    i32::from(*amount)
                } else {
                    self.scroll[index] += i32::from(*amount);
                    let amount = self.scroll[index] / 120 * 120;
                    self.scroll[index] -= amount;
                    amount
                };
                if amount == 0 {
                    return Ok(None);
                }
                self.pending.push_auto(Self::mouse(
                    0,
                    0,
                    amount as u32,
                    if *horizontal {
                        MOUSEEVENTF_HWHEEL
                    } else {
                        MOUSEEVENTF_WHEEL
                    },
                ));
            }
            Keyboard {
                key,
                down,
                flags,
                modifiers,
            } => {
                let mut source = keyboard_identity(*key, *modifiers);
                // A client can set the extended-key bit on the press but not
                // on the release: release the key that is held, or it stays
                // down and keeps repeating.
                if !*down
                    && !self.key_sources.contains_key(&source)
                    && self
                        .key_sources
                        .contains_key(&(source ^ EXPLICIT_EXTENDED_KEY))
                {
                    source ^= EXPLICIT_EXTENDED_KEY;
                }
                let key = if *down {
                    if self.key_sources.contains_key(&source) {
                        return Ok(None);
                    }
                    let key = mapped_keyboard_identity(&self.policy, *key, *modifiers);
                    if self.keys.contains(&key) {
                        self.key_sources.insert(source, key);
                        return Ok(None);
                    }
                    key
                } else {
                    let Some(key) = self.key_sources.remove(&source) else {
                        return Ok(None);
                    };
                    // Two client keys may map to one host key. It stays down
                    // until the last source lets go, without restarting repeat.
                    if self.key_sources.values().any(|mapped| *mapped == key) {
                        return Ok(None);
                    }
                    key
                };
                let owned = self.keys.contains(&key);
                let flags = if *down {
                    *flags
                } else {
                    self.key_flags.get(&key).copied().unwrap_or(*flags)
                };
                let input = self.key_scan(key, *down, flags);
                let always_send_scancodes = self.policy.always_send_scancodes;
                // HELD is taken once for the whole event: the modifiers to press
                // around the key are decided under it, and the press or release
                // updates it.
                let mut held = HELD.lock().unwrap();
                let modifiers = if *down && !owned {
                    self.synthetic_modifiers(key, *modifiers, &held)
                } else {
                    0
                };
                let pending = &mut self.pending;
                if modifiers == 0 {
                    Self::update_held(&mut held, (true, key), *down, owned, input, |input| {
                        for &item in input {
                            pending.push_auto(item);
                        }
                        Ok(())
                    })?;
                } else {
                    // The client reported a modifier it never pressed as a
                    // key: press it around this key only, as Vibepollo does.
                    let count = held.get(&(true, key)).copied().unwrap_or(0);
                    if count == 0 {
                        for item in Self::with_modifiers(input, modifiers, always_send_scancodes) {
                            pending.push_auto(item);
                        }
                    }
                    held.insert((true, key), count + 1);
                }
                // A press Windows refuses is not held; a release is recorded
                // even when Windows refuses it, or the key keeps repeating.
                if *down {
                    return Ok(Some(Press {
                        keyboard: true,
                        id: key,
                        source,
                        flags,
                        counted: !owned,
                        repeat: (!owned && !is_modifier(key)).then_some(modifiers),
                    }));
                }
                self.keys.remove(&key);
                self.key_flags.remove(&key);
                if self.repeat.is_some_and(|(repeating, ..)| repeating == key) {
                    self.repeat = None;
                }
            }
            Text(s) => {
                for v in s.encode_utf16() {
                    self.pending.push_auto(Self::key(v, true, true));
                    self.pending.push_auto(Self::key(v, false, true));
                }
            }
            Touch {
                event,
                id,
                x,
                y,
                pressure,
                major,
                minor,
                rotation,
            } => self.touch(*event, *id, *x, *y, *pressure, *major, *minor, *rotation)?,
            Pen {
                event,
                tool,
                buttons,
                x,
                y,
                pressure,
                rotation,
                tilt,
                roll,
            } => self.pen(
                *event, *tool, *buttons, *x, *y, *pressure, *rotation, *tilt, *roll,
            )?,
            Haptics(enabled) => {
                self.haptics = *enabled;
                if let Some(gamepads) = &self.gamepads {
                    gamepads.set_haptics(*enabled);
                }
            }
            Touchpad {
                buttons,
                width_mm,
                height_mm,
                contacts,
                contacts_len,
                ..
            } => self.touchpad_packet(
                *buttons,
                *width_mm,
                *height_mm,
                &contacts[..usize::from(*contacts_len)],
            ),
            // Only the VHF Steam Controller profile takes them.
            Controller { .. }
            | SteamAttributes(_)
            | ControllerIdentity(_)
            | Arrival { .. }
            | ControllerTouch { .. }
            | Motion { .. }
            | Battery { .. } => self.gamepads()?.send(e.clone()),
        }
        Ok(None)
    }
}
