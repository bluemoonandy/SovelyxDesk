pub fn keysym(text: &str, control: bool) -> Option<u32> {
    use slint::platform::Key;
    // Slint encodes named modifier keys inside the C0 control-character range.
    // They must be classified before decoding legacy Ctrl+letter text (0x11 is Q).
    if is_modifier(text) {
        return None;
    }
    let special = [
        (Key::Backspace, 0xff08),
        (Key::Tab, 0xff09),
        (Key::Return, 0xff0d),
        (Key::Escape, 0xff1b),
        (Key::Delete, 0xffff),
        (Key::Home, 0xff50),
        (Key::End, 0xff57),
        (Key::LeftArrow, 0xff51),
        (Key::UpArrow, 0xff52),
        (Key::RightArrow, 0xff53),
        (Key::DownArrow, 0xff54),
        (Key::PageUp, 0xff55),
        (Key::PageDown, 0xff56),
        (Key::Insert, 0xff63),
        (Key::F1, 0xffbe),
        (Key::F2, 0xffbf),
        (Key::F3, 0xffc0),
        (Key::F4, 0xffc1),
        (Key::F5, 0xffc2),
        (Key::F6, 0xffc3),
        (Key::F7, 0xffc4),
        (Key::F8, 0xffc5),
        (Key::F9, 0xffc6),
        (Key::F10, 0xffc7),
        (Key::F11, 0xffc8),
        (Key::F12, 0xffc9),
    ];
    for (key, symbol) in special {
        let character: char = key.into();
        if text == character.to_string() {
            return Some(symbol);
        }
    }
    let mut characters = text.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    let code = character as u32;
    if control && (1..=26).contains(&code) {
        return Some(u32::from(b'a') + code - 1);
    }
    if character.is_control() || (0xe000..=0xf8ff).contains(&code) {
        return None;
    }
    Some(super::character_keysym(character))
}

fn is_modifier(text: &str) -> bool {
    use slint::platform::Key;
    [
        Key::Shift,
        Key::ShiftR,
        Key::Control,
        Key::ControlR,
        Key::Alt,
        Key::AltGr,
        Key::Meta,
        Key::MetaR,
        Key::CapsLock,
    ]
    .into_iter()
    .any(|key| text == char::from(key).to_string())
}

pub fn modifier_keysym(text: &str) -> Option<u32> {
    use slint::platform::Key;
    [
        (Key::Shift, super::SHIFT),
        (Key::ShiftR, super::SHIFT),
        (Key::Control, super::CONTROL),
        (Key::ControlR, super::CONTROL),
        (Key::Alt, super::OPTION),
        (Key::AltGr, super::OPTION),
        (Key::Meta, super::COMMAND),
        (Key::MetaR, super::COMMAND),
    ]
    .into_iter()
    .find_map(|(key, symbol)| (text == char::from(key).to_string()).then_some(symbol))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub keysym: u32,
    pub down: bool,
}

impl Key {
    pub fn packet(self) -> [u8; 8] {
        let symbol = self.keysym.to_be_bytes();
        [
            4,
            u8::from(self.down),
            0,
            0,
            symbol[0],
            symbol[1],
            symbol[2],
            symbol[3],
        ]
    }
}

pub struct Event<'a> {
    pub physical: Option<&'a str>,
    pub text: &'a str,
    /// Shift, Control, Alt, Meta from the local event, not the remote state.
    pub modifiers: [bool; 4],
    pub down: bool,
    pub repeat: bool,
}

pub struct Keyboard {
    held: std::collections::BTreeMap<String, u32>,
    remote: [bool; 4],
    mode: bool,
    cleared: bool,
}

impl Keyboard {
    pub fn new(ctrl_as_command: bool) -> Self {
        Self {
            held: Default::default(),
            remote: [false; 4],
            mode: ctrl_as_command,
            cleared: true,
        }
    }

    fn sync(&mut self, wanted: [bool; 4], output: &mut Vec<Key>) {
        let symbols = [super::SHIFT, super::CONTROL, super::OPTION, super::COMMAND];
        // Release the old combination completely before pressing its replacement.
        for down in [false, true] {
            for (index, keysym) in symbols.into_iter().enumerate() {
                if self.remote[index] != wanted[index]
                    && wanted[index] == down
                    && (!down || !self.cleared)
                {
                    output.push(Key { keysym, down });
                    self.remote[index] = down;
                }
            }
        }
    }

    fn wanted(&self, local: [bool; 4]) -> [bool; 4] {
        let [shift, control, alt, meta] = local;
        let space = self.held.values().any(|symbol| *symbol == 0x20);
        let command = self.mode && !space;
        [
            shift,
            control && !command,
            alt,
            meta || (control && command),
        ]
    }

    pub fn set_mode(&mut self, mode: bool) -> Vec<Key> {
        if self.mode == mode {
            return Vec::new();
        }
        let output = self.release();
        self.mode = mode;
        output
    }

    pub fn release(&mut self) -> Vec<Key> {
        let mut output: Vec<_> = std::mem::take(&mut self.held)
            .into_values()
            .map(|keysym| Key {
                keysym,
                down: false,
            })
            .collect();
        self.cleared = true;
        self.sync([false; 4], &mut output);
        output
    }

    pub fn event(&mut self, event: Event<'_>) -> Vec<Key> {
        let mut output = Vec::new();
        let symbol = keysym(event.text, event.modifiers[1]);
        let identifier = event.physical.map(str::to_owned).unwrap_or_else(|| {
            symbol
                .and_then(char::from_u32)
                .map(|c| c.to_ascii_lowercase().to_string())
                .unwrap_or_else(|| event.text.to_owned())
        });
        if event.down {
            if event.repeat && !self.held.contains_key(&identifier) {
                return output; // A repeat after focus loss/mode change must not resurrect a key.
            }
            self.cleared = false;
            let pressed = self.held.get(&identifier).copied().or(symbol);
            if let Some(keysym) = pressed {
                self.held.insert(identifier, keysym);
            }
            self.sync(self.wanted(event.modifiers), &mut output);
            if let Some(keysym) = pressed {
                output.push(Key { keysym, down: true });
            }
        } else {
            if let Some(keysym) = self.held.remove(&identifier) {
                output.push(Key {
                    keysym,
                    down: false,
                });
            }
            // Release the character under its original modifiers before changing the chord.
            self.sync(self.wanted(event.modifiers), &mut output);
        }
        output
    }

    pub fn commit(&mut self, text: &str) -> Vec<Key> {
        // IME commits are complete text, never held keys or Ctrl-letter encodings.
        let mut output = self.release();
        for c in text
            .chars()
            .filter(|c| !c.is_control() && !(0xe000..=0xf8ff).contains(&(*c as u32)))
        {
            let keysym = super::character_keysym(c);
            output.extend([
                Key { keysym, down: true },
                Key {
                    keysym,
                    down: false,
                },
            ]);
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifiers_are_never_control_character_letters() {
        use slint::platform::Key;
        for key in [
            Key::Control,
            Key::ControlR,
            Key::Shift,
            Key::ShiftR,
            Key::Alt,
            Key::AltGr,
            Key::Meta,
            Key::MetaR,
            Key::CapsLock,
        ] {
            let text = char::from(key).to_string();
            assert_eq!(keysym(&text, true), None, "{key:?}");
        }
    }
    #[test]
    fn copy_is_c_and_not_a_window_command() {
        assert_eq!(keysym("\u{3}", true), Some(0x63));
        assert_eq!(keysym("c", true), Some(0x63));
    }
}

#[cfg(test)]
mod sequences {
    use super::*;
    use crate::input::{COMMAND, CONTROL, OPTION, SHIFT};
    fn key(keysym: u32, down: bool) -> Key {
        Key { keysym, down }
    }
    fn send(k: &mut Keyboard, id: &str, text: &str, mods: [bool; 4], down: bool) -> Vec<Key> {
        k.event(Event {
            physical: Some(id),
            text,
            modifiers: mods,
            down,
            repeat: false,
        })
    }
    const CTRL: [bool; 4] = [false, true, false, false];
    const NONE: [bool; 4] = [false; 4];
    #[test]
    fn ctrl_copy_paste_have_exact_balanced_sequences_in_both_modes() {
        for mode in [false, true] {
            let mut k = Keyboard::new(mode);
            let modifier = if mode { COMMAND } else { CONTROL };
            assert_eq!(
                send(&mut k, "ControlLeft", "\u{11}", CTRL, true),
                vec![key(modifier, true)]
            );
            for letter in ["c", "v", "x", "a", "s", "z", "f", "w", "q"] {
                let symbol = u32::from(letter.as_bytes()[0]);
                assert_eq!(
                    send(&mut k, letter, letter, CTRL, true),
                    vec![key(symbol, true)]
                );
                assert_eq!(
                    send(&mut k, letter, letter, CTRL, false),
                    vec![key(symbol, false)]
                );
            }
            assert_eq!(
                send(&mut k, "ControlLeft", "\u{11}", NONE, false),
                vec![key(modifier, false)]
            );
            assert!(k.release().is_empty());
        }
    }
    #[test]
    fn ctrl_alone_and_other_modifiers_never_generate_q_w_or_other_letters() {
        let mut k = Keyboard::new(true);
        let out = send(&mut k, "ControlLeft", "\u{11}", CTRL, true);
        assert_eq!(out, vec![key(COMMAND, true)]);
        let out = send(
            &mut k,
            "ShiftLeft",
            "\u{10}",
            [true, true, false, false],
            true,
        );
        assert_eq!(out, vec![key(SHIFT, true)]);
        let out = send(&mut k, "AltLeft", "\u{12}", [true, true, true, false], true);
        assert_eq!(out, vec![key(OPTION, true)]);
        assert!(k.release().iter().all(|e| e.keysym >= 0xff00));
    }
    #[test]
    fn ctrl_space_releases_command_before_control_and_space() {
        let mut k = Keyboard::new(true);
        send(&mut k, "ControlLeft", "\u{11}", CTRL, true);
        for _ in 0..10 {
            assert_eq!(
                send(&mut k, "Space", " ", CTRL, true),
                vec![key(COMMAND, false), key(CONTROL, true), key(32, true)]
            );
            assert_eq!(
                send(&mut k, "Space", " ", CTRL, false),
                vec![key(32, false), key(CONTROL, false), key(COMMAND, true)]
            );
        }
        assert_eq!(
            send(&mut k, "ControlLeft", "\u{11}", NONE, false),
            vec![key(COMMAND, false)]
        );
        assert!(k.release().is_empty());
    }
    #[test]
    fn native_ctrl_space_never_emits_command() {
        let mut k = Keyboard::new(false);
        let mut all = send(&mut k, "ControlLeft", "\u{11}", CTRL, true);
        all.extend(send(&mut k, "Space", " ", CTRL, true));
        all.extend(send(&mut k, "ControlLeft", "\u{11}", NONE, false));
        all.extend(send(&mut k, "Space", " ", NONE, false));
        assert_eq!(
            all,
            vec![
                key(CONTROL, true),
                key(32, true),
                key(CONTROL, false),
                key(32, false)
            ]
        );
    }
    #[test]
    fn physical_release_matches_down_after_text_and_modifiers_change() {
        let mut k = Keyboard::new(true);
        let mods = [true, true, false, false];
        assert_eq!(
            send(&mut k, "Digit1", "!", mods, true),
            vec![key(SHIFT, true), key(COMMAND, true), key(33, true)]
        );
        assert_eq!(
            send(&mut k, "Digit1", "1", NONE, false),
            vec![key(33, false), key(SHIFT, false), key(COMMAND, false)]
        );
    }
    #[test]
    fn shifted_alt_shortcuts_and_repeats_preserve_original_symbols() {
        let mut k = Keyboard::new(true);
        let mods = [true, true, true, false];
        assert_eq!(
            send(&mut k, "KeyC", "C", mods, true),
            vec![
                key(SHIFT, true),
                key(OPTION, true),
                key(COMMAND, true),
                key(67, true)
            ]
        );
        for _ in 0..20 {
            assert_eq!(
                k.event(Event {
                    physical: Some("KeyC"),
                    text: "c",
                    modifiers: mods,
                    down: true,
                    repeat: true
                }),
                vec![key(67, true)]
            );
        }
        assert_eq!(send(&mut k, "KeyC", "c", mods, false), vec![key(67, false)]);
        k.release();
    }
    #[test]
    fn focus_disconnect_and_reconnect_cleanup_cannot_resurrect_keys() {
        for _ in 0..3 {
            let mut k = Keyboard::new(true);
            send(&mut k, "KeyC", "c", CTRL, true);
            assert_eq!(k.release(), vec![key(99, false), key(COMMAND, false)]);
            assert!(k.release().is_empty());
            assert!(
                k.event(Event {
                    physical: Some("KeyC"),
                    text: "c",
                    modifiers: CTRL,
                    down: true,
                    repeat: true
                })
                .is_empty()
            );
            assert!(send(&mut k, "KeyC", "c", CTRL, false).is_empty());
            assert_eq!(
                send(&mut k, "KeyV", "v", CTRL, true),
                vec![key(COMMAND, true), key(118, true)]
            );
        }
    }
    #[test]
    fn mode_switch_releases_old_chord_and_does_not_replay_held_keys() {
        let mut k = Keyboard::new(true);
        send(&mut k, "KeyC", "c", CTRL, true);
        assert_eq!(k.set_mode(false), vec![key(99, false), key(COMMAND, false)]);
        assert!(send(&mut k, "KeyC", "c", CTRL, false).is_empty());
        assert_eq!(
            send(&mut k, "KeyV", "v", CTRL, true),
            vec![key(CONTROL, true), key(118, true)]
        );
    }
    #[test]
    fn ime_commit_does_not_become_a_shortcut_or_a_stuck_key() {
        let mut k = Keyboard::new(true);
        send(&mut k, "ControlLeft", "\u{11}", CTRL, true);
        let output = k.commit("中文");
        assert_eq!(
            output,
            vec![
                key(COMMAND, false),
                key(0x01004e2d, true),
                key(0x01004e2d, false),
                key(0x01006587, true),
                key(0x01006587, false)
            ]
        );
        assert!(k.release().is_empty());
        assert!(k.commit("\u{3}\u{11}").is_empty());
    }
    #[test]
    fn rapid_copy_paste_cycles_never_press_window_shortcut_letters() {
        let mut k = Keyboard::new(true);
        for _ in 0..100 {
            let mut output = send(&mut k, "ControlLeft", "\u{11}", CTRL, true);
            for letter in ["c", "v"] {
                output.extend(send(&mut k, letter, letter, CTRL, true));
                output.extend(send(&mut k, letter, letter, CTRL, false));
            }
            output.extend(send(&mut k, "ControlLeft", "\u{11}", NONE, false));
            assert_eq!(
                output,
                vec![
                    key(COMMAND, true),
                    key(99, true),
                    key(99, false),
                    key(118, true),
                    key(118, false),
                    key(COMMAND, false)
                ]
            );
            assert!(k.release().is_empty());
        }
    }
    #[test]
    fn releasing_control_before_space_does_not_reintroduce_command() {
        let mut k = Keyboard::new(true);
        send(&mut k, "ControlLeft", "\u{11}", CTRL, true);
        send(&mut k, "Space", " ", CTRL, true);
        assert_eq!(
            send(&mut k, "ControlLeft", "\u{11}", NONE, false),
            vec![key(CONTROL, false)]
        );
        assert_eq!(
            send(&mut k, "Space", " ", NONE, false),
            vec![key(32, false)]
        );
        assert!(k.release().is_empty());
    }
    #[test]
    fn packet_is_exact_rfb_key_event_and_not_a_virtual_keycode() {
        assert_eq!(key(99, true).packet(), [4, 1, 0, 0, 0, 0, 0, 99]);
        assert_eq!(key(COMMAND, false).packet(), [4, 0, 0, 0, 0, 0, 255, 235]);
    }
}
