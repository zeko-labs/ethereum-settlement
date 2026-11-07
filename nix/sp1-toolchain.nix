# Pinned prebuilt SP1 `succinct` Rust toolchain for x86_64-linux.
#
# Provenance: sp1-cli 6.1.0 (`src/lib.rs`: LATEST_SUPPORTED_TOOLCHAIN_VERSION_TAG =
# "succinct-1.93.0-64bit", asset "rust-toolchain-{target}.tar.gz") installs this
# exact GitHub release asset and links it as the rustup toolchain `succinct`
# that sp1-build 6.1.0 and this repository's Dockerfile select with
# RUSTUP_TOOLCHAIN=succinct. The archive ships `bin/rustc` and the bundled
# LLVM tools/rust-lld under lib/rustlib; it does not ship `cargo`, so callers
# pair it with the repository's pinned cargo via `RUSTC=…/bin/rustc`.
# The hash was measured with `nix store prefetch-file` on 2026-10-06.
{ stdenv, fetchurl, autoPatchelfHook, zlib }:
stdenv.mkDerivation {
  pname = "sp1-succinct-rust-toolchain";
  version = "succinct-1.93.0-64bit";
  src = fetchurl {
    url = "https://github.com/succinctlabs/rust/releases/download/succinct-1.93.0-64bit/rust-toolchain-x86_64-unknown-linux-gnu.tar.gz";
    hash = "sha256-meaN2GTdfulogzM0a0KEUscF2CoRCY5RFGOJQX4MgsY=";
  };
  sourceRoot = ".";
  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [ stdenv.cc.cc.lib zlib ];
  dontConfigure = true;
  dontBuild = true;
  installPhase = ''
    runHook preInstall
    mkdir -p $out
    cp -r bin lib $out/
    [ -d share ] && cp -r share $out/ || true
    chmod -R u+w $out
    chmod +x $out/bin/* $out/lib/rustlib/x86_64-unknown-linux-gnu/bin/* \
      $out/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld/* || true
    runHook postInstall
  '';
  # The prebuilt rustc loads librustc_driver and LLVM from its own lib/.
  appendRunpaths = [ "$out/lib" ];
  meta.platforms = [ "x86_64-linux" ];
}
