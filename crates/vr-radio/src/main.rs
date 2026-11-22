use clap::Parser;
use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;
use shadcn_iced::prelude::*;
use vr_radio::app::{self, App, CONFIG, HOTKEY};
use vr_radio::config::Config;

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vr_radio=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let config = Config::parse();
    if let Some(question) = config.ask.clone() {
        let code = ask_once(config, question);
        std::process::exit(code);
    }

    // The manager must live on the event-loop thread for the whole run.
    let manager = GlobalHotKeyManager::new();
    let status = match (&manager, config.hotkey.parse::<HotKey>()) {
        (Ok(m), Ok(key)) => m
            .register(key)
            .map(|()| config.hotkey.clone())
            .map_err(|e| e.to_string()),
        (Err(e), _) => Err(e.to_string()),
        (_, Err(e)) => Err(format!("{}: {e}", config.hotkey)),
    };
    if let Err(e) = &status {
        tracing::warn!("push-to-talk hotkey unavailable: {e}");
    }
    let _ = HOTKEY.set(status);
    // Before `.shadcn_iced()`, which reads the family for the default font.
    let family = config.font.clone().filter(|f| !f.is_empty());
    let family = family.map_or(vr_radio::fonts::FAMILY, |f| Box::leak(f.into_boxed_str()));
    shadcn_iced::font::set_sans(family);
    let _ = CONFIG.set(config);

    let result = iced::application(App::boot, App::update, App::view)
        .title("VR Radio")
        .subscription(app::subscription)
        .theme(|_: &App| shadcn_iced::theme::zinc(Mode::Dark).build())
        .window(iced::window::Settings {
            size: iced::Size::new(1040.0, 680.0),
            min_size: Some(iced::Size::new(760.0, 520.0)),
            // Native frame; on Windows `chrome` hides only the caption.
            ..Default::default()
        })
        .shadcn_iced()
        .font(vr_radio::fonts::FILES[0])
        .font(vr_radio::fonts::FILES[1])
        .font(vr_radio::fonts::FILES[2])
        .run();
    drop(manager);
    result
}

/// `--ask`: one typed question through the real agent and VOICEVOX.
fn ask_once(config: Config, question: String) -> i32 {
    use vr_radio::engine::{Command, Event, Outcome};
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async move {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
        match app::start_engine(&config, cmd_rx, ev_tx).await {
            Ok(v) => eprintln!("VOICEVOX: {} voices", v.len()),
            Err(e) => eprintln!("VOICEVOX: {e}"),
        }
        let mut code = None;
        let mut asked = false;
        let mut spoke = false;
        loop {
            // After the answer, wait for the speech (or give up if none starts).
            let wait = if code.is_some() && !spoke { 3 } else { 600 };
            let ev = match tokio::time::timeout(std::time::Duration::from_secs(wait), ev_rx.recv())
                .await
            {
                Ok(Some(ev)) => ev,
                _ => break,
            };
            match &ev {
                Event::Level(_) => continue,
                Event::Chunk { text, .. } => {
                    print!("{text}");
                    continue;
                }
                _ => eprintln!("{ev:?}"),
            }
            match ev {
                Event::AgentReady { .. } if !asked => {
                    asked = true;
                    let _ = cmd_tx.send(Command::Ask(question.clone()));
                }
                Event::AgentClosed { .. } => return 1,
                Event::TurnDone { outcome, .. } => {
                    println!();
                    code = Some(if outcome == Outcome::Answered { 0 } else { 1 });
                }
                Event::Speaking(true) => spoke = true,
                Event::Speaking(false) if code.is_some() => break,
                _ => {}
            }
        }
        code.unwrap_or(1)
    })
}
