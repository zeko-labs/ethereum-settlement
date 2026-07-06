use ark_std::vec::Vec;

use crate::ark_std::Zero;
use crate::scalar_mul::Projective;
use crate::{
    scalar_mul::glv::GLVConfig,
    scalar_mul::wnaf::WnafContext,
    short_weierstrass::{Affine},
    AffineRepr,
};

/// A precomputed fixed-base entry for one affine point.
///
/// This stores:
/// - the original affine base
/// - its endomorphism image
/// - a wNAF table for the projective base
/// - a wNAF table for the projective endomorphism(base)
#[derive(Clone, Debug)]
pub struct FixedBaseEntry<C>
where
    C: GLVConfig,
{
    pub base: Affine<C>,
    pub endo_base: Affine<C>,
    pub table_base: Vec<Projective<C>>,
    pub table_endo: Vec<Projective<C>>,
}

/// A reusable fixed-base MSM table.
///
/// This is intended for repeated MSM evaluations over the same base set.
#[derive(Clone, Debug)]
pub struct FixedBaseTable<C>
where
    C: GLVConfig,
{
    pub window_size: usize,
    pub entries: Vec<FixedBaseEntry<C>>,
}

#[inline(always)]
pub fn choose_window_size(num_bases: usize) -> usize {
    match num_bases {
        0..=8 => 3,
        9..=32 => 4,
        33..=128 => 5,
        129..=512 => 6,
        513..=2048 => 7,
        2049..=8192 => 8,
        8193..=32768 => 9,
        _ => 10,
    }
}

impl<C> FixedBaseTable<C>
where
    C: GLVConfig,
{
    /// Build a reusable table for a fixed set of affine bases.
    pub fn new(bases: &[Affine<C>], window_size: usize) -> Self {
        let ctx = WnafContext::new(window_size);

        let entries = bases
            .iter()
            .copied()
            .map(|base| {
                let endo_base = C::endomorphism_affine(&base);
                let table_base = ctx.table(base.into_group());
                let table_endo = ctx.table(endo_base.into_group());

                FixedBaseEntry {
                    base,
                    endo_base,
                    table_base,
                    table_endo,
                }
            })
            .collect();

        Self {
            window_size,
            entries,
        }
    }

    /// Build a reusable table with a default window size.
    pub fn with_default_window(bases: &[Affine<C>]) -> Self {
        Self::new(bases, choose_window_size(bases.len()))
    }

    /// Multiply one fixed base by one scalar using GLV + wNAF.
    #[inline]
    pub fn mul_scalar(&self, index: usize, scalar: &C::ScalarField) -> Projective<C> {
         #[cfg(feature = "debug-log")]
        {
            std::println!("mul_scalar",);
        }
        let entry = &self.entries[index];
        let ctx = WnafContext::new(self.window_size);

        let ((sgn_k1, k1), (sgn_k2, k2)) = C::scalar_decomposition(*scalar);

        let mut p1 = ctx
            .mul_with_table(&entry.table_base, &k1)
            .expect("wNAF table for base is too small");
        let mut p2 = ctx
            .mul_with_table(&entry.table_endo, &k2)
            .expect("wNAF table for endomorphism(base) is too small");

        if !sgn_k1 {
            p1 = -p1;
        }
        if !sgn_k2 {
            p2 = -p2;
        }

        p1 + p2
    }

    /// Compute a fixed-base MSM using GLV + wNAF tables.
    pub fn msm(&self, scalars: &[C::ScalarField]) -> Projective<C> {
        assert_eq!(
            self.entries.len(),
            scalars.len(),
            "number of bases and scalars must match",
        );

        let mut acc = Projective::<C>::zero();

        for (i, scalar) in scalars.iter().enumerate() {
            if scalar.is_zero() {
                continue;
            }

            acc += self.mul_scalar(i, scalar);
        }

        acc
    }
}

/// One-shot fixed-base MSM without explicitly managing the precompute table.
pub fn msm_fixed_base<C>(bases: &[Affine<C>], scalars: &[C::ScalarField]) -> Projective<C>
where
    C: GLVConfig,
{
    let table = FixedBaseTable::<C>::with_default_window(bases);
    table.msm(scalars)
}
