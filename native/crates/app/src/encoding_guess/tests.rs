use super::*;

const FIXTURES: &str = "../../../src/vs/workbench/services/textfile/test/node/encoding/fixtures";

/// `readExactlyByFile(file, 512 * 8)` of encoding.test.ts.
fn fixture(name: &str) -> Vec<u8> {
    set_app_root(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let bytes = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES).join(name)).unwrap();
    bytes[..bytes.len().min(512 * 8)].to_vec()
}

fn guessed(bytes: &[u8], candidates: &[&str]) -> Option<String> {
    set_app_root(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    detect_from_buffer(bytes, true, Some(json!(candidates)))
}

#[test]
fn auto_guess_encoding_of_the_fixtures() {
    assert_eq!(detect_from_buffer(&fixture("some_file.css"), true, None).as_deref(), Some("utf8"));
    assert_eq!(detect_from_buffer(&fixture("some_ansi.css"), true, None), None);
    assert_eq!(detect_from_buffer(&fixture("some.shiftjis.txt"), true, None).as_deref(), Some("shiftjis"));
    assert_eq!(detect_from_buffer(&fixture("some.cp1252.txt"), true, None).as_deref(), Some("windows1252"));
    assert_eq!(detect_from_buffer(&fixture("some.shiftjis.1.txt"), true, Some(json!(["utf8", "shiftjis", "eucjp"]))).as_deref(), Some("shiftjis"));
}

#[test]
fn auto_guess_encoding_candidate_fallback() {
    let cp950 = [0xa4, 0x40, 0xaf, 0xeb, 0xa4, 0xa4, 0xa4, 0xe5, 0x0a];
    let mostly_ascii = [&cp950[..], "echo hello world\n".repeat(300).as_bytes()].concat();
    let cases: [(&str, &[u8], &[&str], Option<&str>); 16] = [
        ("short CP950", &cp950, &["cp950"], Some("cp950")),
        ("UTF-8 and CP950", &cp950, &["utf8", "cp950"], Some("cp950")),
        ("normalized candidates", &cp950, &["utf-8", "CP950"], Some("cp950")),
        ("duplicate candidates", &cp950, &["cp950", "cp950"], Some("cp950")),
        ("only one lossless candidate", &cp950, &["shiftjis", "cp950", "eucjp"], Some("cp950")),
        ("mostly ASCII", &mostly_ascii, &["cp950"], Some("cp950")),
        ("valid UTF-8", "一般中文\n".as_bytes(), &["cp950"], None),
        ("ASCII", b"echo hello world\n", &["cp950"], None),
        ("empty", b"", &["cp950"], None),
        ("partial UTF-8 character", &[0xe4, 0xb8], &["cp950"], None),
        ("invalid CP950", &[0xff, 0xff], &["cp950"], None),
        ("incomplete CP950", &cp950[..7], &["cp950"], None),
        ("ambiguous candidates", &[0xa4, 0xa4], &["cp950", "eucjp"], None),
        ("no candidates", &cp950, &[], Some("windows1252")),
        ("unsupported candidates", &cp950, &["unsupported"], Some("windows1252")),
        ("BOM takes precedence", &[0xef, 0xbb, 0xbf, 0x41], &["cp950"], Some("utf8bom")),
    ];
    for (name, bytes, candidates, expected) in cases {
        assert_eq!(guessed(bytes, candidates).as_deref(), expected, "{name}");
    }
    assert_eq!(detect_from_buffer(&cp950, false, Some(json!(["cp950"]))), None, "disabled guessing");
}
