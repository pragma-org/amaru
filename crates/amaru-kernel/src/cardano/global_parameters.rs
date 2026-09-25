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

use crate::{Lovelace, Slot};

#[cfg(feature = "clap")]
#[allow(clippy::expect_used)]
fn default_global_parameters() -> &'static GlobalParameters {
    crate::NetworkName::default().as_global_parameters().expect("no default GlobalParameters for default network!?")
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
#[cfg_attr(feature = "clap", command(next_help_heading = "Network global parameters"))]
pub struct GlobalParameters {
    /// The maximum depth of a rollback, also known as the security parameter 'k'.
    ///
    /// This translates down to the length of our volatile storage, containing states of the ledger
    /// which aren't yet considered final.
    #[cfg_attr(feature = "clap", arg(
        long,
        value_name = "UINT",
        env = "AMARU_GLOBAL_CONSENSUS_SECURITY_PARAM",
        hide_short_help = true,
        default_value_t = default_global_parameters().consensus_security_param,
    ))]
    pub consensus_security_param: u64,

    /// Multiplier applied to the `consensus_security_param` to determine the epoch length.
    #[cfg_attr(feature = "clap", arg(
        long,
        value_name = "UINT",
        env = "AMARU_GLOBAL_EPOCH_LENGTH_SCALE_FACTOR",
        hide_short_help = true,
        default_value_t = default_global_parameters().epoch_length_scale_factor,
    ))]
    pub epoch_length_scale_factor: u64,

    /// Inverse of the active slot coefficient (i.e. 1/f);
    #[cfg_attr(feature = "clap", arg(
        long,
        value_name = "UINT",
        env = "AMARU_GLOBAL_ACTIVE_SLOT_COEFF_INVERSE",
        hide_short_help = true,
        default_value_t = default_global_parameters().active_slot_coeff_inverse,
    ))]
    pub active_slot_coeff_inverse: u64,

    /// Maximum supply of Ada, in lovelace (1 Ada = 1,000,000 Lovelace)
    #[cfg_attr(feature = "clap", arg(
        long,
        value_name = "LOVELACE",
        env = "AMARU_GLOBAL_MAX_LOVELACE_SUPPLY",
        hide_short_help = true,
        default_value_t = default_global_parameters().max_lovelace_supply,
    ))]
    pub max_lovelace_supply: Lovelace,

    /// Number of slots for a single KES validity period.
    #[cfg_attr(feature = "clap", arg(
        long,
        value_name = "UINT",
        env = "AMARU_GLOBAL_SLOTS_PER_KES_PERIOD",
        hide_short_help = true,
        default_value_t = default_global_parameters().slots_per_kes_period,
    ))]
    pub slots_per_kes_period: u64,

    /// Maximum number of KES key evolution. Combined with SLOTS_PER_KES_PERIOD, these values
    /// indicates the validity period of a KES key before a new one is required.
    #[cfg_attr(feature = "clap", arg(
        long,
        value_name = "U8",
        env = "AMARU_GLOBAL_MAX_KES_EVOLUTION",
        hide_short_help = true,
        default_value_t = default_global_parameters().max_kes_evolution,
    ))]
    pub max_kes_evolution: u8,

    /// POSIX time (milliseconds) of the System Start.
    #[cfg_attr(feature = "clap", arg(
        long,
        value_name = "MILLIS",
        env = "AMARU_GLOBAL_SYSTEM_START",
        hide_short_help = true,
        default_value_t = default_global_parameters().system_start,
    ))]
    pub system_start: u64,
}

impl GlobalParameters {
    /// Relative slot from which data of the previous epoch can be considered stable.
    pub fn stability_window(&self) -> Slot {
        Slot::new(3 * self.consensus_security_param * self.active_slot_coeff_inverse)
    }

    /// Number of slots at the end of each epoch which do NOT contribute randomness to the candidate
    /// nonce of the following epoch.
    pub fn randomness_stabilization_window(&self) -> u64 {
        4 * self.consensus_security_param * self.active_slot_coeff_inverse
    }

    /// Number of slots in an epoch
    pub fn epoch_length(&self) -> u64 {
        self.active_slot_coeff_inverse * self.epoch_length_scale_factor * self.consensus_security_param
    }
}

#[cfg(feature = "clap")]
impl GlobalParameters {
    /// Hide the global parameters options from the given command; to only show them on-demand.
    pub fn hide_options(mut cmd: clap::Command) -> clap::Command {
        use clap::Args as _;

        for arg in GlobalParameters::augment_args(clap::Command::new("global-parameters")).get_arguments() {
            cmd = cmd.mut_arg(arg.get_id(), |arg| arg.hide(true));
        }

        cmd
    }

    /// Show a custom help on demand for global parameters options, instead of cluttering the
    /// standard --help.
    pub fn show_help() -> Result<(), std::io::Error> {
        use clap::Args as _;

        let cmd = clap::Command::new("--help-global-parameters").about(
            "The following options are hidden by default, but available on some commands (e.g. 'run' or 'bootstrap').",
        );

        Self::augment_args(cmd).disable_help_flag(true).disable_help_subcommand(true).print_long_help()
    }
}

// see Shelley Genesis https://book.world.dev.cardano.org/env-mainnet.html
pub static MAINNET_GLOBAL_PARAMETERS: GlobalParameters = {
    GlobalParameters {
        system_start: 1506203091000, // 2017-09-23T21:44:51Z
        consensus_security_param: 2160,
        epoch_length_scale_factor: 10,
        active_slot_coeff_inverse: 20,
        max_lovelace_supply: 45_000_000_000_000_000,
        slots_per_kes_period: 129_600,
        max_kes_evolution: 62,
    }
};

// see Shelley Genesis https://book.world.dev.cardano.org/env-preprod.html
pub static PREPROD_GLOBAL_PARAMETERS: GlobalParameters = {
    GlobalParameters {
        system_start: 1654041600000, // 2022-06-01T00:00:00Z
        ..MAINNET_GLOBAL_PARAMETERS
    }
};

// see Shelley Genesis https://book.world.dev.cardano.org/env-preview.html
pub static PREVIEW_GLOBAL_PARAMETERS: GlobalParameters = {
    GlobalParameters {
        system_start: 1666656000000, // 2022-10-25T00:00Z
        consensus_security_param: 432,
        ..MAINNET_GLOBAL_PARAMETERS
    }
};
