//! Windows CP1361: Hangul bit fields and rearranged KS X 1001 rows.
//! Mapping reference: Unicode MAPPINGS/OBSOLETE/EASTASIA/KSC/JOHAB.TXT.

const VOWELS: [u16; 21] = [
    3, 4, 5, 6, 7, 10, 11, 12, 13, 14, 15, 18, 19, 20, 21, 22, 23, 26, 27, 28, 29,
];
const JAMO: [u16; 51] = [
    0x8841, 0x8C41, 0x8444, 0x9041, 0x8446, 0x8447, 0x9441, 0x9841, 0x9C41, 0x844A, 0x844B, 0x844C,
    0x844D, 0x844E, 0x844F, 0x8450, 0xA041, 0xA441, 0xA841, 0x8454, 0xAC41, 0xB041, 0xB441, 0xB841,
    0xBC41, 0xC041, 0xC441, 0xC841, 0xCC41, 0xD041, 0x8461, 0x8481, 0x84A1, 0x84C1, 0x84E1, 0x8541,
    0x8561, 0x8581, 0x85A1, 0x85C1, 0x85E1, 0x8641, 0x8661, 0x8681, 0x86A1, 0x86C1, 0x86E1, 0x8741,
    0x8761, 0x8781, 0x87A1,
];

fn decode_pair(lead: u8, trail: u8) -> Option<char> {
    if (0x84..=0xD3).contains(&lead) && matches!(trail, 0x41..=0x7E | 0x81..=0xFE) {
        let packed = u16::from_be_bytes([lead, trail]);
        if let Some(index) = JAMO.iter().position(|&code| code == packed) {
            return char::from_u32(0x3131 + index as u32);
        }
        let initial = (packed >> 10) & 31;
        let medial = (packed >> 5) & 31;
        let final_ = packed & 31;
        if (2..=20).contains(&initial) && matches!(final_, 1..=17 | 19..=29) {
            let vowel = VOWELS.iter().position(|&code| code == medial)? as u32;
            let final_ = if final_ > 18 { final_ - 2 } else { final_ - 1 } as u32;
            return char::from_u32(0xAC00 + ((initial as u32 - 2) * 21 + vowel) * 28 + final_);
        }
        return None;
    }
    if !matches!(lead, 0xD9..=0xDE | 0xE0..=0xF9)
        || !matches!(trail, 0x31..=0x7E | 0x91..=0xFE)
        || lead == 0xDA && (0xA1..=0xD3).contains(&trail)
    {
        return None;
    }
    // Each lead covers two 94-column KS rows; the trail skips 0x7F..0x90.
    let row = if lead < 0xE0 {
        2 * (lead as u16 - 0xD9)
    } else {
        2 * lead as u16 - 0x197
    };
    let column = trail as u16 - if trail < 0x91 { 0x31 } else { 0x43 };
    let bytes = [(row + column / 94 + 0xA1) as u8, (column % 94 + 0xA1) as u8];
    let (text, _, errors) = encoding_rs::EUC_KR.decode(&bytes);
    if errors {
        None
    } else {
        text.chars().next()
    }
}

pub(super) fn decode(bytes: &[u8]) -> (String, bool) {
    let mut text = String::with_capacity(bytes.len());
    let mut errors = false;
    let mut index = 0;
    while index < bytes.len() {
        let lead = bytes[index];
        if lead < 0x80 {
            text.push(lead as char);
            index += 1;
        } else if let Some(ch) = bytes
            .get(index + 1)
            .and_then(|&trail| decode_pair(lead, trail))
        {
            text.push(ch);
            index += 2;
        } else {
            text.push('\u{FFFD}');
            errors = true;
            index += 1;
        }
    }
    (text, errors)
}

pub(super) fn encode_char(ch: char) -> Option<([u8; 2], usize)> {
    let scalar = ch as u32;
    if ch.is_ascii() {
        return Some(([ch as u8, 0], 1));
    }
    let packed = if (0x3131..=0x3163).contains(&scalar) {
        JAMO[(scalar - 0x3131) as usize]
    } else if (0xAC00..=0xD7A3).contains(&scalar) {
        let syllable = scalar - 0xAC00;
        let initial = (syllable / (21 * 28)) as u16 + 2;
        let vowel = VOWELS[(syllable / 28 % 21) as usize];
        let final_ = (syllable % 28) as u16;
        let final_ = final_ + if final_ < 17 { 1 } else { 2 };
        0x8000 | initial << 10 | vowel << 5 | final_
    } else {
        let mut buffer = [0; 4];
        let (bytes, _, errors) = encoding_rs::EUC_KR.encode(ch.encode_utf8(&mut buffer));
        if errors || bytes.len() != 2 {
            return None;
        }
        let row = bytes[0] as u16;
        let column = bytes[1] as u16;
        if !matches!(row, 0xA1..=0xAC | 0xCA..=0xFD) || !(0xA1..=0xFE).contains(&column) {
            return None;
        }
        let row = row - 0xA1 + if row < 0xCA { 0x1B2 } else { 0x197 };
        let column = (row % 2) * 94 + column - 0xA1;
        let lead = (row / 2) as u8;
        let trail = (column + if column < 78 { 0x31 } else { 0x43 }) as u8;
        // Prevent UHC aliases or the removed compatibility-jamo row from
        // producing bytes that decode to another scalar.
        if decode_pair(lead, trail) != Some(ch) {
            return None;
        }
        u16::from_be_bytes([lead, trail])
    };
    Some((packed.to_be_bytes(), 2))
}
