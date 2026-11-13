//! The emulated screen. BREW ran on handsets from 128x128 to 240x400, and a game was usually
//! built for exactly one of them.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Resolution {
    pub width: u16,
    pub height: u16,
}

impl Resolution {
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    /// The most common BREW handset screens. Image-based detection only accepts these: a 100x37
    /// image inside a `.bar` is a button, not the screen.
    pub const PRESETS: [Resolution; 10] = [
        Self::new(128, 128),
        Self::new(128, 160),
        Self::new(132, 176),
        Self::new(176, 208),
        Self::new(176, 220),
        Self::new(208, 208),
        Self::new(240, 260),
        Self::new(240, 320),
        Self::new(320, 240),
        Self::new(240, 400),
    ];

    /// The most frequent screen among BREW 2.x/3.x handsets.
    pub const FALLBACK: Resolution = Self::new(176, 220);

    pub fn area(self) -> u32 {
        u32::from(self.width) * u32::from(self.height)
    }

    /// A plausible size for a phone screen of the era; filters out the `1x1` and `1024x768` that
    /// show up by accident in file names.
    pub fn is_plausible(self) -> bool {
        (96..=480).contains(&self.width) && (96..=480).contains(&self.height)
    }

    pub fn is_preset(self) -> bool {
        Self::PRESETS.contains(&self)
    }

    /// The preset an image of this size was drawn to fill, if any.
    ///
    /// Full-screen art is often a few pixels short: the handset keeps the annunciator bar, so a
    /// 176x220 phone gives the app 176x205..207. The real `roms/` dumps have splash screens of
    /// 176x205, 176x206, 176x207, 240x298 and 240x378. An image counts for the smallest preset of
    /// the same width whose height it fills by at least 85%.
    pub fn screen_for_image(width: u32, height: u32) -> Option<Resolution> {
        Self::PRESETS
            .into_iter()
            .filter(|p| u32::from(p.width) == width)
            .filter(|p| {
                let h = u32::from(p.height);
                height <= h && height * 100 >= h * 85
            })
            .min_by_key(|p| p.height)
    }

    /// The usual screen for a single number in a package name (`Game_176`, `tetris240`).
    ///
    /// The number is a width more often than not, but `DragonIsland_320` ships 240x320 art: a
    /// 320 alone means the portrait 240x320. A landscape game says `320x240` in full.
    pub fn for_hint(n: u16) -> Option<Resolution> {
        Some(match n {
            128 => Self::new(128, 160),
            132 => Self::new(132, 176),
            176 | 220 => Self::new(176, 220),
            208 => Self::new(176, 208),
            240 | 320 => Self::new(240, 320),
            400 => Self::new(240, 400),
            _ => return None,
        })
    }

    /// Whether `n` is one of this screen's sides; used to keep image evidence consistent with a
    /// name hint.
    pub fn has_side(self, n: u16) -> bool {
        self.width == n || self.height == n
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResolutionError(pub String);

impl fmt::Display for ParseResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "resolução inválida: {:?} (use LARGURAxALTURA, ex. 176x220)",
            self.0
        )
    }
}

impl std::error::Error for ParseResolutionError {}

impl FromStr for Resolution {
    type Err = ParseResolutionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseResolutionError(s.to_owned());
        let (w, h) = s.trim().split_once(['x', 'X', '*']).ok_or_else(err)?;
        let width: u16 = w.trim().parse().map_err(|_| err())?;
        let height: u16 = h.trim().parse().map_err(|_| err())?;
        if width == 0 || height == 0 {
            return Err(err());
        }
        Ok(Self::new(width, height))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResolutionSelect {
    #[default]
    Auto,
    Manual(Resolution),
}

impl FromStr for ResolutionSelect {
    type Err = ParseResolutionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().eq_ignore_ascii_case("auto") {
            Ok(Self::Auto)
        } else {
            s.parse().map(Self::Manual)
        }
    }
}

/// Looks for `WIDTHxHEIGHT` in any text (folder or file name). Many BREW dumps carry the
/// resolution in the name, like `chess_240x320` or `MaximBalls320x240`.
pub fn find_in_text(text: &str) -> Option<Resolution> {
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if !matches!(b, b'x' | b'X' | b'*') || i == 0 {
            continue;
        }
        let start = (0..i)
            .rev()
            .take_while(|&j| bytes[j].is_ascii_digit())
            .last();
        let end = (i + 1..bytes.len())
            .take_while(|&j| bytes[j].is_ascii_digit())
            .last();
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        let (Ok(w), Ok(h)) = (text[start..i].parse(), text[i + 1..=end].parse()) else {
            continue;
        };
        let found = Resolution::new(w, h);
        if found.is_plausible() {
            return Some(found);
        }
    }
    None
}

/// The first standalone number in `text` that [`Resolution::for_hint`] knows: `Game_176`,
/// `tetris240`, `bowl3d_176only`. Digit runs are taken whole, so `DukeNukem3D_114` gives 3 and
/// 114, and neither is a screen side.
pub fn find_hint(text: &str) -> Option<u16> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter_map(|run| run.parse::<u16>().ok())
        .find(|&n| Resolution::for_hint(n).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolution() {
        assert_eq!("176x220".parse(), Ok(Resolution::new(176, 220)));
        assert_eq!(" 240 X 320 ".parse(), Ok(Resolution::new(240, 320)));
        assert!("0x10".parse::<Resolution>().is_err());
        assert!("176".parse::<Resolution>().is_err());
    }

    #[test]
    fn finds_resolution_in_names() {
        assert_eq!(
            find_in_text("roms/Game_176x208/a.mod"),
            Some(Resolution::new(176, 208))
        );
        assert_eq!(
            find_in_text("tetris-240X320.mod"),
            Some(Resolution::new(240, 320))
        );
        // `1x1` is not a screen; the next candidate in the text is.
        assert_eq!(
            find_in_text("ico1x1_128x160"),
            Some(Resolution::new(128, 160))
        );
        assert_eq!(find_in_text("xadrez.mod"), None);
        assert_eq!(find_in_text("1024x768"), None);
    }

    #[test]
    fn finds_single_number_hints() {
        assert_eq!(find_hint("SuperDropMania176"), Some(176));
        assert_eq!(find_hint("bowl3d_176only"), Some(176));
        assert_eq!(find_hint("DragonIsland_320"), Some(320));
        assert_eq!(find_hint("DukeNukem3D_114"), None);
        assert_eq!(find_hint("Millionaire09"), None);
    }

    #[test]
    fn near_full_screen_images_map_to_presets() {
        // Splash sizes measured in the real dumps.
        assert_eq!(
            Resolution::screen_for_image(176, 206),
            Some(Resolution::new(176, 208))
        );
        assert_eq!(
            Resolution::screen_for_image(176, 212),
            Some(Resolution::new(176, 220))
        );
        assert_eq!(
            Resolution::screen_for_image(240, 298),
            Some(Resolution::new(240, 320))
        );
        assert_eq!(
            Resolution::screen_for_image(240, 378),
            Some(Resolution::new(240, 400))
        );
        assert_eq!(Resolution::screen_for_image(240, 162), None);
        assert_eq!(Resolution::screen_for_image(180, 220), None);
    }
}
