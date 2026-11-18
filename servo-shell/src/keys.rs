use servo::{Code, Key, KeyState, KeyboardEvent, Location, Modifiers, NamedKey};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{
    Key as WinitKey, KeyLocation, ModifiersState, NamedKey as WinitNamed, PhysicalKey,
};

pub fn key(key: &WinitKey) -> Key {
    match key {
        WinitKey::Character(text) => Key::Character(text.to_string()),
        WinitKey::Named(WinitNamed::Space) => Key::Character(" ".into()),
        WinitKey::Named(WinitNamed::Super) => Key::Named(NamedKey::Meta),
        WinitKey::Named(named) => format!("{named:?}")
            .parse::<NamedKey>()
            .map(Key::Named)
            .unwrap_or(Key::Named(NamedKey::Unidentified)),
        _ => Key::Named(NamedKey::Unidentified),
    }
}

pub fn code(physical: &PhysicalKey) -> Code {
    let PhysicalKey::Code(code) = physical else {
        return Code::Unidentified;
    };
    let name = format!("{code:?}")
        .replace("SuperLeft", "MetaLeft")
        .replace("SuperRight", "MetaRight");
    name.parse().unwrap_or(Code::Unidentified)
}

pub fn location(location: KeyLocation) -> Location {
    match location {
        KeyLocation::Left => Location::Left,
        KeyLocation::Right => Location::Right,
        KeyLocation::Numpad => Location::Numpad,
        _ => Location::Standard,
    }
}

pub fn modifiers(state: ModifiersState) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::SHIFT, state.shift_key());
    modifiers.set(Modifiers::CONTROL, state.control_key());
    modifiers.set(Modifiers::ALT, state.alt_key());
    modifiers.set(Modifiers::META, state.super_key());
    modifiers
}

pub fn event(event: &KeyEvent, held: ModifiersState) -> KeyboardEvent {
    KeyboardEvent::new_without_event(
        if event.state == ElementState::Pressed {
            KeyState::Down
        } else {
            KeyState::Up
        },
        key(&event.logical_key),
        code(&event.physical_key),
        location(event.location),
        modifiers(held),
        event.repeat,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    #[test]
    fn turns_window_keys_into_page_keys() {
        assert_eq!(
            key(&WinitKey::Named(WinitNamed::ArrowLeft)),
            Key::Named(NamedKey::ArrowLeft)
        );
        assert_eq!(
            key(&WinitKey::Named(WinitNamed::Enter)),
            Key::Named(NamedKey::Enter)
        );
        assert_eq!(
            key(&WinitKey::Named(WinitNamed::Backspace)),
            Key::Named(NamedKey::Backspace)
        );
        assert_eq!(
            key(&WinitKey::Named(WinitNamed::Escape)),
            Key::Named(NamedKey::Escape)
        );
        assert_eq!(
            key(&WinitKey::Named(WinitNamed::Space)),
            Key::Character(" ".into())
        );
        assert_eq!(
            key(&WinitKey::Named(WinitNamed::Super)),
            Key::Named(NamedKey::Meta)
        );
        assert_eq!(
            key(&WinitKey::Character("é".into())),
            Key::Character("é".into())
        );
    }

    #[test]
    fn keeps_the_physical_key() {
        assert_eq!(code(&PhysicalKey::Code(KeyCode::KeyA)), Code::KeyA);
        assert_eq!(code(&PhysicalKey::Code(KeyCode::Digit1)), Code::Digit1);
        assert_eq!(code(&PhysicalKey::Code(KeyCode::SuperLeft)), Code::MetaLeft);
        assert_eq!(code(&PhysicalKey::Code(KeyCode::ArrowUp)), Code::ArrowUp);
        assert_eq!(location(KeyLocation::Numpad), Location::Numpad);
    }

    #[test]
    fn carries_the_held_modifiers() {
        let held = modifiers(ModifiersState::CONTROL | ModifiersState::SHIFT);
        assert!(held.contains(Modifiers::CONTROL));
        assert!(held.contains(Modifiers::SHIFT));
        assert!(!held.contains(Modifiers::ALT));
    }
}
