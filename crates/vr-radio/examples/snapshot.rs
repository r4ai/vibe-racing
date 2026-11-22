//! Renders the window headlessly to PNG files, for reviewing the UI
//! without a microphone, an agent or VOICEVOX.
//!
//! ```powershell
//! $env:ICED_TEST_BACKEND = "tiny-skia"; cargo run -p vr-radio --example snapshot
//! ```
//!
//! Writes `target/snapshots/vr-radio-<scene>.png`.

use iced::{Size, Theme};
use iced_test::simulator::Simulator;
use shadcn_iced::Mode;
use vr_radio::app::{App, Message};
use vr_radio::engine::Event;

fn settings() -> iced::Settings {
    let mut fonts = shadcn_iced::font::all();
    fonts.extend(
        vr_radio::fonts::FILES
            .iter()
            .map(|f| std::borrow::Cow::Borrowed(*f)),
    );
    shadcn_iced::font::set_sans(vr_radio::fonts::FAMILY);
    iced::Settings {
        fonts,
        default_font: shadcn_iced::font::sans(),
        ..Default::default()
    }
}

fn redraw(ui: &mut Simulator<'_, Message>) {
    let _ = ui.simulate([iced::Event::Window(iced::window::Event::RedrawRequested(
        iced::time::Instant::now(),
    ))]);
}

/// Renders `app`, optionally clicking the text `click` first.
fn render(name: &str, app: &App, theme: &Theme, click: Option<&str>) {
    let size = Size::new(1040.0, 680.0);
    let mut ui = Simulator::with_size(settings(), size, app.view());
    redraw(&mut ui);
    if let Some(label) = click
        && let Err(e) = ui.click(label)
    {
        eprintln!("click {label:?}: {e}");
    }
    // Let opening animations finish.
    std::thread::sleep(std::time::Duration::from_millis(400));
    redraw(&mut ui);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let path = out.join(format!("vr-radio-{name}.png"));
    let _ = std::fs::remove_file(&path);
    let written = out.join(format!("vr-radio-{name}-tiny-skia.png"));
    let _ = std::fs::remove_file(&written);
    ui.snapshot(theme).unwrap().matches_image(&path).unwrap();
    println!("{}", written.display());
}

fn main() {
    let theme = shadcn_iced::theme::zinc(Mode::Dark).build();

    render("idle", &App::new(), &theme, None);
    render("conversation", &App::sample(), &theme, None);

    let mut recording = App::sample();
    recording.handle(Event::TurnDone {
        turn: 2,
        outcome: vr_radio::engine::Outcome::Answered,
        ms: 2900,
    });
    recording.handle(Event::Recording(true));
    recording.handle(Event::Level(0.12));
    render("recording", &recording, &theme, None);

    let mut settings_open = App::sample();
    let _ = settings_open.update(Message::Settings(true));
    render("settings", &settings_open, &theme, None);
    render(
        "settings-select",
        &settings_open,
        &theme,
        Some("ずんだもん"),
    );
}
