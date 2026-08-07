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

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use amaru_kernel::{EraHistory, GlobalParameters, ProtocolParameters};
use clap::{Arg, CommandFactory};

pub trait RuntimeSettingsSource: CommandFactory {
    fn value_for(&self, id: &str) -> Option<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupContext {
    pub process: ProcessInfo,
    pub protocol_version: String,
    pub mempool_max_bytes: u64,
    pub epoch_length: u64,
    pub active_slot_coeff_inverse: u64,
    pub consensus_security_param: u64,
    pub max_lovelace_supply: u64,
    pub system_start_millis: u64,
    pub era_history: Option<EraHistory>,
    pub runtime_sections: Vec<ConfigSection>,
    pub protocol_sections: Vec<ConfigSection>,
}

impl StartupContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pid: u32,
        network: impl Into<String>,
        software_version: impl Into<String>,
        target: impl Into<String>,
        mempool_max_bytes: u64,
        global_parameters: &GlobalParameters,
        protocol_parameters: Option<&ProtocolParameters>,
        era_history: Option<EraHistory>,
        runtime_sections: Vec<ConfigSection>,
    ) -> Self {
        Self {
            process: ProcessInfo::new(pid, network, software_version, target),
            protocol_version: protocol_parameters
                .map(|parameters| parameters.protocol_version.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            mempool_max_bytes,
            epoch_length: global_parameters.epoch_length(),
            active_slot_coeff_inverse: global_parameters.active_slot_coeff_inverse,
            consensus_security_param: global_parameters.consensus_security_param,
            max_lovelace_supply: global_parameters.max_lovelace_supply,
            system_start_millis: global_parameters.system_start,
            era_history,
            runtime_sections,
            protocol_sections: protocol_sections(protocol_parameters),
        }
    }

    pub fn target_slot(&self) -> Option<u64> {
        self.target_slot_at(SystemTime::now())
    }

    pub fn target_slot_at(&self, now: SystemTime) -> Option<u64> {
        self.target_slot_and_epoch_at(now).map(|(slot, _)| slot)
    }

    pub fn target_epoch_at(&self, now: SystemTime) -> Option<u64> {
        self.target_slot_and_epoch_at(now).map(|(_, epoch)| epoch)
    }

    pub fn is_near_target_slot_at(&self, slot: u64, now: SystemTime) -> Option<bool> {
        self.target_slot_at(now).map(|target_slot| target_slot.saturating_sub(slot) <= self.consensus_security_param)
    }

    fn target_slot_and_epoch_at(&self, now: SystemTime) -> Option<(u64, u64)> {
        let era_history = self.era_history.as_ref()?;
        let system_start = UNIX_EPOCH + Duration::from_millis(self.system_start_millis);
        let slot = era_history.posix_time_to_slot(now, system_start).ok()?;
        let epoch = era_history.slot_to_epoch_unchecked_horizon(slot).ok()?;
        Some((slot.as_u64(), epoch.into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub network: String,
    pub software_version: String,
    pub target: String,
}

impl ProcessInfo {
    pub fn new(
        pid: u32,
        network: impl Into<String>,
        software_version: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self { pid, network: network.into(), software_version: software_version.into(), target: target.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSection {
    pub title: String,
    pub entries: Vec<ConfigEntry>,
}

impl ConfigSection {
    pub fn new(title: impl Into<String>, entries: Vec<ConfigEntry>) -> Self {
        Self { title: title.into(), entries }
    }

    pub fn from_runtime_settings<T: RuntimeSettingsSource>(source: &T) -> Vec<Self> {
        let mut sections = Vec::<ConfigSection>::new();

        for arg in T::command().get_arguments() {
            let Some((heading, entry)) = ConfigEntry::from_arg(arg, source) else {
                continue;
            };

            if let Some(section) = sections.iter_mut().find(|section| section.title == heading) {
                section.entries.push(entry);
            } else {
                sections.push(Self::new(heading, vec![entry]));
            }
        }

        sections
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    pub label: String,
    pub option: Option<String>,
    pub env_var: Option<String>,
    pub value: String,
}

impl ConfigEntry {
    pub fn new(
        label: impl Into<String>,
        option: Option<impl Into<String>>,
        env_var: Option<impl Into<String>>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            option: option.map(Into::into),
            env_var: env_var.map(Into::into),
            value: value.into(),
        }
    }

    fn from_arg<T: RuntimeSettingsSource>(arg: &Arg, source: &T) -> Option<(String, ConfigEntry)> {
        if !is_runtime_config_arg(arg) {
            return None;
        }

        let long = arg.get_long()?;
        let value = source.value_for(arg.get_id().as_str())?;
        let heading = arg.get_help_heading().map(ToString::to_string).unwrap_or_else(|| "Essential".to_string());

        Some((
            heading,
            ConfigEntry::new(
                long.replace('-', " "),
                Some(format!("--{long}")),
                arg.get_env().map(|env| env.to_string_lossy().to_string()),
                value,
            ),
        ))
    }
}

fn is_runtime_config_arg(arg: &Arg) -> bool {
    let id = arg.get_id().as_str();
    !matches!(id, "help" | "version" | "help_global_parameters") && arg.get_long().is_some()
}

fn protocol_sections(protocol_parameters: Option<&ProtocolParameters>) -> Vec<ConfigSection> {
    let Some(protocol_parameters) = protocol_parameters else {
        return Vec::default();
    };

    vec![
        ConfigSection::new(
            "Protocol Parameters · Network",
            vec![
                config_entry("max block body size", protocol_parameters.max_block_body_size.to_string()),
                config_entry("max transaction size", protocol_parameters.max_transaction_size.to_string()),
                config_entry("max block header size", protocol_parameters.max_block_header_size.to_string()),
                config_entry("max tx ex units", protocol_parameters.max_tx_ex_units.to_string()),
                config_entry("max block ex units", protocol_parameters.max_block_ex_units.to_string()),
                config_entry("max value size", protocol_parameters.max_value_size.to_string()),
                config_entry("max collateral inputs", protocol_parameters.max_collateral_inputs.to_string()),
            ],
        ),
        ConfigSection::new(
            "Protocol Parameters · Economic",
            vec![
                config_entry("min fee a", protocol_parameters.min_fee_a.to_string()),
                config_entry("min fee b", protocol_parameters.min_fee_b.to_string()),
                config_entry("stake credential deposit", protocol_parameters.stake_credential_deposit.to_string()),
                config_entry("stake pool deposit", protocol_parameters.stake_pool_deposit.to_string()),
                config_entry("monetary expansion", protocol_parameters.monetary_expansion_rate.to_string()),
                config_entry("treasury expansion", protocol_parameters.treasury_expansion_rate.to_string()),
                config_entry("min pool cost", protocol_parameters.min_pool_cost.to_string()),
                config_entry("lovelace per UTxO byte", protocol_parameters.lovelace_per_utxo_byte.to_string()),
                config_entry("prices", protocol_parameters.prices.to_string()),
                config_entry("collateral percentage", protocol_parameters.collateral_percentage.to_string()),
                config_entry(
                    "ref script fee per byte",
                    protocol_parameters.min_fee_ref_script_lovelace_per_byte.to_string(),
                ),
                config_entry("max ref script size per tx", protocol_parameters.max_ref_script_size_per_tx.to_string()),
                config_entry(
                    "max ref script size per block",
                    protocol_parameters.max_ref_script_size_per_block.to_string(),
                ),
                config_entry("ref script stride", protocol_parameters.ref_script_cost_stride.to_string()),
                config_entry("ref script multiplier", protocol_parameters.ref_script_cost_multiplier.to_string()),
            ],
        ),
        ConfigSection::new(
            "Protocol Parameters · Governance",
            vec![
                config_entry(
                    "pool max retirement epoch",
                    protocol_parameters.stake_pool_max_retirement_epoch.to_string(),
                ),
                config_entry("optimal stake pools", protocol_parameters.optimal_stake_pools_count.to_string()),
                config_entry("pledge influence", protocol_parameters.pledge_influence.to_string()),
                config_entry("min committee size", protocol_parameters.min_committee_size.to_string()),
                config_entry("max committee term length", protocol_parameters.max_committee_term_length.to_string()),
                config_entry("gov action lifetime", protocol_parameters.gov_action_lifetime.to_string()),
                config_entry("gov action deposit", protocol_parameters.gov_action_deposit.to_string()),
                config_entry("drep deposit", protocol_parameters.drep_deposit.to_string()),
                config_entry("drep expiry", protocol_parameters.drep_expiry.to_string()),
            ],
        ),
    ]
}

fn config_entry(label: &str, value: impl Into<String>) -> ConfigEntry {
    ConfigEntry::new(label, None::<String>, None::<String>, value)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use amaru_kernel::{PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS};
    use clap::Parser;

    use super::{ConfigSection, RuntimeSettingsSource, StartupContext};

    #[derive(Debug, Parser)]
    struct FixtureSettings {
        #[arg(long, help_heading = "Essential")]
        network: String,

        #[arg(long, help_heading = "Network Global Parameters Overrides")]
        consensus_security_param: u64,
    }

    impl RuntimeSettingsSource for FixtureSettings {
        fn value_for(&self, id: &str) -> Option<String> {
            match id {
                "network" => Some(self.network.clone()),
                "consensus_security_param" => Some(self.consensus_security_param.to_string()),
                _ => None,
            }
        }
    }

    #[test]
    fn target_slot_at_uses_era_history_slot_lengths() {
        let startup = StartupContext::new(
            42,
            "preprod",
            "test",
            "test",
            180_224,
            &PREPROD_GLOBAL_PARAMETERS,
            None,
            Some(PREPROD_ERA_HISTORY.clone()),
            Vec::default(),
        );

        let now =
            UNIX_EPOCH + Duration::from_millis(PREPROD_GLOBAL_PARAMETERS.system_start) + Duration::from_secs(2_160_000);
        assert_eq!(startup.target_slot_at(now), Some(518_400));
    }

    #[test]
    fn target_slot_proximity_uses_security_param_as_lag_tolerance() {
        let startup = StartupContext::new(
            42,
            "preprod",
            "test",
            "test",
            180_224,
            &PREPROD_GLOBAL_PARAMETERS,
            None,
            Some(PREPROD_ERA_HISTORY.clone()),
            Vec::default(),
        );

        let now =
            UNIX_EPOCH + Duration::from_millis(PREPROD_GLOBAL_PARAMETERS.system_start) + Duration::from_secs(2_160_000);
        let target_slot = startup.target_slot_at(now).expect("target slot");

        assert_eq!(startup.is_near_target_slot_at(target_slot, now), Some(true));
        assert_eq!(
            startup.is_near_target_slot_at(
                target_slot.saturating_sub(PREPROD_GLOBAL_PARAMETERS.consensus_security_param),
                now
            ),
            Some(true)
        );
        assert_eq!(
            startup.is_near_target_slot_at(
                target_slot.saturating_sub(PREPROD_GLOBAL_PARAMETERS.consensus_security_param + 1),
                now
            ),
            Some(false)
        );
    }

    #[test]
    fn runtime_sections_include_global_parameter_overrides() {
        let settings = FixtureSettings { network: "preview".to_string(), consensus_security_param: 42 };
        let sections = ConfigSection::from_runtime_settings(&settings);

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "Essential");
        assert_eq!(sections[0].entries.len(), 1);
        assert_eq!(sections[0].entries[0].label, "network");
        assert_eq!(sections[1].title, "Network Global Parameters Overrides");
        assert_eq!(sections[1].entries.len(), 1);
        assert_eq!(sections[1].entries[0].label, "consensus security param");
    }
}
