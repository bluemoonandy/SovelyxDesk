//! Window-local physical-key metadata. Slint still handles focus and consumes the key event.
use std::{cell::RefCell, rc::Rc};

use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use winit::{
    event::WindowEvent,
    keyboard::{Key, PhysicalKey},
    platform::scancode::PhysicalKeyExtScancode,
};

pub enum Pending {
    Key {
        physical: Option<String>,
        scancode: Option<u32>,
        text: Option<String>,
        synthetic: bool,
    },
}

pub fn observe(
    window: &slint::Window,
    mut release: impl FnMut(&'static str) + 'static,
    mut commit: impl FnMut(&str) -> bool + 'static,
) -> Rc<RefCell<Option<Pending>>> {
    let pending = Rc::new(RefCell::new(None));
    let observed = pending.clone();
    window.on_winit_window_event(move |_, event| {
        *observed.borrow_mut() = match event {
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => Some(Pending::Key {
                physical: match event.physical_key {
                    PhysicalKey::Code(code) => Some(format!("{code:?}")),
                    PhysicalKey::Unidentified(_) => None,
                },
                scancode: event.physical_key.to_scancode(),
                // The logical character distinguishes Ctrl+C from named modifier codes
                // in Slint's overlapping C0 namespace. Never serialize this text to logs.
                text: match &event.logical_key {
                    Key::Character(text) => Some(text.to_string()),
                    _ => None,
                },
                synthetic: *is_synthetic,
            }),
            _ => None,
        };
        // FocusScope does not forward composition commits to key-pressed callbacks.
        // Consume a commit only while the caller owns the remote canvas focus.
        if let WindowEvent::Ime(winit::event::Ime::Commit(text)) = event
            && commit(text)
        {
            return EventResult::PreventDefault;
        }
        match event {
            WindowEvent::Focused(false) => release("window_focus_lost"),
            WindowEvent::Occluded(true) => release("window_occluded"),
            WindowEvent::Resized(size) if size.width == 0 || size.height == 0 => {
                release("window_minimized")
            }
            _ => (),
        }
        EventResult::Propagate
    });
    pending
}
