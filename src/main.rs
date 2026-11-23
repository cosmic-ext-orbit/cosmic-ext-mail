// SPDX-License-Identifier: GPL-3.0-only

mod app;
mod config;
mod features;
mod i18n;

fn main() -> cosmic::iced::Result {
    // Get the system's preferred languages.
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    // Enable localizations to be applied.
    i18n::init(&requested_languages);

    let settings = cosmic::app::Settings::default()
        .exit_on_close(false)
        .no_main_window(true);
    cosmic::app::run_single_instance::<app::AppModel>(settings, app::Flags)
}
