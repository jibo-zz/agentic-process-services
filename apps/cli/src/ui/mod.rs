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
/// Coordinate system MUST match `wrap_chars` below — that function is what
/// produces the visual lines, this one points at the caret within them.
/// Both use the same simple rule: count chars, break at `width`.
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

/// Hard-wraps `text` at `width` characters, returning one visible line per row.
/// Used for textareas where the caret position must match the rendered text:
/// `Paragraph::wrap(...)` does word-boundary wrapping which drifts away from
/// `caret_xy`'s character count and leaves the cursor behind the text.
pub(super) fn wrap_chars(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return vec![text.to_owned()];
    }
    let w = width as usize;
    let mut out: Vec<String> = Vec::new();
    for source_line in text.split('\n') {
        if source_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut buf = String::new();
        let mut count = 0;
        for c in source_line.chars() {
            if count == w {
                out.push(std::mem::take(&mut buf));
                count = 0;
            }
            buf.push(c);
            count += 1;
        }
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_and_wrap_agree_after_newline() {
        let text = "hello\nworld";
        let (x, y) = caret_xy(text, 80);
        let lines = wrap_chars(text, 80);
        assert_eq!((x, y), (5, 1));
        assert_eq!(lines, vec!["hello".to_owned(), "world".to_owned()]);
    }

    #[test]
    fn caret_and_wrap_agree_after_char_wrap() {
        // 14 chars, width 5 → wraps every 5 chars: rows are "aaaaa", "bbbbb", "cccc"
        let text = "aaaaabbbbbcccc";
        let (x, y) = caret_xy(text, 5);
        let lines = wrap_chars(text, 5);
        assert_eq!(
            lines,
            vec!["aaaaa".to_owned(), "bbbbb".to_owned(), "cccc".to_owned()]
        );
        // caret at end (char 14): row 2, col 4
        assert_eq!((x, y), (4, 2));
    }

    #[test]
    fn caret_at_exact_wrap_boundary() {
        // 10 chars, width 5 → two full rows. Caret is at start of row 2.
        let text = "aaaaabbbbb";
        let (x, y) = caret_xy(text, 5);
        let lines = wrap_chars(text, 5);
        assert_eq!(lines, vec!["aaaaa".to_owned(), "bbbbb".to_owned()]);
        assert_eq!((x, y), (0, 2));
    }
}
