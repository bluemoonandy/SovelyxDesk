use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{COMMAND, CONTROL, OPTION, SHIFT, keyboard::Key};

pub struct Trace {
    enabled: bool,
    modifiers: [bool; 4],
    shortcuts: BTreeSet<u32>,
}

impl Default for Trace {
    fn default() -> Self {
        Self::new(std::env::var("MACCANVAS_KEYBOARD_DIAGNOSTICS").as_deref() == Ok("1"))
    }
}

impl Trace {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            modifiers: [false; 4],
            shortcuts: BTreeSet::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn input(
        &self,
        key: Key,
        modifiers: [bool; 4],
        physical: Option<&str>,
        scancode: Option<u32>,
        repeat: bool,
    ) -> Option<Value> {
        if !self.enabled
            || !(modifier(key.keysym).is_some()
                || (shortcut(key.keysym)
                    && (modifiers[1] || modifiers[3] || self.shortcuts.contains(&key.keysym))))
        {
            return None;
        }
        Some(
            json!({"input.stage":"local_event", "input.physical_key":physical,
            "input.scancode":scancode, "input.virtual_key":null,
            "input.keysym":key.keysym,"input.down":key.down,"input.repeat":repeat,
            "input.modifiers":modifiers}),
        )
    }

    /// Log only modifier transitions and a small explicit shortcut allowlist.
    /// Ordinary text and IME commits never enter the diagnostic record.
    pub fn sent(&mut self, key: Key, stage: &'static str) -> Option<Value> {
        if !self.enabled {
            return None;
        }
        let record = if let Some(index) = modifier(key.keysym) {
            self.modifiers[index] = key.down;
            true
        } else if key.down && shortcut(key.keysym) && (self.modifiers[1] || self.modifiers[3]) {
            self.shortcuts.insert(key.keysym);
            true
        } else if !key.down {
            self.shortcuts.remove(&key.keysym)
        } else {
            false
        };
        record.then(|| {
            json!({"input.stage":stage,"input.keysym":key.keysym,
            "input.down":key.down,"input.modifiers":self.modifiers,"input.rfb_packet":key.packet()})
        })
    }
}

pub fn modifier(symbol: u32) -> Option<usize> {
    [SHIFT, CONTROL, OPTION, COMMAND]
        .iter()
        .position(|s| *s == symbol)
}

fn shortcut(symbol: u32) -> bool {
    matches!(
        symbol,
        32 | 65
            | 67
            | 70
            | 81
            | 83
            | 86
            | 87
            | 88
            | 90
            | 97
            | 99
            | 102
            | 113
            | 115
            | 118
            | 119
            | 120
            | 122
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_mode_and_ordinary_text_never_produce_records() {
        for enabled in [false, true] {
            let mut trace = Trace::new(enabled);
            for keysym in [97, 99, 118, 0x01004e2d] {
                let key = Key { keysym, down: true };
                assert!(
                    trace
                        .input(key, [false; 4], Some("KeyC"), Some(46), false)
                        .is_none()
                );
                assert!(trace.sent(key, "test").is_none());
            }
            if !enabled {
                assert!(
                    trace
                        .sent(
                            Key {
                                keysym: COMMAND,
                                down: true
                            },
                            "test"
                        )
                        .is_none()
                );
            }
        }
    }
    #[test]
    fn shortcut_release_is_logged_even_after_control_release() {
        let mut trace = Trace::new(true);
        trace
            .sent(
                Key {
                    keysym: COMMAND,
                    down: true,
                },
                "test",
            )
            .unwrap();
        let record = trace
            .sent(
                Key {
                    keysym: 99,
                    down: true,
                },
                "test",
            )
            .unwrap();
        assert_eq!(record["input.rfb_packet"], json!([4, 1, 0, 0, 0, 0, 0, 99]));
        trace
            .sent(
                Key {
                    keysym: COMMAND,
                    down: false,
                },
                "test",
            )
            .unwrap();
        assert!(
            trace
                .sent(
                    Key {
                        keysym: 99,
                        down: false
                    },
                    "test"
                )
                .is_some()
        );
        assert!(
            trace
                .sent(
                    Key {
                        keysym: 99,
                        down: true
                    },
                    "test"
                )
                .is_none()
        );
    }
}
