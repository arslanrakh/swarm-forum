//! Text primitives shared by the version 1 key rules and the principal-key rules.
//!
//! Both rule sets are frozen once released (K2 for keys, CS-1 for principals), so nothing here may
//! change behaviour: a later normaliser version that needs a different primitive gets its own.

use unicode_normalization::UnicodeNormalization;

use crate::KeyError;
use crate::unicode17;

/// How many extra rounds [`fixed_point`] runs before it gives up. One round suffices for every input
/// the tests know; the others are a margin for Unicode corner cases (a lower-cased letter that
/// composes differently, whitespace uncovered at an edge).
const EXTRA_ROUNDS: usize = 3;

/// The longest scheme, in bytes.
const SCHEME_MAX: usize = 16;

/// NFC, then the surrounding whitespace trimmed.
pub(crate) fn clean(raw: &str) -> String {
    let nfc: String = raw.nfc().collect();
    nfc.trim().to_owned()
}

/// A kind of default-ignorable code point that the rules refuse. Each kind has its own refusal, so
/// that a client learns which character to remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Invisible {
    /// U+00AD soft hyphen.
    SoftHyphen,
    /// U+034F combining grapheme joiner.
    GraphemeJoiner,
    /// The 12 bidirectional controls (`Bidi_Control`): U+061C Arabic letter mark, U+200E and
    /// U+200F (the left-to-right and right-to-left marks), U+202A to U+202E (the embeddings and
    /// overrides and their terminator), and U+2066 to U+2069 (the isolates and their terminator).
    /// Each reorders the text around it when it is rendered (`team:\u{202E}stnemyaP` renders as
    /// `team:Payments`).
    BidiControl,
    /// The Hangul fillers: U+115F and U+1160 (the choseong and jungseong fillers), U+3164, and
    /// U+FFA0 (its halfwidth form). Each is a letter that renders as nothing.
    HangulFiller,
    /// U+17B4 and U+17B5, the Khmer inherent vowels, whose use Unicode discourages.
    KhmerVowel,
    /// U+180E Mongolian vowel separator.
    MongolianSeparator,
    /// The 9 zero-width format characters: U+200B to U+200D (zero width space, non-joiner, and
    /// joiner), U+2060 to U+2064 (word joiner and the invisible mathematical operators), and
    /// U+FEFF (zero width no-break space, the byte order mark).
    ZeroWidth,
    /// U+206A to U+206F, the deprecated format characters (symmetric swapping, Arabic form
    /// shaping, and digit shapes).
    Deprecated,
    /// U+1BCA0 to U+1BCA3, the shorthand format controls.
    Shorthand,
    /// U+1D173 to U+1D17A, the musical symbol format controls (beams, ties, slurs, and phrases).
    Musical,
    /// U+E0001 language tag and U+E0020 to U+E007F, the tag characters (which also spell the
    /// subdivision flags after U+1F3F4).
    Tag,
}

impl Invisible {
    /// The refusal of a key that holds a character of this kind.
    const fn refusal(self) -> &'static str {
        match self {
            Invisible::SoftHyphen => "the key holds a soft hyphen (U+00AD)",
            Invisible::GraphemeJoiner => "the key holds a combining grapheme joiner (U+034F)",
            Invisible::BidiControl => {
                "the key holds a bidirectional control character (U+061C, U+200E, U+200F, \
                 U+202A to U+202E, or U+2066 to U+2069)"
            }
            Invisible::HangulFiller => {
                "the key holds a Hangul filler (U+115F, U+1160, U+3164, or U+FFA0)"
            }
            Invisible::KhmerVowel => "the key holds a Khmer inherent vowel (U+17B4 or U+17B5)",
            Invisible::MongolianSeparator => "the key holds a Mongolian vowel separator (U+180E)",
            Invisible::ZeroWidth => {
                "the key holds a zero-width format character (U+200B to U+200D, U+2060 to \
                 U+2064, or U+FEFF)"
            }
            Invisible::Deprecated => {
                "the key holds a deprecated format character (U+206A to U+206F)"
            }
            Invisible::Shorthand => "the key holds a shorthand format control (U+1BCA0 to U+1BCA3)",
            Invisible::Musical => {
                "the key holds a musical symbol format control (U+1D173 to U+1D17A)"
            }
            Invisible::Tag => "the key holds a tag character (U+E0001, or U+E0020 to U+E007F)",
        }
    }
}

/// The refused default-ignorable code points: every code point that Unicode 17.0 assigns and marks
/// `Default_Ignorable_Code_Point`, except the variation selectors (the `Variation_Selector`
/// property: the Mongolian free variation selectors U+180B to U+180D and U+180F, U+FE00 to U+FE0F,
/// and U+E0100 to U+E01EF); 145 code points in 19 inclusive ranges, sorted and disjoint, each with
/// its kind.
///
/// A default-ignorable code point renders as nothing when a renderer does not support it, and most
/// of them render as nothing in any case, so each can make a key or a principal read as another.
/// The variation selectors are kept because they choose a glyph for the character before them
/// (U+FE0F asks for the emoji presentation, as in `\u{2764}\u{FE0F}`, and a Mongolian free
/// variation selector chooses the form of a Mongolian letter), so a key may need one. One that
/// chooses no glyph for the character before it shows nothing, so `team:alice\u{180B}` reads as
/// `team:alice` and is another principal: like a letter that looks like another letter, that way
/// for two keys to read alike stays open. The default-ignorable code points that Unicode 17.0
/// leaves unassigned (U+2065, U+FFF0 to U+FFF8, and U+E0000 to U+E0FFF outside the assigned ones)
/// are refused as unassigned ([`unicode17`]).
///
/// The table was derived from ICU4X's Unicode 17.0 data (`icu_properties` 2.3.0), and the unit
/// tests below derive it again from the property's ranges in Unicode 17.0's
/// `DerivedCoreProperties.txt` and [`unicode17`]'s table. None of its code points is `White_Space`,
/// a control, cased, canonically decomposable, or a mark with a non-zero combining class (the unit
/// tests repeat these checks), and no other character case-maps or normalises (NFC or NFD) into
/// one (checked against the same data), so trimming never removes one, a refusal does not depend
/// on a round of normalisation, and the table is frozen with the rules that use it.
const DEFAULT_IGNORABLE: [(char, char, Invisible); 19] = [
    ('\u{AD}', '\u{AD}', Invisible::SoftHyphen),
    ('\u{34F}', '\u{34F}', Invisible::GraphemeJoiner),
    ('\u{61C}', '\u{61C}', Invisible::BidiControl),
    ('\u{115F}', '\u{1160}', Invisible::HangulFiller),
    ('\u{17B4}', '\u{17B5}', Invisible::KhmerVowel),
    ('\u{180E}', '\u{180E}', Invisible::MongolianSeparator),
    ('\u{200B}', '\u{200D}', Invisible::ZeroWidth),
    ('\u{200E}', '\u{200F}', Invisible::BidiControl),
    ('\u{202A}', '\u{202E}', Invisible::BidiControl),
    ('\u{2060}', '\u{2064}', Invisible::ZeroWidth),
    ('\u{2066}', '\u{2069}', Invisible::BidiControl),
    ('\u{206A}', '\u{206F}', Invisible::Deprecated),
    ('\u{3164}', '\u{3164}', Invisible::HangulFiller),
    ('\u{FEFF}', '\u{FEFF}', Invisible::ZeroWidth),
    ('\u{FFA0}', '\u{FFA0}', Invisible::HangulFiller),
    ('\u{1BCA0}', '\u{1BCA3}', Invisible::Shorthand),
    ('\u{1D173}', '\u{1D17A}', Invisible::Musical),
    ('\u{E0001}', '\u{E0001}', Invisible::Tag),
    ('\u{E0020}', '\u{E007F}', Invisible::Tag),
];

/// The kind of `c` when it is a refused default-ignorable code point ([`DEFAULT_IGNORABLE`]).
fn invisible(c: char) -> Option<Invisible> {
    DEFAULT_IGNORABLE
        .iter()
        .find(|&&(first, last, _)| (first..=last).contains(&c))
        .map(|&(_, _, kind)| kind)
}

/// Refuses, anywhere in a cleaned key or principal key:
///
/// - a control character (`Cc`: U+0000 to U+001F and U+007F to U+009F);
/// - U+2028 line separator and U+2029 paragraph separator;
/// - a default-ignorable code point other than a variation selector ([`DEFAULT_IGNORABLE`]), each
///   kind with its own refusal;
/// - a code point that Unicode 17.0 leaves unassigned ([`unicode17`]).
///
/// The control characters and the two separators are whitespace where they are line breaks, so
/// cleaning trims them at the edges, as it trims any whitespace, and they are refused inside. The
/// default-ignorable code points are not whitespace, so one is refused at an edge as well.
///
/// A control character or a separator can forge a line in a rendered response, and a control
/// character can cut a key short for a C-string consumer; path steps refuse them for the same
/// reasons (D.2). A default-ignorable code point renders as nothing, or reorders the text around
/// it, so it can make a key read as another. Refusing them closes those ways, not every way:
/// look-alike letters are accepted. An unassigned code point could gain a case mapping or a
/// combining class in a later Unicode version, which would change the normal form of a stored key
/// ([`unicode17`]).
pub(crate) fn check_chars(cleaned: &str) -> Result<(), KeyError> {
    for c in cleaned.chars() {
        if matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}') {
            return Err(KeyError::BadValue("the key holds a control character"));
        }
        if matches!(c, '\u{2028}' | '\u{2029}') {
            return Err(KeyError::BadValue(
                "the key holds a line or paragraph separator (U+2028 or U+2029)",
            ));
        }
        if let Some(kind) = invisible(c) {
            return Err(KeyError::BadValue(kind.refusal()));
        }
        if unicode17::is_unassigned(c) {
            return Err(KeyError::BadValue(
                "the key holds a code point that Unicode 17 leaves unassigned",
            ));
        }
    }
    Ok(())
}

/// `value` without one pair of surrounding angle brackets and the whitespace inside them
/// (`<a@b>`, `< a@b >`), or `value` itself when it is not wrapped in a pair. Nothing inside the
/// pair, or a second pair inside the first (`<<a@b>>`, `<<a@b> >`), is refused: stripping only one
/// pair here and another at the next call would not be idempotent.
pub(crate) fn strip_angle_pair(value: &str) -> Result<&str, KeyError> {
    let Some(inner) = value.strip_prefix('<').and_then(|v| v.strip_suffix('>')) else {
        return Ok(value);
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return Err(KeyError::BadValue(
            "mail: nothing inside the angle brackets",
        ));
    }
    if inner.starts_with('<') && inner.ends_with('>') {
        return Err(KeyError::BadValue(
            "mail: more than one pair of angle brackets",
        ));
    }
    Ok(inner)
}

/// `[a-z][a-z0-9+.-]{0,15}`.
pub(crate) fn is_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && scheme.len() <= SCHEME_MAX
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'))
}

/// Splits `<scheme>:<value>` at the first `:`, with the scheme lower-cased (ASCII) and checked.
pub(crate) fn split_scheme(key: &str) -> Result<(String, &str), KeyError> {
    let (scheme, value) = key.split_once(':').ok_or(KeyError::NoScheme)?;
    if scheme.is_empty() {
        return Err(KeyError::NoScheme);
    }
    let scheme = scheme.to_ascii_lowercase();
    if !is_scheme(&scheme) {
        return Err(KeyError::BadScheme);
    }
    Ok((scheme, value))
}

/// Applies `step` until its output no longer changes, and returns that output.
///
/// Every `Ok` result `k` therefore satisfies `step(k) == Ok(k)`, which makes the normaliser
/// idempotent by construction rather than by a proof about Unicode: lower-casing may in principle
/// produce text that NFC changes, and stripping a delimiter may uncover whitespace that trimming
/// removes. An input that has not settled after the extra rounds is rejected.
pub(crate) fn fixed_point(
    raw: &str,
    step: impl Fn(&str) -> Result<String, KeyError>,
) -> Result<String, KeyError> {
    let mut current = step(raw)?;
    for _ in 0..EXTRA_ROUNDS {
        let next = step(&current)?;
        if next == current {
            return Ok(current);
        }
        current = next;
    }
    Err(KeyError::BadValue(
        "the key does not settle under normalisation",
    ))
}

/// Whether `s` is a non-empty run of ASCII decimal digits.
pub(crate) fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `s` without its leading `prefix`, compared ignoring ASCII case.
pub(crate) fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        s.get(prefix.len()..)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_IGNORABLE, Invisible, check_chars, clean, invisible};
    use crate::KeyError;
    use crate::unicode17::is_unassigned;
    use std::collections::BTreeSet;
    use unicode_normalization::UnicodeNormalization;
    use unicode_normalization::char::canonical_combining_class;

    /// `Default_Ignorable_Code_Point` in Unicode 17.0's `DerivedCoreProperties.txt`, line by line:
    /// 4,174 code points, the unassigned ones and the variation selectors included.
    const DERIVED: [(u32, u32); 27] = [
        (0xAD, 0xAD),
        (0x34F, 0x34F),
        (0x61C, 0x61C),
        (0x115F, 0x1160),
        (0x17B4, 0x17B5),
        (0x180B, 0x180D),
        (0x180E, 0x180E),
        (0x180F, 0x180F),
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x2064),
        (0x2065, 0x2065),
        (0x2066, 0x206F),
        (0x3164, 0x3164),
        (0xFE00, 0xFE0F),
        (0xFEFF, 0xFEFF),
        (0xFFA0, 0xFFA0),
        (0xFFF0, 0xFFF8),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0000, 0xE0000),
        (0xE0001, 0xE0001),
        (0xE0002, 0xE001F),
        (0xE0020, 0xE007F),
        (0xE0080, 0xE00FF),
        (0xE0100, 0xE01EF),
        (0xE01F0, 0xE0FFF),
    ];

    /// The `Variation_Selector` property in Unicode 17.0's `PropList.txt`: 260 code points.
    fn is_variation_selector(c: char) -> bool {
        matches!(
            c,
            '\u{180B}'..='\u{180D}'
                | '\u{180F}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{E0100}'..='\u{E01EF}'
        )
    }

    fn members() -> impl Iterator<Item = (char, Invisible)> {
        DEFAULT_IGNORABLE
            .iter()
            .flat_map(|&(first, last, kind)| (first..=last).map(move |c| (c, kind)))
    }

    fn of_kind(kind: Invisible) -> Vec<char> {
        members()
            .filter(|&(_, k)| k == kind)
            .map(|(c, _)| c)
            .collect()
    }

    #[test]
    fn the_table_is_sorted_disjoint_and_holds_145_code_points() {
        assert!(
            DEFAULT_IGNORABLE
                .windows(2)
                .all(|pair| matches!(pair, [(_, last, _), (next, _, _)] if last < next))
        );
        assert!(
            DEFAULT_IGNORABLE
                .iter()
                .all(|&(first, last, _)| first <= last)
        );
        assert_eq!(members().count(), 145);
        for (kind, count) in [
            (Invisible::SoftHyphen, 1),
            (Invisible::GraphemeJoiner, 1),
            (Invisible::BidiControl, 12),
            (Invisible::HangulFiller, 4),
            (Invisible::KhmerVowel, 2),
            (Invisible::MongolianSeparator, 1),
            (Invisible::ZeroWidth, 9),
            (Invisible::Deprecated, 6),
            (Invisible::Shorthand, 4),
            (Invisible::Musical, 8),
            (Invisible::Tag, 97),
        ] {
            assert_eq!(of_kind(kind).len(), count, "{kind:?}");
        }
    }

    #[test]
    fn the_table_is_unicode_17s_assigned_default_ignorables_without_the_variation_selectors() {
        let derived: Vec<char> = DERIVED
            .iter()
            .flat_map(|&(first, last)| first..=last)
            .filter_map(char::from_u32)
            .collect();
        assert_eq!(derived.len(), 4_174);
        assert_eq!(derived.iter().filter(|&&c| is_unassigned(c)).count(), 3_769);
        assert_eq!(
            derived
                .iter()
                .filter(|&&c| is_variation_selector(c))
                .count(),
            260
        );
        let refused: Vec<char> = derived
            .into_iter()
            .filter(|&c| !is_unassigned(c) && !is_variation_selector(c))
            .collect();
        assert_eq!(members().map(|(c, _)| c).collect::<Vec<_>>(), refused);
    }

    #[test]
    fn the_kinds_hold_the_characters_they_name() {
        let bidi: Vec<char> = ['\u{61C}', '\u{200E}', '\u{200F}']
            .into_iter()
            .chain('\u{202A}'..='\u{202E}')
            .chain('\u{2066}'..='\u{2069}')
            .collect();
        assert_eq!(of_kind(Invisible::BidiControl), bidi);
        let zero_width: Vec<char> = ('\u{200B}'..='\u{200D}')
            .chain('\u{2060}'..='\u{2064}')
            .chain(['\u{FEFF}'])
            .collect();
        assert_eq!(of_kind(Invisible::ZeroWidth), zero_width);
        assert_eq!(
            of_kind(Invisible::HangulFiller),
            ['\u{115F}', '\u{1160}', '\u{3164}', '\u{FFA0}']
        );
        assert_eq!(of_kind(Invisible::MongolianSeparator), ['\u{180E}']);
        let tags: Vec<char> = ['\u{E0001}']
            .into_iter()
            .chain('\u{E0020}'..='\u{E007F}')
            .collect();
        assert_eq!(of_kind(Invisible::Tag), tags);
    }

    #[test]
    fn each_kind_has_its_own_refusal() {
        let mut texts = BTreeSet::new();
        for (c, kind) in members() {
            let cp = u32::from(c);
            assert_eq!(invisible(c), Some(kind), "U+{cp:04X}");
            assert_eq!(
                check_chars(&format!("a{c}b")),
                Err(KeyError::BadValue(kind.refusal())),
                "U+{cp:04X}"
            );
            texts.insert(kind.refusal());
        }
        assert_eq!(texts.len(), 11);
        // And none reads like the refusal of a control character, a separator, or an unassigned
        // code point.
        for (raw, why) in [
            ("a\u{1}b", "control character"),
            ("a\u{2028}b", "line or paragraph separator"),
            ("a\u{2029}b", "line or paragraph separator"),
            ("a\u{378}b", "unassigned"),
        ] {
            let result = check_chars(raw);
            assert!(
                matches!(
                    result,
                    Err(KeyError::BadValue(text)) if text.contains(why) && !texts.contains(text)
                ),
                "{raw:?} gave {result:?}"
            );
        }
    }

    #[test]
    fn the_separators_are_refused_inside_and_trimmed_at_the_edges() {
        for c in ['\u{2028}', '\u{2029}'] {
            assert!(c.is_whitespace());
            assert!(invisible(c).is_none());
            assert_eq!(clean(&format!("{c}x:y{c}")), "x:y");
            assert!(matches!(
                check_chars(&format!("x:a{c}b")),
                Err(KeyError::BadValue(why)) if why.contains("U+2028 or U+2029")
            ));
        }
    }

    #[test]
    fn the_variation_selectors_and_the_neighbours_of_the_table_are_not_refused_by_it() {
        let selectors: Vec<char> = ('\u{180B}'..='\u{180D}')
            .chain(['\u{180F}'])
            .chain('\u{FE00}'..='\u{FE0F}')
            .chain('\u{E0100}'..='\u{E01EF}')
            .collect();
        assert_eq!(selectors.len(), 260);
        for c in selectors {
            let cp = u32::from(c);
            assert!(is_variation_selector(c), "U+{cp:04X}");
            assert!(invisible(c).is_none(), "U+{cp:04X}");
            for raw in [
                format!("a{c}"),
                format!("{c}a"),
                format!("a{c}b"),
                c.to_string(),
            ] {
                assert_eq!(check_chars(&raw), Ok(()), "U+{cp:04X}");
            }
            // Kept as it is by every round: not whitespace, not cased, and a starter that NFC
            // neither composes nor moves.
            let alone = c.to_string();
            assert!(!c.is_whitespace(), "U+{cp:04X}");
            assert_eq!(clean(&alone), alone, "U+{cp:04X}");
            assert_eq!(c.to_lowercase().to_string(), alone, "U+{cp:04X}");
            assert_eq!(canonical_combining_class(c), 0, "U+{cp:04X}");
            assert_eq!(alone.nfd().collect::<String>(), alone, "U+{cp:04X}");
        }
        // Each one is either accepted or refused by another rule (whitespace inside a key is
        // accepted; U+2065, U+FEFE, U+E0000, U+E0002, and U+E0080 are unassigned).
        for c in [
            '\u{AC}',
            '\u{AE}',
            '\u{34E}',
            '\u{350}',
            '\u{600}',
            '\u{61B}',
            '\u{61D}',
            '\u{6DD}',
            '\u{115E}',
            '\u{1161}',
            '\u{17B3}',
            '\u{17B6}',
            '\u{180A}',
            '\u{1810}',
            '\u{200A}',
            '\u{2010}',
            '\u{2027}',
            '\u{202F}',
            '\u{205F}',
            '\u{2065}',
            '\u{2070}',
            '\u{3163}',
            '\u{3165}',
            '\u{FDFF}',
            '\u{FE10}',
            '\u{FEFE}',
            '\u{FF9F}',
            '\u{FFA1}',
            '\u{FFF9}',
            '\u{1BC9F}',
            '\u{1BCA4}',
            '\u{1D172}',
            '\u{1D17B}',
            '\u{E0000}',
            '\u{E0002}',
            '\u{E001F}',
            '\u{E0080}',
            ' ',
            'a',
        ] {
            let cp = u32::from(c);
            assert!(invisible(c).is_none(), "U+{cp:04X}");
            if !is_unassigned(c) {
                assert_eq!(check_chars(&format!("a{c}b")), Ok(()), "U+{cp:04X}");
            }
        }
    }

    #[test]
    fn no_refused_character_is_whitespace_or_changes_under_the_rules() {
        for (c, _) in members() {
            let cp = u32::from(c);
            let alone = c.to_string();
            // Not whitespace, so cleaning keeps it at an edge and the check refuses it there.
            assert!(!c.is_whitespace(), "U+{cp:04X}");
            assert_eq!(clean(&alone), alone, "U+{cp:04X}");
            // Assigned, not a control, and stable under case mapping and NFC, so the refusal does
            // not depend on the order of the checks or on a round of normalisation.
            assert!(!is_unassigned(c), "U+{cp:04X}");
            assert!(!c.is_control(), "U+{cp:04X}");
            assert_eq!(c.to_lowercase().to_string(), alone, "U+{cp:04X}");
            assert_eq!(c.to_uppercase().to_string(), alone, "U+{cp:04X}");
            assert_eq!(canonical_combining_class(c), 0, "U+{cp:04X}");
            assert_eq!(alone.nfd().collect::<String>(), alone, "U+{cp:04X}");
            for raw in [format!("x{c}"), format!("{c}x"), format!("a{c}b"), alone] {
                assert!(
                    matches!(check_chars(&raw), Err(KeyError::BadValue(_))),
                    "U+{cp:04X}"
                );
            }
        }
    }
}
