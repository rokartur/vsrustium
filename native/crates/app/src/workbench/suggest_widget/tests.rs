use super::break_all;

#[test]
fn a_detail_breaks_anywhere_unless_it_has_indented_lines() {
    assert_eq!(break_all("function matchMedia(query: string): MediaQueryList", 41), ["function matchMedia(query: string): Media", "QueryList"]);
    assert_eq!(break_all("abc  def", 3), ["abc", "def"]);
    assert_eq!(break_all("type A = {\n    a: 1\n}", 5), ["type A = {", "    a: 1", "}"]);
}
