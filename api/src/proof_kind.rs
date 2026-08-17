use anyhow::Result;
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProofKind {
    Settlement,
    Bridge,
}

impl ProofKind {
    pub const ALL: [Self; 2] = [Self::Settlement, Self::Bridge];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settlement => "settlement",
            Self::Bridge => "bridge",
        }
    }

    pub const fn conflicting(self) -> Self {
        match self {
            Self::Settlement => Self::Bridge,
            Self::Bridge => Self::Settlement,
        }
    }
}

impl fmt::Display for ProofKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProofKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "settlement" => Ok(Self::Settlement),
            "bridge" => Ok(Self::Bridge),
            _ => anyhow::bail!("unsupported proof kind: {value}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_kinds_round_trip_through_their_database_values() {
        for kind in ProofKind::ALL {
            assert_eq!(kind.as_str().parse::<ProofKind>().unwrap(), kind);
        }
        assert!("withdraw".parse::<ProofKind>().is_err());
    }
}
