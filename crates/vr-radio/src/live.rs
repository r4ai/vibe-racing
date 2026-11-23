//! The live values of the race (`vr live --hz 60`), for the rules that are
//! checked while driving.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::child::{Retry, Supervised};
use crate::expr::Values;

#[derive(Deserialize)]
struct Line {
    values: Values,
}

/// Values as they change, and the process that tells them.
pub struct Feed {
    pub values: UnboundedReceiver<Values>,
    _child: Option<Supervised>,
}

impl Feed {
    /// Follows `command`; `None` when it is empty.
    pub fn start(command: &str, cwd: PathBuf) -> Option<Self> {
        let (tx, values) = unbounded_channel();
        let retry = Retry {
            // `vr serve` is being started at the same time.
            initial: Duration::from_secs(2),
            first: Duration::from_secs(1),
            max: Duration::from_secs(30),
        };
        let child = Supervised::spawn(
            command,
            cwd,
            retry,
            Some(Box::new(move |line| {
                match serde_json::from_str::<Line>(line) {
                    Ok(l) => tx.send(l.values).is_ok(),
                    Err(_) => !tx.is_closed(),
                }
            })),
        )?;
        Some(Self {
            values,
            _child: Some(child),
        })
    }

    /// Values from somewhere else.
    pub fn of(values: UnboundedReceiver<Values>) -> Self {
        Self {
            values,
            _child: None,
        }
    }
}
