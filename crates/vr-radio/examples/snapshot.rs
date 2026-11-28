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
use vr_radio::app::{App, Message, SettingsPage};
use vr_radio::engine::Event;
use vr_radio::voicevox::Voice;

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
    render_at(Size::new(1040.0, 680.0), name, app, theme, click);
}

fn render_at(size: Size, name: &str, app: &App, theme: &Theme, click: Option<&str>) {
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

    // An error over the log: the log and the input stay where they were.
    let mut failed = App::sample();
    failed.handle(Event::Error("VOICEVOX に接続できません".into()));
    render("error", &failed, &theme, None);

    // A long race: the oldest exchanges are no longer kept.
    let mut long = App::new();
    for turn in 1..=260 {
        long.handle(Event::TurnStarted { turn });
        long.handle(Event::Transcript {
            turn,
            text: format!("{turn}周目、前との差は？"),
            ms: 400,
        });
        long.handle(Event::Chunk {
            turn,
            text: "前まで 0.6。毎周 0.4 詰めてる。".into(),
        });
        long.handle(Event::TurnDone {
            turn,
            outcome: vr_radio::engine::Outcome::Answered,
            ms: 2000,
        });
    }
    let _ = long.update(Message::ScrollToEnd);
    render("long", &long, &theme, None);

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
    settings_open.assign_sample();
    let _ = settings_open.update(Message::Settings(true));
    render("settings", &settings_open, &theme, None);

    // Everything in the sheet, with rules the driver asked for.
    let mut rules = vr_radio::rules::Rules::default();
    rules.set_hold(
        vr_radio::rules::Scope::All,
        Some("brake > 5 or abs(lat_g) > 1.2".into()),
    );
    rules.note("順位は聞かれたときだけ言う");
    for (when, say) in [
        ("gap_behind < 1", Some("後ろ{gap_behind}秒".to_string())),
        ("stop_in == 0", None),
    ] {
        rules.watch(vr_radio::rules::Watch {
            when: when.into(),
            planned: say.is_none(),
            act: say.is_none().then(|| "ボックスと言う".into()),
            say,
            ..Default::default()
        });
    }
    settings_open.handle(Event::Rules(rules));
    let _ = settings_open.update(Message::Capture(vr_radio::app::Capture::Wheel));
    render("settings-capture", &settings_open, &theme, None);
    // Every page of the settings, with the rules above.
    let _ = settings_open.update(Message::Settings(true));
    for (name, page) in [
        ("talk", SettingsPage::Talk),
        ("voice", SettingsPage::Voice),
        ("calls", SettingsPage::Calls),
        ("answers", SettingsPage::Answers),
        ("orders", SettingsPage::Orders),
        ("agent", SettingsPage::Agent),
        ("app", SettingsPage::App),
    ] {
        let _ = settings_open.update(Message::SettingsPage(page));
        render(&format!("settings-{name}"), &settings_open, &theme, None);
    }
    // A short window: the page scrolls, the sidebar stays.
    let _ = settings_open.update(Message::SettingsPage(SettingsPage::Calls));
    let short = Size::new(800.0, 480.0);
    render_at(short, "settings-short", &settings_open, &theme, None);
    let _ = settings_open.update(Message::SettingsPage(SettingsPage::Voice));

    // Right after the start: nothing can be used yet.
    let mut starting = App::new();
    starting.handle(Event::Telemetry(false));
    starting.handle(Event::Microphone(false));
    starting.handle(Event::Speakers(true));
    let _ = starting.update(Message::Settings(true));
    render("settings-starting", &starting, &theme, None);
    let _ = starting.update(Message::Settings(false));
    render("starting", &starting, &theme, None);
    render(
        "settings-select",
        &settings_open,
        &theme,
        Some("ずんだもん"),
    );

    // As many characters as VOICEVOX has: the list is taller than the window.
    let mut many = App::new();
    let voices = (0..43).map(|i| Voice {
        id: i + 3,
        speaker: match i {
            0 => "四国めたん".into(),
            _ => format!("キャラクター{i}"),
        },
        style: "ノーマル".into(),
    });
    let _ = many.update(Message::Voices(Ok(voices.collect())));
    let _ = many.update(Message::Settings(true));
    let _ = many.update(Message::SettingsPage(SettingsPage::Voice));
    render("settings-long-select", &many, &theme, Some("四国めたん"));
}
