//! What the engineer says without a model.

use vr_core::*;
use vr_engine::*;

/// Reports made up for each test.
#[derive(Clone)]
struct Made {
    brief: Brief,
    sectors: Option<SectorsReport>,
    car: CarReport,
    forecast: Option<ForecastReport>,
    /// Last laps by car number, `""` for the player.
    laps: Vec<(String, LapRecord)>,
    /// Values of `vr live` besides the tyre age.
    live: Vec<(&'static str, f64)>,
    strategy: Option<StrategyReport>,
}

fn no<T>() -> ApiResult<T> {
    Err(ApiError::Other("not asked".into()))
}

impl RaceApi for Made {
    fn brief(&self) -> ApiResult<Brief> {
        Ok(self.brief.clone())
    }
    fn sectors(&self, _: &SectorsParams) -> ApiResult<SectorsReport> {
        self.sectors
            .clone()
            .ok_or_else(|| ApiError::Other("none".into()))
    }
    fn car(&self) -> ApiResult<CarReport> {
        Ok(self.car.clone())
    }
    fn live(&self) -> ApiResult<LiveReport> {
        Ok(LiveReport {
            meta: self.brief.meta.clone(),
            values: std::iter::once(("tyre_age", 7.0))
                .chain(self.live.iter().copied())
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        })
    }
    fn standings(&self, _: &StandingsParams) -> ApiResult<StandingsReport> {
        no()
    }
    fn gaps(&self, _: &GapParams) -> ApiResult<GapReport> {
        no()
    }
    fn fuel(&self) -> ApiResult<FuelReport> {
        no()
    }
    fn laps(&self, p: &LapsParams) -> ApiResult<LapsReport> {
        let car = p.car.clone().unwrap_or_default();
        let laps = self.laps.iter().filter(|(c, _)| *c == car);
        Ok(LapsReport {
            meta: self.brief.meta.clone(),
            car: None,
            laps: laps.map(|(_, l)| l.clone()).collect(),
            best_lap_s: None,
            avg_clean_lap_s: None,
            trend: None,
        })
    }
    fn events(&self, _: &EventsParams) -> ApiResult<EventsReport> {
        no()
    }
    fn snapshot(&self) -> ApiResult<SnapshotReport> {
        no()
    }
    fn channels(&self, _: &ChannelsParams) -> ApiResult<ChannelsReport> {
        no()
    }
    fn info(&self, _: &InfoParams) -> ApiResult<InfoReport> {
        no()
    }
    fn setup(&self) -> ApiResult<SetupReport> {
        no()
    }
    fn handling(&self, _: &HandlingParams) -> ApiResult<HandlingReport> {
        no()
    }
    fn strategy(&self) -> ApiResult<StrategyReport> {
        self.strategy
            .clone()
            .ok_or_else(|| ApiError::Other("none".into()))
    }
    fn forecast(&self) -> ApiResult<ForecastReport> {
        self.forecast
            .clone()
            .ok_or_else(|| ApiError::Other("none".into()))
    }
}

fn meta() -> Meta {
    Meta {
        game: "g".into(),
        source: SourceKind::Live,
        connected: true,
        age_ms: Some(20),
        history: true,
        session_time_s: Some(1.0),
        capabilities: Capabilities::ALL,
    }
}

fn car(number: &str, pos: u32) -> CarRef {
    CarRef {
        idx: pos as usize,
        number: number.into(),
        driver: "D".into(),
        class_name: None,
        position: Some(pos),
        class_position: Some(pos),
    }
}

fn gap(number: &str, pos: u32, s: f64, trend: Option<f64>) -> GapInfo {
    GapInfo {
        car: car(number, pos),
        gap_s: Some(s + 0.7),
        laps: 0,
        mode: GapMode::RaceTime,
        trend_s_per_lap: trend,
        their_last_lap_s: Some(114.3),
        last_lap_delta_s: Some(-0.7),
        on_pit_road: false,
        live_s: Some(s),
        live_change_s: trend,
        live_ahead: None,
        p2p_left: None,
        p2p_active: None,
    }
}

fn fuel(left: f64, to_finish: f64) -> FuelInfo {
    FuelInfo {
        fuel_l: Some(20.0),
        capacity_l: Some(60.0),
        per_lap_l: Some(2.5),
        basis: FuelBasis::History,
        laps_sampled: 3,
        laps_of_fuel: Some(8.0),
        laps_to_finish: Some(left),
        to_finish_l: Some(to_finish),
        add_with_margin_l: Some((to_finish + 1.25).max(0.0)),
        last_lap_on_fuel: None,
    }
}

/// Fourth of twenty in a race with six laps to go.
fn made() -> Made {
    Made {
        brief: Brief {
            meta: meta(),
            session: Some(SessionSummary {
                kind: SessionKind::Race,
                phase: SessionPhase::Racing,
                track: None,
                lap: Some(5),
                laps_total: None,
                laps_remaining: None,
                time_remaining_s: Some(605.0),
                flags: vec![Flag::Green],
            }),
            player: Some(PlayerSummary {
                car: car("10", 4),
                cars_total: 20,
                cars_in_class: 20,
                last_lap_s: Some(115.04),
                best_lap_s: Some(114.84),
                avg_lap_s: Some(115.3),
                incidents: None,
                on_pit_road: false,
                on_track: true,
                live_position: Some(4),
                lap_pct: None,
                corner_now: None,
                corner_before: None,
            }),
            ahead: Some(gap("16", 3, 0.62, Some(-0.4))),
            behind: Some(gap("30", 5, 2.5, Some(0.3))),
            nearby: Vec::new(),
            fuel: Some(fuel(6.4, -5.0)),
            recent_events: Vec::new(),
            weather: None,
            rain_trend: None,
            car: None,
            leader: Some(gap("1", 1, 7.94, Some(0.2))),
            running_ahead: None,
            running_behind: None,
            field_tyres: Vec::new(),
        },
        sectors: None,
        forecast: None,
        laps: Vec::new(),
        live: Vec::new(),
        strategy: None,
        car: CarReport {
            meta: meta(),
            car_name: None,
            car: Some(CarTelemetry::default()),
            water_temp_c: Some(88.0),
            oil_temp_c: Some(101.0),
            incidents: None,
            adjustments: Vec::new(),
        },
    }
}

fn sectors(delta: [f64; 3]) -> SectorsReport {
    let row = |number: &str, role: &str, last: [f64; 3]| SectorRow {
        car: car(number, 3),
        role: role.into(),
        last_lap_s: None,
        lap: Some(4),
        last: last.map(Some).to_vec(),
        best: last.map(|t| Some(t - 0.1)).to_vec(),
        current: vec![None; 3],
    };
    SectorsReport {
        meta: meta(),
        basis: SectorBasis::GameLines,
        starts: vec![0.0, 0.3, 0.7],
        rows: vec![
            row("10", "you", [30.0, 40.0, 45.0]),
            row("16", "ahead", [30.0, 40.0, 45.0]),
        ],
        versus: Some(car("16", 3)),
        delta: delta.map(Some).to_vec(),
        losses: vec![StretchLoss {
            from: 0.65,
            to: 0.70,
            from_m: None,
            to_m: None,
            sector: 2,
            lost_s: 0.18,
            name: None,
        }],
        gains: Vec::new(),
        corners: Vec::new(),
    }
}

fn say(m: &Made, topic: SpeechTopic) -> String {
    let s = speak(m, topic).unwrap();
    assert_eq!(s.topic, topic);
    s.text
}

#[test]
fn topics_have_names() {
    for t in SpeechTopic::ALL {
        assert_eq!(t.to_string().parse::<SpeechTopic>(), Ok(t));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, format!("\"{t}\""));
    }
    let e = "gap".parse::<SpeechTopic>().unwrap_err();
    assert!(e.starts_with("one of status, position, ahead"), "{e}");
}

#[test]
fn the_car_ahead_comes_with_what_to_do() {
    let mut m = made();
    assert_eq!(
        say(&m, SpeechTopic::Ahead),
        "前と0.6秒。あと2周で追いつく、仕掛けよう。"
    );
    m.sectors = Some(sectors([0.2, -0.3, 0.05]));
    assert_eq!(
        say(&m, SpeechTopic::Ahead),
        "前と0.6秒。あと2周で追いつく、仕掛けよう。狙いはセクター2。"
    );
    m.sectors = Some(sectors([0.1, 0.0, 0.0]));
    assert!(say(&m, SpeechTopic::Ahead).ends_with("仕掛けよう。セクター1を詰めよう。"));
    // Too small to be worth a word.
    m.sectors = Some(sectors([0.09, -0.09, 0.0]));
    assert!(!say(&m, SpeechTopic::Ahead).contains("セクター"));
    // Nobody passes far away.
    m.sectors = Some(sectors([0.0, -0.3, 0.0]));
    m.brief.ahead = Some(gap("16", 3, 2.99, None));
    assert_eq!(say(&m, SpeechTopic::Ahead), "前と3.0秒。狙いはセクター2。");
    m.brief.ahead = Some(gap("16", 3, 3.0, None));
    assert_eq!(say(&m, SpeechTopic::Ahead), "前と3.0秒。");
    m.brief.ahead.as_mut().unwrap().on_pit_road = true;
    assert_eq!(
        say(&m, SpeechTopic::Ahead),
        "前と3.0秒。前はピットだ、今のうちに飛ばそう。"
    );
    m.brief.ahead.as_mut().unwrap().on_pit_road = false;
    m.brief.ahead.as_mut().unwrap().live_s = None;
    m.brief.ahead.as_mut().unwrap().gap_s = None;
    assert_eq!(say(&m, SpeechTopic::Ahead), "前との差はまだ出ていない。");
    m.brief.ahead = None;
    assert_eq!(say(&m, SpeechTopic::Ahead), "前の車は取れていない。");
    m.brief.player.as_mut().unwrap().live_position = Some(1);
    assert!(say(&m, SpeechTopic::Ahead).starts_with("トップだ、前はいない。"));
}

#[test]
fn the_outlook_fits_the_laps_left() {
    let mut m = made();
    let with = |m: &mut Made, gap_s: f64, trend: f64, left: f64| {
        m.brief.ahead = Some(gap("16", 3, gap_s, Some(trend)));
        m.brief.behind = Some(gap("30", 5, gap_s, Some(trend)));
        m.brief.fuel = Some(fuel(left, -5.0));
        (say(m, SpeechTopic::Ahead), say(m, SpeechTopic::Behind))
    };
    let (a, b) = with(&mut m, 2.0, -0.5, 4.0);
    assert_eq!(a, "前と2.0秒。あと4周で追いつく。");
    assert_eq!(b, "後ろと2.0秒。あと4周で来る。内側を守ろう。");
    let (a, b) = with(&mut m, 2.0, -0.5, 3.9);
    assert_eq!(a, "前と2.0秒。終わりまでには届かない。");
    assert_eq!(b, "後ろと2.0秒。逃げ切れる。");
    let (a, _) = with(&mut m, 1.5, -1.6, 3.9);
    assert_eq!(a, "前と1.5秒。この周で追いつく。");
    let (a, _) = with(&mut m, 1.49, -1.6, 3.9);
    assert_eq!(a, "前と1.5秒。この周で追いつく、仕掛けよう。");
    let (a, _) = with(&mut m, 1.49, 0.0, 3.9);
    assert_eq!(a, "前と1.5秒。仕掛けられる。");
    let (a, b) = with(&mut m, 2.0, 0.05, 4.0);
    assert_eq!(a, "前と2.0秒。離されてる。");
    assert_eq!(b, "後ろと2.0秒。離してる、心配ない。");
    let (a, b) = with(&mut m, 2.0, 0.049, 4.0);
    assert_eq!(a, "前と2.0秒。差は変わらない。");
    assert_eq!(b, "後ろと2.0秒。差は変わらない。");
    let (a, _) = with(&mut m, 2.0, -0.049, 4.0);
    assert_eq!(a, "前と2.0秒。差は変わらない。");
    // Without an end in sight nobody looks further than thirty laps.
    m.brief.fuel = None;
    m.brief.ahead = Some(gap("16", 3, 3.1, Some(-0.1)));
    assert!(say(&m, SpeechTopic::Ahead).contains("届かない"));
    m.brief.ahead = Some(gap("16", 3, 2.9, Some(-0.1)));
    assert!(say(&m, SpeechTopic::Ahead).contains("あと29周で追いつく"));
}

#[test]
fn the_car_behind_is_defended_against_where_it_is_quicker() {
    let mut m = made();
    m.sectors = Some(sectors([0.0, 0.25, 0.0]));
    m.brief.behind = Some(gap("30", 5, 0.8, Some(-0.2)));
    assert_eq!(
        say(&m, SpeechTopic::Behind),
        "後ろと0.8秒。近い。セクター2を守ろう。"
    );
    // Close, but going away.
    m.brief.behind = Some(gap("30", 5, 0.8, Some(0.2)));
    assert_eq!(
        say(&m, SpeechTopic::Behind),
        "後ろと0.8秒。離してる、心配ない。"
    );
    m.brief.behind.as_mut().unwrap().on_pit_road = true;
    assert_eq!(
        say(&m, SpeechTopic::Behind),
        "後ろと0.8秒。後ろはピットだ。"
    );
    m.brief.behind = None;
    assert_eq!(say(&m, SpeechTopic::Behind), "後ろは気にしなくていい。");
}

#[test]
fn a_lap_down_is_counted_in_laps() {
    let mut m = made();
    let mut g = gap("16", 3, 0.0, None);
    g.laps = 2;
    g.live_s = None;
    m.brief.ahead = Some(g);
    assert!(say(&m, SpeechTopic::Ahead).starts_with("前と2周。"));
}

#[test]
fn gaps_are_told_in_tenths() {
    let mut m = made();
    let mut ahead = |s: f64| {
        m.brief.ahead = Some(gap("16", 3, s, None));
        say(&m, SpeechTopic::Ahead)
    };
    assert!(ahead(0.0949).starts_with("前と0.09秒。"));
    assert!(ahead(0.095).starts_with("前と0.1秒。"));
    assert!(ahead(12.34).starts_with("前と12.3秒。"));
}

#[test]
fn the_place_and_the_one_thing_to_do() {
    let mut m = made();
    assert_eq!(
        say(&m, SpeechTopic::Position),
        "4番手。前は射程内、仕掛けよう。"
    );
    // As they run, whatever the board says.
    m.brief.player.as_mut().unwrap().live_position = Some(3);
    m.brief.running_ahead = Some(gap("2", 2, 4.0, Some(-0.5)));
    let advice = |m: &Made| {
        let text = say(m, SpeechTopic::Position);
        text.strip_prefix("3番手。").unwrap().to_string()
    };
    assert_eq!(advice(&m), "このペースを守ろう。");
    m.brief.running_ahead = Some(gap("2", 2, 3.0, Some(-0.5)));
    assert_eq!(advice(&m), "前に追いつける、攻めよう。");
    m.brief.behind = Some(gap("30", 5, 3.0, Some(-0.8)));
    assert_eq!(advice(&m), "前に追いつける、攻めよう。");
    // Who comes within three laps goes first.
    m.brief.behind = Some(gap("30", 5, 3.0, Some(-1.0)));
    assert_eq!(advice(&m), "後ろが来る、守ろう。");
    m.brief.running_ahead = None;
    m.brief.ahead = None;
    m.brief.behind = Some(gap("30", 5, 3.0, Some(-0.8)));
    assert_eq!(advice(&m), "後ろが来る、ペースを上げよう。");
    m.brief.behind = Some(gap("30", 5, 1.49, None));
    assert_eq!(advice(&m), "後ろが近い、守ろう。");
    m.brief.behind = Some(gap("30", 5, 1.49, Some(0.3)));
    assert_eq!(advice(&m), "このペースを守ろう。");
    m.brief.behind = Some(gap("30", 5, 1.5, None));
    assert_eq!(advice(&m), "");
    m.brief.player.as_mut().unwrap().live_position = None;
    m.brief.player.as_mut().unwrap().car.position = None;
    m.brief.player.as_mut().unwrap().car.class_position = None;
    assert_eq!(say(&m, SpeechTopic::Position), "まだ順位が出ていない。");
}

#[test]
fn a_question_nobody_understood_gets_the_state_of_things() {
    let mut m = made();
    assert_eq!(
        say(&m, SpeechTopic::Status),
        "4番手。前は射程内、仕掛けよう。残り7周。"
    );
    m.brief.fuel = Some(fuel(6.4, 3.25));
    assert_eq!(
        say(&m, SpeechTopic::Status),
        "4番手。燃料が足りない、ピットが要る。残り7周。"
    );
    m.brief.fuel.as_mut().unwrap().basis = FuelBasis::Instant;
    assert!(say(&m, SpeechTopic::Status).contains("仕掛けよう"));
    m.brief.session.as_mut().unwrap().flags = vec![Flag::Blue, Flag::YellowWaving];
    assert!(say(&m, SpeechTopic::Status).contains("近くでイエロー、抑えて。"));
    m.brief.player = None;
    m.brief.ahead = None;
    m.brief.behind = None;
    m.brief.fuel = None;
    m.brief.session.as_mut().unwrap().flags.clear();
    assert_eq!(say(&m, SpeechTopic::Status), "残り10分05秒。");
    m.brief.session.as_mut().unwrap().time_remaining_s = None;
    assert_eq!(say(&m, SpeechTopic::Status), "まだ順位も差も出ていない。");
}

#[test]
fn the_leader() {
    let mut m = made();
    assert_eq!(say(&m, SpeechTopic::Leader), "トップと7.9秒。離されてる。");
    assert_eq!(
        say(&m, SpeechTopic::LeaderLap),
        "トップは1:54.3、こっちが0.7秒遅い。"
    );
    m.brief.leader = Some(gap("1", 1, 2.0, Some(-0.5)));
    assert_eq!(
        say(&m, SpeechTopic::Leader),
        "トップと2.0秒。あと4周で追いつく、攻めよう。"
    );
    m.brief.leader = Some(gap("1", 1, 7.0, Some(-0.5)));
    assert!(say(&m, SpeechTopic::Leader).ends_with("縮めてるけど届かない。"));
    m.brief.leader = Some(gap("1", 1, 7.0, Some(0.0)));
    assert!(say(&m, SpeechTopic::Leader).ends_with("差は変わらない。"));
    m.brief.leader.as_mut().unwrap().last_lap_delta_s = Some(0.05);
    assert!(say(&m, SpeechTopic::LeaderLap).ends_with("、こっちが0.05秒速い。"));
    m.brief.leader.as_mut().unwrap().last_lap_delta_s = Some(0.049);
    assert!(say(&m, SpeechTopic::LeaderLap).ends_with("、同じペース。"));
    m.brief.leader.as_mut().unwrap().last_lap_delta_s = None;
    assert_eq!(say(&m, SpeechTopic::LeaderLap), "トップは1:54.3。");
    // Second: the leader is the car ahead.
    m.brief.leader = None;
    m.brief.ahead = Some(gap("1", 1, 2.0, None));
    assert_eq!(say(&m, SpeechTopic::Leader), "トップと2.0秒。");
    m.brief.ahead = Some(gap("16", 3, 2.0, None));
    assert_eq!(
        say(&m, SpeechTopic::Leader),
        "トップとの差はまだ出ていない。"
    );
    assert_eq!(
        say(&m, SpeechTopic::LeaderLap),
        "トップのラップはまだ出ていない。"
    );
    m.brief.player.as_mut().unwrap().live_position = Some(1);
    assert!(say(&m, SpeechTopic::Leader).starts_with("トップはこっちだ。"));
    assert!(say(&m, SpeechTopic::LeaderLap).starts_with("トップはこっちだ。1:55.0"));
}

#[test]
fn laps_and_where_to_find_time() {
    let mut m = made();
    m.sectors = Some(sectors([0.2, 0.0, 0.0]));
    assert_eq!(
        say(&m, SpeechTopic::Pace),
        "1:55.0、いいペース。セクター1を詰めよう。"
    );
    assert_eq!(
        say(&m, SpeechTopic::AheadLap),
        "前は1:54.3、こっちが0.7秒遅い。"
    );
    assert_eq!(
        say(&m, SpeechTopic::Lap),
        "1:55.0、いいペース。トップと7.9秒、離された。セクター1を詰めよう。"
    );
    m.brief.behind = Some(gap("30", 5, 1.0, None));
    assert!(say(&m, SpeechTopic::Lap).ends_with("離された。後ろが近い。"));
    m.brief.leader = Some(gap("1", 1, 7.9, Some(-0.05)));
    assert!(say(&m, SpeechTopic::Lap).contains("トップと7.9秒、縮めた。"));
    m.brief.leader = Some(gap("1", 1, 7.9, Some(0.04)));
    assert!(say(&m, SpeechTopic::Lap).contains("トップと7.9秒。"));
    let p = m.brief.player.as_mut().unwrap();
    p.best_lap_s = Some(114.7);
    assert!(say(&m, SpeechTopic::Pace).starts_with("1:55.0、ベストから0.3秒落ち。"));
    m.brief.player.as_mut().unwrap().best_lap_s = Some(115.04);
    assert!(say(&m, SpeechTopic::Pace).starts_with("1:55.0、ベストだ。"));
    m.brief.player.as_mut().unwrap().last_lap_s = None;
    assert_eq!(say(&m, SpeechTopic::Pace), "まだ1周の記録が無い。");
    m.brief.ahead.as_mut().unwrap().their_last_lap_s = None;
    assert_eq!(
        say(&m, SpeechTopic::AheadLap),
        "前の車のラップはまだ出ていない。"
    );
    m.brief.ahead = None;
    m.brief.behind = None;
    m.brief.leader = None;
    assert_eq!(say(&m, SpeechTopic::Lap), "まだ1周の記録が無い。");
}

#[test]
fn sector_times_and_where_time_is_lost() {
    let mut m = made();
    assert!(speak(&m, SpeechTopic::Sectors).is_err());
    m.sectors = Some(sectors([0.2, -0.3, 0.04]));
    assert_eq!(
        say(&m, SpeechTopic::Sectors),
        "前の周は自己ベストから0.3秒落ち。負けてるのはセクター1、0.2秒。"
    );
    assert_eq!(
        say(&m, SpeechTopic::Losing),
        "セクター2の終盤で0.2秒負けてる、そこを詰めよう。"
    );
    // A known track names the place.
    m.sectors.as_mut().unwrap().losses[0].name = Some("スプーン".into());
    assert_eq!(
        say(&m, SpeechTopic::Sectors),
        "前の周は自己ベストから0.3秒落ち。スプーンで前に0.2秒負けてる。"
    );
    assert_eq!(
        say(&m, SpeechTopic::Losing),
        "スプーンで0.2秒負けてる、そこを詰めよう。"
    );
    m.sectors.as_mut().unwrap().losses[0].name = None;
    // After a stop of either car the sectors tell nothing.
    let lap = |pit| LapRecord {
        lap: 5,
        time_s: Some(90.0),
        fuel_used_l: None,
        pit,
        caution: false,
        position: None,
        session_time_s: 0.0,
    };
    let pit = "ピットかイエローの周は比べられない、次の周で伝える。";
    m.laps = vec![("".into(), lap(true))];
    assert_eq!(say(&m, SpeechTopic::Losing), pit);
    let them = m.sectors.as_ref().unwrap().versus.clone().unwrap().number;
    m.laps = vec![("".into(), lap(false)), (them.clone(), lap(true))];
    assert_eq!(say(&m, SpeechTopic::Losing), pit);
    m.laps = vec![("".into(), lap(false)), (them, lap(false))];
    assert_eq!(
        say(&m, SpeechTopic::Losing),
        "セクター2の終盤で0.2秒負けてる、そこを詰めよう。"
    );
    m.laps.clear();
    let r = m.sectors.as_mut().unwrap();
    r.losses[0].lost_s = 0.049;
    assert_eq!(
        say(&m, SpeechTopic::Losing),
        "負けてるのはセクター1、0.2秒。"
    );
    m.sectors = Some(sectors([0.04, -0.3, 0.04]));
    m.sectors.as_mut().unwrap().losses.clear();
    assert_eq!(
        say(&m, SpeechTopic::Losing),
        "負けてる区間は無い、いいペースだ。"
    );
    assert_eq!(
        say(&m, SpeechTopic::Sectors),
        "前の周は自己ベストから0.3秒落ち。"
    );
    let r = m.sectors.as_mut().unwrap();
    let last = r.rows[0].last.clone();
    r.rows[0].best = last;
    assert_eq!(say(&m, SpeechTopic::Sectors), "前の周は自己ベスト並み。");
    m.sectors.as_mut().unwrap().versus = None;
    assert_eq!(say(&m, SpeechTopic::Sectors), "前の周は自己ベスト並み。");
    let r = m.sectors.as_mut().unwrap();
    r.versus = Some(car("16", 3));
    r.rows[0].best = r.rows[0].last.iter().map(|t| t.map(|t| t - 0.1)).collect();
    let r = m.sectors.as_mut().unwrap();
    r.versus = None;
    r.rows[0].best[1] = Some(39.5);
    assert!(say(&m, SpeechTopic::Sectors).ends_with("セクター2が自己ベストから0.5秒落ち。"));
    let r = m.sectors.as_mut().unwrap();
    r.rows[0].last = vec![Some(30.0), None, Some(45.0)];
    // Without every sector, no verdict on the lap.
    let partly = say(&m, SpeechTopic::Sectors);
    assert!(
        partly.ends_with("が自己ベストから0.1秒落ち。") && !partly.starts_with("前の周"),
        "{partly}"
    );
    m.sectors.as_mut().unwrap().rows[0].last = vec![None; 3];
    assert_eq!(
        say(&m, SpeechTopic::Sectors),
        "区間タイムは1周走り切ってから出せる。"
    );
    m.sectors.as_mut().unwrap().basis = SectorBasis::None;
    for t in [SpeechTopic::Sectors, SpeechTopic::Losing] {
        assert_eq!(say(&m, t), "区間タイムは1周走り切ってから出せる。");
    }
}

#[test]
fn the_spot_on_the_lap_is_named_by_its_part_of_the_sector() {
    let mut m = made();
    let mut at = |from: f64, sector: usize| {
        let mut r = sectors([0.2, 0.0, 0.0]);
        r.losses[0].from = from;
        r.losses[0].to = from + 0.05;
        r.losses[0].sector = sector;
        m.sectors = Some(r);
        say(&m, SpeechTopic::Losing)
    };
    assert!(at(0.0, 1).contains("セクター1の序盤"));
    assert!(at(0.12, 1).contains("セクター1の中盤"));
    assert!(at(0.25, 1).contains("セクター1の終盤"));
    assert!(at(0.95, 3).contains("セクター3の終盤"));
}

#[test]
fn fuel_is_a_verdict() {
    let mut m = made();
    assert_eq!(
        say(&m, SpeechTopic::Fuel),
        "燃料は大丈夫だ、このまま完走する。"
    );
    assert_eq!(
        say(&m, SpeechTopic::FuelLaps),
        "燃料はあと8周分。完走できる。"
    );
    // On lap five with eight laps in the tank.
    m.brief.fuel = Some(fuel(10.0, 5.0));
    assert_eq!(
        say(&m, SpeechTopic::Fuel),
        "燃料が足りない。12周目までにピット、7リッター入れる。"
    );
    assert_eq!(
        say(&m, SpeechTopic::FuelLaps),
        "燃料はあと8周分。ピットが要る。"
    );
    m.brief.fuel.as_mut().unwrap().laps_of_fuel = Some(0.4);
    assert!(say(&m, SpeechTopic::Fuel).contains("5周目までにピット"));
    m.brief.fuel.as_mut().unwrap().add_with_margin_l = None;
    m.brief.session.as_mut().unwrap().lap = None;
    assert_eq!(say(&m, SpeechTopic::Fuel), "燃料が足りない。");
    m.brief.fuel = Some(fuel(10.0, -0.99));
    assert_eq!(
        say(&m, SpeechTopic::Fuel),
        "燃料はぎりぎりだ、少し抑えて完走しよう。"
    );
    m.brief.fuel = Some(fuel(10.0, -1.0));
    assert!(say(&m, SpeechTopic::Fuel).contains("大丈夫だ"));
    m.brief.session.as_mut().unwrap().kind = SessionKind::Practice;
    assert_eq!(say(&m, SpeechTopic::Fuel), "燃料はあと8周分。");
    m.brief.fuel.as_mut().unwrap().laps_of_fuel = None;
    assert_eq!(
        say(&m, SpeechTopic::Fuel),
        "燃料は20リッター、1周走れば何周持つか出せる。"
    );
    m.brief.fuel = None;
    for t in [SpeechTopic::Fuel, SpeechTopic::FuelLaps] {
        assert_eq!(say(&m, t), "燃料は取れていない。");
    }
}

#[test]
fn what_is_left_of_the_race() {
    let mut m = made();
    assert_eq!(say(&m, SpeechTopic::Left), "残り7周。");
    m.brief.fuel = Some(fuel(0.99, 2.0));
    assert_eq!(
        say(&m, SpeechTopic::Left),
        "この周で終わり。燃料が足りない、ピットが要る。"
    );
    m.brief.fuel = Some(fuel(1.0, -2.0));
    assert_eq!(say(&m, SpeechTopic::Left), "残り1周。");
    m.brief.session.as_mut().unwrap().phase = SessionPhase::Checkered;
    assert_eq!(say(&m, SpeechTopic::Left), "チェッカーは出てる。");
    let s = m.brief.session.as_mut().unwrap();
    s.phase = SessionPhase::Racing;
    s.kind = SessionKind::Practice;
    assert_eq!(say(&m, SpeechTopic::Left), "残り10分05秒。");
    m.brief.session.as_mut().unwrap().time_remaining_s = None;
    assert_eq!(say(&m, SpeechTopic::Left), "残りはまだ分からない。");
}

#[test]
fn outside_races_the_cars_around_matter() {
    let mut m = made();
    m.brief.session.as_mut().unwrap().kind = SessionKind::Practice;
    m.brief.nearby = vec![
        RelativeCar {
            car: car("8", 9),
            rel_s: -0.8,
            same_class: true,
            lap_diff: 0,
        },
        RelativeCar {
            car: car("9", 8),
            rel_s: 1.2,
            same_class: true,
            lap_diff: 0,
        },
    ];
    assert_eq!(say(&m, SpeechTopic::Ahead), "すぐ前に車、1.2秒。");
    assert_eq!(say(&m, SpeechTopic::Behind), "すぐ後ろに車、0.8秒。");
    m.brief.nearby.clear();
    assert_eq!(say(&m, SpeechTopic::Behind), "後ろは空いてる。");
    assert_eq!(say(&m, SpeechTopic::Leader), "トップとはベストで8.6秒差。");
    assert_eq!(say(&m, SpeechTopic::Position), "4番手。");
}

#[test]
fn flags_and_weather() {
    let mut m = made();
    assert_eq!(say(&m, SpeechTopic::Flags), "旗は無い、全開でいい。");
    m.brief.session.as_mut().unwrap().flags = vec![Flag::Green, Flag::Blue, Flag::Debris];
    assert_eq!(
        say(&m, SpeechTopic::Flags),
        "ブルーフラッグ、後ろに譲ろう。デブリに注意。"
    );
    assert_eq!(say(&m, SpeechTopic::Weather), "天気は取れていない。");
    m.brief.weather = Some(Weather {
        wetness: Some(vr_core::Wetness::LightlyWet),
        precipitation: Some(0.31),
        ..Default::default()
    });
    m.brief.rain_trend = Some(RainTrend {
        per_min: 0.04,
        direction: RainDirection::Rising,
    });
    assert_eq!(
        say(&m, SpeechTopic::Weather),
        "路面は少し濡れてる。雨が強くなる、備えよう。"
    );
    m.brief.rain_trend.as_mut().unwrap().direction = RainDirection::Easing;
    assert!(say(&m, SpeechTopic::Weather).ends_with("雨は弱まる。"));
    m.brief.rain_trend.as_mut().unwrap().per_min = -0.05;
    assert_eq!(
        say(&m, SpeechTopic::Weather),
        "路面は少し濡れてる。雨は弱まってる、7分ほどで止む見込み。"
    );
    m.brief.rain_trend.as_mut().unwrap().per_min = 0.04;
    m.brief.rain_trend = None;
    assert!(say(&m, SpeechTopic::Weather).ends_with("雨が降ってる。"));
    let w = m.brief.weather.as_mut().unwrap();
    w.precipitation = Some(0.0);
    assert!(say(&m, SpeechTopic::Weather).ends_with("濡れてる。雨は無い。"));
    m.brief.weather.as_mut().unwrap().wetness = Some(vr_core::Wetness::Dry);
    assert_eq!(say(&m, SpeechTopic::Weather), "ドライ、雨は無い。");
    m.brief.weather.as_mut().unwrap().track_temp_c = Some(31.4);
    assert_eq!(
        say(&m, SpeechTopic::Weather),
        "ドライ、雨は無い。路面31度。"
    );
    m.brief.weather.as_mut().unwrap().wetness = Some(vr_core::Wetness::ExtremelyWet);
    assert!(
        say(&m, SpeechTopic::Weather).starts_with("路面は水浸し。"),
        "{}",
        say(&m, SpeechTopic::Weather)
    );
    m.brief.weather = Some(Weather::default());
    assert_eq!(say(&m, SpeechTopic::Weather), "天気は取れていない。");
}

#[test]
fn damage_is_a_verdict() {
    let mut m = made();
    let fine = "ダメージは無い、このまま行ける。";
    assert_eq!(say(&m, SpeechTopic::Damage), fine);
    let c = m.car.car.as_mut().unwrap();
    c.warnings = vec![Warning::PitLimiter];
    c.pit.repair_left_s = Some(0.4);
    c.pit.optional_repair_left_s = Some(0.4);
    assert_eq!(say(&m, SpeechTopic::Damage), fine);
    // The laps tell how bad it is.
    m.car.car.as_mut().unwrap().warnings = vec![Warning::Damage];
    let light = "ダメージは軽い、このまま行こう。";
    assert_eq!(say(&m, SpeechTopic::Damage), light);
    let lap = |m: &mut Made, last: Option<f64>| {
        m.brief.player.as_mut().unwrap().last_lap_s = last;
        say(m, SpeechTopic::Damage)
    };
    assert_eq!(lap(&mut m, Some(115.83)), light);
    // 1.0 s a lap over the 5.2 laps of 605 s left is less than a stop.
    assert_eq!(
        lap(&mut m, Some(115.85)),
        "ダメージで毎周1.0秒落ち。残り6周なら直さない方が速い、このまま行こう。"
    );
    // Over 40 laps it is more.
    m.brief.session.as_mut().unwrap().laps_remaining = Some(40);
    assert_eq!(
        lap(&mut m, Some(115.85)),
        "ダメージで毎周1.0秒落ちてる。直した方が速い、ピットで直そう。"
    );
    // 1.01 s over 24 laps is 24.2 s, less than the 25 s stop; over 25 it is more.
    m.brief.session.as_mut().unwrap().laps_remaining = Some(24);
    assert_eq!(
        lap(&mut m, Some(115.85)),
        "ダメージで毎周1.0秒落ち。残り24周なら直さない方が速い、このまま行こう。"
    );
    m.brief.session.as_mut().unwrap().laps_remaining = Some(25);
    assert!(lap(&mut m, Some(115.85)).ends_with("ピットで直そう。"));
    m.brief.session.as_mut().unwrap().laps_remaining = None;
    assert_eq!(
        lap(&mut m, None),
        "ダメージがある。まだ周のタイムが無い、次の周で影響を伝える。"
    );
    m.car.car.as_mut().unwrap().warnings.clear();
    assert_eq!(lap(&mut m, None), fine);
    m.car.car.as_mut().unwrap().pit.optional_repair_left_s = Some(0.5);
    assert!(lap(&mut m, None).starts_with("ダメージがある"));
    // Trouble that does not wait.
    m.car.car.as_mut().unwrap().warnings = vec![Warning::OilPressure, Warning::Damage];
    assert_eq!(lap(&mut m, None), "油圧が低い、止めた方がいい。");
    let must = "修理が要る、この周ピットに入ろう。";
    m.car.car.as_mut().unwrap().pit.repair_left_s = Some(0.5);
    assert_eq!(lap(&mut m, None), must);
    m.car.car.as_mut().unwrap().pit.repair_left_s = None;
    m.brief.session.as_mut().unwrap().flags = vec![Flag::Repair];
    assert_eq!(lap(&mut m, None), must);
    m.car.car = None;
    assert_eq!(lap(&mut m, None), "車の状態は取れていない。");
    assert_eq!(say(&m, SpeechTopic::Tyres), "タイヤの状態は取れていない。");
}

#[test]
fn tyres_are_a_verdict() {
    let mut m = made();
    assert_eq!(say(&m, SpeechTopic::Tyres), "タイヤの状態は取れていない。");
    assert_eq!(
        say(&m, SpeechTopic::TyreTemps),
        "タイヤの温度は取れていない。"
    );
    let worst = |m: &mut Made, tread: f64| {
        let t = &mut m.car.car.as_mut().unwrap().tyres;
        t.lf.tread = [Some(0.9), Some(0.85), Some(0.88)];
        t.rr.tread = [Some(tread), None, None];
        say(m, SpeechTopic::Tyres)
    };
    assert_eq!(worst(&mut m, 0.4), "タイヤは大丈夫だ、攻めていい。");
    assert_eq!(worst(&mut m, 0.39), "右後ろが減ってる、労ろう。");
    assert_eq!(worst(&mut m, 0.2), "右後ろが減ってる、労ろう。");
    assert_eq!(worst(&mut m, 0.19), "右後ろが限界だ、替えよう。");
    let hottest = |m: &mut Made, c: f64| {
        let t = &mut m.car.car.as_mut().unwrap().tyres;
        for t in [&mut t.rf, &mut t.lr, &mut t.rr] {
            t.temp_c = [Some(90.0); 3];
        }
        t.lf.temp_c = [Some(c); 3];
        say(m, SpeechTopic::TyreTemps)
    };
    // A third of the way above the others: eight above the mean of all.
    assert_eq!(
        hottest(&mut m, 100.6),
        "最後に止まった時で平均93度。偏りは無い、大丈夫だ。"
    );
    assert_eq!(
        hottest(&mut m, 100.7),
        "最後に止まった時で平均93度。左前だけ熱い、労ろう。"
    );
}

#[test]
fn the_end_of_the_race_is_a_verdict() {
    let mut m = made();
    let text = |m: &Made| say(m, SpeechTopic::Forecast);
    assert!(speak(&m, SpeechTopic::Forecast).is_err());
    let other = |number: &str, gap_s: f64, flag: f64, meets: Option<f64>| ForecastCar {
        car: car(number, 3),
        gap_s,
        pace_s: None,
        slower_s: None,
        at_flag_s: Some(flag),
        meets_in_laps: meets,
    };
    let of = |position: u32, finish: u32, cars: Vec<ForecastCar>| ForecastReport {
        meta: meta(),
        laps_left: Some(6.0),
        position: Some(position),
        finish: Some(finish),
        pace_s: Some(115.0),
        cars,
    };
    m.forecast = Some(of(4, 3, vec![other("16", 2.0, -1.0, Some(4.0))]));
    assert_eq!(text(&m), "このままなら3番手。あと4周で前に追いつく。");
    m.forecast = Some(of(4, 5, vec![other("30", -2.0, 1.0, Some(4.0))]));
    assert_eq!(text(&m), "このままなら5番手。あと4周で後ろが来る、守ろう。");
    // What it would take, if a driver can find it.
    m.forecast = Some(of(
        4,
        4,
        vec![other("1", 9.0, 9.0, None), other("16", 2.0, 1.2, None)],
    ));
    assert_eq!(
        text(&m),
        "このままなら4番手。今より毎周0.2秒速ければ前に届く。"
    );
    m.forecast = Some(of(4, 4, vec![other("16", 2.0, 3.0, None)]));
    assert_eq!(
        text(&m),
        "このままなら4番手。今より毎周0.5秒速ければ前に届く。"
    );
    m.forecast = Some(of(4, 4, vec![other("16", 2.0, 3.1, None)]));
    assert_eq!(text(&m), "このままなら4番手。");
    m.forecast = Some(of(2, 1, vec![other("1", 2.0, -1.0, Some(4.0))]));
    assert_eq!(text(&m), "優勝できる、攻めよう。あと4周で前に追いつく。");
    m.forecast = Some(of(1, 1, vec![other("2", -2.0, -3.0, None)]));
    assert_eq!(text(&m), "このまま優勝できる。");
    m.forecast.as_mut().unwrap().finish = None;
    assert_eq!(text(&m), "あと2周走れば見通しを出せる。");
}

#[test]
fn old_data_is_not_told_as_new() {
    let mut m = made();
    m.brief.meta.age_ms = Some(3000);
    assert!(say(&m, SpeechTopic::Ahead).starts_with("前と"));
    m.brief.meta.age_ms = Some(3001);
    assert_eq!(
        say(&m, SpeechTopic::Ahead),
        "テレメトリが途切れてる。戻ったら伝える。"
    );
    m.brief.meta.age_ms = Some(0);
    m.brief.meta.connected = false;
    assert_eq!(
        say(&m, SpeechTopic::Fuel),
        "テレメトリが途切れてる。戻ったら伝える。"
    );
    m.brief.session = None;
    let s = speak(&m, SpeechTopic::Fuel).unwrap();
    assert_eq!(
        s.to_text(),
        "テレメトリが来ていない。コースに出たら答える。\n"
    );
}

#[test]
fn the_ideal_lap_takes_the_sectors_done_now_and_empty_comparisons_go() {
    let mut r = sectors([0.2, -0.3, 0.04]);
    assert!(
        r.to_text().contains(" | ideal lap 1:54.700"),
        "{}",
        r.to_text()
    );
    // A sector without a best, done on this lap; one done faster than the best.
    r.rows[0].best[0] = None;
    r.rows[0].current = vec![Some(29.0), Some(39.0), None];
    let text = r.to_text();
    assert!(text.contains(" | ideal lap 1:52.900"), "{text}");
    // The lap so far against the best sectors there are: S1 has none.
    assert!(
        text.contains(" | this lap so far -0.90s against your best sectors (S2, + is slower)"),
        "{text}"
    );
    r.rows[0].best[0] = Some(29.5);
    let text = r.to_text();
    assert!(
        text.contains(" | this lap so far -1.40s against your best sectors (S1-S2, + is slower)"),
        "{text}"
    );
    r.rows[0].best[0] = None;
    assert!(text.contains("you vs #16"), "{text}");
    r.delta = vec![None; 3];
    r.rows[0].current = vec![None; 3];
    let text = r.to_text();
    assert!(!text.contains("you vs"), "{text}");
    assert!(!text.contains("ideal lap"), "{text}");
}

/// A strategy of lap 5: the fuel window and the best plans as given.
fn strategy(window: Option<(i32, i32)>, to_end: bool, plans: &[&[i32]]) -> StrategyReport {
    serde_json::from_value(serde_json::json!({
        "meta": meta(),
        "missing": [],
        "lap": 5,
        "laps_total": 20,
        "stops_done": 0,
        "tyre_age": 5,
        "fade": null,
        "pit_loss": {"seconds": null, "basis": "unknown", "stops": 0},
        "fuel_window": window,
        "fuel_to_end": to_end,
        "plans": plans
            .iter()
            .map(|s| serde_json::json!({"stops": s, "behind_best_s": 0.0}))
            .collect::<Vec<_>>(),
        "rejoin_now": null,
        "rivals": [],
        "calls": [],
    }))
    .unwrap()
}

#[test]
fn box_or_stay_out() {
    let mut m = made();
    m.live = vec![("stop_in", 0.0)];
    assert_eq!(
        say(&m, SpeechTopic::Box),
        "ボックス、ボックス。この周入ろう。"
    );
    m.live = vec![("stop_in", 3.0)];
    assert_eq!(
        say(&m, SpeechTopic::Box),
        "ステイアウト。ピットは8周目、あと3周。"
    );
    // Without a plan told: the fuel decides, then the plans.
    m.live = Vec::new();
    m.strategy = Some(strategy(Some((6, 9)), false, &[&[8]]));
    assert_eq!(
        say(&m, SpeechTopic::Box),
        "ステイアウト。燃料で9周目までに入る。"
    );
    m.strategy = Some(strategy(Some((3, 5)), false, &[&[5]]));
    assert_eq!(
        say(&m, SpeechTopic::Box),
        "ボックス、ボックス。燃料が持たない。"
    );
    m.strategy = Some(strategy(None, true, &[&[]]));
    assert_eq!(
        say(&m, SpeechTopic::Box),
        "ステイアウト。ピット無しで完走する。"
    );
    m.strategy = Some(strategy(None, true, &[]));
    assert_eq!(
        say(&m, SpeechTopic::Box),
        "ステイアウト。今は入らなくていい。"
    );
    // What comes before any plan.
    m.brief.session.as_mut().unwrap().flags.push(Flag::Repair);
    assert_eq!(say(&m, SpeechTopic::Box), "ボックス、ボックス。修理だ。");
    m.brief.player.as_mut().unwrap().on_pit_road = true;
    assert_eq!(
        say(&m, SpeechTopic::Box),
        "もうピットレーンだ、速度に注意。"
    );
    m.brief.session.as_mut().unwrap().kind = SessionKind::Practice;
    assert_eq!(
        say(&m, SpeechTopic::Box),
        "レースじゃない、いつ入ってもいい。"
    );
}
