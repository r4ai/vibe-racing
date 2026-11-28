//! The global push-to-talk key: registration and capture from the window.

use std::cell::RefCell;

use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;
use iced::keyboard::{Modifiers, key};

thread_local! {
    /// Lives on the event-loop thread, which receives the key messages.
    static MANAGER: RefCell<Option<(GlobalHotKeyManager, Option<HotKey>)>> =
        const { RefCell::new(None) };
}

/// Presses (`true`) and releases of the key, as they happen.
static EVENTS: std::sync::LazyLock<crate::inbox::Inbox<bool>> =
    std::sync::LazyLock::new(crate::inbox::Inbox::new);

/// The presses and releases, for the one subscription that reads them.
/// The key's messages are handed over where they arrive, so nothing polls.
pub fn events() -> Option<tokio::sync::mpsc::UnboundedReceiver<bool>> {
    use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
    let rx = EVENTS.take()?;
    GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
        EVENTS.send(e.state == HotKeyState::Pressed);
    }));
    // Any that came before the handler.
    while let Ok(e) = GlobalHotKeyEvent::receiver().try_recv() {
        EVENTS.send(e.state == HotKeyState::Pressed);
    }
    Some(rx)
}

/// Registers `accelerator` (e.g. `F9`, `Ctrl+Shift+Space`) in place of the
/// current key. Must be called on the event-loop thread.
pub fn bind(accelerator: &str) -> Result<String, String> {
    let key: HotKey = accelerator
        .parse()
        .map_err(|e| format!("{accelerator}: {e}"))?;
    unbind();
    MANAGER.with_borrow_mut(|slot| {
        if slot.is_none() {
            let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
            *slot = Some((manager, None));
        }
        let (manager, current) = slot.as_mut().expect("set above");
        manager
            .register(key)
            .map_err(|e| format!("{accelerator}: {e}"))?;
        *current = Some(key);
        Ok(accelerator.to_string())
    })
}

/// Gives the key back to the other applications.
pub fn unbind() {
    MANAGER.with_borrow_mut(|slot| {
        if let Some((manager, current)) = slot
            && let Some(key) = current.take()
        {
            let _ = manager.unregister(key);
        }
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    Key(String),
    Cancel,
    /// Would be taken away from every other application while typing.
    NeedsModifier,
}

/// What a key press means while assigning a key; `None` keeps waiting.
pub fn capture(code: key::Code, modifiers: Modifiers) -> Option<Captured> {
    if code == key::Code::Escape {
        return Some(Captured::Cancel);
    }
    let name = format!("{code:?}");
    let modifier_keys = ["Control", "Shift", "Alt", "Super", "Meta", "Hyper"];
    if modifier_keys.iter().any(|m| name.starts_with(m)) {
        return None;
    }
    let function_key = name
        .strip_prefix('F')
        .is_some_and(|n| n.parse::<u8>().is_ok());
    let standalone = function_key
        || name.starts_with("Numpad")
        || ["Pause", "ScrollLock", "Insert"].contains(&name.as_str());
    let chord = modifiers.control() || modifiers.alt() || modifiers.logo();
    if !standalone && !chord {
        return Some(Captured::NeedsModifier);
    }
    let mut parts = Vec::new();
    for (on, label) in [
        (modifiers.control(), "Ctrl"),
        (modifiers.alt(), "Alt"),
        (modifiers.shift(), "Shift"),
        (modifiers.logo(), "Super"),
    ] {
        if on {
            parts.push(label);
        }
    }
    let short = name.strip_prefix("Key").or(name.strip_prefix("Digit"));
    parts.push(short.unwrap_or(&name));
    Some(Captured::Key(parts.join("+")))
}

#[cfg(test)]
mod tests {
    use iced::keyboard::key::Code;

    use super::*;

    #[test]
    fn captures_keys() {
        let none = Modifiers::empty();
        let key = |s: &str| Some(Captured::Key(s.into()));
        assert_eq!(capture(Code::F13, none), key("F13"));
        assert_eq!(capture(Code::Numpad5, none), key("Numpad5"));
        assert_eq!(capture(Code::Pause, none), key("Pause"));
        assert_eq!(
            capture(Code::KeyT, Modifiers::CTRL | Modifiers::SHIFT),
            key("Ctrl+Shift+T")
        );
        assert_eq!(capture(Code::Digit1, Modifiers::ALT), key("Alt+1"));
        assert_eq!(capture(Code::Space, Modifiers::LOGO), key("Super+Space"));
        assert_eq!(capture(Code::Escape, none), Some(Captured::Cancel));
    }

    #[test]
    fn refuses_typing_keys_and_waits_for_modifiers() {
        let typing = Some(Captured::NeedsModifier);
        assert_eq!(capture(Code::KeyT, Modifiers::empty()), typing);
        assert_eq!(capture(Code::Space, Modifiers::SHIFT), typing);
        assert_eq!(capture(Code::ControlLeft, Modifiers::CTRL), None);
        assert_eq!(capture(Code::ShiftRight, Modifiers::SHIFT), None);
    }

    #[test]
    fn bad_accelerators_are_reported() {
        assert!(bind("Nope+").unwrap_err().starts_with("Nope+: "));
        unbind();
    }
}
