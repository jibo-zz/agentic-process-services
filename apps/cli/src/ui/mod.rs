mod home;
mod pages;

use crate::app::{App, Route};
use ratatui::Frame;

pub fn render(frame: &mut Frame, app: &App) {
    match app.route() {
        Route::Home => frame.render_widget(home::HomeScreen::new(app), frame.area()),
        Route::About => frame.render_widget(pages::AboutPage::new(app), frame.area()),
        Route::Settings => frame.render_widget(pages::SettingsPage::new(app), frame.area()),
        Route::Missing(path) => {
            frame.render_widget(pages::MissingPage::new(app, path), frame.area());
        }
    }
}
