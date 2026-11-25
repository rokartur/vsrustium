//! Grapheme cluster steps for cursor moves and deletes, as strings.ts computes them with its own
//! simplified UAX #29 rules (GB11 and GB12/13 without state). Offsets are char indices into a line.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphemeBreakType {
    Other,
    Prepend,
    CR,
    LF,
    Control,
    Extend,
    RegionalIndicator,
    SpacingMark,
    L,
    V,
    T,
    LV,
    LVT,
    ZWJ,
    ExtendedPictographic,
}

use GraphemeBreakType::*;

/// `strings.nextCharLength`: the chars of the grapheme cluster starting at `offset`.
pub fn next_char_length(text: &[char], offset: usize) -> usize {
    let mut end = offset + 1;
    let mut break_type = grapheme_break_type(text[offset]);
    while end < text.len() {
        let next_break_type = grapheme_break_type(text[end]);
        if break_between(break_type, next_break_type) {
            break;
        }
        break_type = next_break_type;
        end += 1;
    }
    end - offset
}

/// `strings.prevCharLength`: the chars of the grapheme cluster ending at `offset`.
pub fn prev_char_length(text: &[char], offset: usize) -> usize {
    let mut start = offset - 1;
    let mut break_type = grapheme_break_type(text[start]);
    while start > 0 {
        let prev_break_type = grapheme_break_type(text[start - 1]);
        if break_between(prev_break_type, break_type) {
            break;
        }
        break_type = prev_break_type;
        start -= 1;
    }
    offset - start
}

/// `strings.getLeftDeleteOffset`: before the last emoji component, else one char back.
pub fn left_delete_offset(offset: usize, text: &[char]) -> usize {
    if offset == 0 {
        return 0;
    }
    offset_before_last_emoji_component(offset, text).unwrap_or(offset - 1)
}

/// `getOffsetBeforeLastEmojiComponent`, per https://www.unicode.org/reports/tr51/tr51-14.html#EBNF_and_Regex.
fn offset_before_last_emoji_component(offset: usize, text: &[char]) -> Option<usize> {
    const ZWJ_CHAR: char = '\u{200D}';
    const EMOJI_VARIANT_SELECTOR: char = '\u{FE0F}';
    const ENCLOSING_KEYCAP: char = '\u{20E3}';
    let mut at = offset - 1;
    while matches!(text[at], '\u{1F3FB}'..='\u{1F3FF}' | EMOJI_VARIANT_SELECTOR | ENCLOSING_KEYCAP) {
        if at == 0 {
            return None;
        }
        at -= 1;
    }
    if !is_emoji_imprecise(text[at]) {
        return None;
    }
    if at > 0 && text[at - 1] == ZWJ_CHAR {
        return Some(at - 1);
    }
    Some(at)
}

/// `strings.isEmojiImprecise`.
pub fn is_emoji_imprecise(c: char) -> bool {
    matches!(
        c as u32,
        0x1F1E6..=0x1F1FF | 8986 | 8987 | 9200 | 9203 | 9728..=10175 | 11088 | 11093 | 127744..=128591
            | 128640..=128764 | 128992..=129008 | 129280..=129535 | 129648..=129782
    )
}

fn break_between(a: GraphemeBreakType, b: GraphemeBreakType) -> bool {
    if a == Other {
        return b != Extend && b != SpacingMark;
    }
    if a == CR && b == LF {
        return false; // GB3
    }
    if matches!(a, Control | CR | LF) || matches!(b, Control | CR | LF) {
        return true; // GB4, GB5
    }
    if a == L && matches!(b, L | V | LV | LVT) {
        return false; // GB6
    }
    if matches!(a, LV | V) && matches!(b, V | T) {
        return false; // GB7
    }
    if matches!(a, LVT | T) && b == T {
        return false; // GB8
    }
    if matches!(b, Extend | ZWJ | SpacingMark) || a == Prepend {
        return false; // GB9, GB9a, GB9b
    }
    if a == ZWJ && b == ExtendedPictographic {
        return false; // GB11 without the Extended_Pictographic Extend* prefix
    }
    if a == RegionalIndicator && b == RegionalIndicator {
        return false; // GB12, GB13 without pairing
    }
    true // GB999
}

/// `GraphemeBreakTree.getGraphemeBreakType`: a binary search over an implicit tree, node `i` with
/// children `2i` and `2i + 1`; slot 0 is unused.
pub fn grapheme_break_type(c: char) -> GraphemeBreakType {
    let code_point = c as u32;
    if code_point < 32 {
        return match c {
            '\n' => LF,
            '\r' => CR,
            _ => Control,
        };
    }
    if code_point < 127 {
        return Other;
    }
    let mut index = 1;
    while index < GRAPHEME_BREAK_DATA.len() {
        let (start, end, break_type) = GRAPHEME_BREAK_DATA[index];
        if code_point < start {
            index *= 2;
        } else if code_point > end {
            index = 2 * index + 1;
        } else {
            return break_type;
        }
    }
    Other
}

// Generated from strings.ts getGraphemeBreakRawData (https://github.com/alexdima/unicode-utils/blob/main/grapheme-break.js).
#[rustfmt::skip]
const GRAPHEME_BREAK_DATA: [(u32, u32, GraphemeBreakType); 1879] = [
    (0, 0, Other), (51229, 51255, LVT), (44061, 44087, LVT), (127462, 127487, RegionalIndicator), (7083, 7085, Extend),
    (47645, 47671, LVT), (54813, 54839, LVT), (128678, 128678, ExtendedPictographic), (3270, 3270, Extend),
    (9919, 9923, ExtendedPictographic), (45853, 45879, LVT), (49437, 49463, LVT), (53021, 53047, LVT),
    (71216, 71218, SpacingMark), (128398, 128399, ExtendedPictographic), (129360, 129374, ExtendedPictographic),
    (2519, 2519, Extend), (4448, 4519, V), (9742, 9742, ExtendedPictographic), (12336, 12336, ExtendedPictographic),
    (44957, 44983, LVT), (46749, 46775, LVT), (48541, 48567, LVT), (50333, 50359, LVT), (52125, 52151, LVT),
    (53917, 53943, LVT), (69888, 69890, Extend), (73018, 73018, Extend), (127990, 127990, ExtendedPictographic),
    (128558, 128559, ExtendedPictographic), (128759, 128760, ExtendedPictographic),
    (129653, 129655, ExtendedPictographic), (2027, 2035, Extend), (2891, 2892, SpacingMark), (3761, 3761, Extend),
    (6683, 6683, Extend), (8293, 8293, Control), (9825, 9826, ExtendedPictographic),
    (9999, 9999, ExtendedPictographic), (43452, 43453, Extend), (44509, 44535, LVT), (45405, 45431, LVT),
    (46301, 46327, LVT), (47197, 47223, LVT), (48093, 48119, LVT), (48989, 49015, LVT), (49885, 49911, LVT),
    (50781, 50807, LVT), (51677, 51703, LVT), (52573, 52599, LVT), (53469, 53495, LVT), (54365, 54391, LVT),
    (65279, 65279, Control), (70471, 70472, SpacingMark), (72145, 72147, SpacingMark), (119173, 119179, Extend),
    (127799, 127818, ExtendedPictographic), (128240, 128244, ExtendedPictographic),
    (128512, 128512, ExtendedPictographic), (128652, 128652, ExtendedPictographic),
    (128721, 128722, ExtendedPictographic), (129292, 129292, ExtendedPictographic),
    (129445, 129450, ExtendedPictographic), (129734, 129743, ExtendedPictographic), (1476, 1477, Extend),
    (2366, 2368, SpacingMark), (2750, 2752, SpacingMark), (3076, 3076, Extend), (3415, 3415, Extend),
    (4141, 4144, Extend), (6109, 6109, Extend), (6964, 6964, Extend), (7394, 7400, Extend),
    (9197, 9198, ExtendedPictographic), (9770, 9770, ExtendedPictographic), (9877, 9877, ExtendedPictographic),
    (9968, 9969, ExtendedPictographic), (10084, 10084, ExtendedPictographic), (43052, 43052, Extend),
    (43713, 43713, Extend), (44285, 44311, LVT), (44733, 44759, LVT), (45181, 45207, LVT), (45629, 45655, LVT),
    (46077, 46103, LVT), (46525, 46551, LVT), (46973, 46999, LVT), (47421, 47447, LVT), (47869, 47895, LVT),
    (48317, 48343, LVT), (48765, 48791, LVT), (49213, 49239, LVT), (49661, 49687, LVT), (50109, 50135, LVT),
    (50557, 50583, LVT), (51005, 51031, LVT), (51453, 51479, LVT), (51901, 51927, LVT), (52349, 52375, LVT),
    (52797, 52823, LVT), (53245, 53271, LVT), (53693, 53719, LVT), (54141, 54167, LVT), (54589, 54615, LVT),
    (55037, 55063, LVT), (69506, 69509, Extend), (70191, 70193, Extend), (70841, 70841, SpacingMark),
    (71463, 71467, Extend), (72330, 72342, Extend), (94031, 94031, Extend), (123628, 123631, Extend),
    (127763, 127765, ExtendedPictographic), (127941, 127941, ExtendedPictographic),
    (128043, 128062, ExtendedPictographic), (128302, 128317, ExtendedPictographic),
    (128465, 128467, ExtendedPictographic), (128539, 128539, ExtendedPictographic),
    (128640, 128640, ExtendedPictographic), (128662, 128662, ExtendedPictographic),
    (128703, 128703, ExtendedPictographic), (128745, 128745, ExtendedPictographic),
    (129004, 129007, ExtendedPictographic), (129329, 129330, ExtendedPictographic),
    (129402, 129402, ExtendedPictographic), (129483, 129483, ExtendedPictographic),
    (129686, 129704, ExtendedPictographic), (130048, 131069, ExtendedPictographic), (173, 173, Control),
    (1757, 1757, Prepend), (2200, 2207, Extend), (2434, 2435, SpacingMark), (2631, 2632, Extend), (2817, 2817, Extend),
    (3008, 3008, Extend), (3201, 3201, Extend), (3387, 3388, Extend), (3542, 3542, Extend), (3902, 3903, SpacingMark),
    (4190, 4192, Extend), (6002, 6003, Extend), (6439, 6440, Extend), (6765, 6770, SpacingMark), (7019, 7027, Extend),
    (7154, 7155, SpacingMark), (8205, 8205, ZWJ), (8505, 8505, ExtendedPictographic),
    (9654, 9654, ExtendedPictographic), (9757, 9757, ExtendedPictographic), (9792, 9792, ExtendedPictographic),
    (9852, 9853, ExtendedPictographic), (9890, 9894, ExtendedPictographic), (9937, 9937, ExtendedPictographic),
    (9981, 9981, ExtendedPictographic), (10035, 10036, ExtendedPictographic), (11035, 11036, ExtendedPictographic),
    (42654, 42655, Extend), (43346, 43347, SpacingMark), (43587, 43587, Extend), (44006, 44007, SpacingMark),
    (44173, 44199, LVT), (44397, 44423, LVT), (44621, 44647, LVT), (44845, 44871, LVT), (45069, 45095, LVT),
    (45293, 45319, LVT), (45517, 45543, LVT), (45741, 45767, LVT), (45965, 45991, LVT), (46189, 46215, LVT),
    (46413, 46439, LVT), (46637, 46663, LVT), (46861, 46887, LVT), (47085, 47111, LVT), (47309, 47335, LVT),
    (47533, 47559, LVT), (47757, 47783, LVT), (47981, 48007, LVT), (48205, 48231, LVT), (48429, 48455, LVT),
    (48653, 48679, LVT), (48877, 48903, LVT), (49101, 49127, LVT), (49325, 49351, LVT), (49549, 49575, LVT),
    (49773, 49799, LVT), (49997, 50023, LVT), (50221, 50247, LVT), (50445, 50471, LVT), (50669, 50695, LVT),
    (50893, 50919, LVT), (51117, 51143, LVT), (51341, 51367, LVT), (51565, 51591, LVT), (51789, 51815, LVT),
    (52013, 52039, LVT), (52237, 52263, LVT), (52461, 52487, LVT), (52685, 52711, LVT), (52909, 52935, LVT),
    (53133, 53159, LVT), (53357, 53383, LVT), (53581, 53607, LVT), (53805, 53831, LVT), (54029, 54055, LVT),
    (54253, 54279, LVT), (54477, 54503, LVT), (54701, 54727, LVT), (54925, 54951, LVT), (55149, 55175, LVT),
    (68101, 68102, Extend), (69762, 69762, SpacingMark), (70067, 70069, SpacingMark), (70371, 70378, Extend),
    (70720, 70721, SpacingMark), (71087, 71087, Extend), (71341, 71341, Extend), (71995, 71996, Extend),
    (72249, 72249, SpacingMark), (72850, 72871, Extend), (73109, 73109, Extend), (118576, 118598, Extend),
    (121505, 121519, Extend), (127245, 127247, ExtendedPictographic), (127568, 127569, ExtendedPictographic),
    (127777, 127777, ExtendedPictographic), (127872, 127891, ExtendedPictographic),
    (127956, 127967, ExtendedPictographic), (128015, 128016, ExtendedPictographic),
    (128110, 128172, ExtendedPictographic), (128259, 128259, ExtendedPictographic),
    (128367, 128368, ExtendedPictographic), (128424, 128424, ExtendedPictographic),
    (128488, 128488, ExtendedPictographic), (128530, 128532, ExtendedPictographic),
    (128550, 128551, ExtendedPictographic), (128566, 128566, ExtendedPictographic),
    (128647, 128647, ExtendedPictographic), (128656, 128656, ExtendedPictographic),
    (128667, 128673, ExtendedPictographic), (128691, 128693, ExtendedPictographic),
    (128715, 128715, ExtendedPictographic), (128728, 128732, ExtendedPictographic),
    (128752, 128752, ExtendedPictographic), (128765, 128767, ExtendedPictographic),
    (129096, 129103, ExtendedPictographic), (129311, 129311, ExtendedPictographic),
    (129344, 129349, ExtendedPictographic), (129394, 129394, ExtendedPictographic),
    (129413, 129425, ExtendedPictographic), (129466, 129471, ExtendedPictographic),
    (129511, 129535, ExtendedPictographic), (129664, 129666, ExtendedPictographic),
    (129719, 129722, ExtendedPictographic), (129760, 129767, ExtendedPictographic), (917536, 917631, Extend),
    (13, 13, CR), (1160, 1161, Extend), (1564, 1564, Control), (1807, 1807, Prepend), (2085, 2087, Extend),
    (2307, 2307, SpacingMark), (2382, 2383, SpacingMark), (2497, 2500, Extend), (2563, 2563, SpacingMark),
    (2677, 2677, Extend), (2763, 2764, SpacingMark), (2879, 2879, Extend), (2914, 2915, Extend), (3021, 3021, Extend),
    (3142, 3144, Extend), (3263, 3263, Extend), (3285, 3286, Extend), (3398, 3400, SpacingMark), (3530, 3530, Extend),
    (3633, 3633, Extend), (3864, 3865, Extend), (3974, 3975, Extend), (4155, 4156, SpacingMark), (4229, 4230, Extend),
    (5909, 5909, SpacingMark), (6078, 6085, SpacingMark), (6277, 6278, Extend), (6451, 6456, SpacingMark),
    (6744, 6750, Extend), (6846, 6846, Extend), (6972, 6972, Extend), (7074, 7077, Extend), (7146, 7148, SpacingMark),
    (7222, 7223, Extend), (7416, 7417, Extend), (8234, 8238, Control), (8417, 8417, Extend),
    (9000, 9000, ExtendedPictographic), (9203, 9203, ExtendedPictographic), (9730, 9731, ExtendedPictographic),
    (9748, 9749, ExtendedPictographic), (9762, 9763, ExtendedPictographic), (9776, 9783, ExtendedPictographic),
    (9800, 9811, ExtendedPictographic), (9831, 9831, ExtendedPictographic), (9872, 9873, ExtendedPictographic),
    (9882, 9882, ExtendedPictographic), (9900, 9903, ExtendedPictographic), (9929, 9933, ExtendedPictographic),
    (9941, 9960, ExtendedPictographic), (9974, 9974, ExtendedPictographic), (9989, 9989, ExtendedPictographic),
    (10006, 10006, ExtendedPictographic), (10062, 10062, ExtendedPictographic), (10160, 10160, ExtendedPictographic),
    (11647, 11647, Extend), (12953, 12953, ExtendedPictographic), (43019, 43019, Extend), (43232, 43249, Extend),
    (43443, 43443, Extend), (43567, 43568, SpacingMark), (43696, 43696, Extend), (43765, 43765, SpacingMark),
    (44013, 44013, Extend), (44117, 44143, LVT), (44229, 44255, LVT), (44341, 44367, LVT), (44453, 44479, LVT),
    (44565, 44591, LVT), (44677, 44703, LVT), (44789, 44815, LVT), (44901, 44927, LVT), (45013, 45039, LVT),
    (45125, 45151, LVT), (45237, 45263, LVT), (45349, 45375, LVT), (45461, 45487, LVT), (45573, 45599, LVT),
    (45685, 45711, LVT), (45797, 45823, LVT), (45909, 45935, LVT), (46021, 46047, LVT), (46133, 46159, LVT),
    (46245, 46271, LVT), (46357, 46383, LVT), (46469, 46495, LVT), (46581, 46607, LVT), (46693, 46719, LVT),
    (46805, 46831, LVT), (46917, 46943, LVT), (47029, 47055, LVT), (47141, 47167, LVT), (47253, 47279, LVT),
    (47365, 47391, LVT), (47477, 47503, LVT), (47589, 47615, LVT), (47701, 47727, LVT), (47813, 47839, LVT),
    (47925, 47951, LVT), (48037, 48063, LVT), (48149, 48175, LVT), (48261, 48287, LVT), (48373, 48399, LVT),
    (48485, 48511, LVT), (48597, 48623, LVT), (48709, 48735, LVT), (48821, 48847, LVT), (48933, 48959, LVT),
    (49045, 49071, LVT), (49157, 49183, LVT), (49269, 49295, LVT), (49381, 49407, LVT), (49493, 49519, LVT),
    (49605, 49631, LVT), (49717, 49743, LVT), (49829, 49855, LVT), (49941, 49967, LVT), (50053, 50079, LVT),
    (50165, 50191, LVT), (50277, 50303, LVT), (50389, 50415, LVT), (50501, 50527, LVT), (50613, 50639, LVT),
    (50725, 50751, LVT), (50837, 50863, LVT), (50949, 50975, LVT), (51061, 51087, LVT), (51173, 51199, LVT),
    (51285, 51311, LVT), (51397, 51423, LVT), (51509, 51535, LVT), (51621, 51647, LVT), (51733, 51759, LVT),
    (51845, 51871, LVT), (51957, 51983, LVT), (52069, 52095, LVT), (52181, 52207, LVT), (52293, 52319, LVT),
    (52405, 52431, LVT), (52517, 52543, LVT), (52629, 52655, LVT), (52741, 52767, LVT), (52853, 52879, LVT),
    (52965, 52991, LVT), (53077, 53103, LVT), (53189, 53215, LVT), (53301, 53327, LVT), (53413, 53439, LVT),
    (53525, 53551, LVT), (53637, 53663, LVT), (53749, 53775, LVT), (53861, 53887, LVT), (53973, 53999, LVT),
    (54085, 54111, LVT), (54197, 54223, LVT), (54309, 54335, LVT), (54421, 54447, LVT), (54533, 54559, LVT),
    (54645, 54671, LVT), (54757, 54783, LVT), (54869, 54895, LVT), (54981, 55007, LVT), (55093, 55119, LVT),
    (55243, 55291, T), (66045, 66045, Extend), (68325, 68326, Extend), (69688, 69702, Extend), (69817, 69818, Extend),
    (69957, 69958, SpacingMark), (70089, 70092, Extend), (70198, 70199, Extend), (70462, 70462, Extend),
    (70502, 70508, Extend), (70750, 70750, Extend), (70846, 70846, SpacingMark), (71100, 71101, Extend),
    (71230, 71230, SpacingMark), (71351, 71351, Extend), (71737, 71738, Extend), (72000, 72000, SpacingMark),
    (72160, 72160, Extend), (72273, 72278, Extend), (72752, 72758, Extend), (72882, 72883, Extend),
    (73031, 73031, Extend), (73461, 73462, SpacingMark), (94192, 94193, SpacingMark), (119149, 119149, SpacingMark),
    (121403, 121452, Extend), (122915, 122916, Extend), (126980, 126980, ExtendedPictographic),
    (127358, 127359, ExtendedPictographic), (127535, 127535, ExtendedPictographic),
    (127759, 127759, ExtendedPictographic), (127771, 127771, ExtendedPictographic),
    (127792, 127793, ExtendedPictographic), (127825, 127867, ExtendedPictographic),
    (127897, 127899, ExtendedPictographic), (127945, 127945, ExtendedPictographic),
    (127985, 127986, ExtendedPictographic), (128000, 128007, ExtendedPictographic),
    (128021, 128021, ExtendedPictographic), (128066, 128100, ExtendedPictographic),
    (128184, 128235, ExtendedPictographic), (128249, 128252, ExtendedPictographic),
    (128266, 128276, ExtendedPictographic), (128335, 128335, ExtendedPictographic),
    (128379, 128390, ExtendedPictographic), (128407, 128419, ExtendedPictographic),
    (128444, 128444, ExtendedPictographic), (128481, 128481, ExtendedPictographic),
    (128499, 128499, ExtendedPictographic), (128526, 128526, ExtendedPictographic),
    (128536, 128536, ExtendedPictographic), (128543, 128543, ExtendedPictographic),
    (128556, 128556, ExtendedPictographic), (128564, 128564, ExtendedPictographic),
    (128577, 128580, ExtendedPictographic), (128643, 128645, ExtendedPictographic),
    (128649, 128649, ExtendedPictographic), (128654, 128654, ExtendedPictographic),
    (128660, 128660, ExtendedPictographic), (128664, 128664, ExtendedPictographic),
    (128675, 128675, ExtendedPictographic), (128686, 128689, ExtendedPictographic),
    (128695, 128696, ExtendedPictographic), (128705, 128709, ExtendedPictographic),
    (128717, 128719, ExtendedPictographic), (128725, 128725, ExtendedPictographic),
    (128736, 128741, ExtendedPictographic), (128747, 128748, ExtendedPictographic),
    (128755, 128755, ExtendedPictographic), (128762, 128762, ExtendedPictographic),
    (128981, 128991, ExtendedPictographic), (129009, 129023, ExtendedPictographic),
    (129160, 129167, ExtendedPictographic), (129296, 129304, ExtendedPictographic),
    (129320, 129327, ExtendedPictographic), (129340, 129342, ExtendedPictographic),
    (129356, 129356, ExtendedPictographic), (129388, 129392, ExtendedPictographic),
    (129399, 129400, ExtendedPictographic), (129404, 129407, ExtendedPictographic),
    (129432, 129442, ExtendedPictographic), (129454, 129455, ExtendedPictographic),
    (129473, 129474, ExtendedPictographic), (129485, 129487, ExtendedPictographic),
    (129648, 129651, ExtendedPictographic), (129659, 129660, ExtendedPictographic),
    (129671, 129679, ExtendedPictographic), (129709, 129711, ExtendedPictographic),
    (129728, 129730, ExtendedPictographic), (129751, 129753, ExtendedPictographic),
    (129776, 129782, ExtendedPictographic), (917505, 917505, Control), (917760, 917999, Extend), (10, 10, LF),
    (127, 159, Control), (768, 879, Extend), (1471, 1471, Extend), (1536, 1541, Prepend), (1648, 1648, Extend),
    (1767, 1768, Extend), (1840, 1866, Extend), (2070, 2073, Extend), (2137, 2139, Extend), (2274, 2274, Prepend),
    (2363, 2363, SpacingMark), (2377, 2380, SpacingMark), (2402, 2403, Extend), (2494, 2494, Extend),
    (2507, 2508, SpacingMark), (2558, 2558, Extend), (2622, 2624, SpacingMark), (2641, 2641, Extend),
    (2691, 2691, SpacingMark), (2759, 2760, Extend), (2786, 2787, Extend), (2876, 2876, Extend), (2881, 2884, Extend),
    (2901, 2902, Extend), (3006, 3006, Extend), (3014, 3016, SpacingMark), (3072, 3072, Extend), (3134, 3136, Extend),
    (3157, 3158, Extend), (3260, 3260, Extend), (3266, 3266, Extend), (3274, 3275, SpacingMark), (3328, 3329, Extend),
    (3391, 3392, SpacingMark), (3405, 3405, Extend), (3457, 3457, Extend), (3536, 3537, SpacingMark),
    (3551, 3551, Extend), (3636, 3642, Extend), (3764, 3772, Extend), (3895, 3895, Extend), (3967, 3967, SpacingMark),
    (3993, 4028, Extend), (4146, 4151, Extend), (4182, 4183, SpacingMark), (4226, 4226, Extend), (4253, 4253, Extend),
    (4957, 4959, Extend), (5940, 5940, SpacingMark), (6070, 6070, SpacingMark), (6087, 6088, SpacingMark),
    (6158, 6158, Control), (6432, 6434, Extend), (6448, 6449, SpacingMark), (6679, 6680, Extend), (6742, 6742, Extend),
    (6754, 6754, Extend), (6783, 6783, Extend), (6912, 6915, Extend), (6966, 6970, Extend), (6978, 6978, Extend),
    (7042, 7042, SpacingMark), (7080, 7081, Extend), (7143, 7143, SpacingMark), (7150, 7150, SpacingMark),
    (7212, 7219, Extend), (7380, 7392, Extend), (7412, 7412, Extend), (8203, 8203, Control), (8232, 8232, Control),
    (8265, 8265, ExtendedPictographic), (8400, 8412, Extend), (8421, 8432, Extend), (8617, 8618, ExtendedPictographic),
    (9167, 9167, ExtendedPictographic), (9200, 9200, ExtendedPictographic), (9410, 9410, ExtendedPictographic),
    (9723, 9726, ExtendedPictographic), (9733, 9733, ExtendedPictographic), (9745, 9745, ExtendedPictographic),
    (9752, 9752, ExtendedPictographic), (9760, 9760, ExtendedPictographic), (9766, 9766, ExtendedPictographic),
    (9774, 9774, ExtendedPictographic), (9786, 9786, ExtendedPictographic), (9794, 9794, ExtendedPictographic),
    (9823, 9823, ExtendedPictographic), (9828, 9828, ExtendedPictographic), (9833, 9850, ExtendedPictographic),
    (9855, 9855, ExtendedPictographic), (9875, 9875, ExtendedPictographic), (9880, 9880, ExtendedPictographic),
    (9885, 9887, ExtendedPictographic), (9896, 9897, ExtendedPictographic), (9906, 9916, ExtendedPictographic),
    (9926, 9927, ExtendedPictographic), (9935, 9935, ExtendedPictographic), (9939, 9939, ExtendedPictographic),
    (9962, 9962, ExtendedPictographic), (9972, 9972, ExtendedPictographic), (9978, 9978, ExtendedPictographic),
    (9986, 9986, ExtendedPictographic), (9997, 9997, ExtendedPictographic), (10002, 10002, ExtendedPictographic),
    (10017, 10017, ExtendedPictographic), (10055, 10055, ExtendedPictographic), (10071, 10071, ExtendedPictographic),
    (10133, 10135, ExtendedPictographic), (10548, 10549, ExtendedPictographic), (11093, 11093, ExtendedPictographic),
    (12330, 12333, Extend), (12441, 12442, Extend), (42608, 42610, Extend), (43010, 43010, Extend),
    (43045, 43046, Extend), (43188, 43203, SpacingMark), (43302, 43309, Extend), (43392, 43394, Extend),
    (43446, 43449, Extend), (43493, 43493, Extend), (43571, 43572, SpacingMark), (43597, 43597, SpacingMark),
    (43703, 43704, Extend), (43756, 43757, Extend), (44003, 44004, SpacingMark), (44009, 44010, SpacingMark),
    (44033, 44059, LVT), (44089, 44115, LVT), (44145, 44171, LVT), (44201, 44227, LVT), (44257, 44283, LVT),
    (44313, 44339, LVT), (44369, 44395, LVT), (44425, 44451, LVT), (44481, 44507, LVT), (44537, 44563, LVT),
    (44593, 44619, LVT), (44649, 44675, LVT), (44705, 44731, LVT), (44761, 44787, LVT), (44817, 44843, LVT),
    (44873, 44899, LVT), (44929, 44955, LVT), (44985, 45011, LVT), (45041, 45067, LVT), (45097, 45123, LVT),
    (45153, 45179, LVT), (45209, 45235, LVT), (45265, 45291, LVT), (45321, 45347, LVT), (45377, 45403, LVT),
    (45433, 45459, LVT), (45489, 45515, LVT), (45545, 45571, LVT), (45601, 45627, LVT), (45657, 45683, LVT),
    (45713, 45739, LVT), (45769, 45795, LVT), (45825, 45851, LVT), (45881, 45907, LVT), (45937, 45963, LVT),
    (45993, 46019, LVT), (46049, 46075, LVT), (46105, 46131, LVT), (46161, 46187, LVT), (46217, 46243, LVT),
    (46273, 46299, LVT), (46329, 46355, LVT), (46385, 46411, LVT), (46441, 46467, LVT), (46497, 46523, LVT),
    (46553, 46579, LVT), (46609, 46635, LVT), (46665, 46691, LVT), (46721, 46747, LVT), (46777, 46803, LVT),
    (46833, 46859, LVT), (46889, 46915, LVT), (46945, 46971, LVT), (47001, 47027, LVT), (47057, 47083, LVT),
    (47113, 47139, LVT), (47169, 47195, LVT), (47225, 47251, LVT), (47281, 47307, LVT), (47337, 47363, LVT),
    (47393, 47419, LVT), (47449, 47475, LVT), (47505, 47531, LVT), (47561, 47587, LVT), (47617, 47643, LVT),
    (47673, 47699, LVT), (47729, 47755, LVT), (47785, 47811, LVT), (47841, 47867, LVT), (47897, 47923, LVT),
    (47953, 47979, LVT), (48009, 48035, LVT), (48065, 48091, LVT), (48121, 48147, LVT), (48177, 48203, LVT),
    (48233, 48259, LVT), (48289, 48315, LVT), (48345, 48371, LVT), (48401, 48427, LVT), (48457, 48483, LVT),
    (48513, 48539, LVT), (48569, 48595, LVT), (48625, 48651, LVT), (48681, 48707, LVT), (48737, 48763, LVT),
    (48793, 48819, LVT), (48849, 48875, LVT), (48905, 48931, LVT), (48961, 48987, LVT), (49017, 49043, LVT),
    (49073, 49099, LVT), (49129, 49155, LVT), (49185, 49211, LVT), (49241, 49267, LVT), (49297, 49323, LVT),
    (49353, 49379, LVT), (49409, 49435, LVT), (49465, 49491, LVT), (49521, 49547, LVT), (49577, 49603, LVT),
    (49633, 49659, LVT), (49689, 49715, LVT), (49745, 49771, LVT), (49801, 49827, LVT), (49857, 49883, LVT),
    (49913, 49939, LVT), (49969, 49995, LVT), (50025, 50051, LVT), (50081, 50107, LVT), (50137, 50163, LVT),
    (50193, 50219, LVT), (50249, 50275, LVT), (50305, 50331, LVT), (50361, 50387, LVT), (50417, 50443, LVT),
    (50473, 50499, LVT), (50529, 50555, LVT), (50585, 50611, LVT), (50641, 50667, LVT), (50697, 50723, LVT),
    (50753, 50779, LVT), (50809, 50835, LVT), (50865, 50891, LVT), (50921, 50947, LVT), (50977, 51003, LVT),
    (51033, 51059, LVT), (51089, 51115, LVT), (51145, 51171, LVT), (51201, 51227, LVT), (51257, 51283, LVT),
    (51313, 51339, LVT), (51369, 51395, LVT), (51425, 51451, LVT), (51481, 51507, LVT), (51537, 51563, LVT),
    (51593, 51619, LVT), (51649, 51675, LVT), (51705, 51731, LVT), (51761, 51787, LVT), (51817, 51843, LVT),
    (51873, 51899, LVT), (51929, 51955, LVT), (51985, 52011, LVT), (52041, 52067, LVT), (52097, 52123, LVT),
    (52153, 52179, LVT), (52209, 52235, LVT), (52265, 52291, LVT), (52321, 52347, LVT), (52377, 52403, LVT),
    (52433, 52459, LVT), (52489, 52515, LVT), (52545, 52571, LVT), (52601, 52627, LVT), (52657, 52683, LVT),
    (52713, 52739, LVT), (52769, 52795, LVT), (52825, 52851, LVT), (52881, 52907, LVT), (52937, 52963, LVT),
    (52993, 53019, LVT), (53049, 53075, LVT), (53105, 53131, LVT), (53161, 53187, LVT), (53217, 53243, LVT),
    (53273, 53299, LVT), (53329, 53355, LVT), (53385, 53411, LVT), (53441, 53467, LVT), (53497, 53523, LVT),
    (53553, 53579, LVT), (53609, 53635, LVT), (53665, 53691, LVT), (53721, 53747, LVT), (53777, 53803, LVT),
    (53833, 53859, LVT), (53889, 53915, LVT), (53945, 53971, LVT), (54001, 54027, LVT), (54057, 54083, LVT),
    (54113, 54139, LVT), (54169, 54195, LVT), (54225, 54251, LVT), (54281, 54307, LVT), (54337, 54363, LVT),
    (54393, 54419, LVT), (54449, 54475, LVT), (54505, 54531, LVT), (54561, 54587, LVT), (54617, 54643, LVT),
    (54673, 54699, LVT), (54729, 54755, LVT), (54785, 54811, LVT), (54841, 54867, LVT), (54897, 54923, LVT),
    (54953, 54979, LVT), (55009, 55035, LVT), (55065, 55091, LVT), (55121, 55147, LVT), (55177, 55203, LVT),
    (65024, 65039, Extend), (65520, 65528, Control), (66422, 66426, Extend), (68152, 68154, Extend),
    (69291, 69292, Extend), (69633, 69633, Extend), (69747, 69748, Extend), (69811, 69814, Extend),
    (69826, 69826, Extend), (69932, 69932, SpacingMark), (70016, 70017, Extend), (70079, 70080, SpacingMark),
    (70095, 70095, Extend), (70196, 70196, Extend), (70367, 70367, Extend), (70402, 70403, SpacingMark),
    (70464, 70464, Extend), (70487, 70487, Extend), (70709, 70711, SpacingMark), (70725, 70725, SpacingMark),
    (70833, 70834, SpacingMark), (70843, 70844, SpacingMark), (70849, 70849, SpacingMark), (71090, 71093, Extend),
    (71103, 71104, Extend), (71227, 71228, SpacingMark), (71339, 71339, Extend), (71344, 71349, Extend),
    (71458, 71461, Extend), (71727, 71735, Extend), (71985, 71989, SpacingMark), (71998, 71998, Extend),
    (72002, 72002, SpacingMark), (72154, 72155, Extend), (72193, 72202, Extend), (72251, 72254, Extend),
    (72281, 72283, Extend), (72344, 72345, Extend), (72766, 72766, SpacingMark), (72874, 72880, Extend),
    (72885, 72886, Extend), (73023, 73029, Extend), (73104, 73105, Extend), (73111, 73111, Extend),
    (92912, 92916, Extend), (94095, 94098, Extend), (113824, 113827, Control), (119142, 119142, SpacingMark),
    (119155, 119162, Control), (119362, 119364, Extend), (121476, 121476, Extend), (122888, 122904, Extend),
    (123184, 123190, Extend), (125252, 125258, Extend), (127183, 127183, ExtendedPictographic),
    (127340, 127343, ExtendedPictographic), (127377, 127386, ExtendedPictographic),
    (127491, 127503, ExtendedPictographic), (127548, 127551, ExtendedPictographic),
    (127744, 127756, ExtendedPictographic), (127761, 127761, ExtendedPictographic),
    (127769, 127769, ExtendedPictographic), (127773, 127774, ExtendedPictographic),
    (127780, 127788, ExtendedPictographic), (127796, 127797, ExtendedPictographic),
    (127820, 127823, ExtendedPictographic), (127869, 127869, ExtendedPictographic),
    (127894, 127895, ExtendedPictographic), (127902, 127903, ExtendedPictographic),
    (127943, 127943, ExtendedPictographic), (127947, 127950, ExtendedPictographic),
    (127972, 127972, ExtendedPictographic), (127988, 127988, ExtendedPictographic),
    (127992, 127994, ExtendedPictographic), (128009, 128011, ExtendedPictographic),
    (128019, 128019, ExtendedPictographic), (128023, 128041, ExtendedPictographic),
    (128064, 128064, ExtendedPictographic), (128102, 128107, ExtendedPictographic),
    (128174, 128181, ExtendedPictographic), (128238, 128238, ExtendedPictographic),
    (128246, 128247, ExtendedPictographic), (128254, 128254, ExtendedPictographic),
    (128264, 128264, ExtendedPictographic), (128278, 128299, ExtendedPictographic),
    (128329, 128330, ExtendedPictographic), (128348, 128359, ExtendedPictographic),
    (128371, 128377, ExtendedPictographic), (128392, 128393, ExtendedPictographic),
    (128401, 128404, ExtendedPictographic), (128421, 128421, ExtendedPictographic),
    (128433, 128434, ExtendedPictographic), (128450, 128452, ExtendedPictographic),
    (128476, 128478, ExtendedPictographic), (128483, 128483, ExtendedPictographic),
    (128495, 128495, ExtendedPictographic), (128506, 128506, ExtendedPictographic),
    (128519, 128520, ExtendedPictographic), (128528, 128528, ExtendedPictographic),
    (128534, 128534, ExtendedPictographic), (128538, 128538, ExtendedPictographic),
    (128540, 128542, ExtendedPictographic), (128544, 128549, ExtendedPictographic),
    (128552, 128555, ExtendedPictographic), (128557, 128557, ExtendedPictographic),
    (128560, 128563, ExtendedPictographic), (128565, 128565, ExtendedPictographic),
    (128567, 128576, ExtendedPictographic), (128581, 128591, ExtendedPictographic),
    (128641, 128642, ExtendedPictographic), (128646, 128646, ExtendedPictographic),
    (128648, 128648, ExtendedPictographic), (128650, 128651, ExtendedPictographic),
    (128653, 128653, ExtendedPictographic), (128655, 128655, ExtendedPictographic),
    (128657, 128659, ExtendedPictographic), (128661, 128661, ExtendedPictographic),
    (128663, 128663, ExtendedPictographic), (128665, 128666, ExtendedPictographic),
    (128674, 128674, ExtendedPictographic), (128676, 128677, ExtendedPictographic),
    (128679, 128685, ExtendedPictographic), (128690, 128690, ExtendedPictographic),
    (128694, 128694, ExtendedPictographic), (128697, 128702, ExtendedPictographic),
    (128704, 128704, ExtendedPictographic), (128710, 128714, ExtendedPictographic),
    (128716, 128716, ExtendedPictographic), (128720, 128720, ExtendedPictographic),
    (128723, 128724, ExtendedPictographic), (128726, 128727, ExtendedPictographic),
    (128733, 128735, ExtendedPictographic), (128742, 128744, ExtendedPictographic),
    (128746, 128746, ExtendedPictographic), (128749, 128751, ExtendedPictographic),
    (128753, 128754, ExtendedPictographic), (128756, 128758, ExtendedPictographic),
    (128761, 128761, ExtendedPictographic), (128763, 128764, ExtendedPictographic),
    (128884, 128895, ExtendedPictographic), (128992, 129003, ExtendedPictographic),
    (129008, 129008, ExtendedPictographic), (129036, 129039, ExtendedPictographic),
    (129114, 129119, ExtendedPictographic), (129198, 129279, ExtendedPictographic),
    (129293, 129295, ExtendedPictographic), (129305, 129310, ExtendedPictographic),
    (129312, 129319, ExtendedPictographic), (129328, 129328, ExtendedPictographic),
    (129331, 129338, ExtendedPictographic), (129343, 129343, ExtendedPictographic),
    (129351, 129355, ExtendedPictographic), (129357, 129359, ExtendedPictographic),
    (129375, 129387, ExtendedPictographic), (129393, 129393, ExtendedPictographic),
    (129395, 129398, ExtendedPictographic), (129401, 129401, ExtendedPictographic),
    (129403, 129403, ExtendedPictographic), (129408, 129412, ExtendedPictographic),
    (129426, 129431, ExtendedPictographic), (129443, 129444, ExtendedPictographic),
    (129451, 129453, ExtendedPictographic), (129456, 129465, ExtendedPictographic),
    (129472, 129472, ExtendedPictographic), (129475, 129482, ExtendedPictographic),
    (129484, 129484, ExtendedPictographic), (129488, 129510, ExtendedPictographic),
    (129536, 129647, ExtendedPictographic), (129652, 129652, ExtendedPictographic),
    (129656, 129658, ExtendedPictographic), (129661, 129663, ExtendedPictographic),
    (129667, 129670, ExtendedPictographic), (129680, 129685, ExtendedPictographic),
    (129705, 129708, ExtendedPictographic), (129712, 129718, ExtendedPictographic),
    (129723, 129727, ExtendedPictographic), (129731, 129733, ExtendedPictographic),
    (129744, 129750, ExtendedPictographic), (129754, 129759, ExtendedPictographic),
    (129768, 129775, ExtendedPictographic), (129783, 129791, ExtendedPictographic), (917504, 917504, Control),
    (917506, 917535, Control), (917632, 917759, Control), (918000, 921599, Control), (0, 9, Control),
    (11, 12, Control), (14, 31, Control), (169, 169, ExtendedPictographic), (174, 174, ExtendedPictographic),
    (1155, 1159, Extend), (1425, 1469, Extend), (1473, 1474, Extend), (1479, 1479, Extend), (1552, 1562, Extend),
    (1611, 1631, Extend), (1750, 1756, Extend), (1759, 1764, Extend), (1770, 1773, Extend), (1809, 1809, Extend),
    (1958, 1968, Extend), (2045, 2045, Extend), (2075, 2083, Extend), (2089, 2093, Extend), (2192, 2193, Prepend),
    (2250, 2273, Extend), (2275, 2306, Extend), (2362, 2362, Extend), (2364, 2364, Extend), (2369, 2376, Extend),
    (2381, 2381, Extend), (2385, 2391, Extend), (2433, 2433, Extend), (2492, 2492, Extend), (2495, 2496, SpacingMark),
    (2503, 2504, SpacingMark), (2509, 2509, Extend), (2530, 2531, Extend), (2561, 2562, Extend), (2620, 2620, Extend),
    (2625, 2626, Extend), (2635, 2637, Extend), (2672, 2673, Extend), (2689, 2690, Extend), (2748, 2748, Extend),
    (2753, 2757, Extend), (2761, 2761, SpacingMark), (2765, 2765, Extend), (2810, 2815, Extend),
    (2818, 2819, SpacingMark), (2878, 2878, Extend), (2880, 2880, SpacingMark), (2887, 2888, SpacingMark),
    (2893, 2893, Extend), (2903, 2903, Extend), (2946, 2946, Extend), (3007, 3007, SpacingMark),
    (3009, 3010, SpacingMark), (3018, 3020, SpacingMark), (3031, 3031, Extend), (3073, 3075, SpacingMark),
    (3132, 3132, Extend), (3137, 3140, SpacingMark), (3146, 3149, Extend), (3170, 3171, Extend),
    (3202, 3203, SpacingMark), (3262, 3262, SpacingMark), (3264, 3265, SpacingMark), (3267, 3268, SpacingMark),
    (3271, 3272, SpacingMark), (3276, 3277, Extend), (3298, 3299, Extend), (3330, 3331, SpacingMark),
    (3390, 3390, Extend), (3393, 3396, Extend), (3402, 3404, SpacingMark), (3406, 3406, Prepend), (3426, 3427, Extend),
    (3458, 3459, SpacingMark), (3535, 3535, Extend), (3538, 3540, Extend), (3544, 3550, SpacingMark),
    (3570, 3571, SpacingMark), (3635, 3635, SpacingMark), (3655, 3662, Extend), (3763, 3763, SpacingMark),
    (3784, 3789, Extend), (3893, 3893, Extend), (3897, 3897, Extend), (3953, 3966, Extend), (3968, 3972, Extend),
    (3981, 3991, Extend), (4038, 4038, Extend), (4145, 4145, SpacingMark), (4153, 4154, Extend), (4157, 4158, Extend),
    (4184, 4185, Extend), (4209, 4212, Extend), (4228, 4228, SpacingMark), (4237, 4237, Extend), (4352, 4447, L),
    (4520, 4607, T), (5906, 5908, Extend), (5938, 5939, Extend), (5970, 5971, Extend), (6068, 6069, Extend),
    (6071, 6077, Extend), (6086, 6086, Extend), (6089, 6099, Extend), (6155, 6157, Extend), (6159, 6159, Extend),
    (6313, 6313, Extend), (6435, 6438, SpacingMark), (6441, 6443, SpacingMark), (6450, 6450, Extend),
    (6457, 6459, Extend), (6681, 6682, SpacingMark), (6741, 6741, SpacingMark), (6743, 6743, SpacingMark),
    (6752, 6752, Extend), (6757, 6764, Extend), (6771, 6780, Extend), (6832, 6845, Extend), (6847, 6862, Extend),
    (6916, 6916, SpacingMark), (6965, 6965, Extend), (6971, 6971, SpacingMark), (6973, 6977, SpacingMark),
    (6979, 6980, SpacingMark), (7040, 7041, Extend), (7073, 7073, SpacingMark), (7078, 7079, SpacingMark),
    (7082, 7082, SpacingMark), (7142, 7142, Extend), (7144, 7145, Extend), (7149, 7149, Extend), (7151, 7153, Extend),
    (7204, 7211, SpacingMark), (7220, 7221, SpacingMark), (7376, 7378, Extend), (7393, 7393, SpacingMark),
    (7405, 7405, Extend), (7415, 7415, SpacingMark), (7616, 7679, Extend), (8204, 8204, Extend), (8206, 8207, Control),
    (8233, 8233, Control), (8252, 8252, ExtendedPictographic), (8288, 8292, Control), (8294, 8303, Control),
    (8413, 8416, Extend), (8418, 8420, Extend), (8482, 8482, ExtendedPictographic), (8596, 8601, ExtendedPictographic),
    (8986, 8987, ExtendedPictographic), (9096, 9096, ExtendedPictographic), (9193, 9196, ExtendedPictographic),
    (9199, 9199, ExtendedPictographic), (9201, 9202, ExtendedPictographic), (9208, 9210, ExtendedPictographic),
    (9642, 9643, ExtendedPictographic), (9664, 9664, ExtendedPictographic), (9728, 9729, ExtendedPictographic),
    (9732, 9732, ExtendedPictographic), (9735, 9741, ExtendedPictographic), (9743, 9744, ExtendedPictographic),
    (9746, 9746, ExtendedPictographic), (9750, 9751, ExtendedPictographic), (9753, 9756, ExtendedPictographic),
    (9758, 9759, ExtendedPictographic), (9761, 9761, ExtendedPictographic), (9764, 9765, ExtendedPictographic),
    (9767, 9769, ExtendedPictographic), (9771, 9773, ExtendedPictographic), (9775, 9775, ExtendedPictographic),
    (9784, 9785, ExtendedPictographic), (9787, 9791, ExtendedPictographic), (9793, 9793, ExtendedPictographic),
    (9795, 9799, ExtendedPictographic), (9812, 9822, ExtendedPictographic), (9824, 9824, ExtendedPictographic),
    (9827, 9827, ExtendedPictographic), (9829, 9830, ExtendedPictographic), (9832, 9832, ExtendedPictographic),
    (9851, 9851, ExtendedPictographic), (9854, 9854, ExtendedPictographic), (9856, 9861, ExtendedPictographic),
    (9874, 9874, ExtendedPictographic), (9876, 9876, ExtendedPictographic), (9878, 9879, ExtendedPictographic),
    (9881, 9881, ExtendedPictographic), (9883, 9884, ExtendedPictographic), (9888, 9889, ExtendedPictographic),
    (9895, 9895, ExtendedPictographic), (9898, 9899, ExtendedPictographic), (9904, 9905, ExtendedPictographic),
    (9917, 9918, ExtendedPictographic), (9924, 9925, ExtendedPictographic), (9928, 9928, ExtendedPictographic),
    (9934, 9934, ExtendedPictographic), (9936, 9936, ExtendedPictographic), (9938, 9938, ExtendedPictographic),
    (9940, 9940, ExtendedPictographic), (9961, 9961, ExtendedPictographic), (9963, 9967, ExtendedPictographic),
    (9970, 9971, ExtendedPictographic), (9973, 9973, ExtendedPictographic), (9975, 9977, ExtendedPictographic),
    (9979, 9980, ExtendedPictographic), (9982, 9985, ExtendedPictographic), (9987, 9988, ExtendedPictographic),
    (9992, 9996, ExtendedPictographic), (9998, 9998, ExtendedPictographic), (10000, 10001, ExtendedPictographic),
    (10004, 10004, ExtendedPictographic), (10013, 10013, ExtendedPictographic), (10024, 10024, ExtendedPictographic),
    (10052, 10052, ExtendedPictographic), (10060, 10060, ExtendedPictographic), (10067, 10069, ExtendedPictographic),
    (10083, 10083, ExtendedPictographic), (10085, 10087, ExtendedPictographic), (10145, 10145, ExtendedPictographic),
    (10175, 10175, ExtendedPictographic), (11013, 11015, ExtendedPictographic), (11088, 11088, ExtendedPictographic),
    (11503, 11505, Extend), (11744, 11775, Extend), (12334, 12335, Extend), (12349, 12349, ExtendedPictographic),
    (12951, 12951, ExtendedPictographic), (42607, 42607, Extend), (42612, 42621, Extend), (42736, 42737, Extend),
    (43014, 43014, Extend), (43043, 43044, SpacingMark), (43047, 43047, SpacingMark), (43136, 43137, SpacingMark),
    (43204, 43205, Extend), (43263, 43263, Extend), (43335, 43345, Extend), (43360, 43388, L),
    (43395, 43395, SpacingMark), (43444, 43445, SpacingMark), (43450, 43451, SpacingMark), (43454, 43456, SpacingMark),
    (43561, 43566, Extend), (43569, 43570, Extend), (43573, 43574, Extend), (43596, 43596, Extend),
    (43644, 43644, Extend), (43698, 43700, Extend), (43710, 43711, Extend), (43755, 43755, SpacingMark),
    (43758, 43759, SpacingMark), (43766, 43766, Extend), (44005, 44005, Extend), (44008, 44008, Extend),
    (44012, 44012, SpacingMark), (44032, 44032, LV), (44060, 44060, LV), (44088, 44088, LV), (44116, 44116, LV),
    (44144, 44144, LV), (44172, 44172, LV), (44200, 44200, LV), (44228, 44228, LV), (44256, 44256, LV),
    (44284, 44284, LV), (44312, 44312, LV), (44340, 44340, LV), (44368, 44368, LV), (44396, 44396, LV),
    (44424, 44424, LV), (44452, 44452, LV), (44480, 44480, LV), (44508, 44508, LV), (44536, 44536, LV),
    (44564, 44564, LV), (44592, 44592, LV), (44620, 44620, LV), (44648, 44648, LV), (44676, 44676, LV),
    (44704, 44704, LV), (44732, 44732, LV), (44760, 44760, LV), (44788, 44788, LV), (44816, 44816, LV),
    (44844, 44844, LV), (44872, 44872, LV), (44900, 44900, LV), (44928, 44928, LV), (44956, 44956, LV),
    (44984, 44984, LV), (45012, 45012, LV), (45040, 45040, LV), (45068, 45068, LV), (45096, 45096, LV),
    (45124, 45124, LV), (45152, 45152, LV), (45180, 45180, LV), (45208, 45208, LV), (45236, 45236, LV),
    (45264, 45264, LV), (45292, 45292, LV), (45320, 45320, LV), (45348, 45348, LV), (45376, 45376, LV),
    (45404, 45404, LV), (45432, 45432, LV), (45460, 45460, LV), (45488, 45488, LV), (45516, 45516, LV),
    (45544, 45544, LV), (45572, 45572, LV), (45600, 45600, LV), (45628, 45628, LV), (45656, 45656, LV),
    (45684, 45684, LV), (45712, 45712, LV), (45740, 45740, LV), (45768, 45768, LV), (45796, 45796, LV),
    (45824, 45824, LV), (45852, 45852, LV), (45880, 45880, LV), (45908, 45908, LV), (45936, 45936, LV),
    (45964, 45964, LV), (45992, 45992, LV), (46020, 46020, LV), (46048, 46048, LV), (46076, 46076, LV),
    (46104, 46104, LV), (46132, 46132, LV), (46160, 46160, LV), (46188, 46188, LV), (46216, 46216, LV),
    (46244, 46244, LV), (46272, 46272, LV), (46300, 46300, LV), (46328, 46328, LV), (46356, 46356, LV),
    (46384, 46384, LV), (46412, 46412, LV), (46440, 46440, LV), (46468, 46468, LV), (46496, 46496, LV),
    (46524, 46524, LV), (46552, 46552, LV), (46580, 46580, LV), (46608, 46608, LV), (46636, 46636, LV),
    (46664, 46664, LV), (46692, 46692, LV), (46720, 46720, LV), (46748, 46748, LV), (46776, 46776, LV),
    (46804, 46804, LV), (46832, 46832, LV), (46860, 46860, LV), (46888, 46888, LV), (46916, 46916, LV),
    (46944, 46944, LV), (46972, 46972, LV), (47000, 47000, LV), (47028, 47028, LV), (47056, 47056, LV),
    (47084, 47084, LV), (47112, 47112, LV), (47140, 47140, LV), (47168, 47168, LV), (47196, 47196, LV),
    (47224, 47224, LV), (47252, 47252, LV), (47280, 47280, LV), (47308, 47308, LV), (47336, 47336, LV),
    (47364, 47364, LV), (47392, 47392, LV), (47420, 47420, LV), (47448, 47448, LV), (47476, 47476, LV),
    (47504, 47504, LV), (47532, 47532, LV), (47560, 47560, LV), (47588, 47588, LV), (47616, 47616, LV),
    (47644, 47644, LV), (47672, 47672, LV), (47700, 47700, LV), (47728, 47728, LV), (47756, 47756, LV),
    (47784, 47784, LV), (47812, 47812, LV), (47840, 47840, LV), (47868, 47868, LV), (47896, 47896, LV),
    (47924, 47924, LV), (47952, 47952, LV), (47980, 47980, LV), (48008, 48008, LV), (48036, 48036, LV),
    (48064, 48064, LV), (48092, 48092, LV), (48120, 48120, LV), (48148, 48148, LV), (48176, 48176, LV),
    (48204, 48204, LV), (48232, 48232, LV), (48260, 48260, LV), (48288, 48288, LV), (48316, 48316, LV),
    (48344, 48344, LV), (48372, 48372, LV), (48400, 48400, LV), (48428, 48428, LV), (48456, 48456, LV),
    (48484, 48484, LV), (48512, 48512, LV), (48540, 48540, LV), (48568, 48568, LV), (48596, 48596, LV),
    (48624, 48624, LV), (48652, 48652, LV), (48680, 48680, LV), (48708, 48708, LV), (48736, 48736, LV),
    (48764, 48764, LV), (48792, 48792, LV), (48820, 48820, LV), (48848, 48848, LV), (48876, 48876, LV),
    (48904, 48904, LV), (48932, 48932, LV), (48960, 48960, LV), (48988, 48988, LV), (49016, 49016, LV),
    (49044, 49044, LV), (49072, 49072, LV), (49100, 49100, LV), (49128, 49128, LV), (49156, 49156, LV),
    (49184, 49184, LV), (49212, 49212, LV), (49240, 49240, LV), (49268, 49268, LV), (49296, 49296, LV),
    (49324, 49324, LV), (49352, 49352, LV), (49380, 49380, LV), (49408, 49408, LV), (49436, 49436, LV),
    (49464, 49464, LV), (49492, 49492, LV), (49520, 49520, LV), (49548, 49548, LV), (49576, 49576, LV),
    (49604, 49604, LV), (49632, 49632, LV), (49660, 49660, LV), (49688, 49688, LV), (49716, 49716, LV),
    (49744, 49744, LV), (49772, 49772, LV), (49800, 49800, LV), (49828, 49828, LV), (49856, 49856, LV),
    (49884, 49884, LV), (49912, 49912, LV), (49940, 49940, LV), (49968, 49968, LV), (49996, 49996, LV),
    (50024, 50024, LV), (50052, 50052, LV), (50080, 50080, LV), (50108, 50108, LV), (50136, 50136, LV),
    (50164, 50164, LV), (50192, 50192, LV), (50220, 50220, LV), (50248, 50248, LV), (50276, 50276, LV),
    (50304, 50304, LV), (50332, 50332, LV), (50360, 50360, LV), (50388, 50388, LV), (50416, 50416, LV),
    (50444, 50444, LV), (50472, 50472, LV), (50500, 50500, LV), (50528, 50528, LV), (50556, 50556, LV),
    (50584, 50584, LV), (50612, 50612, LV), (50640, 50640, LV), (50668, 50668, LV), (50696, 50696, LV),
    (50724, 50724, LV), (50752, 50752, LV), (50780, 50780, LV), (50808, 50808, LV), (50836, 50836, LV),
    (50864, 50864, LV), (50892, 50892, LV), (50920, 50920, LV), (50948, 50948, LV), (50976, 50976, LV),
    (51004, 51004, LV), (51032, 51032, LV), (51060, 51060, LV), (51088, 51088, LV), (51116, 51116, LV),
    (51144, 51144, LV), (51172, 51172, LV), (51200, 51200, LV), (51228, 51228, LV), (51256, 51256, LV),
    (51284, 51284, LV), (51312, 51312, LV), (51340, 51340, LV), (51368, 51368, LV), (51396, 51396, LV),
    (51424, 51424, LV), (51452, 51452, LV), (51480, 51480, LV), (51508, 51508, LV), (51536, 51536, LV),
    (51564, 51564, LV), (51592, 51592, LV), (51620, 51620, LV), (51648, 51648, LV), (51676, 51676, LV),
    (51704, 51704, LV), (51732, 51732, LV), (51760, 51760, LV), (51788, 51788, LV), (51816, 51816, LV),
    (51844, 51844, LV), (51872, 51872, LV), (51900, 51900, LV), (51928, 51928, LV), (51956, 51956, LV),
    (51984, 51984, LV), (52012, 52012, LV), (52040, 52040, LV), (52068, 52068, LV), (52096, 52096, LV),
    (52124, 52124, LV), (52152, 52152, LV), (52180, 52180, LV), (52208, 52208, LV), (52236, 52236, LV),
    (52264, 52264, LV), (52292, 52292, LV), (52320, 52320, LV), (52348, 52348, LV), (52376, 52376, LV),
    (52404, 52404, LV), (52432, 52432, LV), (52460, 52460, LV), (52488, 52488, LV), (52516, 52516, LV),
    (52544, 52544, LV), (52572, 52572, LV), (52600, 52600, LV), (52628, 52628, LV), (52656, 52656, LV),
    (52684, 52684, LV), (52712, 52712, LV), (52740, 52740, LV), (52768, 52768, LV), (52796, 52796, LV),
    (52824, 52824, LV), (52852, 52852, LV), (52880, 52880, LV), (52908, 52908, LV), (52936, 52936, LV),
    (52964, 52964, LV), (52992, 52992, LV), (53020, 53020, LV), (53048, 53048, LV), (53076, 53076, LV),
    (53104, 53104, LV), (53132, 53132, LV), (53160, 53160, LV), (53188, 53188, LV), (53216, 53216, LV),
    (53244, 53244, LV), (53272, 53272, LV), (53300, 53300, LV), (53328, 53328, LV), (53356, 53356, LV),
    (53384, 53384, LV), (53412, 53412, LV), (53440, 53440, LV), (53468, 53468, LV), (53496, 53496, LV),
    (53524, 53524, LV), (53552, 53552, LV), (53580, 53580, LV), (53608, 53608, LV), (53636, 53636, LV),
    (53664, 53664, LV), (53692, 53692, LV), (53720, 53720, LV), (53748, 53748, LV), (53776, 53776, LV),
    (53804, 53804, LV), (53832, 53832, LV), (53860, 53860, LV), (53888, 53888, LV), (53916, 53916, LV),
    (53944, 53944, LV), (53972, 53972, LV), (54000, 54000, LV), (54028, 54028, LV), (54056, 54056, LV),
    (54084, 54084, LV), (54112, 54112, LV), (54140, 54140, LV), (54168, 54168, LV), (54196, 54196, LV),
    (54224, 54224, LV), (54252, 54252, LV), (54280, 54280, LV), (54308, 54308, LV), (54336, 54336, LV),
    (54364, 54364, LV), (54392, 54392, LV), (54420, 54420, LV), (54448, 54448, LV), (54476, 54476, LV),
    (54504, 54504, LV), (54532, 54532, LV), (54560, 54560, LV), (54588, 54588, LV), (54616, 54616, LV),
    (54644, 54644, LV), (54672, 54672, LV), (54700, 54700, LV), (54728, 54728, LV), (54756, 54756, LV),
    (54784, 54784, LV), (54812, 54812, LV), (54840, 54840, LV), (54868, 54868, LV), (54896, 54896, LV),
    (54924, 54924, LV), (54952, 54952, LV), (54980, 54980, LV), (55008, 55008, LV), (55036, 55036, LV),
    (55064, 55064, LV), (55092, 55092, LV), (55120, 55120, LV), (55148, 55148, LV), (55176, 55176, LV),
    (55216, 55238, V), (64286, 64286, Extend), (65056, 65071, Extend), (65438, 65439, Extend), (65529, 65531, Control),
    (66272, 66272, Extend), (68097, 68099, Extend), (68108, 68111, Extend), (68159, 68159, Extend),
    (68900, 68903, Extend), (69446, 69456, Extend), (69632, 69632, SpacingMark), (69634, 69634, SpacingMark),
    (69744, 69744, Extend), (69759, 69761, Extend), (69808, 69810, SpacingMark), (69815, 69816, SpacingMark),
    (69821, 69821, Prepend), (69837, 69837, Prepend), (69927, 69931, Extend), (69933, 69940, Extend),
    (70003, 70003, Extend), (70018, 70018, SpacingMark), (70070, 70078, Extend), (70082, 70083, Prepend),
    (70094, 70094, SpacingMark), (70188, 70190, SpacingMark), (70194, 70195, SpacingMark), (70197, 70197, SpacingMark),
    (70206, 70206, Extend), (70368, 70370, SpacingMark), (70400, 70401, Extend), (70459, 70460, Extend),
    (70463, 70463, SpacingMark), (70465, 70468, SpacingMark), (70475, 70477, SpacingMark), (70498, 70499, SpacingMark),
    (70512, 70516, Extend), (70712, 70719, Extend), (70722, 70724, Extend), (70726, 70726, Extend),
    (70832, 70832, Extend), (70835, 70840, Extend), (70842, 70842, Extend), (70845, 70845, Extend),
    (70847, 70848, Extend), (70850, 70851, Extend), (71088, 71089, SpacingMark), (71096, 71099, SpacingMark),
    (71102, 71102, SpacingMark), (71132, 71133, Extend), (71219, 71226, Extend), (71229, 71229, Extend),
    (71231, 71232, Extend), (71340, 71340, SpacingMark), (71342, 71343, SpacingMark), (71350, 71350, SpacingMark),
    (71453, 71455, Extend), (71462, 71462, SpacingMark), (71724, 71726, SpacingMark), (71736, 71736, SpacingMark),
    (71984, 71984, Extend), (71991, 71992, SpacingMark), (71997, 71997, SpacingMark), (71999, 71999, Prepend),
    (72001, 72001, Prepend), (72003, 72003, Extend), (72148, 72151, Extend), (72156, 72159, SpacingMark),
    (72164, 72164, SpacingMark), (72243, 72248, Extend), (72250, 72250, Prepend), (72263, 72263, Extend),
    (72279, 72280, SpacingMark), (72324, 72329, Prepend), (72343, 72343, SpacingMark), (72751, 72751, SpacingMark),
    (72760, 72765, Extend), (72767, 72767, Extend), (72873, 72873, SpacingMark), (72881, 72881, SpacingMark),
    (72884, 72884, SpacingMark), (73009, 73014, Extend), (73020, 73021, Extend), (73030, 73030, Prepend),
    (73098, 73102, SpacingMark), (73107, 73108, SpacingMark), (73110, 73110, SpacingMark), (73459, 73460, Extend),
    (78896, 78904, Control), (92976, 92982, Extend), (94033, 94087, SpacingMark), (94180, 94180, Extend),
    (113821, 113822, Extend), (118528, 118573, Extend), (119141, 119141, Extend), (119143, 119145, Extend),
    (119150, 119154, Extend), (119163, 119170, Extend), (119210, 119213, Extend), (121344, 121398, Extend),
    (121461, 121461, Extend), (121499, 121503, Extend), (122880, 122886, Extend), (122907, 122913, Extend),
    (122918, 122922, Extend), (123566, 123566, Extend), (125136, 125142, Extend),
    (126976, 126979, ExtendedPictographic), (126981, 127182, ExtendedPictographic),
    (127184, 127231, ExtendedPictographic), (127279, 127279, ExtendedPictographic),
    (127344, 127345, ExtendedPictographic), (127374, 127374, ExtendedPictographic),
    (127405, 127461, ExtendedPictographic), (127489, 127490, ExtendedPictographic),
    (127514, 127514, ExtendedPictographic), (127538, 127546, ExtendedPictographic),
    (127561, 127567, ExtendedPictographic), (127570, 127743, ExtendedPictographic),
    (127757, 127758, ExtendedPictographic), (127760, 127760, ExtendedPictographic),
    (127762, 127762, ExtendedPictographic), (127766, 127768, ExtendedPictographic),
    (127770, 127770, ExtendedPictographic), (127772, 127772, ExtendedPictographic),
    (127775, 127776, ExtendedPictographic), (127778, 127779, ExtendedPictographic),
    (127789, 127791, ExtendedPictographic), (127794, 127795, ExtendedPictographic),
    (127798, 127798, ExtendedPictographic), (127819, 127819, ExtendedPictographic),
    (127824, 127824, ExtendedPictographic), (127868, 127868, ExtendedPictographic),
    (127870, 127871, ExtendedPictographic), (127892, 127893, ExtendedPictographic),
    (127896, 127896, ExtendedPictographic), (127900, 127901, ExtendedPictographic),
    (127904, 127940, ExtendedPictographic), (127942, 127942, ExtendedPictographic),
    (127944, 127944, ExtendedPictographic), (127946, 127946, ExtendedPictographic),
    (127951, 127955, ExtendedPictographic), (127968, 127971, ExtendedPictographic),
    (127973, 127984, ExtendedPictographic), (127987, 127987, ExtendedPictographic),
    (127989, 127989, ExtendedPictographic), (127991, 127991, ExtendedPictographic), (127995, 127999, Extend),
    (128008, 128008, ExtendedPictographic), (128012, 128014, ExtendedPictographic),
    (128017, 128018, ExtendedPictographic), (128020, 128020, ExtendedPictographic),
    (128022, 128022, ExtendedPictographic), (128042, 128042, ExtendedPictographic),
    (128063, 128063, ExtendedPictographic), (128065, 128065, ExtendedPictographic),
    (128101, 128101, ExtendedPictographic), (128108, 128109, ExtendedPictographic),
    (128173, 128173, ExtendedPictographic), (128182, 128183, ExtendedPictographic),
    (128236, 128237, ExtendedPictographic), (128239, 128239, ExtendedPictographic),
    (128245, 128245, ExtendedPictographic), (128248, 128248, ExtendedPictographic),
    (128253, 128253, ExtendedPictographic), (128255, 128258, ExtendedPictographic),
    (128260, 128263, ExtendedPictographic), (128265, 128265, ExtendedPictographic),
    (128277, 128277, ExtendedPictographic), (128300, 128301, ExtendedPictographic),
    (128326, 128328, ExtendedPictographic), (128331, 128334, ExtendedPictographic),
    (128336, 128347, ExtendedPictographic), (128360, 128366, ExtendedPictographic),
    (128369, 128370, ExtendedPictographic), (128378, 128378, ExtendedPictographic),
    (128391, 128391, ExtendedPictographic), (128394, 128397, ExtendedPictographic),
    (128400, 128400, ExtendedPictographic), (128405, 128406, ExtendedPictographic),
    (128420, 128420, ExtendedPictographic), (128422, 128423, ExtendedPictographic),
    (128425, 128432, ExtendedPictographic), (128435, 128443, ExtendedPictographic),
    (128445, 128449, ExtendedPictographic), (128453, 128464, ExtendedPictographic),
    (128468, 128475, ExtendedPictographic), (128479, 128480, ExtendedPictographic),
    (128482, 128482, ExtendedPictographic), (128484, 128487, ExtendedPictographic),
    (128489, 128494, ExtendedPictographic), (128496, 128498, ExtendedPictographic),
    (128500, 128505, ExtendedPictographic), (128507, 128511, ExtendedPictographic),
    (128513, 128518, ExtendedPictographic), (128521, 128525, ExtendedPictographic),
    (128527, 128527, ExtendedPictographic), (128529, 128529, ExtendedPictographic),
    (128533, 128533, ExtendedPictographic), (128535, 128535, ExtendedPictographic),
    (128537, 128537, ExtendedPictographic),
];

#[cfg(test)]
mod tests;
