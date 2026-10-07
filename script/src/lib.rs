use anyhow::{Context, Result};
use mina_curves::pasta::{Pallas, Vesta};
use pickles_verifier::serialize::{decode_verifier_blob, encode_verifier_blob};
use pickles_verifier::types::{VerifiableProof, Verifier};
use pickles_verifier::verify;
use pickles_verifier::wire::{
    canonical_wrap_vk_json, parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof,
};
use poly_commitment::ipa::SRS;
use sha2::{Digest, Sha256};
use sp1_core_executor::Program;
use sp1_core_executor_runner::MinimalExecutorRunner;
use sp1_sdk::{Elf, SP1Stdin};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use zeko_sp1_lib::SettlementWitnessV1;

/// Environment variable naming the operator-mounted artifacts directory.
pub const ARTIFACTS_DIR_ENV: &str = "ZEKO_ARTIFACTS_DIR";
/// In-container default artifacts directory (declared by the Nix gateway image).
pub const DEFAULT_ARTIFACTS_DIR: &str = "/var/lib/zeko-proof-api/artifacts";
/// File names expected inside the artifacts directory. These match the
/// `zeko-settlement-guests` Nix output layout.
pub const SETTLEMENT_ELF_FILE: &str = "settlement-program";
pub const BRIDGE_ELF_FILE: &str = "bridge-program";
pub const SETTLEMENT_VK_FILE: &str = "settlement-vk.serde.json";

// The Pasta SRS is a universal Mina constant, not a per-rollup artifact. These
// are the same prod files `poly_commitment::precomputed_srs::get_srs` reads
// from the source checkout, embedded because that checkout is absent at
// runtime.
static VESTA_SRS_BYTES: &[u8] = include_bytes!("../../vendor/proof-systems/srs/vesta.srs");
static PALLAS_SRS_BYTES: &[u8] = include_bytes!("../../vendor/proof-systems/srs/pallas.srs");

/// Native (host-side) Pickles verifier pinned to one settlement VK.
pub struct NativeSettlementVerifier {
    vk_canonical: String,
    vk_hash: [u8; 32],
    verifier: Verifier,
}

impl NativeSettlementVerifier {
    /// Build the verifier from the settlement VK JSON exactly as the former
    /// build script did: parse the wrap VK, encode the verifier blob with the
    /// prod Pasta SRS, then decode it so the wrap Lagrange basis is seeded.
    /// `vk_hash` is the SHA-256 of the VK file bytes, matching the on-chain
    /// `SETTLEMENT_VK_HASH` convention.
    pub fn from_vk_json(vk_json: &str) -> Result<Self> {
        let wrap_vk = parse_wrap_vk(vk_json)
            .map_err(anyhow::Error::msg)
            .context("parse settlement VK JSON")?;
        let vk_canonical = canonical_wrap_vk_json(&wrap_vk)
            .map_err(anyhow::Error::msg)
            .context("canonicalize settlement VK")?;
        let vesta_srs: SRS<Vesta> =
            rmp_serde::from_slice(VESTA_SRS_BYTES).context("decode embedded Vesta SRS")?;
        let pallas_srs: SRS<Pallas> =
            rmp_serde::from_slice(PALLAS_SRS_BYTES).context("decode embedded Pallas SRS")?;
        let blob = encode_verifier_blob(&vesta_srs, &pallas_srs, 1, &wrap_vk);
        let blob = AlignedBlob::new(&blob);
        let verifier = decode_verifier_blob(blob.as_bytes());
        Ok(Self {
            vk_canonical,
            vk_hash: Sha256::digest(vk_json.as_bytes()).into(),
            verifier,
        })
    }

    pub fn vk_hash(&self) -> [u8; 32] {
        self.vk_hash
    }
}

/// `decode_verifier_blob` requires 8-byte aligned input; a `Vec<u8>` does not
/// guarantee that, so the blob is copied into `u64` storage first.
struct AlignedBlob {
    words: Vec<u64>,
    len: usize,
}

impl AlignedBlob {
    fn new(bytes: &[u8]) -> Self {
        let mut words = vec![0u64; bytes.len().div_ceil(8)];
        // SAFETY: `words` owns `words.len() * 8 >= bytes.len()` initialized
        // bytes; writing `bytes.len()` bytes through its `u8` view stays in
        // bounds and `u64` has no invalid bit patterns.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                words.as_mut_ptr().cast::<u8>(),
                bytes.len(),
            );
        }
        Self {
            words,
            len: bytes.len(),
        }
    }

    fn as_bytes(&self) -> &[u8] {
        // SAFETY: the first `len` bytes of `words` are initialized above and
        // the returned slice borrows `self`.
        unsafe { std::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), self.len) }
    }
}

/// Per-rollup public artifacts loaded at startup from an operator-mounted
/// directory: both SP1 guest ELFs and the settlement VK with its derived native
/// verifier. Nothing is embedded at compile time and no fixture fallback exists.
pub struct Artifacts {
    pub dir: PathBuf,
    pub settlement_elf: Elf,
    pub bridge_elf: Elf,
    pub settlement_vk_json: String,
    pub native: NativeSettlementVerifier,
}

impl Artifacts {
    /// Directory from `ZEKO_ARTIFACTS_DIR`, else [`DEFAULT_ARTIFACTS_DIR`].
    pub fn dir_from_env() -> PathBuf {
        std::env::var_os(ARTIFACTS_DIR_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ARTIFACTS_DIR))
    }

    /// Load and validate every artifact; any missing or unparseable file is an
    /// error naming the path.
    pub fn load(dir: &Path) -> Result<Self> {
        anyhow::ensure!(
            dir.is_dir(),
            "settlement artifacts directory {} does not exist or is not a directory (set {ARTIFACTS_DIR_ENV})",
            dir.display()
        );
        let read_bytes = |name: &str| -> Result<Vec<u8>> {
            let path = dir.join(name);
            let bytes = std::fs::read(&path)
                .with_context(|| format!("failed to read artifact {}", path.display()))?;
            anyhow::ensure!(!bytes.is_empty(), "artifact {} is empty", path.display());
            Ok(bytes)
        };
        let settlement_elf = Elf::from(read_bytes(SETTLEMENT_ELF_FILE)?);
        let bridge_elf = Elf::from(read_bytes(BRIDGE_ELF_FILE)?);
        let vk_path = dir.join(SETTLEMENT_VK_FILE);
        let settlement_vk_json = String::from_utf8(read_bytes(SETTLEMENT_VK_FILE)?)
            .with_context(|| format!("artifact {} is not UTF-8", vk_path.display()))?;
        let native = NativeSettlementVerifier::from_vk_json(&settlement_vk_json)
            .with_context(|| {
                format!(
                    "artifact {} is not a usable settlement VK",
                    vk_path.display()
                )
            })?;
        Ok(Self {
            dir: dir.to_path_buf(),
            settlement_elf,
            bridge_elf,
            settlement_vk_json,
            native,
        })
    }

    pub fn load_from_env() -> Result<Self> {
        Self::load(&Self::dir_from_env())
    }
}

static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();

/// Load the artifacts from `dir` once for the whole process. A second call
/// with the same directory returns the already-loaded set; a different
/// directory is an error.
pub fn init_artifacts(dir: &Path) -> Result<&'static Artifacts> {
    if let Some(loaded) = ARTIFACTS.get() {
        anyhow::ensure!(
            loaded.dir == dir,
            "settlement artifacts were already loaded from {}",
            loaded.dir.display()
        );
        return Ok(loaded);
    }
    let loaded = Artifacts::load(dir)?;
    Ok(ARTIFACTS.get_or_init(|| loaded))
}

pub fn init_artifacts_from_env() -> Result<&'static Artifacts> {
    init_artifacts(&Artifacts::dir_from_env())
}

/// Process-wide artifacts; fails closed when [`init_artifacts`] has not run.
pub fn artifacts() -> Result<&'static Artifacts> {
    ARTIFACTS
        .get()
        .context("settlement artifacts are not loaded; call init_artifacts at startup")
}

/// Execute without proving using the low-memory runner. SP1 SDK 6.1's execute
/// wrapper can fail while extracting the public-value digest for this very
/// large program even after successful execution.
pub fn execute_minimal(elf: Elf, stdin: SP1Stdin) -> Result<(Vec<u8>, u64)> {
    let program = Arc::new(
        Program::from(&elf).map_err(|error| anyhow::anyhow!("disassemble program: {error}"))?,
    );
    let mut executor = MinimalExecutorRunner::simple(program);
    for input in stdin.buffer {
        executor.with_input(&input);
    }
    while executor
        .try_execute_chunk()
        .map_err(|error| anyhow::anyhow!("execute chunk: {error}"))?
        .is_some()
    {}
    let exit_code = executor.exit_code();
    if exit_code != 0 {
        anyhow::bail!("program exited with status {exit_code}");
    }
    let cycles = executor.global_clk();
    Ok((executor.into_public_values_stream(), cycles))
}

pub use zeko_sp1_lib::SettlementProofBundle;

pub fn settlement_stdin(fixture_dir: &str) -> Result<SP1Stdin> {
    let verifiable = load_verifiable(Path::new(fixture_dir))?;
    Ok(settlement_stdin_for_verifiable(&verifiable, None))
}

pub fn settlement_stdin_from_bundle(bundle: &SettlementProofBundle) -> Result<SP1Stdin> {
    let verifiable = load_verifiable_bundle(bundle)?;
    let witness = settlement_witness(bundle)?;
    Ok(settlement_stdin_for_verifiable(&verifiable, witness))
}

pub fn native_settlement_preflight(
    native: &NativeSettlementVerifier,
    bundle: &SettlementProofBundle,
) -> Result<Vec<u8>> {
    let verifiable = native_verify_settlement(native, bundle)?;
    let witness = settlement_witness(bundle)?
        .context("settlement binding and context are required for native preflight")?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        settlement_program::derive_receipt_bytes(&verifiable, witness, native.vk_hash)
    }))
    .map_err(|_| anyhow::anyhow!("settlement receipt derivation failed"))
}

fn native_verify_settlement(
    native: &NativeSettlementVerifier,
    bundle: &SettlementProofBundle,
) -> Result<VerifiableProof> {
    let supplied_vk = parse_wrap_vk(&bundle.vk_json)
        .map_err(anyhow::Error::msg)
        .context("parse settlement VK JSON")?;
    let supplied_vk = canonical_wrap_vk_json(&supplied_vk).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        supplied_vk == native.vk_canonical,
        "settlement VK does not match the pinned verifier"
    );
    let verifiable = load_verifiable_bundle(bundle)?;
    anyhow::ensure!(
        verify(&native.verifier, &verifiable),
        "Pickles proof verification failed"
    );
    Ok(verifiable)
}

fn settlement_witness(bundle: &SettlementProofBundle) -> Result<Option<SettlementWitnessV1>> {
    match (&bundle.binding, &bundle.context) {
        (Some(binding), Some(context)) => Ok(Some(SettlementWitnessV1 {
            binding: binding.clone(),
            context: context.clone(),
            inner_action_batch: bundle.inner_action_batch.clone(),
            asset_registry_checkpoint: bundle.asset_registry_checkpoint.clone(),
            asset_registry_batch: bundle.asset_registry_batch.clone(),
        })),
        (None, None) => Ok(None),
        _ => anyhow::bail!("settlement binding and context must either both be present or absent"),
    }
}

fn settlement_stdin_for_verifiable(
    verifiable: &VerifiableProof,
    witness: Option<SettlementWitnessV1>,
) -> SP1Stdin {
    let mut stdin = SP1Stdin::new();
    stdin.write(verifiable);
    stdin.write(&witness);
    stdin
}

fn load_verifiable(fixture_dir: &Path) -> Result<VerifiableProof> {
    let read = |name: &str| -> Result<String> {
        let path = fixture_dir.join(name);
        std::fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))
    };

    let vk_json = read("vk.serde.json")?;
    let proof_json = read("proof.serde.json")?;
    let skeleton_json = read("public_input_skeleton.json")?;
    let app_stmt_json = read("app_statement.json")?;

    load_verifiable_bundle(&SettlementProofBundle {
        vk_json,
        proof_json,
        public_input_skeleton_json: skeleton_json,
        app_statement_json: app_stmt_json,
        binding: None,
        context: None,
        inner_action_batch: None,
        asset_registry_checkpoint: None,
        asset_registry_batch: None,
    })
}

pub fn load_verifiable_bundle(bundle: &SettlementProofBundle) -> Result<VerifiableProof> {
    let wrap_vk = parse_wrap_vk(&bundle.vk_json)
        .map_err(anyhow::Error::msg)
        .context("parse vk JSON")?;
    let wrap_proof = parse_wrap_proof(&bundle.proof_json)
        .map_err(anyhow::Error::msg)
        .context("parse proof JSON")?;
    let ocaml = OcamlProof::parse(&bundle.public_input_skeleton_json)
        .map_err(anyhow::Error::msg)
        .context("parse public input skeleton JSON")?;
    let app_statement = parse_app_statement_fields(&bundle.app_statement_json)
        .map_err(anyhow::Error::msg)
        .context("parse application statement JSON")?;

    ocaml
        .into_verifiable(wrap_proof, &wrap_vk, &app_statement)
        .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_fixture() -> SettlementProofBundle {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../proofs/mainnet-blockchain-snark");
        let read = |name: &str| {
            std::fs::read_to_string(fixture.join(name))
                .unwrap_or_else(|error| panic!("read {name}: {error}"))
        };
        SettlementProofBundle {
            vk_json: read("vk.serde.json"),
            proof_json: read("proof.serde.json"),
            public_input_skeleton_json: read("public_input_skeleton.json"),
            app_statement_json: read("app_statement.json"),
            binding: None,
            context: None,
            inner_action_batch: None,
            asset_registry_checkpoint: None,
            asset_registry_batch: None,
        }
    }

    fn pinned_native_verifier() -> NativeSettlementVerifier {
        NativeSettlementVerifier::from_vk_json(&pinned_fixture().vk_json)
            .expect("pinned fixture VK must build a native verifier")
    }

    #[test]
    fn native_verifier_accepts_pinned_fixture_and_json_whitespace() {
        let native = pinned_native_verifier();
        let mut fixture = pinned_fixture();
        fixture.vk_json.push('\n');
        native_verify_settlement(&native, &fixture).expect("native verification");
    }

    #[test]
    fn artifacts_fail_closed_on_missing_directory() {
        let missing = std::env::temp_dir().join("zeko-artifacts-does-not-exist");
        let error = match Artifacts::load(&missing) {
            Ok(_) => panic!("missing directory must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(ARTIFACTS_DIR_ENV));
    }

    #[test]
    fn artifacts_fail_closed_on_missing_files() {
        let dir = std::env::temp_dir().join(format!("zeko-artifacts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let error = match Artifacts::load(&dir) {
            Ok(_) => panic!("empty directory must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(SETTLEMENT_ELF_FILE));
        std::fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn native_verifier_rejects_mutated_statement() {
        let mut fixture = pinned_fixture();
        let last_digit = fixture
            .app_statement_json
            .rfind('1')
            .expect("fixture statement contains a one");
        fixture
            .app_statement_json
            .replace_range(last_digit..=last_digit, "2");
        let error = match native_verify_settlement(&pinned_native_verifier(), &fixture) {
            Ok(_) => panic!("mutation must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("verification failed"));
    }

    #[test]
    fn native_verifier_rejects_mutated_verification_key() {
        let mut fixture = pinned_fixture();
        let mut vk: serde_json::Value = serde_json::from_str(&fixture.vk_json).unwrap();
        vk["generic_comm"] = vk["psm_comm"].clone();
        fixture.vk_json = serde_json::to_string(&vk).unwrap();
        assert!(native_verify_settlement(&pinned_native_verifier(), &fixture).is_err());
    }
}
