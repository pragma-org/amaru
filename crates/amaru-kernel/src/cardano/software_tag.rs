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

//! CIP-0203 block producer identifier; Amaru's payload is specified in `docs/cip-0203/README.md`.

use std::fmt;

/// Block producer identifier carried in the minor component of a header's `protocol_version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SoftwareTag {
    NoSignal,
    OptOut,
    Attributed { id: ProducerId, payload: Payload },
}

impl SoftwareTag {
    const SCHEME_SHIFT: u32 = 30;

    const OPT_OUT_ID: u8 = 255;
}

impl TryFrom<u32> for SoftwareTag {
    type Error = SoftwareTagError;

    fn try_from(minor: u32) -> Result<Self, Self::Error> {
        let scheme = (minor >> Self::SCHEME_SHIFT) as u8;
        if scheme != 0 {
            return Err(SoftwareTagError::ReservedScheme(scheme));
        }
        match minor as u8 {
            0 => Ok(Self::NoSignal),
            Self::OPT_OUT_ID => Ok(Self::OptOut),
            id => Ok(Self::Attributed { id: ProducerId(id), payload: Payload((minor >> 8) & Payload::MASK) }),
        }
    }
}

impl From<SoftwareTag> for u32 {
    fn from(tag: SoftwareTag) -> Self {
        match tag {
            SoftwareTag::NoSignal => 0,
            SoftwareTag::OptOut => SoftwareTag::OPT_OUT_ID as u32,
            SoftwareTag::Attributed { id, payload } => (payload.0 << 8) | id.0 as u32,
        }
    }
}

impl fmt::Display for SoftwareTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSignal => write!(f, "no signal"),
            Self::OptOut => write!(f, "operator opted out"),
            Self::Attributed { id, payload } => write!(f, "producer {id}, payload {payload}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SoftwareTagError {
    #[error("scheme {0} is reserved")]
    ReservedScheme(u8),
}

/// Registered producer id; `0` and `255` are not ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct ProducerId(u8);

impl ProducerId {
    pub const fn legacy_cardano_node(&self) -> bool {
        self.0 <= 15
    }
}

impl TryFrom<u8> for ProducerId {
    type Error = ProducerIdError;

    fn try_from(id: u8) -> Result<Self, Self::Error> {
        match id {
            0 | SoftwareTag::OPT_OUT_ID => Err(ProducerIdError(id)),
            id => Ok(Self(id)),
        }
    }
}

impl From<ProducerId> for u8 {
    fn from(id: ProducerId) -> Self {
        id.0
    }
}

impl fmt::Display for ProducerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == AmaruTag::ID {
            write!(f, "{} (Amaru)", self.0)
        } else if self.legacy_cardano_node() {
            write!(f, "{} (legacy cardano-node)", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0} is not a producer id")]
pub struct ProducerIdError(u8);

/// Implementation-defined 22 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct Payload(u32);

impl Payload {
    const MASK: u32 = (1 << 22) - 1;
}

impl TryFrom<u32> for Payload {
    type Error = PayloadError;

    fn try_from(payload: u32) -> Result<Self, Self::Error> {
        if payload > Self::MASK {
            return Err(PayloadError(payload));
        }
        Ok(Self(payload))
    }
}

impl From<Payload> for u32 {
    fn from(payload: Payload) -> Self {
        payload.0
    }
}

impl fmt::Display for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:06x}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("payload {0} does not fit in 22 bits")]
pub struct PayloadError(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AmaruTag {
    release_day: ReleaseDay,
}

impl AmaruTag {
    pub const ID: ProducerId = ProducerId(65);

    const LAYOUT_VERSION: u32 = 0;

    const FLAGS_MASK: u32 = (1 << 6) - 1;

    pub const fn new(release_day: ReleaseDay) -> Self {
        Self { release_day }
    }

    pub const fn release_day(&self) -> ReleaseDay {
        self.release_day
    }

    pub const fn software_tag(&self) -> SoftwareTag {
        let payload = (Self::LAYOUT_VERSION << 20) | ((self.release_day.0 as u32) << 6);
        SoftwareTag::Attributed { id: Self::ID, payload: Payload(payload) }
    }
}

impl fmt::Display for AmaruTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "layout {}, release day {} ({}), no flags",
            Self::LAYOUT_VERSION,
            self.release_day.0,
            self.release_day
        )
    }
}

impl TryFrom<SoftwareTag> for AmaruTag {
    type Error = AmaruTagError;

    fn try_from(tag: SoftwareTag) -> Result<Self, Self::Error> {
        let payload = match tag {
            SoftwareTag::NoSignal => return Err(AmaruTagError::NoSignal),
            SoftwareTag::OptOut => return Err(AmaruTagError::OptOut),
            SoftwareTag::Attributed { id, .. } if id != Self::ID => return Err(AmaruTagError::Id(id)),
            SoftwareTag::Attributed { payload, .. } => payload.0,
        };
        let layout_version = payload >> 20;
        if layout_version != Self::LAYOUT_VERSION {
            return Err(AmaruTagError::LayoutVersion(layout_version));
        }
        let flags = payload & Self::FLAGS_MASK;
        if flags != 0 {
            return Err(AmaruTagError::UnassignedFlags(flags));
        }
        let days = (payload >> 6) & ReleaseDay::MAX.0 as u32;
        Ok(Self::new(ReleaseDay(days as u16)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AmaruTagError {
    #[error("no signal")]
    NoSignal,
    #[error("operator opted out")]
    OptOut,
    #[error("id {0} is not Amaru's")]
    Id(ProducerId),
    #[error("payload layout version {0} is unknown")]
    LayoutVersion(u32),
    #[error("unassigned flag bits set: {0:#08b}")]
    UnassignedFlags(u32),
}

/// Days since 2026-01-01 (UTC); `0` is an unreleased build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct ReleaseDay(u16);

impl ReleaseDay {
    pub const UNRELEASED: Self = Self(0);

    pub const MAX: Self = Self((1 << 14) - 1);

    const EPOCH: CalendarDate = CalendarDate { year: 2026, month: 1, day: 1 };

    pub const fn from_version_patch(patch: &str) -> Result<Self, ReleaseDayError> {
        let digits = patch.as_bytes();
        if digits.len() == 1 && digits[0] == b'0' {
            return Ok(Self::UNRELEASED);
        }
        if digits.len() != 8 {
            return Err(ReleaseDayError::Malformed);
        }
        let mut value: u32 = 0;
        let mut i = 0;
        while i < digits.len() {
            if !digits[i].is_ascii_digit() {
                return Err(ReleaseDayError::Malformed);
            }
            value = value * 10 + (digits[i] - b'0') as u32;
            i += 1;
        }
        Self::from_date(CalendarDate {
            year: (value / 10_000) as u16,
            month: ((value / 100) % 100) as u8,
            day: (value % 100) as u8,
        })
    }

    pub const fn from_date(date: CalendarDate) -> Result<Self, ReleaseDayError> {
        if !date.is_valid() {
            return Err(ReleaseDayError::InvalidDate(date));
        }
        let days = date.days_from_civil() - Self::EPOCH.days_from_civil();
        if days < 1 || days > Self::MAX.0 as i64 {
            return Err(ReleaseDayError::OutOfRange(date));
        }
        Ok(Self(days as u16))
    }

    pub const fn date(&self) -> Option<CalendarDate> {
        if self.0 == 0 {
            return None;
        }
        Some(CalendarDate::civil_from_days(Self::EPOCH.days_from_civil() + self.0 as i64))
    }

    pub const fn days(&self) -> u16 {
        self.0
    }
}

impl fmt::Display for ReleaseDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.date() {
            Some(date) => write!(f, "{date}"),
            None => write!(f, "unreleased"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseDayError {
    #[error("expected 0 or a YYYYMMDD day")]
    Malformed,
    #[error("{0} is not a calendar date")]
    InvalidDate(CalendarDate),
    #[error("{0} is outside the representable range, 2026-01-02 to 2070-11-09")]
    OutOfRange(CalendarDate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CalendarDate {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl CalendarDate {
    pub const fn is_valid(&self) -> bool {
        self.month >= 1 && self.month <= 12 && self.day >= 1 && self.day <= self.days_in_month()
    }

    const fn days_in_month(&self) -> u8 {
        match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if self.is_leap_year() => 29,
            2 => 28,
            _ => 0,
        }
    }

    const fn is_leap_year(&self) -> bool {
        self.year.is_multiple_of(4) && (!self.year.is_multiple_of(100) || self.year.is_multiple_of(400))
    }

    const fn days_from_civil(&self) -> i64 {
        let y = self.year as i64 - if self.month <= 2 { 1 } else { 0 };
        let m = self.month as i64;
        let d = self.day as i64;
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    const fn civil_from_days(days: i64) -> Self {
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Self { year: (if m <= 2 { y + 1 } else { y }) as u16, month: m as u8, day: d as u8 }
    }
}

impl fmt::Display for CalendarDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    const fn date(year: u16, month: u8, day: u8) -> CalendarDate {
        CalendarDate { year, month, day }
    }

    #[test_case("0" => Ok(ReleaseDay::UNRELEASED); "unreleased")]
    #[test_case("20260611" => Ok(ReleaseDay(161)); "first release")]
    #[test_case("20260925" => Ok(ReleaseDay(267)); "later release")]
    #[test_case("20701109" => Ok(ReleaseDay::MAX); "last representable day")]
    #[test_case("20701110" => Err(ReleaseDayError::OutOfRange(date(2070, 11, 10))); "past the last day")]
    #[test_case("20260101" => Err(ReleaseDayError::OutOfRange(date(2026, 1, 1))); "epoch day would read as unreleased")]
    #[test_case("20251231" => Err(ReleaseDayError::OutOfRange(date(2025, 12, 31))); "before the epoch")]
    #[test_case("20260230" => Err(ReleaseDayError::InvalidDate(date(2026, 2, 30))); "no such day")]
    #[test_case("2026061" => Err(ReleaseDayError::Malformed); "too short")]
    #[test_case("00" => Err(ReleaseDayError::Malformed); "zero padded")]
    #[test_case("2026-6-1" => Err(ReleaseDayError::Malformed); "not digits")]
    fn release_day_from_version_patch(patch: &str) -> Result<ReleaseDay, ReleaseDayError> {
        ReleaseDay::from_version_patch(patch)
    }

    #[test_case(0 => None; "unreleased")]
    #[test_case(161 => Some(date(2026, 6, 11)); "first release")]
    #[test_case(267 => Some(date(2026, 9, 25)); "later release")]
    #[test_case(16383 => Some(date(2070, 11, 9)); "last representable day")]
    fn release_day_to_date(days: u16) -> Option<CalendarDate> {
        ReleaseDay(days).date()
    }

    #[test]
    fn every_representable_day_round_trips() {
        for days in 1..=ReleaseDay::MAX.0 {
            let day = ReleaseDay(days);
            let date = day.date().unwrap();
            assert!(date.is_valid(), "{date}");
            assert_eq!(ReleaseDay::from_date(date), Ok(day), "{date}");
        }
    }

    #[test_case(ReleaseDay::UNRELEASED => 0x0000_0041; "unreleased")]
    #[test_case(ReleaseDay(161) => 0x0028_4041; "release 20260611")]
    #[test_case(ReleaseDay(267) => 0x0042_C041; "release 20260925")]
    #[test_case(ReleaseDay::MAX => 0x0FFF_C041; "last representable day")]
    fn amaru_tag_minor(release_day: ReleaseDay) -> u32 {
        u32::from(AmaruTag::new(release_day).software_tag())
    }

    #[test_case(0x0000_0041 => Ok(AmaruTag::new(ReleaseDay::UNRELEASED)); "unreleased")]
    #[test_case(0x0042_C041 => Ok(AmaruTag::new(ReleaseDay(267))); "release 20260925")]
    #[test_case(0x0042_C141 => Err(AmaruTagError::UnassignedFlags(1)); "flag bit 0 set")]
    #[test_case(0x1042_C041 => Err(AmaruTagError::LayoutVersion(1)); "layout version 1")]
    #[test_case(0x0000_00FF => Err(AmaruTagError::OptOut); "opted out")]
    #[test_case(0x0000_0000 => Err(AmaruTagError::NoSignal); "no signal")]
    #[test_case(0x0000_0045 => Err(AmaruTagError::Id(ProducerId(69))); "another producer")]
    fn amaru_tag_from_minor(minor: u32) -> Result<AmaruTag, AmaruTagError> {
        AmaruTag::try_from(SoftwareTag::try_from(minor).unwrap())
    }

    #[test_case(0x0000_0000 => Ok(SoftwareTag::NoSignal); "no signal")]
    #[test_case(0x0000_00FF => Ok(SoftwareTag::OptOut); "opted out")]
    #[test_case(0x0000_0045 => Ok(SoftwareTag::Attributed { id: ProducerId(69), payload: Payload(0) }); "attributed")]
    #[test_case(0x3FFF_FF45 => Ok(SoftwareTag::Attributed { id: ProducerId(69), payload: Payload(Payload::MASK) }); "full payload")]
    #[test_case(0x4000_0041 => Err(SoftwareTagError::ReservedScheme(1)); "reserved scheme")]
    #[test_case(0xC000_0000 => Err(SoftwareTagError::ReservedScheme(3)); "top scheme")]
    fn software_tag_from_minor(minor: u32) -> Result<SoftwareTag, SoftwareTagError> {
        SoftwareTag::try_from(minor)
    }

    #[test]
    fn software_tag_round_trips() {
        for minor in [0, 255, 0x45, 0x0042_C041, 0x3FFF_FF45] {
            assert_eq!(u32::from(SoftwareTag::try_from(minor).unwrap()), minor);
        }
    }

    #[test]
    fn amaru_tag_round_trips() {
        for days in [0, 1, 161, 267, ReleaseDay::MAX.0] {
            let tag = AmaruTag::new(ReleaseDay(days));
            assert_eq!(AmaruTag::try_from(tag.software_tag()), Ok(tag));
        }
    }
}
