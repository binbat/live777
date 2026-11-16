//! Remote input (keyboard/pointer) for the wayland source.
//!
//! Events arrive as JSON messages on a datachannel cable (label `input`;
//! undeclared cables fall back to the stream's anonymous default bus, so a
//! plain `input=1` on the wayland URL needs no cable config). The compositor
//! injects them into the seat: pointer motion/buttons with hit-tested focus,
//! keys to the pointer-focus window. The browser side lives in
//! `web/player-core` (`input` option on the WHEP player).
//!
//! Wire format (JSON, one object per datachannel message):
//!   {"t":"key","code":"KeyA","down":1}     // UI Events `code` (DomCode)
//!   {"t":"move","x":512.0,"y":384.0}       // canvas coordinates
//!   {"t":"button","b":0,"down":1}          // 0 left, 1 middle, 2 right
//!   {"t":"wheel","x":0.0,"y":-3.0}         // wheel notches (lines)

/// One parsed remote-input event, crossing into the compositor thread over a
/// calloop channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// evdev keycode (KEY_*), already translated from the DomCode.
    Key { keycode: u32, down: bool },
    /// Absolute canvas position.
    PointerMove { x: f64, y: f64 },
    /// evdev button (BTN_*).
    PointerButton { button: u32, down: bool },
    /// Wheel steps in lines (positive = down/right).
    Wheel { dx: f64, dy: f64 },
}

/// Parse one datachannel message; `None` for anything that is not a
/// well-formed input event (the bus is shared — noise must not kill input).
pub fn parse(data: &[u8]) -> Option<InputEvent> {
    let v: serde_json::Value = serde_json::from_slice(data).ok()?;
    let down = |v: &serde_json::Value| v.get("down")?.as_i64().map(|d| d != 0);
    match v.get("t")?.as_str()? {
        "key" => Some(InputEvent::Key {
            keycode: domcode_to_evdev(v.get("code")?.as_str()?)?,
            down: down(&v)?,
        }),
        "move" => Some(InputEvent::PointerMove {
            x: v.get("x")?.as_f64()?,
            y: v.get("y")?.as_f64()?,
        }),
        "button" => Some(InputEvent::PointerButton {
            button: match v.get("b")?.as_i64()? {
                0 => 0x110, // BTN_LEFT
                1 => 0x112, // BTN_MIDDLE
                2 => 0x111, // BTN_RIGHT
                _ => return None,
            },
            down: down(&v)?,
        }),
        "wheel" => Some(InputEvent::Wheel {
            dx: v.get("x")?.as_f64()?,
            dy: v.get("y")?.as_f64()?,
        }),
        _ => None,
    }
}

/// Browser `KeyboardEvent.code` (UI Events DomCode) → evdev keycode.
/// Covers the full US-QWERTY position set plus function/navigation/editing
/// keys and modifiers; unknown codes return `None` and the event is dropped.
fn domcode_to_evdev(code: &str) -> Option<u32> {
    Some(match code {
        // Letter row: DomCode is positional, so this table is layout-safe.
        "KeyQ" => 16,
        "KeyW" => 17,
        "KeyE" => 18,
        "KeyR" => 19,
        "KeyT" => 20,
        "KeyY" => 21,
        "KeyU" => 22,
        "KeyI" => 23,
        "KeyO" => 24,
        "KeyP" => 25,
        "KeyA" => 30,
        "KeyS" => 31,
        "KeyD" => 32,
        "KeyF" => 33,
        "KeyG" => 34,
        "KeyH" => 35,
        "KeyJ" => 36,
        "KeyK" => 37,
        "KeyL" => 38,
        "KeyZ" => 44,
        "KeyX" => 45,
        "KeyC" => 46,
        "KeyV" => 47,
        "KeyB" => 48,
        "KeyN" => 49,
        "KeyM" => 50,
        // Digit row
        "Digit1" => 2,
        "Digit2" => 3,
        "Digit3" => 4,
        "Digit4" => 5,
        "Digit5" => 6,
        "Digit6" => 7,
        "Digit7" => 8,
        "Digit8" => 9,
        "Digit9" => 10,
        "Digit0" => 11,
        // Punctuation
        "Minus" => 12,
        "Equal" => 13,
        "BracketLeft" => 26,
        "BracketRight" => 27,
        "Semicolon" => 39,
        "Quote" => 40,
        "Backquote" => 41,
        "Backslash" => 43,
        "Comma" => 51,
        "Period" => 52,
        "Slash" => 53,
        "IntlBackslash" => 86,
        // Editing / navigation
        "Escape" => 1,
        "Backspace" => 14,
        "Tab" => 15,
        "Enter" => 28,
        "NumpadEnter" => 96,
        "Space" => 57,
        "CapsLock" => 58,
        "Insert" => 110,
        "Delete" => 111,
        "Home" => 102,
        "End" => 107,
        "PageUp" => 104,
        "PageDown" => 109,
        "ArrowLeft" => 105,
        "ArrowUp" => 103,
        "ArrowRight" => 106,
        "ArrowDown" => 108,
        "PrintScreen" => 99,
        "ScrollLock" => 70,
        "Pause" => 119,
        "NumLock" => 69,
        // Modifiers
        "ShiftLeft" => 42,
        "ShiftRight" => 54,
        "ControlLeft" => 29,
        "ControlRight" => 97,
        "AltLeft" => 56,
        "AltRight" => 100,
        "MetaLeft" => 125,
        "MetaRight" => 126,
        "ContextMenu" => 127,
        // Function row
        "F1" => 59,
        "F2" => 60,
        "F3" => 61,
        "F4" => 62,
        "F5" => 63,
        "F6" => 64,
        "F7" => 65,
        "F8" => 66,
        "F9" => 67,
        "F10" => 68,
        "F11" => 87,
        "F12" => 88,
        // Numpad
        "Numpad0" => 82,
        "Numpad1" => 79,
        "Numpad2" => 80,
        "Numpad3" => 81,
        "Numpad4" => 75,
        "Numpad5" => 76,
        "Numpad6" => 77,
        "Numpad7" => 71,
        "Numpad8" => 72,
        "Numpad9" => 73,
        "NumpadDecimal" => 83,
        "NumpadAdd" => 78,
        "NumpadSubtract" => 74,
        "NumpadMultiply" => 55,
        "NumpadDivide" => 98,
        "NumpadEqual" => 117,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_events() {
        assert_eq!(
            parse(br#"{"t":"key","code":"KeyA","down":1}"#),
            Some(InputEvent::Key {
                keycode: 30,
                down: true
            })
        );
        assert_eq!(
            parse(br#"{"t":"move","x":1.5,"y":2}"#),
            Some(InputEvent::PointerMove { x: 1.5, y: 2.0 })
        );
        assert_eq!(
            parse(br#"{"t":"button","b":2,"down":0}"#),
            Some(InputEvent::PointerButton {
                button: 0x111,
                down: false
            })
        );
        assert_eq!(
            parse(br#"{"t":"wheel","x":0,"y":-3}"#),
            Some(InputEvent::Wheel { dx: 0.0, dy: -3.0 })
        );
        // Noise: never panics, never an event.
        assert_eq!(parse(b"hello"), None);
        assert_eq!(parse(br#"{"t":"key","code":"MediaPlay","down":1}"#), None);
        assert_eq!(parse(br#"{"t":"button","b":9,"down":1}"#), None);
        assert_eq!(parse(br#"{"t":"key","code":"KeyA"}"#), None);
    }
}
