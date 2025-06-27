use crate::{Bound, Epoch, EraHistory, EraParams, Slot, Summary};
pub fn one_era() -> EraHistory {
    EraHistory {
        eras: vec![Summary {
            start: Bound {
                time_ms: 0,
                slot: Slot(0),
                epoch: Epoch(0),
            },
            end: Bound {
                time_ms: 864000000,
                slot: Slot(864000),
                epoch: Epoch(10),
            },
            #[allow(clippy::unwrap_used)]
            params: EraParams::new(86400, 1000).unwrap(),
        }],
    }
}
