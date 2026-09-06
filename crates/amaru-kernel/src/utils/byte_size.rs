// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{fmt, str::FromStr};

/// A count of bytes that can be parsed from and displayed as a human size.
///
/// Parsing distinguishes SI units (powers of 1000: `kB`, `MB`, `GB`) from IEC units
/// (powers of 1024: `KiB`, `MiB`, `GiB`). Display uses IEC units by default, which is
/// the usual scale for memory in logs; [`ByteSize::si`] formats with SI units.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ByteSize(u64);

impl ByteSize {
    pub const KIB: u64 = 1024;
    pub const MIB: u64 = 1024 * 1024;
    pub const GIB: u64 = 1024 * 1024 * 1024;
    pub const TIB: u64 = 1024 * 1024 * 1024 * 1024;
    pub const PIB: u64 = 1024 * 1024 * 1024 * 1024 * 1024;

    pub const KB: u64 = 1000;
    pub const MB: u64 = 1000 * 1000;
    pub const GB: u64 = 1000 * 1000 * 1000;
    pub const TB: u64 = 1000 * 1000 * 1000 * 1000;
    pub const PB: u64 = 1000 * 1000 * 1000 * 1000 * 1000;

    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    pub const fn from_kib(n: u64) -> Self {
        Self(n.saturating_mul(Self::KIB))
    }

    pub const fn from_mib(n: u64) -> Self {
        Self(n.saturating_mul(Self::MIB))
    }

    pub const fn from_gib(n: u64) -> Self {
        Self(n.saturating_mul(Self::GIB))
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Format using SI units (`kB`, `MB`, …).
    pub const fn si(self) -> SiByteSize {
        SiByteSize(self)
    }
}

impl From<u64> for ByteSize {
    fn from(bytes: u64) -> Self {
        Self::from_bytes(bytes)
    }
}

impl From<usize> for ByteSize {
    fn from(bytes: usize) -> Self {
        Self::from_bytes(bytes as u64)
    }
}

impl From<ByteSize> for u64 {
    fn from(size: ByteSize) -> Self {
        size.0
    }
}

impl TryFrom<ByteSize> for usize {
    type Error = ByteSizeError;

    fn try_from(size: ByteSize) -> Result<Self, Self::Error> {
        usize::try_from(size.0).map_err(|_| ByteSizeError::Overflow(size.to_string()))
    }
}

impl fmt::Debug for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_scaled(f, self.0, IEC_UNITS)
    }
}

/// SI rendering of a [`ByteSize`] (`kB` = 1000 bytes).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SiByteSize(ByteSize);

impl fmt::Display for SiByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_scaled(f, self.0.as_u64(), SI_UNITS)
    }
}

impl fmt::Debug for SiByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

const IEC_UNITS: [(&str, u64); 5] = [
    ("PiB", ByteSize::PIB),
    ("TiB", ByteSize::TIB),
    ("GiB", ByteSize::GIB),
    ("MiB", ByteSize::MIB),
    ("KiB", ByteSize::KIB),
];

const SI_UNITS: [(&str, u64); 5] =
    [("PB", ByteSize::PB), ("TB", ByteSize::TB), ("GB", ByteSize::GB), ("MB", ByteSize::MB), ("kB", ByteSize::KB)];

fn write_scaled(f: &mut fmt::Formatter<'_>, bytes: u64, units: [(&str, u64); 5]) -> fmt::Result {
    for (name, unit) in units {
        if bytes >= unit {
            if bytes.is_multiple_of(unit) {
                return write!(f, "{} {name}", bytes / unit);
            }
            let value = bytes as f64 / unit as f64;
            return write!(f, "{value:.1} {name}");
        }
    }
    write!(f, "{bytes} B")
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ByteSizeError {
    #[error("expected a byte size such as 100MiB or 10kB")]
    Empty,
    #[error("expected a number before unit in {0:?}")]
    MissingNumber(String),
    #[error("invalid byte size {0:?}")]
    InvalidNumber(String),
    #[error("unknown size unit {0:?}; use kB/MB/GB (1000) or KiB/MiB/GiB (1024)")]
    UnknownUnit(String),
    #[error("byte size {0:?} is too large")]
    Overflow(String),
}

impl FromStr for ByteSize {
    type Err = ByteSizeError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ByteSizeError::Empty);
        }

        let split = trimmed.char_indices().find(|(_, ch)| !ch.is_ascii_digit()).map(|(index, _)| index);
        let (number, suffix) = match split {
            Some(index) => (&trimmed[..index], trimmed[index..].trim()),
            None => (trimmed, ""),
        };

        if number.is_empty() {
            return Err(ByteSizeError::MissingNumber(input.to_string()));
        }

        let value: u64 = number.parse().map_err(|_| ByteSizeError::InvalidNumber(input.to_string()))?;
        let multiplier = parse_unit(suffix).ok_or_else(|| ByteSizeError::UnknownUnit(suffix.to_string()))?;
        let bytes = value.checked_mul(multiplier).ok_or_else(|| ByteSizeError::Overflow(input.to_string()))?;
        Ok(Self(bytes))
    }
}

fn parse_unit(suffix: &str) -> Option<u64> {
    if suffix.is_empty() || suffix.eq_ignore_ascii_case("b") {
        return Some(1);
    }

    let lower = suffix.to_ascii_lowercase();
    let (prefix, binary) = if let Some(prefix) = lower.strip_suffix("ib") {
        (prefix, true)
    } else if let Some(prefix) = lower.strip_suffix('b') {
        (prefix, false)
    } else if matches!(lower.as_str(), "ki" | "mi" | "gi" | "ti" | "pi") {
        (lower.as_str(), true)
    } else {
        (lower.as_str(), false)
    };

    if prefix.is_empty() {
        return None;
    }

    let exponent = match prefix {
        "k" | "ki" => 1u32,
        "m" | "mi" => 2,
        "g" | "gi" => 3,
        "t" | "ti" => 4,
        "p" | "pi" => 5,
        _ => return None,
    };

    Some(if binary { ByteSize::KIB.pow(exponent) } else { ByteSize::KB.pow(exponent) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_si_and_iec_distinctly() {
        assert_eq!("1kB".parse(), Ok(ByteSize::from_bytes(1_000)));
        assert_eq!("1kb".parse(), Ok(ByteSize::from_bytes(1_000)));
        assert_eq!("1KiB".parse(), Ok(ByteSize::from_bytes(1_024)));
        assert_eq!("1kiB".parse(), Ok(ByteSize::from_bytes(1_024)));
        assert_eq!("1kib".parse(), Ok(ByteSize::from_bytes(1_024)));
        assert_eq!("1k".parse(), Ok(ByteSize::from_bytes(1_000)));
        assert_eq!("1ki".parse(), Ok(ByteSize::from_bytes(1_024)));
        assert_eq!("100MB".parse(), Ok(ByteSize::from_bytes(100_000_000)));
        assert_eq!("100MiB".parse(), Ok(ByteSize::from_mib(100)));
        assert_eq!("100 MiB".parse(), Ok(ByteSize::from_mib(100)));
        assert_eq!("100M".parse(), Ok(ByteSize::from_bytes(100_000_000)));
        assert_eq!("2KiB".parse(), Ok(ByteSize::from_kib(2)));
        assert_eq!("1GiB".parse(), Ok(ByteSize::from_gib(1)));
        assert_eq!("2048".parse(), Ok(ByteSize::from_bytes(2_048)));
        assert_eq!("1TiB".parse(), Ok(ByteSize::from_bytes(ByteSize::TIB)));
    }

    #[test]
    fn display_uses_iec_in_logs() {
        assert_eq!(ByteSize::from_bytes(0).to_string(), "0 B");
        assert_eq!(ByteSize::from_bytes(100).to_string(), "100 B");
        assert_eq!(ByteSize::from_kib(2).to_string(), "2 KiB");
        assert_eq!(ByteSize::from_mib(100).to_string(), "100 MiB");
        assert_eq!(ByteSize::from_bytes(1_536).to_string(), "1.5 KiB");
        assert_eq!(ByteSize::from_bytes(1_000).to_string(), "1000 B");
        assert_eq!(ByteSize::from_bytes(1_000).si().to_string(), "1 kB");
        assert_eq!(ByteSize::from_bytes(100_000_000).si().to_string(), "100 MB");
        assert_eq!(format!("{:?}", ByteSize::from_mib(1)), "1 MiB");
    }

    #[test]
    fn rejects_invalid_input() {
        assert_eq!("".parse::<ByteSize>(), Err(ByteSizeError::Empty));
        assert!(matches!("MiB".parse::<ByteSize>(), Err(ByteSizeError::MissingNumber(_))));
        assert!(matches!("100XiB".parse::<ByteSize>(), Err(ByteSizeError::UnknownUnit(_))));
        assert!(matches!("100ib".parse::<ByteSize>(), Err(ByteSizeError::UnknownUnit(_))));
    }
}
