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

//! Admin `peer-mix` formula: floors, proportional weights, per-source malus half-lives.
//!
//! A comma-separated token that is only `@duration` (no source name) sets the default
//! half-life for **following** entries until another naked `@…` appears. Per-entry `@…`
//! still overrides that default for that source only.
//!
//! See [EDR-031](../../../../../engineering-decision-records/031-peer-source-mix.md).

use std::{collections::BTreeMap, fmt, str::FromStr, time::Duration};

use thiserror::Error;

/// Default formula shipped with the node (static floor, then shared / snapshot / ledger proportions).
pub const DEFAULT_PEER_MIX: &str = "static!2@15m, shared~6, snapshot~3@1h, ledger~3@24h";

/// Initial running half-life before any naked `@…` token (and fallback when none is set).
pub const DEFAULT_MALUS_HALF_LIFE: Duration = Duration::from_secs(6 * 60 * 60);

/// Named outbound candidate source (extensible registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum PeerSource {
    Static,
    Shared,
    Snapshot,
    Ledger,
}

impl PeerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PeerSource::Static => "static",
            PeerSource::Shared => "shared",
            PeerSource::Snapshot => "snapshot",
            PeerSource::Ledger => "ledger",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "static" => Some(PeerSource::Static),
            "shared" => Some(PeerSource::Shared),
            "snapshot" => Some(PeerSource::Snapshot),
            "ledger" => Some(PeerSource::Ledger),
            _ => None,
        }
    }
}

impl fmt::Display for PeerSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One term in a peer-mix formula.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MixEntry {
    pub source: PeerSource,
    /// Minimum slots to assign when eligible peers exist (`!n`).
    pub floor: u32,
    /// Proportional weight (`~n`). Zero excludes the source from proportional fill (floors still apply).
    pub weight: u32,
    /// Malus half-life for this source (`@duration` on the entry, else the running default
    /// from a preceding naked `@…` token, else [`DEFAULT_MALUS_HALF_LIFE`]).
    pub half_life: Duration,
}

/// Parsed peer-mix configuration (declaration order preserved for spill).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerMix {
    entries: Vec<MixEntry>,
}

impl Default for PeerMix {
    fn default() -> Self {
        // Keep in sync with [`DEFAULT_PEER_MIX`].
        Self {
            entries: vec![
                MixEntry { source: PeerSource::Static, floor: 2, weight: 1, half_life: Duration::from_secs(15 * 60) },
                MixEntry { source: PeerSource::Shared, floor: 0, weight: 6, half_life: Duration::from_secs(6 * 3600) },
                MixEntry { source: PeerSource::Snapshot, floor: 0, weight: 3, half_life: Duration::from_secs(3600) },
                MixEntry { source: PeerSource::Ledger, floor: 0, weight: 3, half_life: Duration::from_secs(24 * 3600) },
            ],
        }
    }
}

impl PeerMix {
    pub fn entries(&self) -> &[MixEntry] {
        &self.entries
    }

    pub fn parse(s: &str) -> Result<Self, PeerMixParseError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(PeerMixParseError::Empty);
        }
        let mut entries = Vec::new();
        let mut seen = BTreeMap::new();
        // Running default half-life for entries that omit `@…`. Updated by naked `@duration` tokens.
        let mut running_half_life = DEFAULT_MALUS_HALF_LIFE;
        for (part_idx, part) in s.split(',').enumerate() {
            let part = part.trim();
            if part.is_empty() {
                return Err(PeerMixParseError::EmptyEntry { index: part_idx });
            }
            if let Some(default_hl) = parse_naked_half_life(part, part_idx)? {
                running_half_life = default_hl;
                continue;
            }
            let entry = parse_entry(part, part_idx, running_half_life)?;
            if let Some(prev) = seen.insert(entry.source, part_idx) {
                return Err(PeerMixParseError::DuplicateSource {
                    name: entry.source.to_string(),
                    first: prev,
                    second: part_idx,
                });
            }
            entries.push(entry);
        }
        if entries.is_empty() {
            return Err(PeerMixParseError::Empty);
        }
        Ok(Self { entries })
    }

    /// How many new outbound slots to take from each source for `open` free slots.
    ///
    /// `eligible` counts candidates already filtered (not outbound, not cooling, canonical origin).
    /// Short buckets **spill** remaining demand to later sources in declaration order.
    pub fn allot(&self, open: usize, eligible: &BTreeMap<PeerSource, usize>) -> BTreeMap<PeerSource, usize> {
        if open == 0 || self.entries.is_empty() {
            return BTreeMap::new();
        }

        let ideal = ideal_allotments(open, &self.entries);
        let mut remaining_eligible: BTreeMap<PeerSource, usize> = eligible.clone();
        let mut remaining_open = open;
        let mut got: BTreeMap<PeerSource, usize> = BTreeMap::new();

        for (i, entry) in self.entries.iter().enumerate() {
            let avail = remaining_eligible.get(&entry.source).copied().unwrap_or(0);
            let take = ideal[i].min(avail).min(remaining_open);
            if take > 0 {
                *got.entry(entry.source).or_default() += take;
                remaining_open -= take;
                *remaining_eligible.entry(entry.source).or_default() -= take;
            }
        }

        // Spill leftovers in declaration order (skip fully disabled ~0 with no floor).
        for entry in &self.entries {
            if remaining_open == 0 {
                break;
            }
            if entry.weight == 0 && entry.floor == 0 {
                continue;
            }
            let avail = remaining_eligible.get(&entry.source).copied().unwrap_or(0);
            let take = avail.min(remaining_open);
            if take > 0 {
                *got.entry(entry.source).or_default() += take;
                remaining_open -= take;
                *remaining_eligible.entry(entry.source).or_default() -= take;
            }
        }

        got
    }
}

impl FromStr for PeerMix {
    type Err = PeerMixParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for PeerMix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.entries.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", e.source)?;
            if e.floor > 0 {
                write!(f, "!{}", e.floor)?;
            }
            // Always emit weight when non-default or when floor is zero so round-trips stay clear.
            if e.weight != 1 || e.floor == 0 {
                write!(f, "~{}", e.weight)?;
            }
            if e.half_life != DEFAULT_MALUS_HALF_LIFE {
                write!(f, "@{}", format_duration(e.half_life))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PeerMixParseError {
    #[error("peer-mix formula is empty")]
    Empty,
    #[error("peer-mix entry {index} is empty")]
    EmptyEntry { index: usize },
    #[error("unknown peer source `{name}` in entry {index}")]
    UnknownSource { name: String, index: usize },
    #[error("duplicate source `{name}` in entries {first} and {second}")]
    DuplicateSource { name: String, first: usize, second: usize },
    #[error("invalid peer-mix entry {index}: {detail}")]
    InvalidEntry { index: usize, detail: String },
    #[error("invalid duration `{raw}` in entry {index}")]
    InvalidDuration { raw: String, index: usize },
}

/// A token that is only `@duration` (no source name): updates the running default half-life.
fn parse_naked_half_life(part: &str, index: usize) -> Result<Option<Duration>, PeerMixParseError> {
    let part = part.trim();
    if !part.starts_with('@') {
        return Ok(None);
    }
    // Must not look like a source entry (sources start with a letter).
    let rest = part[1..].trim_start();
    if rest.is_empty() {
        return Err(PeerMixParseError::InvalidEntry { index, detail: "expected duration after naked '@'".into() });
    }
    // Reject `@2hstatic` style: the whole rest must be a single duration token.
    let (tok, after) = take_token(rest);
    if !after.trim().is_empty() {
        return Ok(None); // not a pure naked default; let parse_entry fail if malformed
    }
    // If it doesn't parse as a duration, treat as invalid naked default rather than unknown source.
    let Some(d) = parse_duration(tok) else {
        // Could be `@foo` — still invalid as both naked default and source entry.
        if PeerSource::parse(part).is_none() && !part.as_bytes().get(1).is_some_and(|b| b.is_ascii_alphabetic()) {
            return Err(PeerMixParseError::InvalidDuration { raw: tok.to_string(), index });
        }
        return Ok(None);
    };
    Ok(Some(d))
}

fn parse_entry(part: &str, index: usize, running_half_life: Duration) -> Result<MixEntry, PeerMixParseError> {
    // name [ !floor ] [ ~weight ] [ @duration ]
    let bytes = part.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
        i += 1;
    }
    if i == 0 {
        return Err(PeerMixParseError::InvalidEntry { index, detail: "missing source name".into() });
    }
    let name = &part[..i];
    let source =
        PeerSource::parse(name).ok_or_else(|| PeerMixParseError::UnknownSource { name: name.to_string(), index })?;

    let mut floor = 0u32;
    let mut weight: Option<u32> = None;
    let mut half_life = running_half_life;
    let mut rest = part[i..].trim_start();

    while !rest.is_empty() {
        let marker = rest.as_bytes()[0] as char;
        rest = &rest[1..];
        match marker {
            '!' => {
                let (n, next) = take_u32(rest).ok_or_else(|| PeerMixParseError::InvalidEntry {
                    index,
                    detail: "expected integer after '!'".into(),
                })?;
                floor = n;
                rest = next.trim_start();
            }
            '~' => {
                let (n, next) = take_u32(rest).ok_or_else(|| PeerMixParseError::InvalidEntry {
                    index,
                    detail: "expected integer after '~'".into(),
                })?;
                weight = Some(n);
                rest = next.trim_start();
            }
            '@' => {
                let (tok, next) = take_token(rest);
                if tok.is_empty() {
                    return Err(PeerMixParseError::InvalidEntry {
                        index,
                        detail: "expected duration after '@'".into(),
                    });
                }
                half_life = parse_duration(tok)
                    .ok_or_else(|| PeerMixParseError::InvalidDuration { raw: tok.to_string(), index })?;
                rest = next.trim_start();
            }
            _ => {
                return Err(PeerMixParseError::InvalidEntry { index, detail: format!("unexpected `{marker}`") });
            }
        }
    }

    Ok(MixEntry { source, floor, weight: weight.unwrap_or(1), half_life })
}

fn take_u32(s: &str) -> Option<(u32, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let n: u32 = s[..i].parse().ok()?;
    Some((n, &s[i..]))
}

fn take_token(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        // duration tokens are like 12h; stop before another marker only if we already have alnum
        if i > 0 && matches!(bytes[i], b'!' | b'~' | b'@') {
            break;
        }
        i += 1;
    }
    (&s[..i], &s[i..])
}

fn parse_duration(raw: &str) -> Option<Duration> {
    if raw.len() < 2 {
        return None;
    }
    let (num, unit) = raw.split_at(raw.len() - 1);
    let n: u64 = num.parse().ok()?;
    match unit {
        "s" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_secs(n.checked_mul(60)?)),
        "h" => Some(Duration::from_secs(n.checked_mul(3600)?)),
        "d" => Some(Duration::from_secs(n.checked_mul(86400)?)),
        _ => None,
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs > 0 && secs.is_multiple_of(86400) {
        format!("{}d", secs / 86400)
    } else if secs > 0 && secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs > 0 && secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Ideal slot counts before eligibility capping and spill (floors then largest-remainder weights).
fn ideal_allotments(open: usize, entries: &[MixEntry]) -> Vec<usize> {
    let mut ideal = vec![0usize; entries.len()];
    let mut left = open;

    for (i, e) in entries.iter().enumerate() {
        let take = (e.floor as usize).min(left);
        ideal[i] = take;
        left -= take;
    }

    if left == 0 {
        return ideal;
    }

    let total_w: u32 = entries.iter().map(|e| e.weight).sum();
    if total_w == 0 {
        return ideal;
    }

    let mut rem: Vec<(usize, f64)> = Vec::new();
    let mut assigned = 0usize;
    for (i, e) in entries.iter().enumerate() {
        if e.weight == 0 {
            continue;
        }
        let exact = left as f64 * (e.weight as f64) / (total_w as f64);
        let base = exact.floor() as usize;
        ideal[i] += base;
        assigned += base;
        rem.push((i, exact - base as f64));
    }
    let mut leftover = left.saturating_sub(assigned);
    rem.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    for (i, _) in rem {
        if leftover == 0 {
            break;
        }
        ideal[i] += 1;
        leftover -= 1;
    }
    ideal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_formula_parses() {
        let m = PeerMix::default();
        assert_eq!(m.entries().len(), 4);
        assert_eq!(m.entries()[0].source, PeerSource::Static);
        assert_eq!(m.entries()[0].floor, 2);
        assert_eq!(m.entries()[1].weight, 6);

        let def = PeerMix::parse(DEFAULT_PEER_MIX).unwrap();
        assert_eq!(m, def);
    }

    #[test]
    fn parse_weights_and_decay() {
        let m = PeerMix::parse("static!2@2h, shared~6@6h, ledger~0").unwrap();
        assert_eq!(m.entries()[0].half_life, Duration::from_secs(2 * 3600));
        assert_eq!(m.entries()[2].weight, 0);
        // No `@` and no preceding naked default ⇒ built-in 6h default applied at parse.
        assert_eq!(m.entries()[2].half_life, DEFAULT_MALUS_HALF_LIFE);
    }

    #[test]
    fn naked_half_life_sets_default_for_following_entries() {
        let m = PeerMix::parse("@12h, static!2, shared~6, ledger~4@48h").unwrap();
        assert_eq!(m.entries().len(), 3);
        assert_eq!(m.entries()[0].half_life, Duration::from_secs(12 * 3600));
        assert_eq!(m.entries()[1].half_life, Duration::from_secs(12 * 3600));
        // Per-entry `@` overrides the running default for that entry only.
        assert_eq!(m.entries()[2].half_life, Duration::from_secs(48 * 3600));
    }

    #[test]
    fn naked_half_life_can_change_mid_formula() {
        let m = PeerMix::parse("static!1, @2h, shared~1, @1d, ledger~1").unwrap();
        assert_eq!(m.entries()[0].half_life, DEFAULT_MALUS_HALF_LIFE);
        assert_eq!(m.entries()[1].half_life, Duration::from_secs(2 * 3600));
        assert_eq!(m.entries()[2].half_life, Duration::from_secs(24 * 3600));
    }

    #[test]
    fn only_naked_defaults_is_empty() {
        assert!(matches!(PeerMix::parse("@6h, @12h"), Err(PeerMixParseError::Empty)));
    }

    #[test]
    fn reject_unknown_and_duplicate() {
        assert!(matches!(PeerMix::parse("foo~1"), Err(PeerMixParseError::UnknownSource { .. })));
        assert!(matches!(PeerMix::parse("static~1, static~2"), Err(PeerMixParseError::DuplicateSource { .. })));
    }

    #[test]
    fn allot_floors_then_weights() {
        let m = PeerMix::parse("static!2, shared~1, ledger~1").unwrap();
        let mut elig = BTreeMap::new();
        elig.insert(PeerSource::Static, 10);
        elig.insert(PeerSource::Shared, 10);
        elig.insert(PeerSource::Ledger, 10);
        let got = m.allot(6, &elig);
        // floors: static 2; remainder 4 split by weights (static default ~1, shared~1, ledger~1)
        // → base +1 each and +1 remainder to static ⇒ static 4, shared 1, ledger 1
        assert_eq!(got.get(&PeerSource::Static).copied().unwrap_or(0), 4);
        assert_eq!(got.get(&PeerSource::Shared).copied().unwrap_or(0), 1);
        assert_eq!(got.get(&PeerSource::Ledger).copied().unwrap_or(0), 1);
    }

    #[test]
    fn empty_bucket_spills() {
        let m = PeerMix::parse("shared~10, ledger~1").unwrap();
        let mut elig = BTreeMap::new();
        elig.insert(PeerSource::Shared, 0);
        elig.insert(PeerSource::Ledger, 5);
        let got = m.allot(3, &elig);
        assert_eq!(got.get(&PeerSource::Shared).copied().unwrap_or(0), 0);
        assert_eq!(got.get(&PeerSource::Ledger).copied().unwrap_or(0), 3);
    }

    #[test]
    fn weight_zero_without_floor_does_not_receive_spill() {
        let m = PeerMix::parse("shared~5, ledger~0").unwrap();
        let mut elig = BTreeMap::new();
        elig.insert(PeerSource::Shared, 0);
        elig.insert(PeerSource::Ledger, 4);
        let got = m.allot(2, &elig);
        assert_eq!(got.get(&PeerSource::Ledger).copied().unwrap_or(0), 0);
        assert_eq!(got.values().sum::<usize>(), 0);
    }

    #[test]
    fn spill_to_other_positive_weight_source() {
        let m = PeerMix::parse("shared~5, ledger~1").unwrap();
        let mut elig = BTreeMap::new();
        elig.insert(PeerSource::Shared, 0);
        elig.insert(PeerSource::Ledger, 4);
        let got = m.allot(2, &elig);
        assert_eq!(got.get(&PeerSource::Ledger).copied().unwrap_or(0), 2);
    }
}
