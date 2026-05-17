mod chat;
mod home;
mod pages;
mod sessions;
mod tools;

use crate::app::{App, Route};
use ratatui::Frame;

pub fn render(frame: &mut Frame, app: &mut App) {
    let route = app.route().clone();
    match route {
        Route::Home => home::render(frame, app),
        Route::Chat => chat::render(frame, app),
        Route::Sessions => frame.render_widget(sessions::SessionsScreen::new(app), frame.area()),
        Route::Tools => tools::render(frame, app),
        Route::Missing(path) => {
            frame.render_widget(pages::MissingPage::new(app, &path), frame.area());
        }
    }
}

/// Caret coordinates relative to a wrapping text area of the given width.
///
/// Mirrors ratatui's `Wrap` only at the character level — close enough for
/// caret-at-end-of-input, which is the only insertion point the TUI supports.
pub(super) fn caret_xy(text: &str, width: u16) -> (u16, u16) {
    if width == 0 {
        return (0, 0);
    }
    let w = width as usize;
    let mut y: usize = 0;
    let mut last = "";
    let mut iter = text.split('\n').peekable();
    while let Some(line) = iter.next() {
        if iter.peek().is_none() {
            last = line;
            break;
        }
        let count = line.chars().count();
        let rows = if count == 0 { 1 } else { count.div_ceil(w) };
        y += rows;
    }
    let last_chars = last.chars().count();
    y += last_chars / w;
    let x = (last_chars % w) as u16;
    (x, y as u16)
}
