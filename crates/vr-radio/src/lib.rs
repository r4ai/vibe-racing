//! Voice radio to an ACP agent: push-to-talk → speech-to-text → agent →
//! sentence-by-sentence VOICEVOX read-back.

pub mod agent;
pub mod app;
pub mod audio;
pub mod chrome;
pub mod config;
pub mod engine;
pub mod fonts;
pub mod http;
pub mod policy;
pub mod stt;
pub mod text;
pub mod voicevox;
pub mod wav;
