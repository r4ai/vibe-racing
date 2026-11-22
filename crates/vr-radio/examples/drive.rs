//! The radio without a window, microphone or speakers: lines on stdin are
//! questions, events go to stdout. For trying the watches, the holds and the
//! planning against a race (`vr serve --game synthetic`).
//!
//! `cargo run -p vr-radio --example drive -- --agent "..."`

use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use tokio::sync::mpsc::unbounded_channel;
use vr_radio::audio::{Output, Recorder, Recording};
use vr_radio::config::Config;
use vr_radio::engine::{self, Command, Event};
use vr_radio::live::Feed;
use vr_radio::voicevox::Voicevox;
use vr_radio::watch::Follow;
use vr_radio::{app, gate};

struct NoMic;

impl Recorder for NoMic {
    fn start(&self) {}
    fn stop(&self) -> Recording {
        Recording::default()
    }
    fn level(&self) -> f32 {
        0.0
    }
    fn peek(&self, _: usize) -> Recording {
        Recording::default()
    }
}

/// Takes as long as the speakers would.
struct Silent {
    until: Mutex<Instant>,
    began: Instant,
}

impl Output for Silent {
    fn play(&self, wav: Vec<u8>) {
        let len = gate::length(&wav);
        let mut until = self.until.lock().unwrap();
        *until = (*until).max(Instant::now()) + len;
        println!(
            "{:7.2} [speaks {:.1}s]",
            self.began.elapsed().as_secs_f64(),
            len.as_secs_f64()
        );
    }
    fn stop(&self) {
        *self.until.lock().unwrap() = Instant::now();
    }
    fn is_idle(&self) -> bool {
        Instant::now() >= *self.until.lock().unwrap()
    }
}

#[tokio::main]
async fn main() {
    let config = Config::parse();
    let began = Instant::now();
    let output = Arc::new(Silent {
        until: Mutex::new(began),
        began,
    });
    let synth = Arc::new(Voicevox::new(config.voicevox_url.clone(), config.speed));
    let mut deps = app::deps(&config, Arc::new(NoMic), output, synth);
    deps.watch = Follow::start(&config.watch, config.agent_cwd());
    deps.live = Feed::start(&config.live, config.agent_cwd());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (ev_tx, mut ev_rx) = unbounded_channel();
    tokio::spawn(engine::run(deps, cmd_rx, ev_tx));
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            // `# 20` waits.
            match line.strip_prefix('#') {
                Some(s) => {
                    let s = s.trim().parse().unwrap_or(0.0);
                    std::thread::sleep(Duration::from_secs_f64(s));
                }
                None => {
                    let _ = cmd_tx.send(Command::Ask(line));
                }
            }
        }
        std::process::exit(0);
    });
    while let Some(e) = ev_rx.recv().await {
        let t = began.elapsed().as_secs_f64();
        match e {
            Event::Level(_) | Event::Usage { .. } | Event::Cost { .. } | Event::Speaking(_) => {}
            Event::Chunk { turn, text } => println!("{t:7.2} {turn}> {text}"),
            e => println!("{t:7.2} {e:?}"),
        }
    }
}
