//! Run once to generate the static SRS with lagrange bases (rkyv format):
//! cargo run --release -p generate-srs

use ark_poly::Radix2EvaluationDomain;
use ledger::{
    proofs::{verifiers::make_zkapp_verifier_index, BACKEND_TICK_ROUNDS_N, BACKEND_TOCK_ROUNDS_N},
    VerificationKey,
};
use mina_curves::pasta::{Pallas, Vesta};
use mina_p2p_messages::v2::MinaBaseVerificationKeyWireStableV1;
use poly_commitment::{ipa::SRS, SRS as SRSTrait};
use zeko_sp1_lib::RkyvSRS;

fn pallas_to_flat(p: &Pallas) -> [u8; 65] {
    let mut out = [0u8; 65];
    out[..32].copy_from_slice(&bytemuck::bytes_of(&p.x.0 .0));
    out[32..64].copy_from_slice(&bytemuck::bytes_of(&p.y.0 .0));
    out[64] = p.infinity as u8;
    out
}

fn vesta_to_flat(p: &Vesta) -> [u8; 65] {
    let mut out = [0u8; 65];
    out[..32].copy_from_slice(&bytemuck::bytes_of(&p.x.0 .0));
    out[32..64].copy_from_slice(&bytemuck::bytes_of(&p.y.0 .0));
    out[64] = p.infinity as u8;
    out
}

fn main() {
    println!("Generating rkyv SRS files...");

    // ------------------------------------------------------------------
    // 1. Load VK to get the real domain size
    // ------------------------------------------------------------------
    let vk_b64 = std::fs::read_to_string("proofs/vk.txt").expect("read proofs/vk.txt");
    let vk_wire =
        MinaBaseVerificationKeyWireStableV1::from_base64(vk_b64.trim()).expect("decode vk base64");
    let vk: VerificationKey = (&vk_wire).try_into().expect("vk wire -> runtime");

    let verifier_index = make_zkapp_verifier_index(&vk);

    use ark_poly::EvaluationDomain;
    let domain_size = verifier_index.domain.size();
    println!(
        "  domain_size: {} (1 << {})",
        domain_size,
        domain_size.trailing_zeros()
    );

    // ------------------------------------------------------------------
    // 2. Create Pallas SRS and compute lagrange bases for Kimchi verification
    // ------------------------------------------------------------------
    let pallas_degree = 1 << BACKEND_TOCK_ROUNDS_N;
    println!("  pallas srs degree: {}", pallas_degree);

    let pallas_srs = SRS::<Pallas>::create_parallel(pallas_degree);
    println!("  Pallas SRS created ({} points)", pallas_srs.g.len());

    let domain = Radix2EvaluationDomain::new(domain_size).unwrap();
    pallas_srs.get_lagrange_basis(domain);
    println!("  Pallas lagrange bases computed");

    // ------------------------------------------------------------------
    // 3. Create Vesta SRS for the Pickles accumulator check
    // ------------------------------------------------------------------
    let vesta_degree = 1 << BACKEND_TICK_ROUNDS_N;
    println!("  vesta srs degree:  {}", vesta_degree);

    let vesta_srs = SRS::<Vesta>::create_parallel(vesta_degree);
    println!("  Vesta SRS created ({} points)", vesta_srs.g.len());

    // ------------------------------------------------------------------
    // 4. Convert to rkyv-serializable structs
    // ------------------------------------------------------------------
    println!("  converting to rkyv format...");

    let bases = pallas_srs.get_lagrange_basis_from_domain_size(domain_size);

    let pallas_rkyv_srs = RkyvSRS {
        g_flat: pallas_srs.g.iter().map(pallas_to_flat).collect(),
        h_flat: pallas_to_flat(&pallas_srs.h),
        domain_size,
        lagrange_flat: bases.iter().map(|c| pallas_to_flat(&c.chunks[0])).collect(),
    };

    let vesta_rkyv_srs = RkyvSRS {
        g_flat: vesta_srs.g.iter().map(vesta_to_flat).collect(),
        h_flat: vesta_to_flat(&vesta_srs.h),
        domain_size: 0,
        lagrange_flat: vec![],
    };

    println!("  pallas g points:       {}", pallas_rkyv_srs.g_flat.len());
    println!(
        "  pallas lagrange bases: {}",
        pallas_rkyv_srs.lagrange_flat.len()
    );
    println!("  vesta g points:        {}", vesta_rkyv_srs.g_flat.len());

    // ------------------------------------------------------------------
    // 5. Serialize with rkyv
    // ------------------------------------------------------------------
    let pallas_rkyv_bytes =
        rkyv::to_bytes::<rkyv::rancor::Error>(&pallas_rkyv_srs).expect("rkyv serialize pallas srs");
    let vesta_rkyv_bytes =
        rkyv::to_bytes::<rkyv::rancor::Error>(&vesta_rkyv_srs).expect("rkyv serialize vesta srs");

    // ------------------------------------------------------------------
    // 6. Write files to settlement program
    // ------------------------------------------------------------------
    std::fs::create_dir_all("program/settlement/src").expect("create program/settlement/src");
    std::fs::write(
        "program/settlement/src/srs_pallas_kimchi_rkyv.bin",
        &pallas_rkyv_bytes,
    )
    .expect("write pallas srs");
    std::fs::write(
        "program/settlement/src/srs_vesta_accumulator_rkyv.bin",
        &vesta_rkyv_bytes,
    )
    .expect("write vesta srs");

    println!(
        "✓ srs_pallas_kimchi_rkyv.bin: {} bytes",
        pallas_rkyv_bytes.len()
    );
    println!(
        "✓ srs_vesta_accumulator_rkyv.bin: {} bytes",
        vesta_rkyv_bytes.len()
    );
}
