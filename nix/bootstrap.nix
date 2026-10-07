# Ethereum-side preparation packaging: the `zeko-bootstrap` image (Foundry with
# the bundled contracts, the `vkey` tool, and an in-image SP1 guest build from
# the pinned `succinct` toolchain, the guest sources and the locked vendored
# crates) plus the pinned SP1 toolchain as its own output. The Zeko half of
# bootstrapping (circuit config, signers, DA, prover, first-proof export) runs
# from the zekolabs/zeko and zekolabs/zeko-da images (zeko/nix/bootstrap-roles);
# nix/bootstrap/compose.yaml shows both halves together.
#
# The image shares the base environment of all settlement images
# (nix/base-env.nix, passed in as `baseEnv` with its `baseImageEnv` variables
# by nix/docker.nix). Nothing here is built by agents; realization is user-owned.
{ runCommand
, dockerTools
, stdenv
, bash
, clang
, foundry
, rustToolchain
, rustPlatform
, callPackage
, source
, sourceRevision
, gatewayPackage
, artifactsDir
, baseEnv
, baseImageEnv
}:
let
  sp1Toolchain = callPackage ./sp1-toolchain.nix { };

  # Locked crate set of the workspace (same lock and git-dependency hashes as
  # the gateway build in nix/docker.nix). importCargoLock symlinks each crate
  # into one directory and writes a .cargo/config.toml whose
  # `directory = "cargo-vendor-dir"` is relative to the caller's build tree;
  # the bundle below dereferences the symlinks and rewrites that entry to the
  # absolute in-image path so `cargo --offline --frozen` resolves every
  # registry and git source from inside the container.
  cargoVendor = rustPlatform.importCargoLock {
    lockFile = "${source}/Cargo.lock";
    outputHashes = {
      "crypto-bigint-0.5.5" = "sha256-jtN5kCTIjNxzIAsJ24wxe2qFE60IZNfQ9OOnprko7p8=";
    };
  };

  # /opt/zeko/guest-src: the workspace manifests and lock, the guest programs
  # with their path dependencies (lib/, crates/, vendor/ including the SRS
  # files read by program/settlement/build.rs) and the other member manifests
  # cargo needs to load the workspace. /opt/zeko/vendor: the dereferenced crate
  # set. Both are read-only in the image; cargo writes only to
  # CARGO_TARGET_DIR and CARGO_HOME under the mounted state directory.
  guestSource = runCommand "zeko-bootstrap-guest-source" { } ''
    mkdir -p $out/opt/zeko/guest-src/.cargo $out/opt/zeko
    cp -rL ${cargoVendor} $out/opt/zeko/vendor
    chmod -R u+w $out/opt/zeko/vendor
    rm -rf $out/opt/zeko/vendor/.cargo
    for entry in Cargo.toml Cargo.lock rust-toolchain api script lib crates program vendor; do
      cp -r ${source}/$entry $out/opt/zeko/guest-src/$entry
    done
    chmod -R u+w $out/opt/zeko/guest-src
    sed 's|directory = "cargo-vendor-dir"|directory = "/opt/zeko/vendor"|' \
      ${cargoVendor}/.cargo/config.toml > $out/opt/zeko/guest-src/.cargo/config.toml
    grep -q 'directory = "/opt/zeko/vendor"' $out/opt/zeko/guest-src/.cargo/config.toml
  '';

  # /opt/zeko/contracts: Foundry workspace sources for the predict-address,
  # deploy and forge roles (src, script, lib, foundry.toml, remappings);
  # broadcast/, out/ and cache/ are intentionally excluded. The entrypoint
  # copies this into the writable state directory before running forge.
  contractsBundle = runCommand "zeko-bootstrap-contracts" { } ''
    mkdir -p $out/opt/zeko/contracts
    cp -r ${source}/contracts/src $out/opt/zeko/contracts/src
    cp -r ${source}/contracts/script $out/opt/zeko/contracts/script
    cp -r ${source}/contracts/lib $out/opt/zeko/contracts/lib
    cp ${source}/contracts/foundry.toml $out/opt/zeko/contracts/foundry.toml
    if [ -f ${source}/contracts/remappings.txt ]; then
      cp ${source}/contracts/remappings.txt $out/opt/zeko/contracts/remappings.txt
    fi
  '';

  # hostCc is the nixpkgs wrapped host C compiler with binutils (cc, ar, ld):
  # build-guests passes `--target riscv64im-succinct-zkvm-elf`, but cargo still
  # compiles and links x86_64 build scripts (program/settlement/build.rs with
  # its [build-dependencies]) and proc macros with the succinct rustc, whose
  # host target drives the link through `cc`. The image has no cc in /bin, so
  # the entrypoint names this compiler explicitly; clang stays the C compiler
  # of the riscv guest target only.
  entrypoint = runCommand "zeko-bootstrap-entrypoint" { } ''
    mkdir -p $out/bin
    substitute ${./bootstrap/zeko-bootstrap} $out/bin/zeko-bootstrap \
      --subst-var-by bash ${bash} \
      --subst-var-by foundry ${foundry} \
      --subst-var-by gatewayPackage ${gatewayPackage} \
      --subst-var-by cargo ${rustToolchain} \
      --subst-var-by sp1Toolchain ${sp1Toolchain} \
      --subst-var-by sp1ToolchainVersion ${sp1Toolchain.version} \
      --subst-var-by clang ${clang} \
      --subst-var-by hostCc ${stdenv.cc} \
      --subst-var-by sourceRevision ${sourceRevision}
    chmod 0755 $out/bin/zeko-bootstrap
    ${bash}/bin/bash -n $out/bin/zeko-bootstrap
  '';

  # streamLayeredImage: the output is a script that streams the image tarball
  # (`"$out" | docker load`, nix/README.md).
  bootstrapImage = dockerTools.streamLayeredImage {
    name = "docker.io/zekolabs/zeko-bootstrap";
    tag = "latest";
    # baseEnv supplies bash, coreutils (cp, install, sha256sum, cut), jq and
    # the CA bundle used by the entrypoint. The toolchains (succinct rustc,
    # cargo, clang, the host cc/binutils wrapper) are referenced by store path
    # from the entrypoint, which puts them in the image closure without
    # linking them into /bin, so the two rustc binaries never collide.
    contents = [
      baseEnv
      entrypoint
      foundry
      gatewayPackage
      contractsBundle
      guestSource
    ];
    # No fixed user: the roles write into bind mounts owned by the host user,
    # so the container runs as that uid:gid (`docker run --user`, Compose
    # `user:`). The writable locations are root-owned 0755 in the image, so an
    # unmounted run fails closed in the entrypoint instead of writing into the
    # container layer. No image volumes are declared.
    fakeRootCommands = ''
      mkdir -p ./var/lib/zeko-bootstrap .${artifactsDir} ./tmp
      chmod 0755 ./var/lib/zeko-bootstrap .${artifactsDir}
      chmod 1777 ./tmp
    '';
    config = {
      Entrypoint = [ "${entrypoint}/bin/zeko-bootstrap" ];
      Cmd = [ "help" ];
      Env = baseImageEnv ++ [
        "PATH=/bin"
        "HOME=/var/lib/zeko-bootstrap"
        "ZEKO_BOOTSTRAP_STATE_DIR=/var/lib/zeko-bootstrap"
        "ZEKO_ARTIFACTS_DIR=${artifactsDir}"
      ];
      WorkingDir = "/var/lib/zeko-bootstrap";
    };
  };
in {
  zeko-bootstrap-image = bootstrapImage;
  zeko-sp1-toolchain = sp1Toolchain;
}
