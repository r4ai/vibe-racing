//! Two long-lived ACP sessions with the agent (Claude Code, Codex...): the
//! radio, and the desk, where the engineer looks into something at length.
//!
//! The process and the sessions stay up between questions, so a question
//! only costs a `session/prompt` round trip. Replies stream back as chunks.
//! What the desk reads stays out of the radio's session, and nobody waits for
//! it: the driver can ask about something else meanwhile.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, CloseSessionRequest, ContentBlock, Implementation, InitializeRequest,
    NewSessionRequest, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelectOptions, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, TextContent, ToolCallStatus,
};
use agent_client_protocol::{Agent, Client, ConnectTo};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::policy::{self, Permissions};

pub type Turn = u64;

/// Claude Code's mode that asks the client for permission.
const ASK_MODE: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCmd {
    /// Starts a turn. A turn still running is cancelled first.
    Prompt {
        turn: Turn,
        text: String,
        /// The driver's words: they reach the agent even when a newer prompt
        /// overtakes them.
        keep: bool,
    },
    /// Cancels the running turn, if any.
    Cancel,
    /// Changes options of the agent, also of the sessions to come.
    Set(Vec<(String, String)>),
}

/// What an option of the agent is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Model,
    Effort,
    Speed,
}

/// A choice the agent offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOption {
    pub id: String,
    pub kind: OptionKind,
    pub value: String,
    /// Values and what they are called.
    pub choices: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    Ready {
        name: String,
    },
    /// A new session: it knows nothing of what was said.
    Renewed,
    /// What can be chosen, and what is.
    Options(Vec<AgentOption>),
    Chunk {
        turn: Turn,
        text: String,
    },
    Tool {
        turn: Turn,
        id: String,
        title: Option<String>,
        state: ToolState,
    },
    Permission {
        turn: Turn,
        title: String,
        allowed: bool,
    },
    TurnEnd {
        turn: Turn,
        stop: String,
        usage: Option<TokenUsage>,
    },
    /// What the session has cost so far, when the agent says.
    Cost {
        turn: Turn,
        amount: f64,
        currency: String,
    },
    TurnFailed {
        turn: Turn,
        error: String,
    },
    /// The connection ended; no more events follow.
    Closed {
        error: Option<String>,
    },
}

/// How a session is set up.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub cwd: PathBuf,
    pub permissions: Permissions,
    /// Options of the agent, e.g. `("model", "haiku")`.
    pub options: Vec<(String, String)>,
    /// The rules, sent along with the first prompt of a session: a prompt of
    /// their own would cost as much as a question.
    pub primer: Option<String>,
    /// The rules of the desk, and the options that differ there.
    pub desk_primer: Option<String>,
    pub desk_options: Vec<(String, String)>,
    pub renew: Renew,
}

/// When a session is replaced by a new one. What was said before is read from
/// the cache at a tenth of the price, also after 20 minutes: a session is kept
/// as long as that is cheap. In an hour of racing it once grew to 450 000
/// tokens a prompt, and then was compacted, which lost the cache and more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Renew {
    /// Prompts of one session; 0: no limit.
    pub turns: u32,
    /// Tokens of one prompt, from which on the next begins anew; 0: no limit.
    pub tokens: u64,
}

impl Renew {
    pub const NEVER: Self = Self {
        turns: 0,
        tokens: 0,
    };

    fn due(self, turns: u32, tokens: u64) -> bool {
        (self.turns > 0 && turns >= self.turns) || (self.tokens > 0 && tokens >= self.tokens)
    }
}

/// The two ends of a running session. Dropping it ends the session and, by
/// cancelling its task, the agent process, even while it is still starting.
pub struct AgentLink {
    pub tx: UnboundedSender<AgentCmd>,
    /// The desk. Its turns are told by their numbers.
    pub desk: UnboundedSender<AgentCmd>,
    pub rx: UnboundedReceiver<AgentEvent>,
    task: Option<tokio::task::AbortHandle>,
}

impl AgentLink {
    /// Runs the session on the current tokio runtime.
    pub fn spawn(agent: impl ConnectTo<Client> + 'static, config: SessionConfig) -> Self {
        let (tx, cmds) = unbounded_channel();
        let (desk, desk_cmds) = unbounded_channel();
        let (events, rx) = unbounded_channel();
        let task = tokio::spawn(run(agent, config, cmds, desk_cmds, events));
        Self {
            tx,
            desk,
            rx,
            task: Some(task.abort_handle()),
        }
    }

    /// A link to an agent that could not be started.
    pub fn failed(error: String) -> Self {
        let (tx, _) = unbounded_channel();
        let (events, rx) = unbounded_channel();
        let _ = events.send(AgentEvent::Closed { error: Some(error) });
        Self {
            desk: tx.clone(),
            tx,
            rx,
            task: None,
        }
    }
}

impl Drop for AgentLink {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// What the protocol handlers pass to the session loop.
enum Inbound {
    Update(Box<SessionUpdate>),
    Permission { title: String, allowed: bool },
}

type PromptFuture =
    Pin<Box<dyn Future<Output = agent_client_protocol::Result<PromptResponse>> + Send>>;

fn tool_state(status: Option<ToolCallStatus>) -> ToolState {
    match status {
        Some(ToolCallStatus::Completed) => ToolState::Done,
        Some(ToolCallStatus::Failed) => ToolState::Failed,
        _ => ToolState::Running,
    }
}

/// Maps a session update of `turn` to an event (`None` for updates the radio ignores).
fn map_update(turn: Turn, update: SessionUpdate) -> Option<AgentEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(t) => Some(AgentEvent::Chunk { turn, text: t.text }),
            _ => None,
        },
        SessionUpdate::ToolCall(call) => Some(AgentEvent::Tool {
            turn,
            id: call.tool_call_id.to_string(),
            title: Some(call.title),
            state: tool_state(Some(call.status)),
        }),
        SessionUpdate::ToolCallUpdate(update) => Some(AgentEvent::Tool {
            turn,
            id: update.tool_call_id.to_string(),
            title: update.fields.title,
            state: tool_state(update.fields.status),
        }),
        SessionUpdate::UsageUpdate(u) => u.cost.map(|c| AgentEvent::Cost {
            turn,
            amount: c.amount,
            currency: c.currency,
        }),
        _ => None,
    }
}

/// Tokens of one turn, as the agent reports them (ACP, unstable).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub total: u64,
    /// Not read from the cache.
    pub input: u64,
    pub cached_read: u64,
    pub cached_write: u64,
    /// Including the thought.
    pub output: u64,
    pub thought: u64,
}

impl From<agent_client_protocol::v1::schema::Usage> for TokenUsage {
    fn from(u: agent_client_protocol::v1::schema::Usage) -> Self {
        Self {
            total: u.total_tokens,
            input: u.input_tokens,
            cached_read: u.cached_read_tokens.unwrap_or(0),
            cached_write: u.cached_write_tokens.unwrap_or(0),
            output: u.output_tokens,
            thought: u.thought_tokens.unwrap_or(0),
        }
    }
}

/// A prompt that waits for the turn before it to end.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Prompt {
    turn: Turn,
    text: String,
    keep: bool,
}

impl Prompt {
    /// Takes the place of the one that waited, with its words if they count.
    fn after(self, earlier: Option<Prompt>) -> Prompt {
        match earlier {
            Some(e) if e.keep => Prompt {
                text: format!("{}\n\n{}", e.text, self.text),
                keep: true,
                ..self
            },
            _ => self,
        }
    }
}

/// The rules the agent has not read yet, then the question.
fn with_primer(primer: Option<String>, text: String) -> String {
    match primer {
        Some(p) => format!("{p}\n\n---\n\n{text}"),
        None => text,
    }
}

/// The choices among the agent's options.
fn choices(options: &[SessionConfigOption]) -> Vec<AgentOption> {
    options
        .iter()
        .filter_map(|o| {
            let id = o.id.to_string();
            let kind = match &o.category {
                Some(SessionConfigOptionCategory::Model) => OptionKind::Model,
                Some(SessionConfigOptionCategory::ThoughtLevel) => OptionKind::Effort,
                _ if id.contains("fast") => OptionKind::Speed,
                _ => return None,
            };
            let SessionConfigKind::Select(select) = &o.kind else {
                return None;
            };
            let all = match &select.options {
                SessionConfigSelectOptions::Ungrouped(o) => o.clone(),
                SessionConfigSelectOptions::Grouped(g) => {
                    g.iter().flat_map(|g| g.options.clone()).collect()
                }
                _ => return None,
            };
            Some(AgentOption {
                id,
                kind,
                value: select.current_value.to_string(),
                choices: all
                    .into_iter()
                    .map(|c| (c.value.to_string(), c.name))
                    .collect(),
            })
        })
        .collect()
}

/// The same options with others in their place.
pub fn merged(base: &[(String, String)], over: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = base.to_vec();
    for (id, value) in over {
        match out.iter_mut().find(|(k, _)| k == id) {
            Some(o) => o.1.clone_from(value),
            None => out.push((id.clone(), value.clone())),
        }
    }
    out
}

/// Options of the sessions to come.
type Shared = Arc<Mutex<Vec<(String, String)>>>;
type SessionFuture = Pin<
    Box<dyn Future<Output = agent_client_protocol::Result<(SessionId, Vec<AgentOption>)>> + Send>,
>;

/// Sets one option; what can be chosen then, `None` when it was refused.
async fn set_option(
    cx: &ConnectionTo<Agent>,
    session: &SessionId,
    id: &str,
    value: &str,
) -> Option<Vec<AgentOption>> {
    let req = SetSessionConfigOptionRequest::new(
        session.clone(),
        id.to_string(),
        SessionConfigOptionValue::value_id(value.to_string()),
    );
    match cx.send_request(req).block_task().await {
        Ok(r) => Some(choices(&r.config_options)),
        Err(e) => {
            tracing::warn!("agent option {id}={value}: {e}");
            None
        }
    }
}

/// A new session, set up.
async fn open(
    cx: ConnectionTo<Agent>,
    cwd: PathBuf,
    permissions: Permissions,
    mut options: Vec<(String, String)>,
) -> agent_client_protocol::Result<(SessionId, Vec<AgentOption>)> {
    let session = cx
        .send_request(NewSessionRequest::new(cwd))
        .block_task()
        .await?;
    let session_id = session.session_id;
    let mut offered = choices(&session.config_options.unwrap_or_default());
    // Modes in which the agent decides by itself would bypass the
    // policy: ask, and the policy answers.
    let modes = session.modes.map(|m| m.available_modes).unwrap_or_default();
    if permissions != Permissions::All && modes.iter().any(|m| m.id.to_string() == ASK_MODE) {
        let req = SetSessionModeRequest::new(session_id.clone(), ASK_MODE);
        if let Err(e) = cx.send_request(req).block_task().await {
            tracing::warn!("agent mode: {e}");
        }
    }
    // The model first: the others depend on it.
    options.sort_by_key(|(id, _)| id != "model");
    for (id, value) in options {
        if let Some(o) = offered.iter().find(|o| o.id == id) {
            if o.value == value {
                continue;
            }
            // Kept for another agent, or gone since.
            if !o.choices.iter().any(|(v, _)| *v == value) {
                tracing::warn!("agent option {id}: no {value}");
                continue;
            }
        }
        if let Some(now) = set_option(&cx, &session_id, &id, &value).await
            && !now.is_empty()
        {
            offered = now;
        }
    }
    Ok((session_id, offered))
}

/// Runs the sessions until `cmds` closes or the agent goes away.
/// Always ends with [`AgentEvent::Closed`].
pub async fn run(
    agent: impl ConnectTo<Client> + 'static,
    config: SessionConfig,
    cmds: UnboundedReceiver<AgentCmd>,
    desk_cmds: UnboundedReceiver<AgentCmd>,
    events: UnboundedSender<AgentEvent>,
) {
    let result = sessions(agent, config, cmds, desk_cmds, events.clone()).await;
    let _ = events.send(AgentEvent::Closed {
        error: result.err().map(|e| e.to_string()),
    });
}

async fn sessions(
    agent: impl ConnectTo<Client> + 'static,
    config: SessionConfig,
    cmds: UnboundedReceiver<AgentCmd>,
    desk_cmds: UnboundedReceiver<AgentCmd>,
    events: UnboundedSender<AgentEvent>,
) -> agent_client_protocol::Result<()> {
    let permissions = config.permissions;
    // Either session takes its own out of it.
    let (inbound, radio_rx) = unbounded_channel::<(SessionId, Inbound)>();
    let (desk_inbound, desk_rx) = unbounded_channel::<(SessionId, Inbound)>();
    let routes = [inbound, desk_inbound];
    let updates = routes.clone();

    Client
        .v1()
        .on_receive_notification(
            async move |n: SessionNotification, _cx| {
                for r in &updates {
                    let update = Box::new(n.update.clone());
                    let _ = r.send((n.session_id.clone(), Inbound::Update(update)));
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |req: RequestPermissionRequest, responder, _cx| {
                let allowed = policy::allows(permissions, &req.tool_call.fields);
                let title = req
                    .tool_call
                    .fields
                    .title
                    .clone()
                    .unwrap_or_else(|| "tool call".into());
                for r in &routes {
                    let title = title.clone();
                    let _ = r.send((
                        req.session_id.clone(),
                        Inbound::Permission { title, allowed },
                    ));
                }
                let outcome = match policy::choose(allowed, &req.options) {
                    Some(o) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        o.option_id.clone(),
                    )),
                    None => RequestPermissionOutcome::Cancelled,
                };
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |cx: ConnectionTo<Agent>| {
            let init = cx
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_info(Implementation::new("vr-radio", env!("CARGO_PKG_VERSION"))),
                )
                .block_task()
                .await?;
            let desk_options = merged(&config.options, &config.desk_options);
            let lane = |primer, options, inbound| Lane {
                cx: cx.clone(),
                cwd: config.cwd.clone(),
                permissions,
                options: Arc::new(Mutex::new(options)),
                primer,
                renew: config.renew,
                inbound,
                events: events.clone(),
                used: 0,
                name: None,
            };
            let mut radio = lane(config.primer.clone(), config.options.clone(), radio_rx);
            radio.name = Some(
                init.agent_info
                    .map(|i| i.title.unwrap_or(i.name))
                    .unwrap_or_else(|| "agent".into()),
            );
            let desk = lane(config.desk_primer.clone(), desk_options, desk_rx);
            let desk = async move {
                if let Err(e) = desk.run(desk_cmds).await {
                    tracing::warn!("desk: {e}");
                }
                // The radio goes on without it.
                std::future::pending().await
            };
            tokio::select! {
                r = radio.run(cmds) => r,
                r = desk => r,
            }
        })
        .await
}

/// One session, and those that take its place.
struct Lane {
    cx: ConnectionTo<Agent>,
    cwd: PathBuf,
    permissions: Permissions,
    options: Shared,
    primer: Option<String>,
    renew: Renew,
    inbound: UnboundedReceiver<(SessionId, Inbound)>,
    events: UnboundedSender<AgentEvent>,
    /// Tokens of the last prompt, when the agent says.
    used: u64,
    /// Of the agent; only the radio tells. The desk opens when it is needed.
    name: Option<String>,
}

impl Lane {
    fn fresh(&self) -> SessionFuture {
        let options = self.options.lock().unwrap().clone();
        Box::pin(open(
            self.cx.clone(),
            self.cwd.clone(),
            self.permissions,
            options,
        ))
    }

    fn start(&self, session: &SessionId, turn: Turn, text: String) -> (Turn, PromptFuture) {
        let cx = self.cx.clone();
        let text = ContentBlock::Text(TextContent::new(text));
        let req = PromptRequest::new(session.clone(), vec![text]);
        (
            turn,
            Box::pin(async move { cx.send_request(req).block_task().await }),
        )
    }

    /// Passes on what came for `session`, as part of `turn`.
    fn forward(
        &mut self,
        turn: Option<Turn>,
        session: &Option<SessionId>,
        from: SessionId,
        inbound: Inbound,
    ) {
        let Some(turn) = turn.filter(|_| session.as_ref() == Some(&from)) else {
            return;
        };
        if let Inbound::Update(u) = &inbound
            && let SessionUpdate::UsageUpdate(u) = u.as_ref()
        {
            self.used = u.used;
        }
        let event = match inbound {
            Inbound::Update(u) => map_update(turn, *u),
            Inbound::Permission { title, allowed } => Some(AgentEvent::Permission {
                turn,
                title,
                allowed,
            }),
        };
        if let Some(e) = event {
            let _ = self.events.send(e);
        }
    }

    fn offer(&self, options: Vec<AgentOption>) {
        if self.name.is_some() && !options.is_empty() {
            let _ = self.events.send(AgentEvent::Options(options));
        }
    }

    /// Options from now on, for the session there is and those to come.
    fn set(&self, session: &Option<SessionId>, new: Vec<(String, String)>) {
        {
            let mut options = self.options.lock().unwrap();
            *options = merged(&options, &new);
        }
        let Some(session) = session.clone() else {
            return;
        };
        let (cx, events, tells) = (self.cx.clone(), self.events.clone(), self.name.is_some());
        tokio::spawn(async move {
            for (id, value) in new {
                if let Some(now) = set_option(&cx, &session, &id, &value).await
                    && tells
                    && !now.is_empty()
                {
                    let _ = events.send(AgentEvent::Options(now));
                }
            }
        });
    }

    async fn run(
        mut self,
        mut cmds: UnboundedReceiver<AgentCmd>,
    ) -> agent_client_protocol::Result<()> {
        let mut session: Option<SessionId> = None;
        if let Some(name) = self.name.clone() {
            let (id, options) = self.fresh().await?;
            session = Some(id);
            let _ = self.events.send(AgentEvent::Ready { name });
            self.offer(options);
        }
        let mut inflight: Option<(Turn, PromptFuture)> = None;
        // What the agent still has to be told before the next question.
        let mut unread = self.primer.clone();
        let mut queued: Option<Prompt> = None;
        // Prompts of the session, and the one to take its place.
        let mut turns = 0;
        let mut next: Option<SessionFuture> = None;
        let cx = self.cx.clone();
        let cancel = |session: &Option<SessionId>| match session {
            Some(s) => cx.send_notification(CancelNotification::new(s.clone())),
            None => Ok(()),
        };

        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    None => break,
                    Some(AgentCmd::Prompt { turn, text, keep }) => {
                        let prompt = Prompt { turn, text, keep };
                        if inflight.is_some() {
                            cancel(&session)?;
                            queued = Some(prompt.after(queued.take()));
                            continue;
                        }
                        if session.is_none() {
                            match self.fresh().await {
                                Ok((id, _)) => session = Some(id),
                                Err(e) => {
                                    let error = e.to_string();
                                    let _ = self.events.send(AgentEvent::TurnFailed { turn, error });
                                    continue;
                                }
                            }
                        }
                        if let Some(id) = &session {
                            turns += 1;
                            inflight = Some(self.start(id, turn, with_primer(unread.take(), prompt.text)));
                        }
                    }
                    Some(AgentCmd::Cancel) => {
                        queued = None;
                        if inflight.is_some() {
                            cancel(&session)?;
                        }
                    }
                    Some(AgentCmd::Set(new)) => self.set(&session, new),
                },
                opened = async {
                    match next.as_mut() {
                        Some(fut) => fut.await,
                        None => std::future::pending().await,
                    }
                }, if inflight.is_none() => {
                    next = None;
                    match opened {
                        Ok((id, _)) => {
                            if let Some(old) = session.replace(id) {
                                // Not every agent closes sessions; it makes no difference.
                                let _ = self.cx.send_request(CloseSessionRequest::new(old)).on_receiving_result(async |_| Ok(()));
                            }
                            turns = 0;
                            self.used = 0;
                            unread = self.primer.clone();
                            if self.name.is_some() {
                                let _ = self.events.send(AgentEvent::Renewed);
                            }
                        }
                        // The old one goes on.
                        Err(e) => tracing::warn!("new session: {e}"),
                    }
                }
                Some((from, inbound)) = self.inbound.recv() => {
                    self.forward(inflight.as_ref().map(|(t, _)| *t), &session, from, inbound);
                }
                result = async {
                    match inflight.as_mut() {
                        Some((_, fut)) => fut.await,
                        None => std::future::pending().await,
                    }
                } => {
                    let (turn, _) = inflight.take().expect("polled only while in flight");
                    // Updates sent before the response belong to this turn.
                    while let Ok((from, inbound)) = self.inbound.try_recv() {
                        self.forward(Some(turn), &session, from, inbound);
                    }
                    // That of a turn counts every look-up again.
                    let tokens = match &result {
                        _ if self.used > 0 => self.used,
                        Ok(r) => r.usage.as_ref().map_or(0, |u| u.total_tokens),
                        Err(_) => 0,
                    };
                    let cut = !matches!(&result, Ok(r) if r.stop_reason != StopReason::Cancelled);
                    if cut && turns == 1 {
                        // It may never have read them.
                        unread = self.primer.clone();
                    }
                    if next.is_none() && self.renew.due(turns, tokens) {
                        next = Some(self.fresh());
                    }
                    let _ = self.events.send(match result {
                        Ok(r) => AgentEvent::TurnEnd {
                            turn,
                            stop: format!("{:?}", r.stop_reason),
                            usage: r.usage.map(TokenUsage::from),
                        },
                        Err(e) => AgentEvent::TurnFailed { turn, error: e.to_string() },
                    });
                    if let (Some(Prompt { turn, text, .. }), Some(id)) = (queued.take(), &session) {
                        turns += 1;
                        inflight = Some(self.start(id, turn, with_primer(unread.take(), text)));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub mod fake {
    //! An in-process ACP agent that answers each prompt with fixed chunks.

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use agent_client_protocol::v1::Responder;
    use agent_client_protocol::v1::schema::{
        AgentCapabilities, CancelNotification, ContentChunk, Cost, InitializeResponse,
        NewSessionResponse, SessionId, SetSessionConfigOptionResponse, ToolCall, ToolCallId,
        ToolCallUpdate, ToolCallUpdateFields, ToolKind, Usage, UsageUpdate,
    };

    use super::*;

    /// Models `a` and `b`, two efforts, and something the radio ignores.
    fn offered(model: &str, effort: &str) -> Vec<SessionConfigOption> {
        let one = |id: &str, category: &str, value: &str, values: &[&str]| {
            let options: Vec<_> = values
                .iter()
                .map(|v| serde_json::json!({"value": v, "name": v.to_uppercase()}))
                .collect();
            serde_json::from_value(serde_json::json!({
                "id": id, "name": id, "category": category, "type": "select",
                "currentValue": value, "options": options,
            }))
            .unwrap()
        };
        vec![
            one("mode", "mode", "ask", &["ask"]),
            one("model", "model", model, &["a", "b"]),
            one("effort", "thought_level", effort, &["low", "high"]),
            one("fast-mode", "model_config", "off", &["off", "on"]),
        ]
    }

    /// Records prompts; replies `reply` split into chunks after a tool call.
    /// A prompt containing "slow" waits until it is cancelled. `@key@text`
    /// is the reply to prompts containing the key, instead of the others.
    pub fn agent(
        reply: &'static [&'static str],
        prompts: Arc<Mutex<Vec<String>>>,
    ) -> impl ConnectTo<Client> + 'static {
        with(reply, prompts, None)
    }

    /// Offers options and records those that are set.
    pub fn agent_set(
        reply: &'static [&'static str],
        prompts: Arc<Mutex<Vec<String>>>,
        set: Arc<Mutex<Vec<String>>>,
    ) -> impl ConnectTo<Client> + 'static {
        with(reply, prompts, Some(set))
    }

    fn with(
        reply: &'static [&'static str],
        prompts: Arc<Mutex<Vec<String>>>,
        set: Option<Arc<Mutex<Vec<String>>>>,
    ) -> impl ConnectTo<Client> + 'static {
        let offers = set.is_some();
        let set = set.unwrap_or_default();
        let cancelled = Arc::new(tokio::sync::Notify::new());
        let on_cancel = cancelled.clone();
        let opened = std::sync::atomic::AtomicUsize::new(1);
        Agent
            .v1()
            .on_receive_request(
                async move |req: InitializeRequest,
                            responder: Responder<InitializeResponse>,
                            _cx| {
                    responder.respond(
                        InitializeResponse::new(req.protocol_version)
                            .agent_capabilities(AgentCapabilities::new())
                            .agent_info(Implementation::new("fake-agent", "0.0.0")),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |_req: NewSessionRequest,
                            responder: Responder<NewSessionResponse>,
                            _cx| {
                    let n = opened.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    responder.respond(
                        NewSessionResponse::new(SessionId::new(format!("s{n}")))
                            .config_options(offers.then(|| offered("a", "low"))),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |req: SetSessionConfigOptionRequest,
                            responder: Responder<SetSessionConfigOptionResponse>,
                            _cx| {
                    let v = serde_json::to_value(&req).unwrap();
                    let (id, value) = (&v["configId"], v["value"].as_str().unwrap());
                    let line = format!("{} {}={value}", v["sessionId"], id.as_str().unwrap());
                    set.lock().unwrap().push(line.replace('"', ""));
                    let options = match id.as_str() {
                        Some("model") => offered(value, "low"),
                        _ => offered("a", value),
                    };
                    responder.respond(SetSessionConfigOptionResponse::new(options))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |_n: CancelNotification, _cx| {
                    on_cancel.notify_one();
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |req: PromptRequest,
                            responder: Responder<PromptResponse>,
                            cx: ConnectionTo<Client>| {
                    let ContentBlock::Text(text) = &req.prompt[0] else {
                        panic!("text prompt expected")
                    };
                    let text = text.text.clone();
                    prompts.lock().unwrap().push(text.clone());
                    let sid = req.session_id.clone();
                    if text.contains("slow") {
                        let cancelled = cancelled.clone();
                        // Answer from a task: the handler must not block the connection.
                        tokio::spawn(async move {
                            let _ =
                                tokio::time::timeout(Duration::from_secs(5), cancelled.notified())
                                    .await;
                            let _ = responder.respond(PromptResponse::new(StopReason::Cancelled));
                        });
                        return Ok(());
                    }
                    cx.send_notification(SessionNotification::new(
                        sid.clone(),
                        SessionUpdate::ToolCall(
                            ToolCall::new(ToolCallId::new("t1"), "`vr brief`")
                                .kind(ToolKind::Execute),
                        ),
                    ))?;
                    cx.send_notification(SessionNotification::new(
                        sid.clone(),
                        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                            ToolCallId::new("t1"),
                            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
                        )),
                    ))?;
                    let keyed = |c: &&'static str| -> Option<(&'static str, &'static str)> {
                        c.strip_prefix('@')?.split_once('@')
                    };
                    let mine: Vec<&str> = reply
                        .iter()
                        .filter_map(keyed)
                        .filter(|(key, _)| text.contains(key))
                        .map(|(_, reply)| reply)
                        .collect();
                    let plain = reply.iter().filter(|c| keyed(c).is_none()).copied();
                    let reply = if mine.is_empty() {
                        plain.collect()
                    } else {
                        mine
                    };
                    for chunk in reply {
                        cx.send_notification(SessionNotification::new(
                            sid.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(chunk)),
                            )),
                        ))?;
                    }
                    cx.send_notification(SessionNotification::new(
                        sid.clone(),
                        SessionUpdate::UsageUpdate(
                            UsageUpdate::new(1200, 200_000).cost(Cost::new(0.045, "USD")),
                        ),
                    ))?;
                    responder.respond(
                        PromptResponse::new(StopReason::EndTurn).usage(
                            Usage::new(1200, 100, 30)
                                .cached_read_tokens(1070)
                                .thought_tokens(20),
                        ),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    fn config(primer: Option<&str>) -> SessionConfig {
        SessionConfig {
            cwd: std::env::temp_dir(),
            permissions: Permissions::Safe,
            options: vec![],
            primer: primer.map(str::to_string),
            desk_primer: Some("デスクの規則".into()),
            desk_options: vec![],
            renew: Renew::NEVER,
        }
    }

    async fn next(rx: &mut UnboundedReceiver<AgentEvent>) -> AgentEvent {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("agent event timed out")
            .expect("agent channel closed")
    }

    #[tokio::test]
    async fn prompt_streams_tool_and_chunks_then_ends() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (ev_tx, mut ev_rx) = unbounded_channel();
        tokio::spawn(run(
            fake::agent(&["前まで", "0.6。"], prompts.clone()),
            config(None),
            cmd_rx,
            unbounded_channel().1,
            ev_tx,
        ));
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::Ready {
                name: "fake-agent".into()
            }
        );
        cmd_tx
            .send(AgentCmd::Prompt {
                turn: 7,
                text: "差は？".into(),
                keep: true,
            })
            .unwrap();
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::Tool {
                turn: 7,
                id: "t1".into(),
                title: Some("`vr brief`".into()),
                state: ToolState::Running
            }
        );
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::Tool {
                turn: 7,
                id: "t1".into(),
                title: None,
                state: ToolState::Done
            }
        );
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::Chunk {
                turn: 7,
                text: "前まで".into()
            }
        );
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::Chunk {
                turn: 7,
                text: "0.6。".into()
            }
        );
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::Cost {
                turn: 7,
                amount: 0.045,
                currency: "USD".into()
            }
        );
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::TurnEnd {
                turn: 7,
                stop: "EndTurn".into(),
                usage: Some(TokenUsage {
                    total: 1200,
                    input: 100,
                    cached_read: 1070,
                    cached_write: 0,
                    output: 30,
                    thought: 20,
                }),
            }
        );
        drop(cmd_tx);
        assert_eq!(next(&mut ev_rx).await, AgentEvent::Closed { error: None });
        assert_eq!(*prompts.lock().unwrap(), vec!["差は？".to_string()]);
    }

    fn prompt(turn: Turn, text: &str, keep: bool) -> AgentCmd {
        AgentCmd::Prompt {
            turn,
            text: text.into(),
            keep,
        }
    }

    /// The first `n` turns to end, and what the agent was asked by then.
    async fn ends(link: &mut AgentLink, n: usize) -> Vec<(Turn, String)> {
        let mut ends = Vec::new();
        while ends.len() < n {
            if let AgentEvent::TurnEnd { turn, stop, .. } = next(&mut link.rx).await {
                ends.push((turn, stop));
            }
        }
        ends
    }

    fn end(turn: Turn, stop: &str) -> (Turn, String) {
        (turn, stop.to_string())
    }

    /// The rules cost nothing of their own: they come along with the first
    /// prompt, once.
    #[tokio::test]
    async fn the_rules_come_along_with_the_first_prompt() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let mut link = AgentLink::spawn(
            fake::agent(&["了解。"], prompts.clone()),
            config(Some("rules")),
        );
        assert!(matches!(next(&mut link.rx).await, AgentEvent::Ready { .. }));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(prompts.lock().unwrap().is_empty(), "nobody asked yet");
        link.tx.send(prompt(1, "燃料", true)).unwrap();
        link.tx.send(prompt(2, "差は", true)).unwrap();
        ends(&mut link, 1).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sent = prompts.lock().unwrap().clone();
        assert_eq!(sent[0], "rules\n\n---\n\n燃料");
        assert_eq!(sent[1].lines().last(), Some("差は"));
    }

    /// Cut off before it was read, they come along with the next one.
    #[tokio::test]
    async fn rules_that_were_cut_off_are_told_again() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let mut link = AgentLink::spawn(
            fake::agent(&["了解。"], prompts.clone()),
            config(Some("rules")),
        );
        assert!(matches!(next(&mut link.rx).await, AgentEvent::Ready { .. }));
        link.tx.send(prompt(1, "slow", true)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        link.tx.send(AgentCmd::Cancel).unwrap();
        assert_eq!(ends(&mut link, 1).await, [end(1, "Cancelled")]);
        link.tx.send(prompt(2, "差は", true)).unwrap();
        assert_eq!(ends(&mut link, 1).await, [end(2, "EndTurn")]);
        link.tx.send(prompt(3, "燃料", true)).unwrap();
        assert_eq!(ends(&mut link, 1).await, [end(3, "EndTurn")]);
        assert_eq!(
            *prompts.lock().unwrap(),
            ["rules\n\n---\n\nslow", "rules\n\n---\n\n差は", "燃料"]
        );
    }

    #[test]
    fn sessions_end_at_their_limits() {
        let r = Renew {
            turns: 3,
            tokens: 100,
        };
        assert!(!r.due(2, 99));
        assert!(r.due(3, 0));
        assert!(r.due(0, 100));
        assert!(!Renew::NEVER.due(1000, 1_000_000));
    }

    /// A long session is replaced between two prompts, with the rules anew.
    #[tokio::test]
    async fn a_session_is_renewed() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let mut link = AgentLink::spawn(
            fake::agent(&["了解。"], prompts.clone()),
            SessionConfig {
                renew: Renew {
                    turns: 2,
                    tokens: 0,
                },
                ..config(Some("rules"))
            },
        );
        assert!(matches!(next(&mut link.rx).await, AgentEvent::Ready { .. }));
        for (turn, text) in [(1, "一"), (2, "二")] {
            link.tx.send(prompt(turn, text, true)).unwrap();
            assert_eq!(ends(&mut link, 1).await, [end(turn, "EndTurn")]);
        }
        while next(&mut link.rx).await != AgentEvent::Renewed {}
        link.tx.send(prompt(3, "三", true)).unwrap();
        assert_eq!(ends(&mut link, 1).await, [end(3, "EndTurn")]);
        assert_eq!(
            *prompts.lock().unwrap(),
            ["rules\n\n---\n\n一", "二", "rules\n\n---\n\n三"]
        );
    }

    /// The fake agent reports 1200 tokens a prompt.
    #[tokio::test]
    async fn a_full_session_is_renewed() {
        for (tokens, renewed) in [(1200, true), (1201, false)] {
            let prompts = Arc::new(Mutex::new(Vec::new()));
            let mut link = AgentLink::spawn(
                fake::agent(&["了解。"], prompts.clone()),
                SessionConfig {
                    renew: Renew { turns: 0, tokens },
                    ..config(Some("rules"))
                },
            );
            assert!(matches!(next(&mut link.rx).await, AgentEvent::Ready { .. }));
            for turn in 1..=2 {
                link.tx.send(prompt(turn, "q", true)).unwrap();
                ends(&mut link, 1).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let again = prompts.lock().unwrap()[1].starts_with("rules");
            assert_eq!(again, renewed, "{tokens}");
        }
    }

    /// Words that waited are sent with the newer ones, calls are not.
    #[tokio::test]
    async fn waiting_words_are_merged_in_order() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let mut link = AgentLink::spawn(fake::agent(&["了解。"], prompts.clone()), config(None));
        assert!(matches!(next(&mut link.rx).await, AgentEvent::Ready { .. }));
        for p in [
            prompt(1, "slow", true),
            prompt(2, "黄旗", false),
            prompt(3, "一つ目", true),
            prompt(4, "二つ目", true),
        ] {
            link.tx.send(p).unwrap();
        }
        assert_eq!(
            ends(&mut link, 2).await,
            [end(1, "Cancelled"), end(4, "EndTurn")]
        );
        assert_eq!(*prompts.lock().unwrap(), ["slow", "一つ目\n\n二つ目"]);
    }

    #[test]
    fn merging_keeps_what_counts() {
        let p = |turn, text: &str, keep| Prompt {
            turn,
            text: text.into(),
            keep,
        };
        assert_eq!(p(2, "b", true).after(None), p(2, "b", true));
        assert_eq!(
            p(2, "b", false).after(Some(p(1, "a", true))),
            p(2, "a\n\nb", true)
        );
        assert_eq!(
            p(2, "b", true).after(Some(p(1, "a", false))),
            p(2, "b", true)
        );
        assert_eq!(with_primer(None, "q".into()), "q");
    }

    #[tokio::test]
    async fn new_prompt_cancels_the_running_turn_first() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (ev_tx, mut ev_rx) = unbounded_channel();
        tokio::spawn(run(
            fake::agent(&["了解。"], prompts.clone()),
            config(None),
            cmd_rx,
            unbounded_channel().1,
            ev_tx,
        ));
        assert!(matches!(next(&mut ev_rx).await, AgentEvent::Ready { .. }));
        cmd_tx
            .send(AgentCmd::Prompt {
                turn: 1,
                text: "slow".into(),
                keep: true,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        cmd_tx
            .send(AgentCmd::Prompt {
                turn: 2,
                text: "燃料".into(),
                keep: true,
            })
            .unwrap();
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::TurnEnd {
                turn: 1,
                stop: "Cancelled".into(),
                usage: None,
            }
        );
        assert!(matches!(
            next(&mut ev_rx).await,
            AgentEvent::Tool { turn: 2, .. }
        ));
        assert!(matches!(
            next(&mut ev_rx).await,
            AgentEvent::Tool { turn: 2, .. }
        ));
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::Chunk {
                turn: 2,
                text: "了解。".into()
            }
        );
        assert!(matches!(
            next(&mut ev_rx).await,
            AgentEvent::Cost { turn: 2, .. }
        ));
        assert!(matches!(
            next(&mut ev_rx).await,
            AgentEvent::TurnEnd { turn: 2, usage: Some(u), .. } if u.total == 1200
        ));
        assert_eq!(
            *prompts.lock().unwrap(),
            vec!["slow".to_string(), "燃料".to_string()]
        );
    }

    #[tokio::test]
    async fn cancel_without_a_turn_is_a_no_op() {
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (ev_tx, mut ev_rx) = unbounded_channel();
        tokio::spawn(run(
            fake::agent(&["a。"], Arc::default()),
            config(None),
            cmd_rx,
            unbounded_channel().1,
            ev_tx,
        ));
        assert!(matches!(next(&mut ev_rx).await, AgentEvent::Ready { .. }));
        cmd_tx.send(AgentCmd::Cancel).unwrap();
        cmd_tx
            .send(AgentCmd::Prompt {
                turn: 3,
                text: "q".into(),
                keep: true,
            })
            .unwrap();
        assert!(matches!(
            next(&mut ev_rx).await,
            AgentEvent::Tool { turn: 3, .. }
        ));
    }

    #[tokio::test]
    async fn dropping_the_link_ends_an_agent_that_is_still_starting() {
        use agent_client_protocol::v1::Responder;
        use agent_client_protocol::v1::schema::InitializeResponse;
        let silent = Agent.v1().on_receive_request(
            async move |_: InitializeRequest, _: Responder<InitializeResponse>, _cx| {
                std::future::pending::<agent_client_protocol::Result<()>>().await
            },
            agent_client_protocol::on_receive_request!(),
        );
        let link = AgentLink::spawn(silent, config(None));
        let task = link.task.clone().unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!task.is_finished());
        drop(link);
        for _ in 0..100 {
            if task.is_finished() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the session outlived its link");
    }

    #[test]
    fn tool_states() {
        assert_eq!(tool_state(None), ToolState::Running);
        assert_eq!(
            tool_state(Some(ToolCallStatus::Pending)),
            ToolState::Running
        );
        assert_eq!(
            tool_state(Some(ToolCallStatus::InProgress)),
            ToolState::Running
        );
        assert_eq!(tool_state(Some(ToolCallStatus::Completed)), ToolState::Done);
        assert_eq!(tool_state(Some(ToolCallStatus::Failed)), ToolState::Failed);
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// What came for the end of `turn`, and all the text of it.
    async fn answer(rx: &mut UnboundedReceiver<AgentEvent>, wanted: Turn) -> String {
        let mut out = String::new();
        loop {
            match next(rx).await {
                AgentEvent::Chunk { turn, text } if turn == wanted => out.push_str(&text),
                AgentEvent::TurnEnd { turn, .. } if turn == wanted => return out,
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn the_desk_works_while_the_radio_answers() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let mut link = AgentLink::spawn(
            fake::agent(&["はい。", "@速度@調べた。"], prompts.clone()),
            config(Some("規則")),
        );
        // The desk waits; the radio does not.
        link.desk.send(prompt(1, "slow 調査", true)).unwrap();
        link.tx.send(prompt(2, "順位", true)).unwrap();
        assert_eq!(answer(&mut link.rx, 2).await, "はい。");
        link.desk.send(AgentCmd::Cancel).unwrap();
        assert_eq!(answer(&mut link.rx, 1).await, "");
        // Cancelled before it read its rules: they come again.
        link.desk.send(prompt(3, "速度", true)).unwrap();
        assert_eq!(answer(&mut link.rx, 3).await, "調べた。");
        let mut p = prompts.lock().unwrap().clone();
        p.sort();
        assert_eq!(
            p,
            [
                "デスクの規則\n\n---\n\nslow 調査",
                "デスクの規則\n\n---\n\n速度",
                "規則\n\n---\n\n順位",
            ]
        );
    }

    #[tokio::test]
    async fn options_are_set_per_session() {
        let set = Arc::new(Mutex::new(Vec::new()));
        let mut link = AgentLink::spawn(
            fake::agent_set(&["はい。"], Arc::default(), set.clone()),
            SessionConfig {
                // The model goes first; what is so already, and what there is
                // not, is not set.
                options: pairs(&[("effort", "high"), ("fast-mode", "off"), ("model", "b")]),
                desk_options: pairs(&[("effort", "low"), ("model", "gone")]),
                ..config(None)
            },
        );
        assert!(matches!(next(&mut link.rx).await, AgentEvent::Ready { .. }));
        let AgentEvent::Options(o) = next(&mut link.rx).await else {
            panic!("options expected")
        };
        let kinds: Vec<_> = o.iter().map(|o| (o.kind, o.value.as_str())).collect();
        assert_eq!(
            kinds,
            [
                (OptionKind::Model, "a"),
                (OptionKind::Effort, "high"),
                (OptionKind::Speed, "off")
            ]
        );
        assert_eq!(o[1].choices, pairs(&[("low", "LOW"), ("high", "HIGH")]));
        assert_eq!(*set.lock().unwrap(), ["s1 model=b", "s1 effort=high"]);
        // Later, for the session there is.
        link.tx
            .send(AgentCmd::Set(pairs(&[("fast-mode", "on")])))
            .unwrap();
        while !matches!(next(&mut link.rx).await, AgentEvent::Options(_)) {}
        assert_eq!(set.lock().unwrap()[2], "s1 fast-mode=on");
        // The desk has none yet: with its first prompt.
        link.desk
            .send(AgentCmd::Set(pairs(&[("effort", "high")])))
            .unwrap();
        link.desk.send(prompt(1, "x", true)).unwrap();
        answer(&mut link.rx, 1).await;
        assert_eq!(set.lock().unwrap()[3..], ["s2 effort=high"]);
    }
}
