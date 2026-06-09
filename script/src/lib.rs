pub mod parser;

use anyhow::{anyhow, Context, Result};
use ark_serialize::CanonicalSerialize;
use kimchi::{circuits::constraints::FeatureFlags, linearization::expr_linearization};
use ledger::{
    proofs::{
        transaction::endos, verification::compute_deferred_values,
        verifiers::make_zkapp_verifier_index,
    },
    scan_state::transaction_logic::{
        verifiable,
        zkapp_command::{verifiable::create, ZkAppCommand},
        TransactionStatus, WithStatus,
    },
    verifier::common::{check, CheckResult},
    VerificationKey, VerificationKeyWire,
};
use mina_curves::pasta::{Fp, Fq};
use mina_p2p_messages::v2::{
    MinaBaseVerificationKeyWireStableV1, PicklesBaseProofsVerifiedStableV1,
};
use sp1_sdk::{include_elf, Elf, SP1Stdin};
use zeko_sp1_lib::{SerializableDeferredValues, SerializablePlonk};

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

    let deferred = compute_deferred_values(&parsed.proof).context("compute deferred values")?;
    let serializable_deferred = SerializableDeferredValues {
        plonk: SerializablePlonk {
            alpha: deferred.plonk.alpha,
            beta: deferred.plonk.beta,
            gamma: deferred.plonk.gamma,
            zeta: deferred.plonk.zeta,
            zeta_to_srs_length: fp_to_bytes(deferred.plonk.zeta_to_srs_length.shifted),
            zeta_to_domain_size: fp_to_bytes(deferred.plonk.zeta_to_domain_size.shifted),
            perm: fp_to_bytes(deferred.plonk.perm.shifted),
            lookup: deferred.plonk.lookup,
            feature_flags_range_check0: deferred.plonk.feature_flags.range_check0,
            feature_flags_range_check1: deferred.plonk.feature_flags.range_check1,
            feature_flags_foreign_field_add: deferred.plonk.feature_flags.foreign_field_add,
            feature_flags_foreign_field_mul: deferred.plonk.feature_flags.foreign_field_mul,
            feature_flags_xor: deferred.plonk.feature_flags.xor,
            feature_flags_rot: deferred.plonk.feature_flags.rot,
            feature_flags_lookup: deferred.plonk.feature_flags.lookup,
            feature_flags_runtime_tables: deferred.plonk.feature_flags.runtime_tables,
        },
        combined_inner_product: fp_to_bytes(deferred.combined_inner_product.shifted),
        b: fp_to_bytes(deferred.b.shifted),
        xi: deferred.xi,
        bulletproof_challenges: deferred
            .bulletproof_challenges
            .iter()
            .map(|value| fp_to_bytes(*value))
            .collect(),
        branch_data_proofs_verified: match deferred.branch_data.proofs_verified {
            PicklesBaseProofsVerifiedStableV1::N0 => 0,
            PicklesBaseProofsVerifiedStableV1::N1 => 1,
            PicklesBaseProofsVerifiedStableV1::N2 => 2,
        },
        branch_data_domain_log2: deferred.branch_data.domain_log2.0.into(),
    };

    let feature_flags = FeatureFlags::default();
    let (linearization, powers_of_alpha) = expr_linearization(Some(&feature_flags), true);
    let (endo_q, _) = endos::<Fq>();
    let mut verifier_index = make_zkapp_verifier_index(&vk);
    verifier_index.linearization = linearization;
    verifier_index.powers_of_alpha = powers_of_alpha;
    verifier_index.endo = endo_q;

    let mut stdin = SP1Stdin::new();
    stdin.write(&vk_wire);
    stdin.write(&parsed.proof);
    stdin.write_slice(&bincode::serialize(&zkapp_stmt)?);
    stdin.write_slice(&bincode::serialize(&serializable_deferred)?);
    stdin.write_slice(&bincode::serialize(&parsed.zkapp_command)?);
    stdin.write_slice(&bincode::serialize(&verifier_index)?);
    Ok(stdin)
}

fn fp_to_bytes(fp: Fp) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    fp.serialize_uncompressed(&mut bytes[..])
        .expect("serialize field element");
    bytes
}
