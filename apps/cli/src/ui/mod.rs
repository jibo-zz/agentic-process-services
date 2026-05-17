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
        Route::Home => frame.render_widget(home::HomeScreen::new(app), frame.area()),
        Route::Chat => frame.render_widget(chat::ChatScreen::new(app), frame.area()),
        Route::Sessions => frame.render_widget(sessions::SessionsScreen::new(app), frame.area()),
        Route::Tools => frame.render_widget(tools::ToolsScreen::new(app), frame.area()),
        Route::Missing(path) => {
            frame.render_widget(pages::MissingPage::new(app, &path), frame.area());
        }
    }
}
