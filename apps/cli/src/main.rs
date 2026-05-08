use color_eyre::eyre::Result;

mod app;
mod tui;
mod ui;

fn main() -> Result<()> {
    color_eyre::install()?;
    tui::install_panic_hook();

    tui::run()
}
