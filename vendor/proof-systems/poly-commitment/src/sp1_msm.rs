//! SP1-optimized MSM for Vesta/Pallas using Pippenger and SP1 uint256 modular multiplication.
//! Field elements are kept in canonical limb form because `sys_bigint` computes `(x * y) mod p`.

use ark_ff::PrimeField;

// ---------------------------------------------------------------------------
// Moduli
// ---------------------------------------------------------------------------
const FQ_MOD: [u64; 4] = [
    0x8c46eb2100000001,
    0x224698fc0994a8dd,
    0x0000000000000000,
    0x4000000000000000,
];

const FP_MOD: [u64; 4] = [
    0x992d30ed00000001,
    0x224698fc094cf91b,
    0x0000000000000000,
    0x4000000000000000,
];

// ---------------------------------------------------------------------------
// Field element in canonical little-endian limbs.
// ---------------------------------------------------------------------------
type Limbs = [u64; 4];

#[inline(always)]
fn mont_mul(a: &Limbs, b: &Limbs, m: &Limbs) -> Limbs {
    #[cfg(target_os = "zkvm")]
    unsafe {
        let mut out = [0u64; 4];
        sp1_lib::sys_bigint(&mut out, 0, a, b, m);
        out
    }

    #[cfg(not(target_os = "zkvm"))]
    {
        let _ = (a, b, m);
        panic!("SP1 MSM modular multiplication is only implemented for zkVM targets");
    }
}

#[inline(always)]
fn mont_add(a: &Limbs, b: &Limbs, m: &Limbs) -> Limbs {
    let mut out = [0u64; 4];
    let mut carry = 0u64;
    for i in 0..4 {
        let (s1, c1) = a[i].overflowing_add(b[i]);
        let (s2, c2) = s1.overflowing_add(carry);
        out[i] = s2;
        carry = (c1 as u64) + (c2 as u64);
    }
    // Conditional subtraction
    if carry != 0 || ge(&out, m) {
        out = sub(&out, m);
    }
    out
}

#[inline(always)]
fn mont_sub(a: &Limbs, b: &Limbs, m: &Limbs) -> Limbs {
    if ge(a, b) {
        sub(a, b)
    } else {
        // a - b + m
        let tmp = sub(m, b);
        mont_add(a, &tmp, m)
    }
}

#[inline(always)]
fn mont_neg(a: &Limbs, m: &Limbs) -> Limbs {
    if is_zero(a) {
        return *a;
    }
    sub(m, a)
}

#[inline(always)]
fn mont_square(a: &Limbs, m: &Limbs) -> Limbs {
    mont_mul(a, a, m)
}

#[inline(always)]
fn is_zero(a: &Limbs) -> bool {
    a[0] == 0 && a[1] == 0 && a[2] == 0 && a[3] == 0
}

#[inline(always)]
fn ge(a: &Limbs, b: &Limbs) -> bool {
    for i in (0..4).rev() {
        if a[i] > b[i] {
            return true;
        }
        if a[i] < b[i] {
            return false;
        }
    }
    true
}

#[inline(always)]
fn sub(a: &Limbs, b: &Limbs) -> Limbs {
    let mut out = [0u64; 4];
    let mut borrow = 0u64;
    for i in 0..4 {
        let (d1, b1) = a[i].overflowing_sub(b[i]);
        let (d2, b2) = d1.overflowing_sub(borrow);
        out[i] = d2;
        borrow = (b1 as u64) + (b2 as u64);
    }
    out
}

// ---------------------------------------------------------------------------
// Jacobian point — coordinates in canonical field form
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug)]
struct JacPoint {
    x: Limbs,
    y: Limbs,
    z: Limbs,
    m: &'static Limbs,
}

impl JacPoint {
    #[inline]
    fn infinity(m: &'static Limbs) -> Self {
        let one = [1, 0, 0, 0];
        JacPoint {
            x: one,
            y: one,
            z: [0, 0, 0, 0],
            m,
        }
    }

    #[inline]
    fn is_zero(&self) -> bool {
        is_zero(&self.z)
    }

    #[inline]
    fn from_affine(x: Limbs, y: Limbs, m: &'static Limbs) -> Self {
        let one = [1, 0, 0, 0];
        JacPoint { x, y, z: one, m }
    }

    /// Jacobian doubling — a=0 for Pasta curves
    fn double(self) -> Self {
        let m = self.m;
        if self.is_zero() || is_zero(&self.y) {
            return Self::infinity(m);
        }
        let a = mont_square(&self.x, m);
        let b = mont_square(&self.y, m);
        let c = mont_square(&b, m);
        // d = 2*((x+b)^2 - a - c)
        let xb = mont_add(&self.x, &b, m);
        let xb2 = mont_square(&xb, m);
        let d = {
            let t = mont_sub(&xb2, &a, m);
            let t = mont_sub(&t, &c, m);
            mont_add(&t, &t, m)
        };
        // e = 3*a
        let e = mont_add(&mont_add(&a, &a, m), &a, m);
        let f = mont_square(&e, m);
        // x3 = f - 2d
        let x3 = mont_sub(&f, &mont_add(&d, &d, m), m);
        // y3 = e*(d-x3) - 8c
        let y3 = {
            let t = mont_sub(&d, &x3, m);
            let t = mont_mul(&e, &t, m);
            let c8 = {
                let c2 = mont_add(&c, &c, m);
                let c4 = mont_add(&c2, &c2, m);
                mont_add(&c4, &c4, m)
            };
            mont_sub(&t, &c8, m)
        };
        // z3 = 2*y*z
        let z3 = mont_mul(&mont_add(&self.y, &self.y, m), &self.z, m);
        JacPoint {
            x: x3,
            y: y3,
            z: z3,
            m,
        }
    }

    /// Mixed addition: Jacobian + affine, with Z2 = 1.
    fn add_affine(self, ax: Limbs, ay: Limbs) -> Self {
        let m = self.m;
        if self.is_zero() {
            return Self::from_affine(ax, ay, m);
        }
        let z1z1 = mont_square(&self.z, m);
        let u2 = mont_mul(&ax, &z1z1, m);
        let s2 = mont_mul(&mont_mul(&ay, &self.z, m), &z1z1, m);
        let h = mont_sub(&u2, &self.x, m);
        let r = mont_sub(&s2, &self.y, m);

        if is_zero(&h) {
            return if is_zero(&r) {
                self.double()
            } else {
                Self::infinity(m)
            };
        }

        let hh = mont_square(&h, m);
        let hhh = mont_mul(&h, &hh, m);
        let v = mont_mul(&self.x, &hh, m);
        // x3 = r^2 - hhh - 2v
        let x3 = mont_sub(
            &mont_sub(&mont_square(&r, m), &hhh, m),
            &mont_add(&v, &v, m),
            m,
        );
        // y3 = r*(v-x3) - y*hhh
        let y3 = mont_sub(
            &mont_mul(&r, &mont_sub(&v, &x3, m), m),
            &mont_mul(&self.y, &hhh, m),
            m,
        );
        // z3 = h*z1
        let z3 = mont_mul(&h, &self.z, m);
        JacPoint {
            x: x3,
            y: y3,
            z: z3,
            m,
        }
    }

    /// Full Jacobian addition
    fn add(self, rhs: Self) -> Self {
        let m = self.m;
        if self.is_zero() {
            return rhs;
        }
        if rhs.is_zero() {
            return self;
        }

        let z1z1 = mont_square(&self.z, m);
        let z2z2 = mont_square(&rhs.z, m);
        let u1 = mont_mul(&self.x, &z2z2, m);
        let u2 = mont_mul(&rhs.x, &z1z1, m);
        let s1 = mont_mul(&mont_mul(&self.y, &rhs.z, m), &z2z2, m);
        let s2 = mont_mul(&mont_mul(&rhs.y, &self.z, m), &z1z1, m);
        let h = mont_sub(&u2, &u1, m);
        let r = mont_sub(&s2, &s1, m);

        if is_zero(&h) {
            return if is_zero(&r) {
                self.double()
            } else {
                Self::infinity(m)
            };
        }

        let hh = mont_square(&h, m);
        let hhh = mont_mul(&h, &hh, m);
        let v = mont_mul(&u1, &hh, m);
        let x3 = mont_sub(
            &mont_sub(&mont_square(&r, m), &hhh, m),
            &mont_add(&v, &v, m),
            m,
        );
        let y3 = mont_sub(
            &mont_mul(&r, &mont_sub(&v, &x3, m), m),
            &mont_mul(&s1, &hhh, m),
            m,
        );
        let z3 = mont_mul(&mont_mul(&h, &self.z, m), &rhs.z, m);
        JacPoint {
            x: x3,
            y: y3,
            z: z3,
            m,
        }
    }
}

// ---------------------------------------------------------------------------
// wNAF digit extraction
// ---------------------------------------------------------------------------
fn make_digits_wnaf(scalar: &Limbs, w: usize, num_bits: usize) -> Vec<i64> {
    let radix: u64 = 1 << w;
    let window_mask: u64 = radix - 1;
    let digits_count = (num_bits + w - 1) / w;
    let mut carry = 0u64;
    let mut digits = Vec::with_capacity(digits_count);

    for i in 0..digits_count {
        let bit_offset = i * w;
        let u64_idx = bit_offset / 64;
        let bit_idx = bit_offset % 64;

        let bit_buf = if bit_idx < 64 - w || u64_idx == scalar.len() - 1 {
            scalar[u64_idx] >> bit_idx
        } else {
            (scalar[u64_idx] >> bit_idx) | (scalar[u64_idx + 1] << (64 - bit_idx))
        };

        let coef = carry + (bit_buf & window_mask);
        carry = (coef + radix / 2) >> w;
        let mut digit = (coef as i64) - (carry << w) as i64;
        if i == digits_count - 1 {
            digit += (carry << w) as i64;
        }
        digits.push(digit);
    }
    digits
}

fn ln_without_floats(n: usize) -> usize {
    let mut log = 0;
    let mut x = n;
    while x > 1 {
        x >>= 1;
        log += 1;
    }
    log
}

// ---------------------------------------------------------------------------
// Pippenger MSM — points already in canonical field form
// ---------------------------------------------------------------------------
fn pippenger(
    points: &[(Limbs, Limbs)], // (x, y) in canonical field form
    scalars: &[Limbs],         // scalars in standard form (BigInt)
    m: &'static Limbs,
) -> JacPoint {
    let n = points.len();
    if n == 0 {
        return JacPoint::infinity(m);
    }

    let c = if n < 32 { 3 } else { ln_without_floats(n) + 2 };
    let num_bits = 255usize;
    let num_windows = (num_bits + c - 1) / c;

    // Pre-compute wNAF digits
    let scalar_digits: Vec<Vec<i64>> = scalars
        .iter()
        .map(|s| make_digits_wnaf(s, c, num_bits))
        .collect();

    let mut window_sums: Vec<JacPoint> = Vec::with_capacity(num_windows);

    for w in 0..num_windows {
        let num_buckets = 1usize << c;
        let mut buckets = vec![JacPoint::infinity(m); num_buckets];

        for (i, (px, py)) in points.iter().enumerate() {
            if is_zero(px) && is_zero(py) {
                continue;
            }
            let digit = scalar_digits[i][w];
            if digit == 0 {
                continue;
            }

            if digit > 0 {
                let idx = (digit - 1) as usize;
                buckets[idx] = buckets[idx].add_affine(*px, *py);
            } else {
                let idx = (-digit - 1) as usize;
                let neg_y = mont_neg(py, m);
                buckets[idx] = buckets[idx].add_affine(*px, neg_y);
            }
        }

        // Running sum trick
        let mut running_sum = JacPoint::infinity(m);
        let mut window_sum = JacPoint::infinity(m);
        for b in (0..num_buckets).rev() {
            running_sum = running_sum.add(buckets[b]);
            window_sum = window_sum.add(running_sum);
        }
        window_sums.push(window_sum);
    }

    // Combine windows: result = sum_w window_w * 2^(w*c)
    let lowest = window_sums[0];
    let upper = window_sums[1..]
        .iter()
        .rev()
        .fold(JacPoint::infinity(m), |mut total, ws| {
            total = total.add(*ws);
            for _ in 0..c {
                total = total.double();
            }
            total
        });
    upper.add(lowest)
}

// ---------------------------------------------------------------------------
// Public API — accepts ark-ff points and scalars
// ---------------------------------------------------------------------------

/// Convert ark-ff affine point to canonical little-endian limbs.
#[inline]
fn ark_to_limbs<G: ark_ec::AffineRepr>(p: &G) -> Option<(Limbs, Limbs)>
where
    G::BaseField: ark_ff::PrimeField,
{
    use ark_serialize::CanonicalSerialize;
    if p.is_zero() {
        return None;
    }
    let mut xb = [0u8; 32];
    let mut yb = [0u8; 32];
    let (ax, ay) = p.xy().unwrap();
    ax.serialize_uncompressed(&mut xb[..]).ok()?;
    ay.serialize_uncompressed(&mut yb[..]).ok()?;
    // serialize_uncompressed gives canonical field form.
    let x_std: Limbs = bytemuck::cast(xb);
    let y_std: Limbs = bytemuck::cast(yb);
    Some((x_std, y_std))
}

/// MSM on Pallas (base field = Fp)
pub fn sp1_pallas_msm_ark<G: ark_ec::AffineRepr>(points: &[G], scalars: &[G::ScalarField]) -> bool
where
    G::BaseField: ark_ff::PrimeField,
    G::ScalarField: ark_ff::PrimeField,
{
    let m: &'static Limbs = &FP_MOD;

    let limb_points: Vec<(Limbs, Limbs)> = points
        .iter()
        .map(|p| ark_to_limbs(p).unwrap_or(([0; 4], [0; 4])))
        .collect();

    let scalar_limbs: Vec<Limbs> = scalars
        .iter()
        .map(|s| unsafe { *(s.into_bigint().as_ref().as_ptr() as *const Limbs) })
        .collect();

    let result = pippenger(&limb_points, &scalar_limbs, m);
    result.is_zero()
}

/// MSM on Vesta (base field = Fq)
pub fn sp1_vesta_msm_ark<G: ark_ec::AffineRepr>(points: &[G], scalars: &[G::ScalarField]) -> bool
where
    G::BaseField: ark_ff::PrimeField,
    G::ScalarField: ark_ff::PrimeField,
{
    let m: &'static Limbs = &FQ_MOD;

    let limb_points: Vec<(Limbs, Limbs)> = points
        .iter()
        .map(|p| ark_to_limbs(p).unwrap_or(([0; 4], [0; 4])))
        .collect();

    let scalar_limbs: Vec<Limbs> = scalars
        .iter()
        .map(|s| unsafe { *(s.into_bigint().as_ref().as_ptr() as *const Limbs) })
        .collect();

    let result = pippenger(&limb_points, &scalar_limbs, m);
    result.is_zero()
}
