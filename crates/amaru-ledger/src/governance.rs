use crate::store::{columns::dreps::Row, Snapshot, StoreError};
use amaru_kernel::{encode_bech32, DRep, StakeCredential};
use serde::ser::SerializeStruct;
use std::collections::BTreeMap;

#[derive(Debug)]
// TODO: Also add DRep information to this summary
pub struct DRepsSummary {
    pub delegations: BTreeMap<StakeCredential, DRep>,
}

impl DRepsSummary {
    pub fn new(db: &impl Snapshot) -> Result<Self, StoreError> {
        let dreps = db
            .iter_dreps()?
            .map(|(k, Row { registered_at, .. })| {
                let drep = match k {
                    StakeCredential::AddrKeyhash(hash) => DRep::Key(hash),
                    StakeCredential::ScriptHash(hash) => DRep::Script(hash),
                };
                (drep, registered_at)
            })
            .collect::<BTreeMap<_, _>>();

        let delegations = db
            .iter_accounts()?
            .filter_map(|(credential, account)| {
                account.drep.and_then(|(drep, since)| {
                    let registered_at = dreps.get(&drep);
                    if let Some(registered_at) = registered_at {
                        if since >= *registered_at {
                            // This is a registration with a previous registration of this DRep, it must be renewed
                            return Some((credential, drep));
                        }
                    }
                    None
                })
            })
            .collect::<BTreeMap<StakeCredential, DRep>>();

        Ok(DRepsSummary { delegations })
    }
}

impl serde::Serialize for DRepsSummary {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("DRepsSummary", 2)?;

        let mut keys = BTreeMap::new();
        let mut scripts = BTreeMap::new();
        self.delegations.iter().for_each(|(credential, drep)| {
            if let Some(v) = into_drep_id(drep) {
                match credential {
                    StakeCredential::AddrKeyhash(hash) => keys.insert(*hash, v),
                    StakeCredential::ScriptHash(hash) => scripts.insert(*hash, v),
                };
            }
        });

        s.serialize_field("keys", &keys)?;
        s.serialize_field("scripts", &scripts)?;

        s.end()
    }
}

/// Serialize a (registerd) DRep to bech32, according to [CIP-0129](https://cips.cardano.org/cip/CIP-0129).
/// The always-Abstain and always-NoConfidence dreps are ignored (i.e. return `None`).
///
/// ```rust
/// use amaru_kernel::{DRep, Hash};
/// use amaru_ledger::governance::into_drep_id;
///
/// let key_drep = DRep::Key(Hash::from(
///   hex::decode("7a719c71d1bc67d2eb4af19f02fd48e7498843d33a22168111344a34")
///     .unwrap()
///     .as_slice()
/// ));
///
/// let script_drep = DRep::Script(Hash::from(
///   hex::decode("429b12461640cefd3a4a192f7c531d8f6c6d33610b727f481eb22d39")
///     .unwrap()
///     .as_slice()
/// ));
///
/// assert_eq!(into_drep_id(&DRep::Abstain), None);
///
/// assert_eq!(into_drep_id(&DRep::NoConfidence), None);
///
/// assert_eq!(
///   into_drep_id(&key_drep).as_deref(),
///   Some("drep1yfa8r8r36x7x05htftce7qhafrn5nzzr6vazy95pzy6y5dqac0ss7"),
/// );
///
/// assert_eq!(
///   into_drep_id(&script_drep).as_deref(),
///   Some("drep1ydpfkyjxzeqvalf6fgvj7lznrk8kcmfnvy9hyl6gr6ez6wgsjaelx"),
/// );
/// ```
pub fn into_drep_id(drep: &DRep) -> Option<String> {
    match drep {
        DRep::Key(hash) => encode_bech32("drep", &[&[34], hash.as_slice()].concat()).ok(),
        DRep::Script(hash) => encode_bech32("drep", &[&[35], hash.as_slice()].concat()).ok(),
        DRep::Abstain | DRep::NoConfidence => None,
    }
}
