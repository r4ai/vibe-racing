//! Voice radio to an ACP agent: push-to-talk → speech-to-text → agent →
//! sentence-by-sentence VOICEVOX read-back.

pub mod agent;
pub mod app;
pub mod audio;
pub mod child;
pub mod chrome;
pub mod config;
pub mod context;
pub mod directive;
pub mod early;
pub mod engine;
pub mod fonts;
pub mod hotkey;
pub mod http;
pub mod policy;
pub mod resident;
pub mod sapi;
pub mod settings;
pub mod stt;
pub mod text;
pub mod vad;
pub mod voicevox;
pub mod watch;
pub mod wav;
pub mod wheel;
