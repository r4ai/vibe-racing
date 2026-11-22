//! The bundled UI font: Noto Sans JP as static weights, renamed
//! (see `assets/fonts/make_fonts.py` and `assets/fonts/OFL.txt`).
//!
//! Bundled because a system variable font (Windows' `NotoSansJP-VF.ttf`
//! defaults to Thin) or a missing one makes cosmic-text pick a different
//! fallback font per script, mixing kanji and kana from unrelated families.

pub const FAMILY: &str = "VR Radio Sans";

pub const FILES: [&[u8]; 3] = [
    include_bytes!("../assets/fonts/VRRadioSans-Regular.ttf"),
    include_bytes!("../assets/fonts/VRRadioSans-Medium.ttf"),
    include_bytes!("../assets/fonts/VRRadioSans-Bold.ttf"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The family name inside each file is what `FAMILY` refers to.
    #[test]
    fn files_carry_the_family_name() {
        let utf16: Vec<u8> = FAMILY.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for file in FILES {
            assert_eq!(&file[..4], &[0, 1, 0, 0], "TrueType outlines");
            assert!(file.windows(utf16.len()).any(|w| w == utf16.as_slice()));
        }
    }
}
