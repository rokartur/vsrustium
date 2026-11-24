//! Text encodings: BOM and binary detection, decoding and encoding of services/textfile/common/encoding.ts, and the
//! `EncodingOracle` of textFileService.ts. encoding_rs stands in for iconv-lite; the DOS code pages it lacks are
//! iconv-lite's own tables.

use std::path::Path;

use config::configuration::Overrides;
use config::service::Settings;
use encoding_rs::{EncoderResult, Encoding};

pub const UTF8: &str = "utf8";
pub const UTF8_WITH_BOM: &str = "utf8bom";
pub const UTF16BE: &str = "utf16be";
pub const UTF16LE: &str = "utf16le";

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const UTF16BE_BOM: [u8; 2] = [0xFE, 0xFF];
const UTF16LE_BOM: [u8; 2] = [0xFF, 0xFE];

/// Bytes looked at to tell binary from UTF-16 (`ZERO_BYTE_DETECTION_BUFFER_MAX_LEN`).
const ZERO_BYTE_DETECTION_BUFFER_MAX_LEN: usize = 512;

/// The upper halves (bytes 0x80 to 0xFF) of the single-byte encodings of `SUPPORTED_ENCODINGS` WHATWG does not define,
/// from iconv-lite 0.7.2 `sbcs-data-generated.js`.
const DOS_TABLES: [(&str, &str); 8] = [
    ("cp437", "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{00A0}"),
    ("cp850", "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜø£Ø×ƒáíóúñÑªº¿®¬½¼¡«»░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐└┴┬├─┼ãÃ╚╔╩╦╠═╬¤ðÐÊËÈıÍÎÏ┘┌█▄¦Ì▀ÓßÔÒõÕµþÞÚÛÙýÝ¯´\u{00AD}±‗¾¶§÷¸°¨·¹³²■\u{00A0}"),
    ("cp852", "ÇüéâäůćçłëŐőîŹÄĆÉĹĺôöĽľŚśÖÜŤťŁ×čáíóúĄąŽžĘę¬źČş«»░▒▓│┤ÁÂĚŞ╣║╗╝Żż┐└┴┬├─┼Ăă╚╔╩╦╠═╬¤đĐĎËďŇÍÎě┘┌█▄ŢŮ▀ÓßÔŃńňŠšŔÚŕŰýÝţ´\u{00AD}˝˛ˇ˘§÷¸°¨˙űŘř■\u{00A0}"),
    ("cp857", "ÇüéâäàåçêëèïîıÄÅÉæÆôöòûùİÖÜø£ØŞşáíóúñÑĞğ¿®¬½¼¡«»░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐└┴┬├─┼ãÃ╚╔╩╦╠═╬¤ºªÊËÈ\u{FFFD}ÍÎÏ┘┌█▄¦Ì▀ÓßÔÒõÕµ\u{FFFD}×ÚÛÙìÿ¯´\u{00AD}±\u{FFFD}¾¶§÷¸°¨·¹³²■\u{00A0}"),
    ("cp865", "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜø£Ø₧ƒáíóúñÑªº¿⌐¬½¼¡«¤░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{00A0}"),
    ("cp1125", "АБВГДЕЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдежзийклмноп░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀рстуфхцчшщъыьэюяЁёҐґЄєІіЇї·√№¤■\u{00A0}"),
    ("koi8ru", "─│┌┐└┘├┤┬┴┼▀▄█▌▐░▒▓⌠■∙√≈≤≥\u{00A0}⌡°²·÷═║╒ёє╔ії╗╘╙╚╛ґў╞╟╠╡ЁЄ╣ІЇ╦╧╨╩╪ҐЎ©юабцдефгхийклмнопярстужвьызшэщчъЮАБЦДЕФГХИЙКЛМНОПЯРСТУЖВЬЫЗШЭЩЧЪ"),
    ("koi8t", "қғ‚Ғ„…†‡\u{FFFD}‰ҳ‹ҲҷҶ\u{FFFD}Қ‘’“”•–—\u{FFFD}™\u{FFFD}›\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}ӯӮё¤ӣ¦§\u{FFFD}\u{FFFD}\u{FFFD}«¬\u{00AD}®\u{FFFD}°±²Ё\u{FFFD}Ӣ¶·\u{FFFD}№\u{FFFD}»\u{FFFD}\u{FFFD}\u{FFFD}©юабцдефгхийклмнопярстужвьызшэщчъЮАБЦДЕФГХИЙКЛМНОПЯРСТУЖВЬЫЗШЭЩЧЪ"),
];

#[derive(Debug, PartialEq)]
pub struct Detected {
    pub encoding: Option<&'static str>,
    pub seems_binary: bool,
}

/// `detectEncodingByBOMFromBuffer`.
pub fn detect_by_bom(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&UTF16BE_BOM) {
        return Some(UTF16BE);
    }
    if bytes.starts_with(&UTF16LE_BOM) {
        return Some(UTF16LE);
    }
    bytes.starts_with(&UTF8_BOM).then_some(UTF8_WITH_BOM)
}

/// `detectEncodingFromBuffer` without auto guessing: the BOM, else UTF-16 when zero bytes sit only at odd (LE) or
/// even (BE) positions of the first 512 bytes; any other zero byte makes the bytes binary.
// shortcut: `files.autoGuessEncoding` (jschardet) is not ported for the files the workbench reads; the extension host guesses for `workspace.decode`.
pub fn detect(bytes: &[u8]) -> Detected {
    let encoding = detect_by_bom(bytes);
    if encoding == Some(UTF16BE) || encoding == Some(UTF16LE) {
        return Detected { encoding, seems_binary: false };
    }
    let (mut could_be_le, mut could_be_be, mut contains_zero) = (true, true, false);
    for (index, &byte) in bytes.iter().take(ZERO_BYTE_DETECTION_BUFFER_MAX_LEN).enumerate() {
        let odd = index % 2 == 1;
        let zero = byte == 0;
        contains_zero |= zero;
        // UTF-16 LE is 0xAA 0x00, BE is 0x00 0xAA.
        could_be_le &= odd == zero;
        could_be_be &= odd != zero;
        if zero && !could_be_le && !could_be_be {
            break;
        }
    }
    match (contains_zero, could_be_le, could_be_be) {
        (false, _, _) => Detected { encoding, seems_binary: false },
        (true, true, _) => Detected { encoding: Some(UTF16LE), seems_binary: false },
        (true, false, true) => Detected { encoding: Some(UTF16BE), seems_binary: false },
        (true, false, false) => Detected { encoding, seems_binary: true },
    }
}

/// `encodingExists`: whether iconv-lite knows the encoding.
pub fn exists(encoding: &str) -> bool {
    codec(encoding).is_some()
}

/// The text of `bytes` as iconv-lite (and TextDecoder for UTF-8) decode it: the encoding's own BOM is dropped and
/// bytes it cannot map become U+FFFD. An unknown encoding decodes as UTF-8.
pub fn decode(bytes: &[u8], encoding: &str) -> String {
    match codec(encoding).unwrap_or(Codec::Whatwg(encoding_rs::UTF_8)) {
        Codec::Whatwg(encoding) => encoding.decode_with_bom_removal(bytes).0.into_owned(),
        Codec::Table(upper) => {
            let upper: Vec<char> = upper.chars().collect();
            bytes.iter().map(|&byte| if byte < 0x80 { byte as char } else { upper[byte as usize - 0x80] }).collect()
        }
    }
}

/// `toEncodeReadable`: `text` in the encoding, the BOM first when `add_bom` and the encoding has one. A character the
/// encoding cannot map becomes `?`, as in iconv-lite. An unknown encoding encodes as UTF-8.
pub fn encode(text: &str, encoding: &str, add_bom: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    match codec(encoding).unwrap_or(Codec::Whatwg(encoding_rs::UTF_8)) {
        Codec::Whatwg(encoding) if encoding == encoding_rs::UTF_8 => {
            if add_bom {
                bytes.extend(UTF8_BOM);
            }
            bytes.extend(text.as_bytes());
        }
        // encoding_rs encodes UTF-16 as UTF-8, as the WHATWG encoders do.
        Codec::Whatwg(encoding) if encoding == encoding_rs::UTF_16LE => {
            if add_bom {
                bytes.extend(UTF16LE_BOM);
            }
            bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        }
        Codec::Whatwg(encoding) if encoding == encoding_rs::UTF_16BE => {
            if add_bom {
                bytes.extend(UTF16BE_BOM);
            }
            bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        }
        Codec::Whatwg(encoding) => {
            let mut encoder = encoding.new_encoder();
            let mut rest = text;
            loop {
                let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(rest, &mut bytes, true);
                rest = &rest[read..];
                match result {
                    EncoderResult::InputEmpty => break,
                    EncoderResult::OutputFull => bytes.reserve(encoder.max_buffer_length_from_utf8_without_replacement(rest.len()).expect("text fits in memory")),
                    EncoderResult::Unmappable(_) => bytes.push(b'?'),
                }
            }
        }
        Codec::Table(upper) => {
            for char in text.chars() {
                let position = upper.chars().position(|mapped| mapped == char && mapped != '\u{FFFD}');
                bytes.push(match position {
                    _ if char.is_ascii() => char as u8,
                    Some(position) => 0x80 + position as u8,
                    None => b'?',
                });
            }
        }
    }
    bytes
}

/// `EncodingOracle.getPreferredReadEncoding`: the encoding a resource is read with, from the requested encoding,
/// the detected one and `files.encoding`.
pub fn read_encoding(settings: &Settings, path: Option<&Path>, requested: Option<&str>, detected: Option<&str>) -> String {
    let preferred = match (requested, detected) {
        // A file read as UTF-8 keeps its BOM.
        (Some(UTF8), Some(UTF8_WITH_BOM)) => Some(UTF8_WITH_BOM),
        (Some(requested), _) => Some(requested),
        (None, Some(detected)) => Some(detected),
        // Without a detected BOM a `utf8bom` file can only be UTF-8.
        (None, None) if configured(settings, path) == UTF8_WITH_BOM => Some(UTF8),
        (None, None) => None,
    };
    validated(settings, path, preferred)
}

/// `EncodingOracle.getWriteEncoding`: the encoding a resource is written with, and whether a BOM goes first.
pub fn write_encoding(settings: &Settings, path: Option<&Path>, requested: Option<&str>) -> (String, bool) {
    let encoding = validated(settings, path, requested);
    let add_bom = has_bom(&encoding);
    (encoding, add_bom)
}

/// The encodings a BOM is enforced for when writing (`getPreferredWriteEncoding`).
pub fn has_bom(encoding: &str) -> bool {
    [UTF16BE, UTF16LE, UTF8_WITH_BOM].contains(&encoding)
}

/// `getValidatedEncodingForResource`: the override, else the preferred encoding, else `files.encoding`; UTF-8 when
/// iconv-lite does not know it.
fn validated(settings: &Settings, path: Option<&Path>, preferred: Option<&str>) -> String {
    let encoding = match (is_utf8_override(settings, path), preferred) {
        (true, _) => UTF8.to_string(),
        (false, Some(preferred)) => preferred.to_string(),
        (false, None) => configured(settings, path),
    };
    match encoding.as_str() {
        "" => UTF8.to_string(),
        other if other != UTF8 && !exists(other) => UTF8.to_string(),
        _ => encoding,
    }
}

// shortcut: `files.encoding` of a language (`"[lang]": {...}`) is not read; textResourceConfigurationService resolves the resource's language first.
fn configured(settings: &Settings, path: Option<&Path>) -> String {
    let value = settings.get_value(Some("files.encoding"), &Overrides { resource: path, ..Overrides::default() });
    value.as_ref().and_then(|value| value.as_str()).unwrap_or_default().to_string()
}

/// `getDefaultEncodingOverrides`: user settings, workspace files and each folder's `.vscode` are always UTF-8.
fn is_utf8_override(settings: &Settings, path: Option<&Path>) -> bool {
    let Some(path) = path else { return false };
    let user_home = settings.user_home();
    if path.starts_with(user_home) || path.extension().is_some_and(|extension| extension == "code-workspace") {
        return true;
    }
    if user_home.parent().is_some_and(|user_data| path.starts_with(user_data.join("Workspaces"))) {
        return true;
    }
    let mut folders = settings.workspace.iter().flat_map(|workspace| &workspace.folders);
    folders.any(|folder| path.starts_with(folder.path.join(".vscode")))
}

enum Codec {
    Whatwg(&'static Encoding),
    /// The upper half of a single-byte encoding; the lower half is ASCII.
    Table(&'static str),
}

/// The codec of a VS Code encoding id or an iconv-lite name; iconv-lite compares names lowercased with only their
/// letters and digits (`_canonicalizeEncoding`).
fn codec(encoding: &str) -> Option<Codec> {
    let name: String = encoding.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
    if let Some((_, upper)) = DOS_TABLES.iter().find(|(id, _)| *id == name) {
        return Some(Codec::Table(upper));
    }
    let label = match name.as_str() {
        UTF8_WITH_BOM => UTF8.to_string(),
        // shortcut: iconv-lite's cp950 is Big5 with Microsoft's extensions; WHATWG Big5 lacks a few of them.
        "cp950" => "big5".into(),
        _ => to_canonical_name(&name),
    };
    Encoding::for_label(label.as_bytes()).filter(|found| *found != encoding_rs::REPLACEMENT).map(Codec::Whatwg)
}

/// `toCanonicalName`: the WHATWG label of a VS Code encoding id.
fn to_canonical_name(name: &str) -> String {
    let label = match name {
        "shiftjis" => "shift-jis",
        "utf16le" => "utf-16le",
        "utf16be" => "utf-16be",
        "big5hkscs" => "big5-hkscs",
        "eucjp" => "euc-jp",
        "euckr" => "euc-kr",
        "koi8r" => "koi8-r",
        "koi8u" => "koi8-u",
        "macroman" => "x-mac-roman",
        // WHATWG knows `iso-8859-16` but not `iso885916`.
        _ if name.starts_with("iso8859") => return format!("iso-8859-{}", &name["iso8859".len()..]),
        _ => return name.strip_prefix("windows").map_or(name.to_string(), |number| format!("windows-{number}")),
    };
    label.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../src/vs/workbench/services/textfile/test/node/encoding/fixtures").join(name);
        let bytes = std::fs::read(path).unwrap();
        bytes[..bytes.len().min(512)].to_vec()
    }

    #[test]
    fn detect_bom() {
        // The some_utf8.css fixture of this checkout has no BOM, though the upstream test expects one.
        assert_eq!(detect_by_bom(&[0xEF, 0xBB, 0xBF, b'a']), Some(UTF8_WITH_BOM));
        assert_eq!(detect_by_bom(&fixture("some_utf16le.css")), Some(UTF16LE));
        assert_eq!(detect_by_bom(&fixture("some_utf16be.css")), Some(UTF16BE));
        assert_eq!(detect_by_bom(&fixture("some_ansi.css")), None);
        assert_eq!(detect_by_bom(&fixture("empty.txt")), None);
    }

    #[test]
    fn detect_encoding_from_buffer() {
        for (name, seems_binary) in [("some.json.png", false), ("some.png.txt", true), ("some.xml.png", false), ("some.qwoff.txt", true), ("some.css.qwoff", false), ("some.pdf", true)] {
            assert_eq!(detect(&fixture(name)).seems_binary, seems_binary, "{name}");
        }
        assert_eq!(detect(&fixture("utf16_le_nobom.txt")), Detected { encoding: Some(UTF16LE), seems_binary: false });
        assert_eq!(detect(&fixture("utf16_be_nobom.txt")), Detected { encoding: Some(UTF16BE), seems_binary: false });
        assert!(detect(&[0, 0, 0, b'H', b'e', 0]).seems_binary);
    }

    #[test]
    fn decode_and_encode() {
        let bytes = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../src/vs/workbench/services/textfile/test/node/encoding/fixtures/some_utf16be.css")).unwrap();
        let text = decode(&bytes, UTF16BE);
        assert!(!text.starts_with('\u{FEFF}'));
        assert_eq!(encode(&text, UTF16BE, true), bytes);

        let cp1252 = [72, 101, 108, 108, 0xF6, 32, 87, 0xF6, 114, 108, 100];
        assert_eq!(decode(&cp1252, "windows1252"), "Hellö Wörld");
        assert_eq!(encode("Hellö Wörld", "cp1252", false), cp1252);
        assert_eq!(encode("Hello \u{4E16}", "cp1252", false), b"Hello ?");
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'a'], UTF8_WITH_BOM), "a");
        assert_eq!(decode(&[0xB0, 0xE1], "cp437"), "░ß");
        assert_eq!(encode("░ß", "cp437", false), [0xB0, 0xE1]);
        assert_eq!(encode("Hello", "foobar123", false), b"Hello");
    }

    #[test]
    fn encode_empty_text_with_bom() {
        assert_eq!(encode("", UTF8, false), b"");
        for (encoding, bom) in [(UTF8, &UTF8_BOM[..]), (UTF8_WITH_BOM, &UTF8_BOM), (UTF16BE, &UTF16BE_BOM), (UTF16LE, &UTF16LE_BOM)] {
            assert_eq!(encode("", encoding, true), bom, "{encoding}");
        }
    }

    #[test]
    fn every_supported_encoding_exists() {
        let supported = [
            "utf8", "utf16le", "utf16be", "windows1252", "iso88591", "iso88593", "iso885915", "macroman", "cp437", "windows1256", "iso88596",
            "windows1257", "iso88594", "iso885914", "windows1250", "iso88592", "cp852", "windows1251", "cp866", "cp1125", "iso88595", "koi8r",
            "koi8u", "iso885913", "windows1253", "iso88597", "windows1255", "iso88598", "iso885910", "iso885916", "windows1254", "iso88599",
            "cp857", "windows1258", "gbk", "gb18030", "cp950", "big5hkscs", "shiftjis", "eucjp", "euckr", "windows874", "iso885911", "koi8ru",
            "koi8t", "gb2312", "cp865", "cp850",
        ];
        for encoding in supported {
            assert!(exists(encoding), "{encoding}");
        }
        assert!(exists("cp1252"));
        assert!(!exists("foobar123"));
    }
}
