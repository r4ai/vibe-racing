//! Layout: a custom title bar, the radio console on the left (state,
//! push-to-talk, stop) and the radio log on the right (questions, tool
//! calls, answers), with the typed question at the bottom of the log.

use iced::widget::{Space, column, container, row, space, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Theme};
use shadcn_iced::button::{self, button, icon_button};
use shadcn_iced::prelude::*;
use shadcn_iced::trace::{self, trace, trace_line};
use shadcn_iced::typography::muted_text;

use super::{
    AgentChoice, AgentStatus, App, Capture, HoldChoice, Message, Phase, PlanChoice, Readiness,
    SCROLLER, TurnView, VoiceState, latency,
};
use crate::agent::ToolState;
use crate::chrome;
use crate::directive::{MAX_VOLUME, SPEED};
use crate::engine::Outcome;
use crate::rules::Scope;
use crate::stt::{self, SttBackend};
use crate::watch::Group;

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
                .description("声・エージェント・操作")
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
        // The same button, wherever it is pressed.
        let wheel = self.settings.wheel.as_ref().map(|w| w.to_string());
        let mut keys = row![].spacing(6).align_y(Alignment::Center);
        if let Some(key) = self.hotkey() {
            keys = keys.push(kbd(key));
        }
        if self.hotkey().is_some() && wheel.is_some() {
            keys = keys.push(muted("または", 12.0));
        }
        if let Some(w) = wheel {
            keys = keys.push(kbd(w));
        }
        let keys = column![
            keys,
            muted(
                if self.hotkey().is_none() && self.settings.wheel.is_none() {
                    "キーとハンドルは設定で割り当て"
                } else {
                    "を押している間も話せます"
                },
                12.0
            ),
        ]
        .spacing(4)
        .align_x(Alignment::Center);

        let volume = row![
            icon_button(if self.muted {
                lucide::VOLUME_X
            } else {
                lucide::VOLUME_2
            })
            .variant(button::Variant::Ghost)
            .size(button::Size::IconSm)
            .on_press(Message::Muted(!self.muted)),
            self.volume_slider(),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let mut info = column![].spacing(6);
        let has_info = self.voice.is_some() || self.turns.iter().any(|t| t.audio_ms.is_some());
        if let Some(v) = &self.voice {
            info = info.push(fact("話者", v.to_string()));
        }
        // Time from the end of speech to the first spoken word: the number that matters.
        if let Some(ms) = self.turns.iter().rev().find_map(|t| t.audio_ms) {
            info = info.push(fact(
                "応答",
                format!("{:.1}秒で読み上げ開始", ms as f64 / 1000.0),
            ));
        }

        let rules = &self.settings.rules;
        let kept = (rules.notes.len(), rules.watches.len());
        if kept != (0, 0) {
            info = info.push(fact("約束", format!("指示 {}・見張り {}", kept.0, kept.1)));
        }
        let has_info = has_info || kept != (0, 0);

        let mut panel = column![
            Space::new().height(Length::FillPortion(1)),
            state,
            Space::new().height(Length::FillPortion(1)),
            column![ptt, stop, container(keys).center_x(Length::Fill)].spacing(8),
            volume,
            separator(),
            self.readiness_list(),
        ]
        .spacing(16);
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

    /// Percent, with its number.
    fn volume_slider(&self) -> Element<'_, Message> {
        row![
            slider(
                0.0..=f32::from(MAX_VOLUME),
                f32::from(self.config.volume),
                |v| Message::Volume(v.round() as u8)
            )
            .step(5.0)
            .on_release(Message::SaveSettings)
            .width(Length::Fill),
            muted(format!("{}%", self.config.volume), 13.0).width(42),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    }

    /// What can be used, two by two.
    fn readiness_list(&self) -> Element<'_, Message> {
        let item = |(label, state): (&'static str, Readiness)| {
            let (color, note): (fn(&Theme) -> Color, &str) = match state {
                Readiness::Ready => (|_| Color::from_rgb8(0x22, 0xc5, 0x5e), ""),
                Readiness::Starting => (|_| Color::from_rgb8(0xf5, 0x9e, 0x0b), "起動中"),
                Readiness::Failed => (|t| palette(t).destructive, "使えません"),
                Readiness::Off => (|t| palette(t).muted_foreground, "オフ"),
            };
            let dot = container(Space::new().width(6).height(6)).style(move |t: &Theme| {
                container::Style {
                    background: Some(Background::Color(color(t))),
                    border: Border {
                        radius: 3.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });
            row![dot, text(label).size(12), muted(note, 11.0)]
                .spacing(6)
                .align_y(Alignment::Center)
        };
        let [agent, voice, telemetry, microphone] = self.readiness().map(item);
        // Wide enough for the longest label with its note.
        let left = 136;
        column![
            row![agent.width(left), voice],
            row![telemetry.width(left), microphone],
        ]
        .spacing(6)
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

        // Always in sight: what the engineer has in mind.
        let plan = self.plan.as_ref().map(|p| {
            column![
                container(
                    row![
                        container(muted("作戦", 12.0)).padding(Padding::new(0.0).top(2)),
                        text(p).size(14)
                    ]
                    .spacing(12)
                )
                .padding(Padding::new(12.0).left(LOG_PADDING).right(LOG_PADDING)),
                separator(),
            ]
        });
        column![
            plan,
            container(content).height(Length::Fill),
            container(bottom).padding(Padding::new(LOG_PADDING).top(0).bottom(16)),
        ]
        .width(Length::Fill)
        .into()
    }

    pub(super) fn settings(&self) -> Element<'_, Message> {
        let voice: Element<'_, Message> = if self.voices.is_empty() {
            muted_text(match self.voice_state {
                VoiceState::Starting => "VOICEVOX を起動中…".to_string(),
                _ => format!("VOICEVOX に接続できません（{}）", self.config.voicevox_url),
            })
            .into()
        } else {
            column![
                speaker_select(&self.voices, self.voice.clone()),
                button(if self.previewing && self.speaking {
                    "再生中…"
                } else {
                    "試聴"
                })
                .icon(lucide::VOLUME_2)
                .variant(button::Variant::Outline)
                .size(button::Size::Sm)
                .on_press_maybe(self.voice.as_ref().map(|_| Message::Preview)),
            ]
            .spacing(8)
            .into()
        };
        let content = column![
            self.talk_settings(),
            separator(),
            switch(!self.muted)
                .label("回答を読み上げる")
                .on_toggle(|on| Message::Muted(!on)),
            field("音量", self.volume_slider()),
            field("話速", self.speed_slider()),
            field("話者", voice),
            separator(),
            self.proactive_settings(),
            separator(),
            self.rule_settings(),
            separator(),
            field("エージェント", self.agent_settings()),
            field("文字起こし", self.stt_settings()),
            separator(),
            self.resident_settings(),
        ]
        .spacing(16)
        .padding(Padding::new(2.0).right(12));
        iced::widget::scrollable(content)
            .height(Length::Fill)
            .direction(iced::widget::scrollable::Direction::Vertical(
                shadcn_iced::scroll_area::scrollbar(),
            ))
            .style(shadcn_iced::scroll_area::style)
            .into()
    }

    fn speed_slider(&self) -> Element<'_, Message> {
        row![
            slider(
                f32::from(*SPEED.start())..=f32::from(*SPEED.end()),
                f32::from(self.config.speed_percent()),
                |v| Message::Speed(v.round() as u16)
            )
            .step(5.0)
            .on_release(Message::SaveSettings)
            .width(Length::Fill),
            muted(format!("{:.2}", self.config.speed), 13.0).width(42),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    }

    /// What the engineer calls about without being asked.
    fn proactive_settings(&self) -> Element<'_, Message> {
        let p = self.settings.proactive;
        let mut col = column![
            switch(p.enabled)
                .label("出来事を自動で連絡する")
                .on_toggle(Message::Proactive)
        ]
        .spacing(10);
        if p.enabled {
            let cell = |g: Group| {
                container(
                    switch(p.get(g))
                        .size(shadcn_iced::switch::Size::Sm)
                        .label(g.label())
                        .on_toggle(move |on| Message::ProactiveGroup(g, on)),
                )
                .width(Length::Fill)
            };
            for pair in Group::ALL.chunks(2) {
                let mut line = row![cell(pair[0])].spacing(8);
                line = match pair.get(1) {
                    Some(g) => line.push(cell(*g)),
                    None => line.push(space::horizontal()),
                };
                col = col.push(line);
            }
        }
        col.into()
    }

    /// How the engineer talks: how often, when not, and what it keeps in mind.
    fn rule_settings(&self) -> Element<'_, Message> {
        let r = &self.settings.rules;
        let interval = row![
            slider(0.0..=180.0, f32::from(r.interval_s), |v| {
                Message::Interval(v.round() as u16)
            })
            .step(2.0)
            .on_release(Message::SaveSettings)
            .width(Length::Fill),
            muted(format!("{}秒", r.interval_s), 13.0).width(42),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let mut plan = select(Some(PlanChoice(r.planning)), Message::Planning).width(Length::Fill);
        for n in PlanChoice::ALL {
            plan = plan.option(PlanChoice(n), PlanChoice(n).to_string());
        }
        column![
            field("自動連絡の間隔", interval),
            field("自動連絡のタイミング", self.hold_settings(Scope::Calls)),
            field("回答のタイミング", self.hold_settings(Scope::All)),
            field("裏での作戦立案", plan),
            field("覚えている指示", self.note_settings()),
            field("見張り", self.watch_settings()),
        ]
        .spacing(16)
        .into()
    }

    fn hold_settings(&self, scope: Scope) -> Element<'_, Message> {
        let draft = &self.hold_drafts[usize::from(scope == Scope::Calls)];
        let when = self.settings.rules.hold(scope);
        let choice = match draft {
            Some(_) => HoldChoice::Custom,
            None => HoldChoice::of(when),
        };
        let mut col = column![
            select(Some(choice), move |c| Message::Hold(scope, c))
                .options(HoldChoice::ALL)
                .width(Length::Fill)
        ]
        .spacing(8);
        if choice == HoldChoice::Custom {
            let shown = draft.as_deref().or(when).unwrap_or_default();
            col = col.push(
                input("brake > 10 or abs(lat_g) > 0.8", shown)
                    .on_input(move |t| Message::HoldDraft(scope, t)),
            );
            if let Err(e) = shown.parse::<crate::expr::Expr>() {
                col = col.push(muted(e, 12.0));
            }
        }
        col.into()
    }

    /// A line of text that can be dropped.
    fn droppable<'a>(&self, text: String, drop: Message) -> Element<'a, Message> {
        row![
            container(iced::widget::text(text).size(13)).width(Length::Fill),
            icon_button(lucide::X)
                .variant(button::Variant::Ghost)
                .size(button::Size::IconSm)
                .on_press(drop),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    }

    fn note_settings(&self) -> Element<'_, Message> {
        let mut col = column![].spacing(4);
        for (i, n) in self.settings.rules.notes.iter().enumerate() {
            col = col.push(self.droppable(n.clone(), Message::DropNote(i)));
        }
        col.push(
            input("例: 順位は聞かれたときだけ言う", &self.note_draft)
                .on_input(Message::NoteDraft)
                .on_submit(Message::AddNote),
        )
        .into()
    }

    fn watch_settings(&self) -> Element<'_, Message> {
        let watches = &self.settings.rules.watches;
        if watches.is_empty() {
            return muted("無線で「後ろが1秒切ったら教えて」のように頼めます", 12.0).into();
        }
        let mut col = column![].spacing(4);
        for (i, w) in watches.iter().enumerate() {
            let by = if w.planned { "（作戦）" } else { "" };
            col = col.push(self.droppable(format!("{w}{by}"), Message::DropWatch(i)));
        }
        col.into()
    }

    fn agent_settings(&self) -> Element<'_, Message> {
        let connected = self.agent_draft.trim() == self.config.agent;
        let label = match &self.agent {
            AgentStatus::Starting if connected => "接続中…",
            _ if connected => "再接続",
            _ => "接続",
        };
        let pick = row![
            select(Some(self.agent_choice), Message::AgentChoice)
                .options(AgentChoice::ALL)
                .width(Length::Fill),
            button(label).on_press_maybe(
                (self.engine.is_some() && !self.agent_draft.trim().is_empty())
                    .then_some(Message::Connect)
            ),
        ]
        .spacing(8);
        let status = match &self.agent {
            AgentStatus::Starting => "起動中".to_string(),
            AgentStatus::Ready(name) => format!("接続済み: {name}"),
            AgentStatus::Closed(e) => format!("停止: {e}"),
        };
        let mut col = column![pick].spacing(8);
        if self.agent_choice == AgentChoice::Custom {
            col = col.push(
                input("ACP アダプタの起動コマンド", &self.agent_draft)
                    .on_input(Message::AgentDraft)
                    .on_submit(Message::Connect),
            );
        }
        col.push(muted(status, 12.0)).into()
    }

    fn stt_settings(&self) -> Element<'_, Message> {
        let note = match self.config.stt {
            SttBackend::Windows => {
                "キー不要・通信なし。レース用語の精度はAPIより低めです".to_string()
            }
            SttBackend::Api if !self.has_stt_key && stt::is_openrouter(&self.config.stt_url) => {
                "APIキーがありません（環境変数か.envの OPENROUTER_API_KEY）".to_string()
            }
            SttBackend::Api => self.config.stt_url.clone(),
        };
        column![
            select(Some(self.stt_choice()), Message::Stt)
                .options(self.stt_choices())
                .width(Length::Fill),
            muted(note, 12.0),
        ]
        .spacing(8)
        .into()
    }

    /// The key and the button of the wheel, which do the same: one card, so
    /// that they do not look like two functions.
    fn talk_settings(&self) -> Element<'_, Message> {
        let (key, works) = match &self.hotkey {
            Some(Ok(key)) => (key.clone(), true),
            Some(Err(e)) => (format!("無効: {e}"), false),
            None => ("なし".into(), false),
        };
        let wheel = self.settings.wheel.as_ref();
        let mut wheel_row = self.talk_row(
            lucide::GAMEPAD_2,
            "ハンドル",
            wheel.map_or("なし".into(), |b| b.to_string()),
            wheel.is_some(),
            "ボタンを押してください",
            Capture::Wheel,
        );
        if wheel.is_some() && self.capture != Capture::Wheel {
            wheel_row = wheel_row.push(
                icon_button(lucide::X)
                    .variant(button::Variant::Ghost)
                    .size(button::Size::IconSm)
                    .on_press(Message::ClearWheel),
            );
        }
        let title = row![
            icon(lucide::MIC).size(16),
            text("押して話す")
                .size(15)
                .font(shadcn_iced::font::semibold()),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let mut card = column![
            title,
            muted("どちらを押しても同じです。離すと送ります", 12.0),
            self.talk_row(
                lucide::KEYBOARD,
                "キーボード",
                key,
                works,
                "キーを押してください（Escで中止）",
                Capture::Key,
            ),
            wheel_row,
        ]
        .spacing(10);
        if let Some(hint) = self.capture_hint {
            card = card.push(muted(hint, 12.0));
        }
        container(card)
            .padding(14)
            .width(Length::Fill)
            .style(|t: &Theme| {
                let p = palette(t);
                container::Style {
                    background: Some(Background::Color(alpha(p.foreground, 0.03))),
                    border: Border {
                        color: p.border,
                        width: 1.0,
                        radius: 10.0.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }

    /// One way to press it: what is assigned, and the button that waits for
    /// another one.
    fn talk_row<'a>(
        &self,
        symbol: shadcn_iced::icon::Icon,
        name: &'static str,
        current: String,
        assigned: bool,
        waiting: &'static str,
        capture: Capture,
    ) -> iced::widget::Row<'a, Message> {
        let active = self.capture == capture;
        let (label, next) = match (active, assigned) {
            (true, _) => ("やめる", Capture::None),
            (false, true) => ("変更", capture),
            (false, false) => ("割り当て", capture),
        };
        let value: Element<'a, Message> = if active {
            text(waiting).size(13).into()
        } else if assigned && capture == Capture::Key {
            kbd(current)
        } else if assigned {
            text(current).size(13).into()
        } else {
            muted(current, 13.0).into()
        };
        row![
            container(
                row![icon(symbol).size(14), muted(name, 13.0)]
                    .spacing(6)
                    .align_y(Alignment::Center)
            )
            .width(96),
            container(value).width(Length::Fill),
            button(label)
                .variant(if active {
                    button::Variant::Default
                } else {
                    button::Variant::Outline
                })
                .size(button::Size::Sm)
                .on_press(Message::Capture(next)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    }

    fn resident_settings(&self) -> Element<'_, Message> {
        if !crate::resident::SUPPORTED {
            return muted("常駐はWindowsで使えます", 12.0).into();
        }
        column![
            switch(self.settings.close_to_tray)
                .label("閉じてもトレイに常駐する")
                .on_toggle(Message::CloseToTray),
            switch(self.starts_with_windows)
                .label("Windows の起動時にトレイで開始する")
                .on_toggle(Message::StartWithWindows),
        ]
        .spacing(12)
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
        Some(q) if t.call => row![
            badge("自動連絡")
                .icon(lucide::BELL_RING)
                .variant(shadcn_iced::badge::Variant::Secondary),
            muted(q, 14.0),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
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
            text(crate::text::joined(t.reply.trim()))
                .size(17)
                .line_height(1.6)
                .into()
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
