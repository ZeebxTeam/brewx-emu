//! Resolução da tela emulada: BREW rodou em aparelhos de 128x128 a 240x400, e o jogo costuma ter
//! sido compilado para um só.

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

    /// As telas mais comuns de aparelhos BREW. A detecção por imagem só aceita estas: uma imagem
    /// de 100x37 dentro do `.bar` é um botão, não a tela.
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

    /// A tela mais frequente entre os aparelhos BREW 2.x/3.x.
    pub const FALLBACK: Resolution = Self::new(176, 220);

    pub fn area(self) -> u32 {
        u32::from(self.width) * u32::from(self.height)
    }

    /// Tamanho plausível para uma tela de celular da época; filtra `1x1` e `1024x768` que
    /// aparecem por acaso em nomes de arquivo.
    pub fn is_plausible(self) -> bool {
        (96..=480).contains(&self.width) && (96..=480).contains(&self.height)
    }

    pub fn is_preset(self) -> bool {
        Self::PRESETS.contains(&self)
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

/// Procura `LARGURAxALTURA` num texto qualquer (nome de pasta, de arquivo). Muitos dumps de BREW
/// carregam a resolução no nome, como `Jogo_176x208.mod`.
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
            find_in_text("roms/Jogo_176x208/a.mod"),
            Some(Resolution::new(176, 208))
        );
        assert_eq!(
            find_in_text("tetris-240X320.mod"),
            Some(Resolution::new(240, 320))
        );
        // `1x1` não é tela; o próximo candidato no texto é.
        assert_eq!(
            find_in_text("ico1x1_128x160"),
            Some(Resolution::new(128, 160))
        );
        assert_eq!(find_in_text("xadrez.mod"), None);
        assert_eq!(find_in_text("1024x768"), None);
    }
}
