//! EKOS RFC 0268: the translation is pure — it reads no file, opens no connection, calls no model.

#[test]
fn the_translation_reads_no_file() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for forbidden in [
            "std::fs",
            "File::open",
            "std::net",
            "reqwest",
            "std::process",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} uses {forbidden}",
                path.display()
            );
        }
    }
}
