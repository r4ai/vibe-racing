//! Layout: a custom title bar, the radio console on the left (state,
//! push-to-talk, stop) and the radio log on the right (questions, tool
//! calls, answers), with the typed question at the bottom of the log.

use iced::widget::{Space, column, container, row, space, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Theme};
use shadcn_iced::button::{self, button, icon_button};
use shadcn_iced::prelude::*;
use shadcn_iced::trace::{self, trace, trace_line};
use shadcn_iced::typography::{muted_text, small};

use super::{AgentStatus, App, HOTKEY, Message, Phase, SCROLLER, TurnView, latency};
use crate::agent::ToolState;
use crate::chrome;
use crate::engine::Outcome;

const CONSOLE_WIDTH: f32 = 288.0;
const LOG_PADDING: f32 = 24.0;

fn palette(theme: &Theme) -> shadcn_iced::Palette {
    shadcn_iced::tokens(theme).palette
}

fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// Text in the muted foreground color at `size` px.
fn muted<'a>(content: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    text(content).size(size).style(|t: &Theme| text::Style {
        color: Some(palette(t).muted_foreground),
    })
}

fn hairline_bottom(theme: &Theme) -> container::Style {
    let p = palette(theme);
    container::Style {
        background: Some(Background::Color(p.background)),
        border: Border {
            color: p.border,
            width: 0.0,
            radius: 0.0.into(),
        },
        shadow: Default::default(),
        ..Default::default()
    }
}

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let body = row![self.console(), vertical_separator(), self.log()].height(Length::Fill);
        let page = column![self.title_bar(), separator(), body];
        tab_order(
            sheet(container(page).style(hairline_bottom), self.settings_open)
                .title("設定")
                .description("読み上げと接続先")
                .content(self.settings())
                .on_close(Message::Settings(false)),
        )
        .into()
    }

    /// Brand, agent connection, settings and, on Windows, the caption
    /// buttons. Dragging, double-click, the system menu and Snap Layouts are
    /// handled natively (see `chrome`).
    fn title_bar(&self) -> Element<'_, Message> {
        let (dot, agent): (fn(&Theme) -> Color, String) = match &self.agent {
            AgentStatus::Starting => (|t| palette(t).muted_foreground, "エージェント起動中".into()),
            AgentStatus::Ready(name) => (|_| Color::from_rgb8(0x22, 0xc5, 0x5e), name.clone()),
            AgentStatus::Closed(e) => (|t| palette(t).destructive, format!("停止: {e}")),
        };
        let status = row![
            container(Space::new().width(8).height(8)).style(move |t: &Theme| container::Style {
                background: Some(Background::Color(dot(t))),
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            muted(agent, 13.0),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let settings = container(
            icon_button(lucide::SETTINGS)
                .variant(button::Variant::Ghost)
                .size(button::Size::IconSm)
                .on_press(Message::Settings(true)),
        )
        .width(chrome::TOOLS)
        .center_x(chrome::TOOLS);

        let mut bar = row![
            text("VR Radio")
                .size(14)
                .font(shadcn_iced::font::semibold()),
            status,
            space::horizontal(),
            settings,
        ]
        .spacing(16)
        .height(chrome::TITLE_BAR)
        .align_y(Alignment::Center)
        .padding(Padding::new(0.0).left(16));
        if chrome::CUSTOM_TITLE_BAR {
            bar = bar.push(self.caption_buttons());
        } else {
            bar = bar.padding(Padding::new(0.0).left(16).right(8));
        }
        bar.into()
    }

    /// Minimize, maximize and close, drawn like Windows 11 draws them.
    fn caption_buttons(&self) -> Element<'_, Message> {
        let glyph = |c: char| {
            text(c.to_string())
                .size(10)
                .font(iced::Font::with_name(chrome::icon_font()))
                .center()
        };
        let caption = |c: char, message: Message, close: bool| {
            iced::widget::button(container(glyph(c)).center(Length::Fill))
                .width(chrome::CAPTION_BUTTON)
                .height(chrome::TITLE_BAR)
                .padding(0)
                .on_press(message)
                .style(move |t: &Theme, status| caption_style(t, status, close))
        };
        // Its input goes to Windows (`HTMAXBUTTON`): the look follows `chrome`.
        let state = self.chrome;
        let maximize = container(glyph(if state.maximized {
            chrome::GLYPH_RESTORE
        } else {
            chrome::GLYPH_MAXIMIZE
        }))
        .width(chrome::CAPTION_BUTTON)
        .height(chrome::TITLE_BAR)
        .center(chrome::CAPTION_BUTTON)
        .style(move |t: &Theme| {
            let status = if state.maximize_pressed {
                iced::widget::button::Status::Pressed
            } else if state.maximize_hovered {
                iced::widget::button::Status::Hovered
            } else {
                iced::widget::button::Status::Active
            };
            let b = caption_style(t, status, false);
            container::Style {
                background: b.background,
                text_color: Some(b.text_color),
                ..Default::default()
            }
        });
        row![
            caption(chrome::GLYPH_MINIMIZE, Message::Minimize, false),
            maximize,
            caption(chrome::GLYPH_CLOSE, Message::Close, true),
        ]
        .into()
    }

    /// What the radio is doing and the controls, in the order of use.
    fn console(&self) -> Element<'_, Message> {
        let phase = self.phase();
        let state = column![
            self.orb(phase),
            text(phase.label())
                .size(20)
                .font(shadcn_iced::font::semibold()),
            muted(phase.hint(), 13.0),
        ]
        .spacing(10)
        .align_x(Alignment::Center)
        .width(Length::Fill);

        let ptt = button(if self.recording {
            "話し終わった"
        } else {
            "押して話す"
        })
        .icon(if self.recording {
            lucide::SEND
        } else {
            lucide::MIC
        })
        .variant(if self.recording {
            button::Variant::Destructive
        } else {
            button::Variant::Default
        })
        .size(button::Size::Lg)
        .width(Length::Fill)
        .on_press_maybe(self.engine.as_ref().map(|_| Message::TogglePtt));
        let stop = button("止める")
            .icon(lucide::SQUARE)
            .variant(button::Variant::Outline)
            .width(Length::Fill)
            .on_press_maybe(phase.busy().then_some(Message::Cancel));
        let hotkey: Element<'_, Message> = match self.hotkey() {
            Some(key) => row![kbd(key), muted("を押している間も話せます", 12.0)]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
            None => muted("ホットキーは使えません", 12.0).into(),
        };

        let mut info = column![].spacing(6);
        let has_info = self.voice.is_some() || self.turns.iter().any(|t| t.audio_ms.is_some());
        if let Some(v) = &self.voice {
            info = info.push(fact("話者", v.to_string()));
        }
        // Time from the end of speech to the first spoken word: the number that matters.
        if let Some(ms) = self.turns.iter().rev().find_map(|t| t.audio_ms) {
            info = info.push(fact(
                "応答",
                format!("{:.1} 秒で読み上げ開始", ms as f64 / 1000.0),
            ));
        }

        let mut panel = column![
            Space::new().height(Length::FillPortion(1)),
            state,
            Space::new().height(Length::FillPortion(1)),
            column![ptt, stop, container(hotkey).center_x(Length::Fill)].spacing(8),
        ]
        .spacing(20);
        if has_info {
            panel = panel.push(separator()).push(info);
        }
        container(panel)
            .width(CONSOLE_WIDTH)
            .height(Length::Fill)
            .padding(20)
            .style(|t: &Theme| container::Style {
                background: Some(Background::Color(palette(t).sidebar)),
                ..Default::default()
            })
            .into()
    }

    /// A ring that grows with the voice while recording.
    fn orb(&self, phase: Phase) -> Element<'_, Message> {
        let level = if phase == Phase::Recording {
            (self.level * 6.0).min(1.0)
        } else {
            0.0
        };
        let center: Element<'_, Message> = match phase {
            Phase::Idle => icon(lucide::RADIO).size(32).into(),
            Phase::Recording => icon(lucide::MIC).size(32).into(),
            Phase::Speaking => icon(lucide::AUDIO_LINES).size(32).into(),
            Phase::Transcribing | Phase::Researching | Phase::Answering => {
                spinner().size(32.0).into()
            }
        };
        let fill = move |t: &Theme| {
            let p = palette(t);
            match phase {
                Phase::Recording => (p.destructive, Color::WHITE),
                Phase::Speaking => (p.primary, p.primary_foreground),
                Phase::Idle => (p.muted, p.muted_foreground),
                _ => (p.secondary, p.secondary_foreground),
            }
        };
        let core = container(center)
            .width(96)
            .height(96)
            .center(96)
            .style(move |t: &Theme| {
                let (bg, fg) = fill(t);
                container::Style {
                    background: Some(Background::Color(bg)),
                    text_color: Some(fg),
                    border: Border {
                        radius: 48.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });
        let ring = 4.0 + 20.0 * level;
        let halo = container(core).padding(ring).style(move |t: &Theme| {
            let (bg, _) = fill(t);
            let a = if matches!(phase, Phase::Recording | Phase::Speaking) {
                0.22
            } else {
                0.0
            };
            container::Style {
                background: Some(Background::Color(alpha(bg, a))),
                border: Border {
                    radius: (48.0 + ring).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
        // Fixed footprint: the ring must not move the layout.
        container(halo).center(144).into()
    }

    /// Questions and answers, newest at the bottom, and the typed question.
    fn log(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = if self.turns.is_empty() {
            container(
                empty("まだ交信はありません")
                    .icon(lucide::RADIO)
                    .description("左のボタンかホットキーを押している間に質問すると、エージェントが調べて読み上げます。"),
            )
            .center(Length::Fill)
            .into()
        } else {
            let turns = self.turns.iter().map(turn_view);
            shadcn_iced::message_scroller::message_scroller(turns)
                .id(SCROLLER)
                .padding(Padding::new(LOG_PADDING))
                .on_scroll(|v| Message::Scrolled(shadcn_iced::message_scroller::is_at_end(&v)))
                .button((!self.at_end).then_some(Message::ScrollToEnd))
                .into()
        };

        let mut bottom = column![].spacing(12);
        if let Some(e) = self.errors.last() {
            bottom = bottom.push(
                alert("エラー")
                    .icon(lucide::CIRCLE_ALERT)
                    .description(e.as_str())
                    .action(
                        icon_button(lucide::X)
                            .variant(button::Variant::Ghost)
                            .size(button::Size::IconSm)
                            .on_press(Message::DismissError),
                    )
                    .variant(shadcn_iced::alert::Variant::Destructive),
            );
        }
        bottom = bottom.push(
            row![
                container(
                    input("文字で質問する", &self.draft)
                        .on_input(Message::Draft)
                        .on_submit(Message::Send)
                )
                .width(Length::Fill),
                icon_button(lucide::SEND)
                    .on_press_maybe((!self.draft.trim().is_empty()).then_some(Message::Send)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );

        column![
            container(content).height(Length::Fill),
            container(bottom).padding(Padding::new(LOG_PADDING).top(0).bottom(16)),
        ]
        .width(Length::Fill)
        .into()
    }

    pub(super) fn settings(&self) -> Element<'_, Message> {
        let cfg = super::CONFIG.get();
        let voice: Element<'_, Message> = if self.voices.is_empty() {
            muted_text("VOICEVOX に接続できません").into()
        } else {
            speaker_select(&self.voices, self.voice.clone())
        };
        let hotkey = match HOTKEY.get() {
            Some(Ok(key)) => format!("{key}（押している間録音）"),
            Some(Err(e)) => format!("無効: {e}"),
            None => "未設定".into(),
        };
        let info = |label: &'static str, value: String| {
            let value = if value.is_empty() {
                "未設定".into()
            } else {
                value
            };
            column![small(label), muted_text(value)].spacing(2)
        };
        column![
            switch(!self.muted)
                .label("回答を読み上げる")
                .on_toggle(|on| Message::Muted(!on)),
            field("話者", voice),
            separator(),
            info("ホットキー", hotkey),
            info(
                "エージェント",
                cfg.map(|c| c.agent.clone()).unwrap_or_default()
            ),
            info(
                "文字起こし",
                cfg.map(|c| format!("{} · {}", c.stt_model, c.stt_url))
                    .unwrap_or_default()
            ),
            info(
                "VOICEVOX",
                cfg.map(|c| c.voicevox_url.clone()).unwrap_or_default()
            ),
        ]
        .spacing(16)
        .into()
    }
}

/// Character, then one of its styles: two short lists instead of one long one.
fn speaker_select<'a>(
    voices: &[crate::voicevox::Voice],
    selected: Option<crate::voicevox::Voice>,
) -> Element<'a, Message> {
    let mut characters: Vec<String> = Vec::new();
    for v in voices {
        if !characters.contains(&v.speaker) {
            characters.push(v.speaker.clone());
        }
    }
    let character = selected.as_ref().map(|v| v.speaker.clone());
    let mut styles = select(selected.clone(), Message::Speaker)
        .placeholder("スタイル")
        .width(Length::Fill);
    for v in voices
        .iter()
        .filter(|v| Some(&v.speaker) == character.as_ref())
    {
        styles = styles.option(v.clone(), v.style.clone());
    }
    row![
        select(character, Message::Character)
            .placeholder("キャラクター")
            .options(characters)
            .width(Length::FillPortion(3)),
        container(styles).width(Length::FillPortion(2)),
    ]
    .spacing(8)
    .into()
}

/// Flat caption button: a light wash on hover, red for close (Windows 11).
fn caption_style(
    theme: &Theme,
    status: iced::widget::button::Status,
    close: bool,
) -> iced::widget::button::Style {
    use iced::widget::button::Status;
    let p = palette(theme);
    let red = Color::from_rgb8(0xc4, 0x2b, 0x1c);
    let (background, text_color) = match (status, close) {
        (Status::Hovered, true) => (Some(red), Color::WHITE),
        (Status::Pressed, true) => (Some(alpha(red, 0.9)), alpha(Color::WHITE, 0.8)),
        (Status::Hovered, false) => (Some(alpha(p.foreground, 0.08)), p.foreground),
        (Status::Pressed, false) => (Some(alpha(p.foreground, 0.05)), alpha(p.foreground, 0.7)),
        _ => (None, p.foreground),
    };
    iced::widget::button::Style {
        background: background.map(Background::Color),
        text_color,
        ..Default::default()
    }
}

/// A label and a value on one line of the console.
fn fact<'a>(label: &'static str, value: String) -> Element<'a, Message> {
    row![muted(label, 12.0).width(40), text(value).size(12)]
        .spacing(8)
        .into()
}

/// One exchange: the question, the tool calls, the answer and its timing.
pub(super) fn turn_view(t: &TurnView) -> Element<'_, Message> {
    let question: Element<'_, Message> = match t.heard_label() {
        Some(q) => row![
            icon(lucide::MIC).size(14).style(|t: &Theme| text::Style {
                color: Some(palette(t).muted_foreground)
            }),
            muted(q, 14.0),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
        None => row![spinner().size(14.0), muted("聞き取り中…", 14.0)]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
    };
    let mut block = column![question].spacing(10);
    if !t.tools.is_empty() {
        block = block.push(trace(t.tools.iter().map(|(_, title, state)| {
            let (icon, status) = match state {
                ToolState::Running => (lucide::LOADER_CIRCLE, trace::Status::Running),
                ToolState::Done => (lucide::CIRCLE_CHECK, trace::Status::Done),
                ToolState::Failed => (lucide::CIRCLE_ALERT, trace::Status::Error),
            };
            trace_line(title.clone()).icon(icon).status(status).into()
        })));
    }
    if t.asked() {
        let answer: Element<'_, Message> = if t.thinking() {
            row![spinner().size(16.0), muted("調べています…", 16.0)]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
        } else if t.reply.trim().is_empty() {
            muted("（回答なし）", 16.0).into()
        } else {
            text(t.reply.trim()).size(17).line_height(1.6).into()
        };
        block = block.push(answer);
    }
    let meta = latency(t);
    if !meta.is_empty() {
        let failed = matches!(t.done, Some((Outcome::Failed(_), _)));
        block = block.push(muted(meta, 12.0).style(move |th: &Theme| text::Style {
            color: Some(if failed {
                palette(th).destructive
            } else {
                palette(th).muted_foreground
            }),
        }));
    }
    container(block)
        .padding(Padding::new(0.0).bottom(20))
        .into()
}
