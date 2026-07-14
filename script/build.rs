use sp1_build::build_program_with_args;
use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../Cargo.lock");
    println!("cargo:rerun-if-changed=../lib");
    println!("cargo:rerun-if-changed=../program/settlement");
    println!("cargo:rerun-if-changed=../program/bridge");
    println!("cargo:rerun-if-changed=../program/withdraw");
    println!("cargo:rerun-if-env-changed=SETTLEMENT_VK_JSON");
    if let Some(path) = env::var_os("SETTLEMENT_VK_JSON") {
        println!("cargo:rerun-if-changed={}", path.to_string_lossy());
    }

    build_program_with_args("../program/settlement", Default::default());
    build_program_with_args("../program/bridge", Default::default());
    build_program_with_args("../program/withdraw", Default::default());
}
