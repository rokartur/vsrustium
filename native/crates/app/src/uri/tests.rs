//! Cases of base/test/common/uri.test.ts.

use super::*;

fn parse(value: &str) -> Uri {
    Uri::parse(value).unwrap()
}

fn from(scheme: &str, authority: &str, path: &str, query: &str, fragment: &str) -> Uri {
    Uri::new(scheme, authority, path, query, fragment).unwrap()
}

#[test]
fn http_to_string() {
    assert_eq!(from("http", "www.EXAMPLE.com", "/my/path", "", "").to_string(false), "http://www.example.com/my/path");
    assert_eq!(from("http", "", "my/path", "", "").to_string(false), "http:/my/path");
    assert_eq!(from("http", "example.com", "/", "test=true", "").to_string(false), "http://example.com/?test%3Dtrue");
    assert_eq!(from("http", "example.com", "/", "", "test=true").to_string(false), "http://example.com/#test%3Dtrue");
}

#[test]
fn http_to_string_without_encoding() {
    assert_eq!(from("http", "example.com", "/", "test=true", "").to_string(true), "http://example.com/?test=true");
    assert_eq!(from("http", "", "/api/files/test.me", "t=1234", "").to_string(true), "http:/api/files/test.me?t=1234");
    let value = parse("file://shares/pröjects/c%23/#l12");
    assert_eq!((value.authority.as_str(), value.path.as_str(), value.fragment.as_str()), ("shares", "/pröjects/c#/", "l12"));
    assert_eq!(value.to_string(false), "file://shares/pr%C3%B6jects/c%23/#l12");
    assert_eq!(value.to_string(true), "file://shares/pröjects/c%23/#l12");
    assert_eq!(parse(&value.to_string(true)), parse(&value.to_string(false)));
}

#[test]
fn minimal_encoding_preserves_delimiter_boundaries() {
    let cases = [
        ("", ""),
        ("plain/path", "plain/path"),
        ("?#", "%3F%23"),
        ("#?", "%23%3F"),
        ("before??##after", "before%3F%3F%23%23after"),
        ("%23/%3F?raw#", "%23/%3F%3Fraw%23"),
        ("\u{e9}/\u{1f431}?\u{7d42}#", "\u{e9}/\u{1f431}%3F\u{7d42}%23"),
    ];
    for (path, expected) in cases {
        assert_eq!(from("test", "", path, "", "").to_string(true), format!("test:{expected}"));
    }
}

#[test]
fn parses_the_five_parts() {
    let parts = |value: &str| {
        let uri = parse(value);
        [uri.scheme, uri.authority, uri.path, uri.query, uri.fragment]
    };
    assert_eq!(parts("http:/api/files/test.me?t=1234"), ["http", "", "/api/files/test.me", "t=1234", ""]);
    assert_eq!(parts("http://api/files/test.me?t=1234"), ["http", "api", "/files/test.me", "t=1234", ""]);
    assert_eq!(parts("file://shares/files/c%23/p.cs"), ["file", "shares", "/files/c#/p.cs", "", ""]);
    assert_eq!(
        parts("file:///c:/Source/Z%C3%BCrich%20or%20Zurich%20(%CB%88zj%CA%8A%C9%99r%C9%AAk,/Code/resources/app/plugins/c%23/plugin.json")[2],
        "/c:/Source/Zürich or Zurich (ˈzjʊərɪk,/Code/resources/app/plugins/c#/plugin.json"
    );
    assert_eq!(parts("file:///c:/test %25/path")[2], "/c:/test %/path");
    assert_eq!(parts("inmemory:"), ["inmemory", "", "", "", ""]);
    assert_eq!(parts("foo:api/files/test"), ["foo", "", "api/files/test", "", ""]);
    assert_eq!(parts("file:?q"), ["file", "", "/", "q", ""]);
    assert_eq!(parts("file:#d"), ["file", "", "/", "", "d"]);
    assert_eq!(parts("f3ile:#d"), ["f3ile", "", "", "", "d"]);
    assert_eq!(parts("foo+bar:path")[0], "foo+bar");
    assert_eq!(parts("foo.bar:path")[0], "foo.bar");
    assert!(Uri::parse("file:////shares/files/p.cs").is_err(), "disallow //path when no authority");
    assert_eq!(parts("file:foo/bar"), ["file", "", "/foo/bar", "", ""]);
    assert_eq!(parse("file:foo/bar").to_string(false), "file:///foo/bar");
}

#[test]
fn to_string_encodes_as_upstream() {
    assert_eq!(parse("stuff:?qüery").to_string(false), "stuff:?q%C3%BCery");
    assert_eq!(parse("file://sh%c3%a4res/path").to_string(false), "file://sh%C3%A4res/path");
    assert_eq!(parse("untitled:C:/Users/jrieken/Code/abc.txt").to_string(false), "untitled:c%3A/Users/jrieken/Code/abc.txt");
    for scheme in ["http", "https", "HtTp", "HTTPS"] {
        let uri = parse(&format!("{scheme}://example.test/C:/Secret"));
        assert_eq!(uri.to_string(false), format!("{scheme}://example.test/C%3A/Secret"));
        assert_eq!(parse(&uri.to_string(false)).to_string(true), format!("{scheme}://example.test/C:/Secret"));
    }
    for scheme in ["file", "untitled", "vscode-remote", "custom"] {
        assert_eq!(parse(&format!("{scheme}://server/C:/Secret")).to_string(true), format!("{scheme}://server/c:/Secret"));
    }
    assert_eq!(parse("http://localhost:8080/far").to_string(false), "http://localhost:8080/far");
    assert_eq!(from("http", "löcalhost:8080", "/far", "", "").to_string(false), "http://l%C3%B6calhost:8080/far");
    assert_eq!(parse("http://foo:bAr@localhost:8080/far").to_string(false), "http://foo:bAr@localhost:8080/far");
    assert_eq!(from("http", "föö:bör@löcalhost:8080", "/far", "", "").to_string(false), "http://f%C3%B6%C3%B6:b%C3%B6r@l%C3%B6calhost:8080/far");
    assert_eq!(parse("http://foo:pa:ss@localhost/far").to_string(false), "http://foo:pa%3Ass@localhost/far");
    assert_eq!(parse("http://foo:pa%3Ass@localhost/far").authority, "foo:pa:ss@localhost");
    assert_eq!(
        parse("http://us[er@[FEDC:BA98:7654:3210:FEDC:BA98:7654:3210]:80/index.html").to_string(false),
        "http://us%5Ber@[fedc:ba98:7654:3210:fedc:ba98:7654:3210]:80/index.html"
    );
}

#[test]
fn queries_round_trip() {
    let uri = parse("https://go.microsoft.com/fwlink/?LinkId=518008&foö&ké¥=üü");
    assert_eq!(uri.to_string(true), "https://go.microsoft.com/fwlink/?LinkId=518008&foö&ké¥=üü");
    assert_eq!(uri.to_string(false), "https://go.microsoft.com/fwlink/?LinkId%3D518008%26fo%C3%B6%26k%C3%A9%C2%A5%3D%C3%BC%C3%BC");
    assert_eq!(parse(&uri.to_string(false)).query, uri.query);
    assert_eq!(parse("https://twitter.com/search?src=typd&q=%23tag").to_string(true), "https://twitter.com/search?src=typd&q=%23tag");
    assert_eq!(parse("http://localhost:3000/#/foo?bar=baz").to_string(true), "http://localhost:3000/#/foo?bar=baz");
}

#[test]
fn malformed_escapes_stay_as_written() {
    assert_eq!(parse("file://some/%.txt").to_string(false), "file://some/%25.txt");
    assert_eq!(parse("file://some/%A0.txt").to_string(false), "file://some/%25A0.txt");
    // decodeURIComponentGraceful keeps the first escape of a failing run and decodes the rest.
    assert_eq!(percent_decode("%41%zz%C3%BC"), "%41%zzü");
    assert_eq!(percent_decode("%C3%BC%41"), "üA");
}
