//! The program of this application is the one the documents show: the quickstart guide and
//! both READMEs must contain `src/main.rs` exactly, so that they never drift apart.

const PROGRAM: &str = include_str!("../src/main.rs");

#[test]
fn the_documents_contain_the_program() {
    let documents = [
        (
            "crates/tilefab/QUICKSTART.md",
            include_str!("../../../crates/tilefab/QUICKSTART.md"),
        ),
        (
            "crates/tilefab/README.md",
            include_str!("../../../crates/tilefab/README.md"),
        ),
        ("README.md", include_str!("../../../README.md")),
    ];
    for (path, text) in documents {
        assert!(
            text.contains(PROGRAM),
            "{path} must contain apps/quickstart/src/main.rs verbatim, in a code block"
        );
    }
}
