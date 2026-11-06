//! The window has no frame of Windows and no title row: the whole picture
//! moves it (metalamp's `frameless.rs`, itself metalmpv's without
//! fullscreen). It is still resizable for Windows (`WS_SIZEBOX` and
//! `WS_MAXIMIZEBOX` give Aero Snap, Win+arrows and maximising to the work
//! area), but winit gives the whole window to the client area, so the edges
//! resize it through strips of the app's own. No window region (qrec's way
//! to round corners): DWM keeps its shadow, and the corners are rounded by
//! DWM itself on Windows 11 (`win::round_corners`).

use egui::{CursorIcon, ResizeDirection as D, Sense, Ui, vec2};

/// The width of the strips along the window's edges that resize it.
pub const EDGE: f32 = 5.0;

/// A free area that moves the window when dragged (the title row, the
/// player bar's free space); `sense` adds what else it answers (clicks).
/// Registered before the widgets on it, which take their clicks first. It
/// leaves the resize strips along the window's edges alone (a later widget
/// would cover them).
pub fn drag_area(ui: &mut Ui, rect: egui::Rect, id: egui::Id, sense: Sense) -> egui::Response {
    let ctx = ui.ctx().clone();
    let response = ui.interact(inside_edges(&ctx, rect), id, sense | Sense::drag());
    if response.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    response
}

/// `rect` less the resize strips along the window's edges (all of it
/// while maximized): where a widget that senses drags may take presses,
/// since one registered after the strips covers them (the playlist's rows,
/// which drag to another place, took the right edge).
pub fn inside_edges(ctx: &egui::Context, rect: egui::Rect) -> egui::Rect {
    if is_maximized(ctx) {
        rect
    } else {
        rect.intersect(ctx.content_rect().shrink(EDGE))
    }
}

/// Strips along the edges and squares at the corners hand a drag to
/// Windows (`ViewportCommand::BeginResize`), as a sizing frame would.
/// Placed before the panels, so that their widgets near an edge still take
/// clicks first. Not while maximized. Without `vertical` (the player
/// alone, as high as its bar) only the left and right edges.
pub fn resize_edges(ui: &mut Ui, vertical: bool) {
    const CORNER: f32 = 14.0;
    let r = ui.ctx().content_rect();
    let grips = [
        (
            D::NorthWest,
            egui::Rect::from_min_size(r.left_top(), vec2(CORNER, CORNER)),
            CursorIcon::ResizeNwSe,
        ),
        (
            D::NorthEast,
            egui::Rect::from_min_size(r.right_top() - vec2(CORNER, 0.0), vec2(CORNER, CORNER)),
            CursorIcon::ResizeNeSw,
        ),
        (
            D::SouthWest,
            egui::Rect::from_min_size(r.left_bottom() - vec2(0.0, CORNER), vec2(CORNER, CORNER)),
            CursorIcon::ResizeNeSw,
        ),
        (
            D::SouthEast,
            egui::Rect::from_min_size(
                r.right_bottom() - vec2(CORNER, CORNER),
                vec2(CORNER, CORNER),
            ),
            CursorIcon::ResizeNwSe,
        ),
        (
            D::North,
            egui::Rect::from_min_max(r.left_top(), egui::pos2(r.right(), r.top() + EDGE)),
            CursorIcon::ResizeVertical,
        ),
        (
            D::South,
            egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - EDGE), r.right_bottom()),
            CursorIcon::ResizeVertical,
        ),
        (
            D::West,
            egui::Rect::from_min_max(r.left_top(), egui::pos2(r.left() + EDGE, r.bottom())),
            CursorIcon::ResizeHorizontal,
        ),
        (
            D::East,
            egui::Rect::from_min_max(egui::pos2(r.right() - EDGE, r.top()), r.right_bottom()),
            CursorIcon::ResizeHorizontal,
        ),
    ];
    // Corners first: where a corner and an edge overlap, the corner wins.
    let pointer = ui.ctx().pointer_hover_pos();
    let Some((direction, rect, icon)) = grips
        .into_iter()
        .filter(|(d, _, _)| vertical || matches!(d, D::West | D::East))
        .find(|(_, rect, _)| pointer.is_some_and(|p| rect.contains(p)))
    else {
        return;
    };
    let response = ui.interact(
        rect,
        ui.id().with(("resize", direction as u8)),
        Sense::drag(),
    );
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(icon);
    }
    if response.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

pub fn is_maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}
