//! URL detection in plain text lines (editor/common/languages/linkComputer.ts), used by the terminal's
//! URI link detector (terminalUriLinkDetector.ts). Columns count chars, where upstream counts UTF-16
//! code units: the two differ only for chars outside the BMP.

/// `ILink` on one line: 1-based `line_number`, 1-based `start_column`, exclusive `end_column`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub line_number: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub url: String,
}

/// `State` of the `http://`, `https://` and `file://` state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Invalid,
    Start,
    H,
    Ht,
    Htt,
    Http,
    F,
    Fi,
    Fil,
    BeforeColon,
    AfterColon,
    AlmostThere,
    End,
    Accept,
}

/// `getStateMachine().nextState`
fn next_state(state: State, ch: char) -> State {
    match (state, ch.to_ascii_lowercase()) {
        (State::Start, 'h') => State::H,
        (State::Start, 'f') => State::F,
        (State::H, 't') => State::Ht,
        (State::Ht, 't') => State::Htt,
        (State::Htt, 'p') => State::Http,
        (State::Http, 's') => State::BeforeColon,
        (State::Http, ':') => State::AfterColon,
        (State::F, 'i') => State::Fi,
        (State::Fi, 'l') => State::Fil,
        (State::Fil, 'e') => State::BeforeColon,
        (State::BeforeColon, ':') => State::AfterColon,
        (State::AfterColon, '/') => State::AlmostThere,
        (State::AlmostThere, '/') => State::End,
        _ => State::Invalid,
    }
}

/// `CharacterClass`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharacterClass {
    None,
    ForceTermination,
    CannotEndIn,
}

/// `getClassifier().get`
fn classify(ch: char) -> CharacterClass {
    const FORCE_TERMINATION_CHARACTERS: &str = " \t<>'\"、。｡､，．：；‘〈「『〔（［｛｢｣｝］）〕』」〉’｀～…|";
    const CANNOT_END_WITH_CHARACTERS: &str = ".,;:";
    if FORCE_TERMINATION_CHARACTERS.contains(ch) {
        CharacterClass::ForceTermination
    } else if CANNOT_END_WITH_CHARACTERS.contains(ch) {
        CharacterClass::CannotEndIn
    } else {
        CharacterClass::None
    }
}

/// `LinkComputer._createLink`
fn create_link(line: &[char], line_number: usize, link_begin_index: usize, link_end_index: usize) -> Link {
    let mut last_included_char_index = link_end_index - 1;
    loop {
        if classify(line[last_included_char_index]) != CharacterClass::CannotEndIn {
            break;
        }
        last_included_char_index -= 1;
        if last_included_char_index <= link_begin_index {
            break;
        }
    }

    if link_begin_index > 0 {
        let before_link = line[link_begin_index - 1];
        let last_in_link = line[last_included_char_index];
        if matches!((before_link, last_in_link), ('(', ')') | ('[', ']') | ('{', '}')) {
            last_included_char_index -= 1;
        }
    }

    Link {
        line_number,
        start_column: link_begin_index + 1,
        end_column: last_included_char_index + 2,
        url: line[link_begin_index..last_included_char_index + 1].iter().collect(),
    }
}

/// `LinkComputer.computeLinks`, over the lines of an `ILinkComputerTarget`.
pub fn compute_links(lines: &[&str]) -> Vec<Link> {
    let mut result = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let line_number = line_index + 1;
        let line: Vec<char> = line.chars().collect();

        let mut link_begin_index = 0;
        let mut link_begin_ch = '\0';
        let mut state = State::Start;
        let mut has_open_parens = false;
        let mut has_open_square_bracket = false;
        let mut in_square_brackets = false;
        let mut has_open_curly_bracket = false;

        for (j, &ch) in line.iter().enumerate() {
            let mut reset_state_machine = false;

            if state == State::Accept {
                let ch_class = match ch {
                    '(' => {
                        has_open_parens = true;
                        CharacterClass::None
                    }
                    ')' if has_open_parens => CharacterClass::None,
                    ')' => CharacterClass::ForceTermination,
                    '[' => {
                        in_square_brackets = true;
                        has_open_square_bracket = true;
                        CharacterClass::None
                    }
                    ']' => {
                        in_square_brackets = false;
                        if has_open_square_bracket { CharacterClass::None } else { CharacterClass::ForceTermination }
                    }
                    '{' => {
                        has_open_curly_bracket = true;
                        CharacterClass::None
                    }
                    '}' if has_open_curly_bracket => CharacterClass::None,
                    '}' => CharacterClass::ForceTermination,
                    // A quote is part of the link only when another quote opened it.
                    '\'' | '"' | '`' if link_begin_ch == ch => CharacterClass::ForceTermination,
                    '\'' | '"' | '`' if matches!(link_begin_ch, '\'' | '"' | '`') => CharacterClass::None,
                    '\'' | '"' | '`' => CharacterClass::ForceTermination,
                    '*' if link_begin_ch == '*' => CharacterClass::ForceTermination,
                    '*' => CharacterClass::None,
                    ' ' if in_square_brackets => CharacterClass::None,
                    ' ' => CharacterClass::ForceTermination,
                    _ => classify(ch),
                };
                if ch_class == CharacterClass::ForceTermination {
                    result.push(create_link(&line, line_number, link_begin_index, j));
                    reset_state_machine = true;
                }
            } else if state == State::End {
                // The authority may be an IPv6 address in [ and ].
                let ch_class = if ch == '[' {
                    has_open_square_bracket = true;
                    CharacterClass::None
                } else {
                    classify(ch)
                };
                if ch_class == CharacterClass::ForceTermination {
                    reset_state_machine = true;
                } else {
                    state = State::Accept;
                }
            } else {
                state = next_state(state, ch);
                if state == State::Invalid {
                    reset_state_machine = true;
                }
            }

            if reset_state_machine {
                state = State::Start;
                has_open_parens = false;
                has_open_square_bracket = false;
                has_open_curly_bracket = false;
                link_begin_index = j + 1;
                link_begin_ch = ch;
            }
        }

        if state == State::Accept {
            result.push(create_link(&line, line_number, link_begin_index, line.len()));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_link(text: &str, extracted_link: &str) {
        let extracted: Vec<char> = extracted_link.chars().collect();
        let is_link_char = |ch: &char| *ch != ' ' && *ch != '\t';
        let start_column = extracted.iter().position(is_link_char).map_or(0, |i| i + 1);
        let end_column = extracted.iter().rposition(is_link_char).map_or(0, |i| i + 2);
        assert_eq!(
            compute_links(&[text]),
            vec![Link {
                line_number: 1,
                start_column,
                end_column,
                url: extracted[start_column - 1..end_column - 1].iter().collect(),
            }],
            "{text}"
        );
    }

    #[test]
    fn parsing() {
        assert_link("x = \"http://foo.bar\";", "     http://foo.bar  ");
        assert_link("x = (http://foo.bar);", "     http://foo.bar  ");
        assert_link("x = [http://foo.bar];", "     http://foo.bar  ");
        assert_link("x = 'http://foo.bar';", "     http://foo.bar  ");
        assert_link("x =  http://foo.bar ;", "     http://foo.bar  ");
        assert_link("x = <http://foo.bar>;", "     http://foo.bar  ");
        assert_link("x = {http://foo.bar};", "     http://foo.bar  ");
        assert_link("(see http://foo.bar)", "     http://foo.bar  ");
        assert_link("[see http://foo.bar]", "     http://foo.bar  ");
        assert_link("{see http://foo.bar}", "     http://foo.bar  ");
        assert_link("<see http://foo.bar>", "     http://foo.bar  ");
        assert_link("<url>http://mylink.com</url>", "     http://mylink.com      ");
        assert_link(
            "// Click here to learn more. https://go.microsoft.com/fwlink/?LinkID=513275&clcid=0x409",
            "                             https://go.microsoft.com/fwlink/?LinkID=513275&clcid=0x409",
        );
        assert_link(
            "// Click here to learn more. https://msdn.microsoft.com/en-us/library/windows/desktop/aa365247(v=vs.85).aspx",
            "                             https://msdn.microsoft.com/en-us/library/windows/desktop/aa365247(v=vs.85).aspx",
        );
        assert_link(
            "// https://github.com/projectkudu/kudu/blob/master/Kudu.Core/Scripts/selectNodeVersion.js",
            "   https://github.com/projectkudu/kudu/blob/master/Kudu.Core/Scripts/selectNodeVersion.js",
        );
        assert_link(
            "<!-- !!! Do not remove !!!   WebContentRef(link:https://go.microsoft.com/fwlink/?LinkId=166007, area:Admin, updated:2015, nextUpdate:2016, tags:SqlServer)   !!! Do not remove !!! -->",
            "                                                https://go.microsoft.com/fwlink/?LinkId=166007                                                                                        ",
        );
        assert_link(
            "For instructions, see https://go.microsoft.com/fwlink/?LinkId=166007.</value>",
            "                      https://go.microsoft.com/fwlink/?LinkId=166007         ",
        );
        assert_link(
            "For instructions, see https://msdn.microsoft.com/en-us/library/windows/desktop/aa365247(v=vs.85).aspx.</value>",
            "                      https://msdn.microsoft.com/en-us/library/windows/desktop/aa365247(v=vs.85).aspx         ",
        );
        assert_link("x = \"https://en.wikipedia.org/wiki/Zürich\";", "     https://en.wikipedia.org/wiki/Zürich  ");
        assert_link(
            "請參閱 http://go.microsoft.com/fwlink/?LinkId=761051。",
            "    http://go.microsoft.com/fwlink/?LinkId=761051 ",
        );
        assert_link(
            "（請參閱 http://go.microsoft.com/fwlink/?LinkId=761051）",
            "     http://go.microsoft.com/fwlink/?LinkId=761051 ",
        );
        assert_link("x = \"file:///foo.bar\";", "     file:///foo.bar  ");
        assert_link("x = \"file://c:/foo.bar\";", "     file://c:/foo.bar  ");
        assert_link("x = \"file://shares/foo.bar\";", "     file://shares/foo.bar  ");
        assert_link("x = \"file://shäres/foo.bar\";", "     file://shäres/foo.bar  ");
        assert_link("Some text, then http://www.bing.com.", "                http://www.bing.com ");
        assert_link(
            "let url = `http://***/_api/web/lists/GetByTitle('Teambuildingaanvragen')/items`;",
            "           http://***/_api/web/lists/GetByTitle('Teambuildingaanvragen')/items  ",
        );
    }

    #[test]
    fn issue_7855() {
        assert_link(
            "7. At this point, ServiceMain has been called.  There is no functionality presently in ServiceMain, but you can consult the [MSDN documentation](https://msdn.microsoft.com/en-us/library/windows/desktop/ms687414(v=vs.85).aspx) to add functionality as desired!",
            "                                                                                                                                                 https://msdn.microsoft.com/en-us/library/windows/desktop/ms687414(v=vs.85).aspx                                  ",
        );
    }

    #[test]
    fn issue_62278_ctrl_click_to_follow_link_for_ipv6_urls() {
        assert_link("let x = \"http://[::1]:5000/connect/token\"", "         http://[::1]:5000/connect/token  ");
    }

    #[test]
    fn issue_70254_bold_links_dont_open_in_markdown_file() {
        assert_link("2. Navigate to **https://portal.azure.com**", "                 https://portal.azure.com  ");
    }

    #[test]
    fn issue_86358_url_wrong_recognition_pattern() {
        assert_link("POST|https://portal.azure.com|2019-12-05|", "     https://portal.azure.com            ");
    }

    #[test]
    fn issue_67022_space_as_end_of_hyperlink_isnt_always_good_idea() {
        assert_link("aa  https://foo.bar/[this is foo site]  aa", "    https://foo.bar/[this is foo site]    ");
    }

    #[test]
    fn issue_100353_link_detection_stops_at_double_byte_ampersand() {
        assert_link(
            "aa  http://tree-mark.chips.jp/レーズン＆ベリーミックス  aa",
            "    http://tree-mark.chips.jp/レーズン＆ベリーミックス    ",
        );
    }

    #[test]
    fn issue_121438_link_detection_stops_at_black_lenticular_brackets() {
        assert_link("aa  https://zh.wikipedia.org/wiki/【我推的孩子】 aa", "    https://zh.wikipedia.org/wiki/【我推的孩子】   ");
    }

    #[test]
    fn issue_121438_link_detection_stops_at_double_angle_brackets() {
        assert_link(
            "aa  https://zh.wikipedia.org/wiki/《新青年》编辑部旧址 aa",
            "    https://zh.wikipedia.org/wiki/《新青年》编辑部旧址   ",
        );
    }

    #[test]
    fn issue_121438_link_detection_stops_at_curly_double_quotes() {
        assert_link(
            "aa  https://zh.wikipedia.org/wiki/“常凯申”误译事件 aa",
            "    https://zh.wikipedia.org/wiki/“常凯申”误译事件   ",
        );
    }

    #[test]
    fn issue_150905_colon_after_bare_hyperlink_is_treated_as_its_part() {
        assert_link("https://site.web/page.html: blah blah blah", "https://site.web/page.html                ");
    }

    #[test]
    fn issue_156875_links_include_quotes() {
        assert_link(
            "\"This file has been converted from https://github.com/jeff-hykin/better-c-syntax/blob/master/autogenerated/c.tmLanguage.json\",",
            "                                   https://github.com/jeff-hykin/better-c-syntax/blob/master/autogenerated/c.tmLanguage.json  ",
        );
    }

    #[test]
    fn issue_225513_cmd_click_doesnt_work_on_jsdoc_link_format() {
        assert_link(
            " * {@link https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Promise/withResolvers|Promise.withResolvers}",
            "          https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Promise/withResolvers                       ",
        );
    }
}
