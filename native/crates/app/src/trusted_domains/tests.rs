//! Cases of platform/url/test/common/trustedDomains.test.ts and urlGlob.test.ts. Strings in a test with a
//! `URI.parse` and a string variant are parsed once: both reach the same `normalizeURL`.

use super::*;

fn trusted(url: &str, domains: &[&str]) -> bool {
    let domains: Vec<String> = domains.iter().map(|domain| domain.to_string()).collect();
    is_url_domain_trusted(&Uri::parse(url).unwrap(), &domains)
}

fn matches(url: &str, glob: &str) -> bool {
    test_url_matches_glob(&Uri::parse(url).unwrap(), glob)
}

#[test]
fn localhost_is_always_trusted() {
    for url in ["http://localhost:3000", "http://127.0.0.1:3000", "http://subdomain.localhost", "https://[::1]", "http://[::1]:3000"] {
        assert!(trusted(url, &[]), "{url}");
    }
}

#[test]
fn backslashes_and_user_information_do_not_borrow_trust() {
    assert!(!trusted(r"https://example.com\.localhost", &[]));
    assert!(!trusted(r"https://example.com\.github.com", &["https://*.github.com"]));
    assert!(!trusted("https://localhost%5C@evil.example/", &[]));
    assert!(!trusted("https://example.com%5C@evil.example/", &["https://example.com"]));
    assert!(!trusted("https://api.github.com%5C@evil.example/", &["https://*.github.com"]));
    for separator in ["%2F", "%2f", "%5c", "%25%32%46", "%25%32%66", "%25%35%43", "%25%35%63"] {
        assert!(!trusted(&format!("https://localhost{separator}@evil.example/"), &[]), "{separator}");
        assert!(!trusted(&format!("https://example.com{separator}@evil.example/"), &["https://example.com"]), "{separator}");
        assert!(!trusted(&format!("https://api.example.com{separator}@evil.example/"), &["https://*.example.com"]), "{separator}");
    }
    for url in [r"https:\localhost%2F@evil.example/resource", r"https:/\localhost%25%32%66@evil.example/resource", r"https:\example.com%5C@evil.example/resource", r"https:/\api.example.com%25%35%43@evil.example/resource"] {
        assert_eq!([trusted(url, &[]), trusted(url, &["https://example.com"]), trusted(url, &["https://*.example.com"]), trusted(url, &["*"])], [false, false, false, true], "{url}");
    }
    assert!(trusted("https://example.com%2F@evil.example/", &["*"]));
}

#[test]
fn domains_match_by_scheme_host_and_path() {
    assert!(trusted("https://github.com/microsoft", &["*"]));
    assert!(trusted("https://example.com/path", &["https://example.com"]));
    assert!(!trusted("http://example.com", &["https://example.com"]));
    assert!(!trusted("https://example.com", &["http://example.com"]));
    assert!(trusted("https://sub.api.github.com", &["https://*.github.com"]));
    assert!(trusted("https://github.com", &["https://*.github.com"]));
    assert!(trusted("https://example.com/api/v1", &["https://example.com/api/*"]));
    assert!(!trusted("https://example.com/api", &["https://example.com/api/*"]));
    assert!(!trusted("https://example.com", &["https://other.com"]));
    assert!(!trusted("https://example.com", &[]));
    assert!(trusted("https://microsoft.com", &["https://github.com", "https://microsoft.com"]));
    assert!(!trusted("https://google.com", &["https://github.com", "https://microsoft.com"]));
    assert!(trusted("https://github.com/Microsoft/VSCode", &["https://github.com/microsoft/vscode"]));
    assert!(trusted("https://github.com/microsoft/vscode", &["https://github.com/Microsoft/VSCode"]));
}

#[test]
fn idn_domains_match_equivalent_spellings() {
    for host in ["bücher.example.test", "xn--bcher-kva.example.test"] {
        for pattern in ["bücher.example.test", "xn--bcher-kva.example.test"] {
            let results = [
                trusted(&format!("https://{host}/page"), &[&format!("https://{pattern}")]),
                trusted(&format!("https://x.{host}/page"), &[&format!("https://*.{pattern}")]),
                trusted("https://other.example.test", &[&format!("https://*.{pattern}")]),
                trusted(&format!("https://{host}.evil.test"), &[&format!("https://*.{pattern}")]),
                trusted(&format!("https://{host}%2F@evil.test"), &[&format!("https://*.{pattern}")]),
                trusted(&format!("https://{host}/allowed/../outside"), &[&format!("https://{pattern}/allowed")]),
            ];
            assert_eq!(results, [true, true, false, false, false, false], "{host} {pattern}");
        }
    }
}

#[test]
fn path_scoped_trust_matches_the_resolved_destination() {
    let paths = ["/allowed/../outside", "/allowed/%2e%2e/outside", "/allowed/.\t./outside", "/allowed/.\n./outside", "/allowed/.\r./outside", "/outside", "/allowed/child/../page"];
    let expected = [false, false, false, false, false, false, true];
    for (path, expected) in paths.iter().zip(expected) {
        for rule in ["https://example.com/allowed", "https://example.com/allowed/*"] {
            assert_eq!(trusted(&format!("https://example.com{path}"), &[rule]), expected, "{path:?} {rule}");
        }
    }
    let url = "https://example.com:user@other.example/allowed/../outside";
    assert_eq!([trusted(url, &["https://example.com:*"]), trusted(url, &["*"]), trusted("https://example.com/allowed/../outside", &[])], [false, true, false]);
}

#[test]
fn globs_ignore_trailing_slashes_queries_and_fragments() {
    assert!(matches("https://example.com//", "https://example.com"));
    assert!(matches("https://example.com", "https://example.com/"));
    assert!(matches("https://example.com/path/", "https://example.com/path"));
    assert!(matches("https://example.com?query=value#fragment", "https://example.com"));
    assert!(!matches("ftp://example.com", "https://example.com"));
    assert!(matches("http://example.com", "example.com"));
    assert!(!matches("ftp://example.com", "example.com"));
}

#[test]
fn glob_wildcards_in_paths_hosts_and_ports() {
    assert!(matches("https://example.com/path/to/resource", "https://example.com/path/*/resource"));
    let results = [
        matches("https://example.test/a", "https://example.test/a*"),
        matches("https://example.test/ab", "https://example.test/a*"),
        matches("https://example.test/ab", "https://example.test/*b"),
        matches("https://example.test/ab", "https://example.test/*b*"),
        matches("https://example.test/a/b", "https://example.test/a"),
        matches("https://example.test/", "https://example.test:*"),
    ];
    assert_eq!(results, [false, true, true, false, true, true]);
    let segment = "x".repeat(8192);
    assert!(matches(&format!("https://example.test/docs/{segment}/match"), "https://example.test/docs/*/match"));
    assert!(!matches(&format!("https://example.test/docs/{segment}/miss"), "https://example.test/docs/*/match"));
    for (url, glob, expected) in [
        ("https://notexample.com", "https://*.example.com", false),
        ("https://evil-microsoft.com", "https://*.microsoft.com", false),
        ("https://notexample.com/path", "https://*.example.com/path", false),
        ("https://a.b.c.microsoft.com", "https://*.microsoft.com", true),
        ("http://evil-microsoft.com", "*.microsoft.com", false),
        ("http://sub.microsoft.com", "*.microsoft.com", true),
        ("https://example.com:8080", "https://example.com:9090", false),
        ("https://example.com", "https://example.com:8080", false),
        ("https://example.com", "https://example.com:*", true),
        ("https://example.com:8080/path", "https://example.com:*/path", true),
        ("https://example.com/path", "https://example.com/", true),
        ("https://sub.example.com", "https://example.com", false),
        ("https://github.com/microsoft/vscode", "https://*.github.com/repos/*/*", false),
        ("https://api.github.com:443/repos/microsoft/vscode", "https://*.github.com:*/repos/*/*", true),
        ("https://example.com", "https://example.com*", false),
        ("https://example.com.extra", "https://example.com*", true),
        ("https://example.com", "*", true),
    ] {
        assert_eq!(matches(url, glob), expected, "{url} {glob}");
    }
}

#[test]
fn dot_segments_cannot_escape_a_path_scoped_glob() {
    let paths = ["/allowed/../outside", "/allowed/child/../../outside", "/allowed/./../outside", "/allowed/%2e%2e/outside", "/allowed/.%2E/outside", "/allowed/%2E./outside", "/allowed/.."];
    for path in paths {
        for glob in ["https://example.com/allowed", "example.com/allowed", "https://*.example.com:*/allowed/*"] {
            assert!(!matches(&format!("https://example.com{path}"), glob), "{path} {glob}");
        }
    }
}

#[test]
fn backslash_authorities_match_only_the_browser_destination() {
    for (url, pattern, expected) in [
        (r"https://evil.example\.github.com/collect?leak=<data>", "https://*.github.com", false),
        (r"https://evil.example\\.github.com/collect?leak=<data>", "https://*.github.com", false),
        (r"https://169.254.169.254\.github.com/latest/meta-data/", "*.github.com", false),
        (r"http://127.0.0.2:38651\.github.com/exfil?data=fixture", "http://*.github.com:*", false),
        (r"https://github.com\.evil.example/resource", "https://*.github.com", true),
        (r"custom://example.com\segment/resource", r"custom://example.com\segment", true),
        (r"custom://example.com\segment/resource", "custom://example.com/segment", false),
    ] {
        assert_eq!(matches(url, pattern), expected, "{url} {pattern}");
    }
}

#[test]
fn idn_glob_normalization_keeps_wildcard_port_and_path_semantics() {
    for host in ["bücher.example.test", "xn--bcher-kva.example.test", "b%C3%BCcher.example.test", "BÜCHER.EXAMPLE.TEST", "XN--BCHER-KVA.EXAMPLE.TEST"] {
        for pattern in ["bücher.example.test", "xn--bcher-kva.example.test"] {
            assert!(matches(&format!("https://{host}"), &format!("https://{pattern}")), "{host} {pattern}");
            assert!(matches(&format!("https://x.{host}"), &format!("https://*.{pattern}")), "{host} {pattern}");
            assert!(matches(&format!("https://{host}"), &format!("https://*.{pattern}")), "{host} {pattern}");
        }
    }
    for (url, pattern, expected) in [
        ("https://api1.xn--bcher-kva.example.test", "https://api*.bücher.example.test", true),
        ("https://xn--bcher-kva.a.example.test", "https://bücher.*.example.test", true),
        ("https://x.xn--bcher-kva.example.test:8443/allowed/page", "https://*.bücher.example.test:*/allowed/*", true),
        ("https://xn--bcher-kva.example.test:443", "https://bücher.example.test", false),
        ("https://xn--bcher-kva.example.test:8443", "https://bücher.example.test:8080", false),
        ("custom://xn--bcher-kva.example.test", "*.bücher.example.test", false),
        ("https://xn--bcher-kva.example.test/Allowed/page", "https://bücher.example.test/allowed", false),
        ("https://xn--bcher-kva.example.test/allowed/child/../page", "https://bücher.example.test/allowed", true),
        ("https://evilxn--bcher-kva.example.test", "https://*.bücher.example.test", false),
        ("https://bücher.example.test", "https://bü*.example.test", true),
        ("https://x.bücher.example.test", "https://＊.bücher.example.test", false),
        ("https://127.0.0.1", "https://127.1", false),
        ("https://[::1]:8443", "https://[::1]:*", true),
    ] {
        assert_eq!(matches(url, pattern), expected, "{url} {pattern}");
    }
}
