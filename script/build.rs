use sp1_build::build_program_with_args;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../Cargo.lock");
    println!("cargo:rerun-if-changed=../lib");
    println!("cargo:rerun-if-changed=../program/settlement");
    println!("cargo:rerun-if-changed=../program/bridge");
    println!("cargo:rerun-if-changed=../program/withdraw");

    build_program_with_args("../program/settlement", Default::default());
    build_program_with_args("../program/bridge", Default::default());
    build_program_with_args("../program/withdraw", Default::default());
}
