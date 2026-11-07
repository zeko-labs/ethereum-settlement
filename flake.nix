{
  description = "Zeko gateway development tools (builds and fake proving tests)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
  inputs.rust-overlay.url = "github:oxalica/rust-overlay";
  inputs.rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

  outputs = { self, nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      devShells = nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain;
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rust clang libclang cmake pkg-config openssl protobuf go
              git curl jq python3 postgresql_16
            ];
            CARGO_BUILD_JOBS = "1";
            CARGO_PROFILE_DEV_DEBUG = "0";
            CARGO_PROFILE_TEST_DEBUG = "0";
            RAYON_NUM_THREADS = "1";
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            shellHook = ''
              printf '%s\n' 'Gateway development shell: one build job; fake proving tests only on the 16 GB Mac.'
            '';
          };
        });
      imagePkgs = import nixpkgs {
        system = "x86_64-linux";
        overlays = [ rust-overlay.overlays.default ];
      };
      imageRust = imagePkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain;
      imageRustPlatform = imagePkgs.makeRustPlatform {
        cargo = imageRust;
        rustc = imageRust;
      };
    in {
      inherit devShells;
      packages.x86_64-linux = imagePkgs.callPackage ./nix/docker.nix {
        rustPlatform = imageRustPlatform;
        rustToolchain = imageRust;
        foundry = imagePkgs.foundry;
        bash = imagePkgs.bash;
        source = ./.;
        # Recorded in the guest provenance written by `zeko-bootstrap
        # build-guests`; unset for dirty or path: references.
        sourceRevision = self.rev or "unknown";
      };
    };
}
