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

/// Geometry for a vertical track whose thumb maps a content window onto `track_len` rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    pub thumb_height: usize,
    pub thumb_offset: usize,
    pub max_offset: usize,
    pub max_position: usize,
}

impl ScrollbarGeometry {
    pub fn new(total: usize, visible: usize, position: usize, track_len: usize) -> Option<Self> {
        if total <= visible || visible == 0 || track_len == 0 {
            return None;
        }

        let thumb_height = track_len.min(2);
        let max_position = total.saturating_sub(visible);
        let max_offset = track_len.saturating_sub(thumb_height);
        let thumb_offset = if max_position == 0 || max_offset == 0 {
            0
        } else {
            position.min(max_position).saturating_mul(max_offset) / max_position
        };

        Some(Self { thumb_height, thumb_offset, max_offset, max_position })
    }

    /// Content position (from the top) for a click or drag on the track.
    pub fn position_for_offset(&self, offset: usize) -> usize {
        let last = self.max_offset.saturating_add(self.thumb_height.saturating_sub(1));
        if last == 0 {
            return 0;
        }

        offset.min(last).saturating_mul(self.max_position) / last
    }

    /// Items moved by one track row.
    pub fn step(&self) -> usize {
        self.max_position.checked_div(self.max_offset).unwrap_or(1).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_track_ends_to_content_ends() {
        let geo = ScrollbarGeometry::new(100, 10, 0, 20).expect("overflowing");
        assert_eq!(geo.max_position, 90);
        assert_eq!(geo.position_for_offset(0), 0);
        assert_eq!(geo.position_for_offset(19), 90);
    }

    #[test]
    fn step_is_at_least_one_item_per_row() {
        let geo = ScrollbarGeometry::new(100, 10, 0, 20).expect("overflowing");
        assert_eq!(geo.step(), 5);
        let tiny = ScrollbarGeometry::new(12, 10, 0, 20).expect("overflowing");
        assert_eq!(tiny.step(), 1);
    }

    #[test]
    fn hidden_when_content_fits() {
        assert!(ScrollbarGeometry::new(8, 10, 0, 10).is_none());
    }
}
