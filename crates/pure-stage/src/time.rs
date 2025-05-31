use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(tokio::time::Instant);

impl Instant {
    pub(crate) fn from_tokio(instant: tokio::time::Instant) -> Self {
        Self(instant)
    }

    #[allow(dead_code)]
    pub(crate) fn now() -> Self {
        Self(tokio::time::Instant::now())
    }

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

#[test]
fn instant() {
    let now = Instant::now();
    let later = now.checked_add(Duration::from_secs(1)).unwrap();

    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));
    assert_eq!(now.checked_since(later), None);

    assert_eq!(later.saturating_since(now), Duration::from_secs(1));
    assert_eq!(now.saturating_since(later), Duration::from_secs(0));

    assert_eq!(now.checked_add(Duration::from_secs(1)).unwrap(), later);
    assert_eq!(later.checked_sub(Duration::from_secs(1)).unwrap(), now);
}
