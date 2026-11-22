//! One long-lived ACP session with the agent (Claude Code, Codex...).
//!
//! The process and the session stay up between questions, so a question
//! only costs a `session/prompt` round trip. Replies stream back as chunks.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, Implementation, InitializeRequest, NewSessionRequest,
    PromptRequest, PromptResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
    TextContent, ToolCallStatus,
};
use agent_client_protocol::{Agent, Client, ConnectTo};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::policy::{self, Permissions};

pub type Turn = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCmd {
    /// Starts a turn. A turn still running is cancelled first.
    Prompt { turn: Turn, text: String },
    /// Cancels the running turn, if any.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    Ready {
        name: String,
    },
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
        _ => None,
    }
}

/// Runs the session until `cmds` closes or the agent goes away.
/// Always ends with [`AgentEvent::Closed`].
pub async fn run(
    agent: impl ConnectTo<Client> + 'static,
    cwd: PathBuf,
    permissions: Permissions,
    cmds: UnboundedReceiver<AgentCmd>,
    events: UnboundedSender<AgentEvent>,
) {
    let result = session(agent, cwd, permissions, cmds, events.clone()).await;
    let _ = events.send(AgentEvent::Closed {
        error: result.err().map(|e| e.to_string()),
    });
}

async fn session(
    agent: impl ConnectTo<Client> + 'static,
    cwd: PathBuf,
    permissions: Permissions,
    mut cmds: UnboundedReceiver<AgentCmd>,
    events: UnboundedSender<AgentEvent>,
) -> agent_client_protocol::Result<()> {
    let (inbound_tx, mut inbound_rx) = unbounded_channel::<Inbound>();
    let updates = inbound_tx.clone();

    Client
        .v1()
        .on_receive_notification(
            async move |n: SessionNotification, _cx| {
                let _ = updates.send(Inbound::Update(Box::new(n.update)));
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
                let _ = inbound_tx.send(Inbound::Permission { title, allowed });
                let outcome = match policy::choose(allowed, &req.options) {
                    Some(o) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(o.option_id.clone())),
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
            let session_id = cx.send_request(NewSessionRequest::new(cwd)).block_task().await?.session_id;
            let name = init
                .agent_info
                .map(|i| i.title.unwrap_or(i.name))
                .unwrap_or_else(|| "agent".into());
            let _ = events.send(AgentEvent::Ready { name });

            let start = |turn: Turn, text: String| -> (Turn, PromptFuture) {
                let cx = cx.clone();
                let req = PromptRequest::new(session_id.clone(), vec![ContentBlock::Text(TextContent::new(text))]);
                (turn, Box::pin(async move { cx.send_request(req).block_task().await }))
            };
            let mut inflight: Option<(Turn, PromptFuture)> = None;
            let mut queued: Option<(Turn, String)> = None;
            let forward = |turn: Option<Turn>, inbound: Inbound| {
                let Some(turn) = turn else { return };
                let event = match inbound {
                    Inbound::Update(u) => map_update(turn, *u),
                    Inbound::Permission { title, allowed } => Some(AgentEvent::Permission { turn, title, allowed }),
                };
                if let Some(e) = event {
                    let _ = events.send(e);
                }
            };

            loop {
                tokio::select! {
                    cmd = cmds.recv() => match cmd {
                        None => break,
                        Some(AgentCmd::Prompt { turn, text }) => {
                            if inflight.is_some() {
                                cx.send_notification(CancelNotification::new(session_id.clone()))?;
                                queued = Some((turn, text));
                            } else {
                                inflight = Some(start(turn, text));
                            }
                        }
                        Some(AgentCmd::Cancel) => {
                            queued = None;
                            if inflight.is_some() {
                                cx.send_notification(CancelNotification::new(session_id.clone()))?;
                            }
                        }
                    },
                    Some(inbound) = inbound_rx.recv() => forward(inflight.as_ref().map(|(t, _)| *t), inbound),
                    result = async {
                        match inflight.as_mut() {
                            Some((_, fut)) => fut.await,
                            None => std::future::pending().await,
                        }
                    } => {
                        let (turn, _) = inflight.take().expect("polled only while in flight");
                        // Updates sent before the response belong to this turn.
                        while let Ok(inbound) = inbound_rx.try_recv() {
                            forward(Some(turn), inbound);
                        }
                        let _ = events.send(match result {
                            Ok(r) => AgentEvent::TurnEnd { turn, stop: format!("{:?}", r.stop_reason) },
                            Err(e) => AgentEvent::TurnFailed { turn, error: e.to_string() },
                        });
                        if let Some((turn, text)) = queued.take() {
                            inflight = Some(start(turn, text));
                        }
                    }
                }
            }
            Ok(())
        })
        .await
}

#[cfg(test)]
pub mod fake {
    //! An in-process ACP agent that answers each prompt with fixed chunks.

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use agent_client_protocol::v1::Responder;
    use agent_client_protocol::v1::schema::{
        AgentCapabilities, CancelNotification, ContentChunk, InitializeResponse,
        NewSessionResponse, SessionId, StopReason, ToolCall, ToolCallId, ToolCallUpdate,
        ToolCallUpdateFields, ToolKind,
    };

    use super::*;

    /// Records prompts; replies `reply` split into chunks after a tool call.
    /// A prompt containing "slow" waits until it is cancelled.
    pub fn agent(
        reply: &'static [&'static str],
        prompts: Arc<Mutex<Vec<String>>>,
    ) -> impl ConnectTo<Client> + 'static {
        let cancelled = Arc::new(tokio::sync::Notify::new());
        let on_cancel = cancelled.clone();
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
                    responder.respond(NewSessionResponse::new(SessionId::new("s1")))
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
                    for chunk in reply {
                        cx.send_notification(SessionNotification::new(
                            sid.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(*chunk)),
                            )),
                        ))?;
                    }
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
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
            std::env::temp_dir(),
            Permissions::Safe,
            cmd_rx,
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
            AgentEvent::TurnEnd {
                turn: 7,
                stop: "EndTurn".into()
            }
        );
        drop(cmd_tx);
        assert_eq!(next(&mut ev_rx).await, AgentEvent::Closed { error: None });
        assert_eq!(*prompts.lock().unwrap(), vec!["差は？".to_string()]);
    }

    #[tokio::test]
    async fn new_prompt_cancels_the_running_turn_first() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (ev_tx, mut ev_rx) = unbounded_channel();
        tokio::spawn(run(
            fake::agent(&["了解。"], prompts.clone()),
            std::env::temp_dir(),
            Permissions::Safe,
            cmd_rx,
            ev_tx,
        ));
        assert!(matches!(next(&mut ev_rx).await, AgentEvent::Ready { .. }));
        cmd_tx
            .send(AgentCmd::Prompt {
                turn: 1,
                text: "slow".into(),
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        cmd_tx
            .send(AgentCmd::Prompt {
                turn: 2,
                text: "燃料".into(),
            })
            .unwrap();
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::TurnEnd {
                turn: 1,
                stop: "Cancelled".into()
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
        assert_eq!(
            next(&mut ev_rx).await,
            AgentEvent::TurnEnd {
                turn: 2,
                stop: "EndTurn".into()
            }
        );
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
            std::env::temp_dir(),
            Permissions::Safe,
            cmd_rx,
            ev_tx,
        ));
        assert!(matches!(next(&mut ev_rx).await, AgentEvent::Ready { .. }));
        cmd_tx.send(AgentCmd::Cancel).unwrap();
        cmd_tx
            .send(AgentCmd::Prompt {
                turn: 3,
                text: "q".into(),
            })
            .unwrap();
        assert!(matches!(
            next(&mut ev_rx).await,
            AgentEvent::Tool { turn: 3, .. }
        ));
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
}
