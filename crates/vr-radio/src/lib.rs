//! Voice radio to an ACP agent: push-to-talk → speech-to-text → agent →
//! sentence-by-sentence VOICEVOX read-back.

pub mod agent;
pub mod aloud;
pub mod app;
pub mod audio;
pub mod calls;
pub mod child;
pub mod chrome;
pub mod config;
pub mod context;
pub mod decide;
pub mod directive;
pub mod early;
pub mod engine;
pub mod expr;
pub mod fonts;
pub mod gate;
pub mod hotkey;
pub mod http;
pub mod inbox;
pub mod live;
pub mod policy;
pub mod resident;
pub mod rules;
pub mod sapi;
pub mod settings;
pub mod spotter;
pub mod stt;
pub mod text;
pub mod topic;
pub mod vad;
pub mod voicevox;
pub mod watch;
pub mod wav;
pub mod wheel;
