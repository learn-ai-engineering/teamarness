//! SPIKE (filesystem-docs prototype) — throwaway. Prints the Canonical Markdown of a file.
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let md = std::fs::read_to_string(path).unwrap();
    print!("{}", teamarness_documents_package::markdown::canonical(&md).unwrap());
}
