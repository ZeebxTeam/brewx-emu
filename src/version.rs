//! As versões de BREW que o emulador conhece, e a escolha entre detectar ou forçar uma.

use std::fmt;
use std::str::FromStr;

/// Uma versão de BREW, até o menor (`3.1.5` vira `3.1`): o patch nunca mudou a ABI das
/// interfaces que um jogo enxerga.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BrewVersion {
    V1_0,
    V1_1,
    V2_0,
    V2_1,
    V3_0,
    V3_1,
}

impl BrewVersion {
    pub const ALL: [BrewVersion; 6] = [
        Self::V1_0,
        Self::V1_1,
        Self::V2_0,
        Self::V2_1,
        Self::V3_0,
        Self::V3_1,
    ];

    /// Quando nada no pacote diz a versão. BREW é retrocompatível, então a mais nova roda a
    /// maioria dos títulos antigos; o contrário quebra assim que o jogo pede uma interface nova.
    pub const FALLBACK: BrewVersion = Self::V3_1;

    pub fn major(self) -> u8 {
        match self {
            Self::V1_0 | Self::V1_1 => 1,
            Self::V2_0 | Self::V2_1 => 2,
            Self::V3_0 | Self::V3_1 => 3,
        }
    }

    pub fn minor(self) -> u8 {
        match self {
            Self::V1_0 | Self::V2_0 | Self::V3_0 => 0,
            Self::V1_1 | Self::V2_1 | Self::V3_1 => 1,
        }
    }

    pub fn from_parts(major: u8, minor: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|v| v.major() == major && v.minor() == minor)
    }
}

impl fmt::Display for BrewVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major(), self.minor())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseVersionError(pub String);

impl fmt::Display for ParseVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "versão de BREW desconhecida: {:?} (use 1.0, 1.1, 2.0, 2.1, 3.0 ou 3.1)",
            self.0
        )
    }
}

impl std::error::Error for ParseVersionError {}

impl FromStr for BrewVersion {
    type Err = ParseVersionError;

    /// Aceita `3.1`, `3.1.5`, `brew 3.1` e `3` (que vira `3.0`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseVersionError(s.to_owned());
        let trimmed = s.trim().to_ascii_lowercase();
        let digits = trimmed.trim_start_matches("brew").trim();
        let mut parts = digits.split('.');
        let major = parts.next().and_then(|p| p.parse().ok()).ok_or_else(err)?;
        let minor = match parts.next() {
            Some(p) => p.parse().map_err(|_| err())?,
            None => 0,
        };
        Self::from_parts(major, minor).ok_or_else(err)
    }
}

/// O que o usuário pediu: detectar pelo pacote ou forçar uma versão.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VersionSelect {
    #[default]
    Auto,
    Manual(BrewVersion),
}

impl FromStr for VersionSelect {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().eq_ignore_ascii_case("auto") {
            Ok(Self::Auto)
        } else {
            s.parse().map(Self::Manual)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_spellings() {
        assert_eq!("3.1".parse(), Ok(BrewVersion::V3_1));
        assert_eq!("3.1.5".parse(), Ok(BrewVersion::V3_1));
        assert_eq!("BREW 2.1".parse(), Ok(BrewVersion::V2_1));
        assert_eq!("1".parse(), Ok(BrewVersion::V1_0));
        assert!("4.0".parse::<BrewVersion>().is_err());
        assert!("x".parse::<BrewVersion>().is_err());
    }

    #[test]
    fn select_accepts_auto() {
        assert_eq!("AUTO".parse(), Ok(VersionSelect::Auto));
        assert_eq!("2.0".parse(), Ok(VersionSelect::Manual(BrewVersion::V2_0)));
    }

    #[test]
    fn display_round_trips() {
        for v in BrewVersion::ALL {
            assert_eq!(v.to_string().parse(), Ok(v));
        }
    }
}
