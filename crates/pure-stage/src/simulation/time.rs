use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(tokio::time::Instant);

impl Instant {
    pub fn pretty(self, now: Self) -> String {
        if let Some(duration) = self.checked_since(now) {
            format!("{:?} in the future", duration)
        } else if let Some(duration) = now.checked_since(self) {
            format!("{:?} ago", duration)
        } else {
            "(time bug)".to_string()
        }
    }

    pub fn saturating_since(&self, other: Self) -> Duration {
        self.0.duration_since(other.0)
    }

    pub fn checked_since(&self, other: Self) -> Option<Duration> {
        self.0.checked_duration_since(other.0)
    }

    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration).map(Self)
    }

    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration).map(Self)
    }
}
