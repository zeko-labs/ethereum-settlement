pub mod parser;

use anyhow::{anyhow, Context, Result};
use ledger::{
    scan_state::transaction_logic::{
        verifiable,
        zkapp_command::{verifiable::create, ZkAppCommand},
        TransactionStatus, WithStatus,
    },
    verifier::common::{check, CheckResult},
    VerificationKey, VerificationKeyWire,
};
use mina_p2p_messages::v2::MinaBaseVerificationKeyWireStableV1;
use sp1_sdk::{include_elf, Elf, SP1Stdin};

pub const SETTLEMENT_ELF: Elf = include_elf!("settlement-program");
pub const BRIDGE_ELF: Elf = include_elf!("bridge-program");
pub const WITHDRAW_ELF: Elf = include_elf!("withdraw-program");

pub fn settlement_stdin(graphql: &str, vk_b64: &str) -> Result<SP1Stdin> {
    let parsed = parser::parse_graphql_zkapp(graphql)?;
    let vk_wire = MinaBaseVerificationKeyWireStableV1::from_base64(vk_b64.trim())
        .context("decode settlement verification key")?;
    let vk: VerificationKey = (&vk_wire)
        .try_into()
        .map_err(|error| anyhow!("convert verification key: {error:?}"))?;
    let cmd: ZkAppCommand = (&parsed.zkapp_command)
        .try_into()
        .map_err(|error| anyhow!("convert zkApp command: {error:?}"))?;

    let cmd_verifiable = create(&cmd, false, |_, _| Ok(VerificationKeyWire::new(vk.clone())))
        .map_err(|error| anyhow!("create verifiable zkApp command: {error}"))?;
    let (_, zkapp_stmt, _) = match check(WithStatus {
        data: verifiable::UserCommand::ZkAppCommand(Box::new(cmd_verifiable)),
        status: TransactionStatus::Applied,
    }) {
        CheckResult::ValidAssuming((_valid, mut values)) => {
            values.pop().context("missing zkApp statement")?
        }
        other => return Err(anyhow!("invalid zkApp statement: {other:?}")),
    };

    let mut stdin = SP1Stdin::new();
    stdin.write(&vk_wire);
    stdin.write(&parsed.proof);
    stdin.write_slice(&bincode::serialize(&zkapp_stmt)?);
    stdin.write_slice(&bincode::serialize(&parsed.zkapp_command)?);
    Ok(stdin)
}
