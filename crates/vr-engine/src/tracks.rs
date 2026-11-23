//! The names drivers give the corners of a track. A corner number counted
//! from the line means nothing on the radio; "Spoon" does.
//!
//! Each place begins where the braking for it begins and is given in shares
//! of the lap from the line. Read from laps driven there (the slowest point
//! and the brake points of each corner), not from a map.

/// One named place of a track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bend {
    pub name: &'static str,
    /// Shares of the lap.
    pub from: f64,
    pub to: f64,
}

const fn bend(name: &'static str, from: f64, to: f64) -> Bend {
    Bend { name, from, to }
}

/// Suzuka Grand Prix, 5.76 km in iRacing. Laps of 2026-11-22/23 (FIA F4).
const SUZUKA: &[Bend] = &[
    bend("1コーナー", 0.120, 0.145),
    bend("2コーナー", 0.145, 0.165),
    bend("S字", 0.165, 0.265),
    bend("逆バンク", 0.265, 0.290),
    bend("ダンロップ", 0.290, 0.350),
    bend("デグナー1つ目", 0.350, 0.405),
    bend("デグナー2つ目", 0.405, 0.440),
    bend("ヘアピン", 0.470, 0.530),
    bend("200R", 0.530, 0.580),
    bend("スプーン", 0.630, 0.720),
    bend("130R", 0.820, 0.890),
    bend("シケイン", 0.900, 0.945),
    bend("最終コーナー", 0.945, 0.990),
];

/// Spa-Francorchamps Grand Prix, 6.93 km in iRacing. Laps of 2026-11-21.
const SPA: &[Bend] = &[
    bend("ラ・ソース", 0.030, 0.080),
    bend("オー・ルージュ", 0.090, 0.150),
    bend("レ・コーム", 0.325, 0.390),
    bend("ブリュッセル", 0.410, 0.455),
    bend("ノーネーム", 0.455, 0.500),
    bend("プーオン", 0.525, 0.580),
    bend("ファニョー", 0.620, 0.660),
    bend("カンパス", 0.660, 0.690),
    bend("スタブロ", 0.690, 0.730),
    bend("ブランシモン", 0.800, 0.860),
    bend("バスストップ", 0.940, 0.990),
];

/// The named places of a track, if it is known. `track` is the name the game
/// gives with its layout; the length tells layouts of the same place apart.
pub fn bends(track: &str, length_m: f64) -> Option<&'static [Bend]> {
    let t = track.to_lowercase();
    let near = |km: f64| (length_m / 1000.0 - km).abs() < 0.12;
    if t.contains("suzuka") && near(5.78) {
        Some(SUZUKA)
    } else if t.contains("spa") && t.contains("francorchamps") && near(6.95) {
        Some(SPA)
    } else {
        None
    }
}

/// The corners of a track as the game names it.
pub fn of_track(t: &vr_core::TrackInfo) -> Option<&'static [Bend]> {
    let name = format!("{} {}", t.name, t.config.as_deref().unwrap_or_default());
    bends(&name, t.length_m?)
}

/// The last corner driven through before a share of the lap: one that ended
/// there or before, over the line from the end of the lap before.
pub fn last_before(bends: &[Bend], at: f64) -> Option<&'static str> {
    let at = at.rem_euclid(1.0);
    bends
        .iter()
        .filter(|b| b.to <= at)
        .max_by(|a, b| a.to.total_cmp(&b.to))
        .or_else(|| bends.iter().max_by(|a, b| a.to.total_cmp(&b.to)))
        .map(|b| b.name)
}

/// The place at a share of the lap.
pub fn name_at(bends: &[Bend], at: f64) -> Option<&'static str> {
    bends
        .iter()
        .find(|b| (b.from..b.to).contains(&at.rem_euclid(1.0)))
        .map(|b| b.name)
}

/// The corners within a stretch of the lap, the most of it first: two at the
/// most, or the nearest one when none lies in it.
pub fn names_in(bends: &[Bend], from: f64, to: f64) -> Option<String> {
    let mut within: Vec<(f64, &str)> = bends
        .iter()
        .map(|b| (b.to.min(to) - b.from.max(from), b.name))
        .filter(|(o, _)| *o > 0.0)
        .collect();
    within.sort_by(|a, b| b.0.total_cmp(&a.0));
    if within.is_empty() {
        let mid = (from + to) / 2.0;
        let near = bends.iter().min_by(|a, b| {
            let d = |x: &Bend| (x.from - mid).abs().min((x.to - mid).abs());
            d(a).total_cmp(&d(b))
        })?;
        return Some(format!("{}付近", near.name));
    }
    Some(
        within
            .iter()
            .take(2)
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join("・"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_corner_just_driven() {
        let s = bends("Suzuka International Racing Course - Grand Prix", 5756.5).unwrap();
        // In Spoon: the last one done is 200R.
        assert_eq!(name_at(s, 0.66), Some("スプーン"));
        assert_eq!(last_before(s, 0.66), Some("200R"));
        // Out of Spoon, towards 130R.
        assert_eq!(last_before(s, 0.75), Some("スプーン"));
        assert_eq!(last_before(s, 0.72), Some("スプーン"));
        // Over the line: the last corner of the lap before.
        assert_eq!(last_before(s, 0.05), Some("最終コーナー"));
        assert_eq!(last_before(&[], 0.5), None);
    }

    #[test]
    fn stretches_are_named_by_their_corners() {
        let s = bends("Suzuka International Racing Course - Grand Prix", 5756.5).unwrap();
        // Mostly the S curves, a little of the second corner.
        assert_eq!(names_in(s, 0.15, 0.20).as_deref(), Some("S字・2コーナー"));
        assert_eq!(names_in(s, 0.25, 0.30).as_deref(), Some("逆バンク・S字"));
        assert_eq!(names_in(s, 0.65, 0.70).as_deref(), Some("スプーン"));
        // Between corners: the nearest one.
        assert_eq!(names_in(s, 0.76, 0.80).as_deref(), Some("130R付近"));
        assert_eq!(names_in(&[], 0.1, 0.2), None);
    }

    #[test]
    fn known_tracks_have_names() {
        let suzuka = bends("Suzuka International Racing Course - Grand Prix", 5756.5).unwrap();
        // The slowest points of a lap there.
        assert_eq!(name_at(suzuka, 0.505), Some("ヘアピン"));
        assert_eq!(name_at(suzuka, 0.684), Some("スプーン"));
        assert_eq!(name_at(suzuka, 0.933), Some("シケイン"));
        assert_eq!(name_at(suzuka, 1.13), Some("1コーナー"));
        // Straights have no name.
        assert_eq!(name_at(suzuka, 0.05), None);
        let spa = bends("Circuit de Spa-Francorchamps - Grand Prix Pits", 6932.6).unwrap();
        assert_eq!(name_at(spa, 0.056), Some("ラ・ソース"));
        assert_eq!(name_at(spa, 0.974), Some("バスストップ"));
        // Other layouts, other tracks.
        assert_eq!(
            bends("Suzuka International Racing Course - East", 2243.0),
            None
        );
        assert_eq!(bends("Suzuka", 5650.0), None);
        assert!(bends("Suzuka", 5900.0).is_none());
        assert!(bends("Monza", 5793.0).is_none());
        assert!(bends("Spa", 6950.0).is_none());
    }

    #[test]
    fn places_follow_each_other() {
        for b in [SUZUKA, SPA] {
            for w in b.windows(2) {
                assert!(w[0].from < w[0].to && w[0].to <= w[1].from, "{w:?}");
            }
        }
    }
}
