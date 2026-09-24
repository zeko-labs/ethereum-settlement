{
  description = "Zeko gateway development tools (builds and fake proving tests)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
  inputs.rust-overlay.url = "github:oxalica/rust-overlay";
  inputs.rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

  outputs = { nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
    in {
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
    };
}
