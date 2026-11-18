use super::display_preferences::{Mode, View};
use super::*;

pub(super) fn register(app: &App, state: &Rc<RefCell<State>>) {
    let weak = app.as_weak();
    let shared = state.clone();
    app.on_display_mode_changed(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(mode) = Mode::from_index(index) else {
            return;
        };
        let result = (|| -> Result<()> {
            let mut state = shared.borrow_mut();
            let Some(id) = state.connection_profile.as_ref().map(|p| p.id) else {
                return Ok(());
            };
            state.release()?;
            state.pan_drag = None;
            state.display.set(
                id,
                View {
                    mode,
                    pan: (0.0, 0.0),
                },
            )?;
            state.display.save()?;
            drop(state);
            refresh_layout(&app, &shared)
        })();
        if let Err(error) = result {
            notify(&app, error);
        }
    });
    let weak = app.as_weak();
    let shared = state.clone();
    app.on_pan_step(move |dx, dy| {
        let Some(app) = weak.upgrade() else { return };
        let result = (|| -> Result<()> {
            let mut state = shared.borrow_mut();
            let Some(id) = state.connection_profile.as_ref().map(|p| p.id) else {
                return Ok(());
            };
            let mut view = state.display.view(id);
            view.pan = if dx == 0.0 && dy == 0.0 {
                (0.0, 0.0)
            } else {
                (
                    (view.pan.0 + dx).clamp(-1.0, 1.0),
                    (view.pan.1 + dy).clamp(-1.0, 1.0),
                )
            };
            state.display.set(id, view)?;
            state.display.save()?;
            drop(state);
            refresh_layout(&app, &shared)
        })();
        if let Err(error) = result {
            notify(&app, error);
        }
    });
    let weak = app.as_weak();
    let shared = state.clone();
    app.on_pan_pointer(move |x, y, event| {
        let Some(app) = weak.upgrade() else { return };
        let result = (|| -> Result<()> {
            let mut state = shared.borrow_mut();
            let Some(id) = state.connection_profile.as_ref().map(|p| p.id) else {
                return Ok(());
            };
            if event.kind == PointerEventKind::Down && event.button == PointerEventButton::Left {
                state.release()?;
                state.pan_drag = state
                    .viewport
                    .map(|viewport| (x, y, state.display.view(id).pan, viewport));
            } else if event.kind == PointerEventKind::Move {
                if let Some((start_x, start_y, pan, viewport)) = state.pan_drag {
                    let mut view = state.display.view(id);
                    view.pan = viewport.pan_after_drag(pan, x - start_x, y - start_y);
                    state.display.set(id, view)?;
                } else {
                    return Ok(());
                }
            } else if event.kind == PointerEventKind::Up || event.kind == PointerEventKind::Cancel {
                state.pan_drag = None;
                state.display.save()?;
            } else {
                return Ok(());
            }
            drop(state);
            refresh_layout(&app, &shared)
        })();
        if let Err(error) = result {
            notify(&app, error);
        }
    });
}
