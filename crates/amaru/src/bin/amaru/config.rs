use clap::Parser;

#[derive(Debug, Clone, Copy, Parser)]
pub enum NetworkName {
    Mainnet,
    Preprod,
    Preview,
}

impl std::fmt::Display for NetworkName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mainnet => "mainnet",
            Self::Preprod => "preprod",
            Self::Preview => "preview",
        }
        .fmt(f)
    }
}

impl std::str::FromStr for NetworkName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mainnet" => Ok(Self::Mainnet),
            "preprod" => Ok(Self::Preprod),
            "preview" => Ok(Self::Preview),
            _ => Err(format!("unknown network name: {s}")),
        }
    }
}

impl NetworkName {
    pub fn possible_values<'a>() -> &'a [&'a str; 3] {
        &["mainnet", "preprod", "preview"]
    }

    pub fn to_network_magic(self) -> u32 {
        match self {
            Self::Mainnet => 764824073,
            Self::Preprod => 1,
            Self::Preview => 2,
        }
    }
}
