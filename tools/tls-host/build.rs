//! Makes the OpenSSL-generated test chains (they carry throwaway private
//! keys, so they are not kept in the repository).
use std::path::Path;
use std::process::Command;

fn main() {
    let data = Path::new("tests/chain_data.rs");
    if data.exists() {
        return;
    }
    let python = if cfg!(windows) { "python" } else { "python3" };
    let status = Command::new(python)
        .arg("../make-tls-chains.py")
        .status()
        .expect("python is needed to generate the TLS test chains");
    assert!(status.success(), "tools/make-tls-chains.py failed (is openssl installed?)");
    println!("cargo:rerun-if-changed=../make-tls-chains.py");
    println!("cargo:rerun-if-changed=../tlspki.py");
}
