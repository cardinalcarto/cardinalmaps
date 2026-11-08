//! # Linguistic records
//!
//! Valhalla stores pronunciations (and the language of names) as linguistic records,
//! both for edge names (as a tagged value in the edge info)
//! and for signs (as a separate sign of type [`SignType::Linguistic`](super::SignType::Linguistic)).
//!
//! Each record is a 3-byte header followed by the pronunciation:
//!
//! ```text
//! byte 0: language (see Language)
//! byte 1: pronunciation length in bytes
//! byte 2: bits 0-2 phonetic alphabet (see PronunciationAlphabet),
//!         bits 3-6 index of the name (or sign) the record describes, bit 7 spare
//! ```
//!
//! A run of records ends with a NUL byte where the next header would start.
//! (Mainline's `linguistic_text_header_t` is a 4-byte bitfield,
//! but only its first three bytes are ever stored.)

use super::sections::SectionDecodingError;
use num_enum::TryFromPrimitive;
use std::borrow::Cow;

/// The size of a stored linguistic record header (`kLinguisticHeaderSize`).
const HEADER_SIZE: usize = 3;

/// The number of distinct name indexes a record can refer to (4 bits).
const MAX_NAME_INDEXES: usize = 16;

/// The phonetic alphabet of a pronunciation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
pub enum PronunciationAlphabet {
    Ipa = 1,
    Katakana = 2,
    Jeita = 3,
    NtSampa = 4,
    /// No pronunciation; the record only gives the language of the name.
    ///
    /// (Older tiles used 0 for this, which broke record parsing,
    /// so 3.9.1 uses 5.)
    None = 5,
}

/// A language, as used by linguistic records.
///
/// The discriminants are the values stored in tiles.
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
pub enum Language {
    Ab = 1,
    Am = 2,
    Ar = 3,
    Az = 4,
    Be = 5,
    Bg = 6,
    Bn = 7,
    Bs = 8,
    Ca = 9,
    Ckb = 10,
    Cs = 11,
    Da = 12,
    De = 13,
    Dv = 14,
    Dz = 15,
    El = 16,
    En = 17,
    Es = 18,
    Et = 19,
    Fa = 20,
    Fi = 21,
    Fr = 22,
    Fy = 23,
    Gl = 24,
    He = 25,
    Hr = 26,
    Hu = 27,
    Hy = 28,
    Id = 29,
    Is = 30,
    It = 31,
    Ja = 32,
    Ka = 33,
    Kl = 34,
    Km = 35,
    Ko = 36,
    Lo = 37,
    Lt = 38,
    Lv = 39,
    Mg = 40,
    Mk = 41,
    Mn = 42,
    Mo = 43,
    Mt = 44,
    My = 45,
    Ne = 46,
    Nl = 47,
    No = 48,
    Oc = 49,
    Pap = 50,
    Pl = 51,
    Ps = 52,
    Pt = 53,
    Rm = 54,
    Ro = 55,
    Ru = 56,
    Sk = 57,
    Sl = 58,
    Sq = 59,
    Sr = 60,
    SrLatn = 61,
    Sv = 62,
    Tg = 63,
    Th = 64,
    Tk = 65,
    Tr = 66,
    Uk = 67,
    Ur = 68,
    Uz = 69,
    Vi = 70,
    Zh = 71,
    Cy = 72,
    Ta = 73,
    Ms = 74,
    None = 255,
}

impl Language {
    /// The language code, as mainline's `to_string(Language)` gives it
    /// (for example, `ca` or `sr-Latn`).
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ab => "ab",
            Self::Am => "am",
            Self::Ar => "ar",
            Self::Az => "az",
            Self::Be => "be",
            Self::Bg => "bg",
            Self::Bn => "bn",
            Self::Bs => "bs",
            Self::Ca => "ca",
            Self::Ckb => "ckb",
            Self::Cs => "cs",
            Self::Da => "da",
            Self::De => "de",
            Self::Dv => "dv",
            Self::Dz => "dz",
            Self::El => "el",
            Self::En => "en",
            Self::Es => "es",
            Self::Et => "et",
            Self::Fa => "fa",
            Self::Fi => "fi",
            Self::Fr => "fr",
            Self::Fy => "fy",
            Self::Gl => "gl",
            Self::He => "he",
            Self::Hr => "hr",
            Self::Hu => "hu",
            Self::Hy => "hy",
            Self::Id => "id",
            Self::Is => "is",
            Self::It => "it",
            Self::Ja => "ja",
            Self::Ka => "ka",
            Self::Kl => "kl",
            Self::Km => "km",
            Self::Ko => "ko",
            Self::Lo => "lo",
            Self::Lt => "lt",
            Self::Lv => "lv",
            Self::Mg => "mg",
            Self::Mk => "mk",
            Self::Mn => "mn",
            Self::Mo => "mo",
            Self::Mt => "mt",
            Self::My => "my",
            Self::Ne => "ne",
            Self::Nl => "nl",
            Self::No => "no",
            Self::Oc => "oc",
            Self::Pap => "pap",
            Self::Pl => "pl",
            Self::Ps => "ps",
            Self::Pt => "pt",
            Self::Rm => "rm",
            Self::Ro => "ro",
            Self::Ru => "ru",
            Self::Sk => "sk",
            Self::Sl => "sl",
            Self::Sq => "sq",
            Self::Sr => "sr",
            Self::SrLatn => "sr-Latn",
            Self::Sv => "sv",
            Self::Tg => "tg",
            Self::Th => "th",
            Self::Tk => "tk",
            Self::Tr => "tr",
            Self::Uk => "uk",
            Self::Ur => "ur",
            Self::Uz => "uz",
            Self::Vi => "vi",
            Self::Zh => "zh",
            Self::Cy => "cy",
            Self::Ta => "ta",
            Self::Ms => "ms",
            Self::None => "none",
        }
    }
}

/// A single linguistic record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinguisticRecord<'a> {
    language: u8,
    phonetic_alphabet: u8,
    name_index: u8,
    /// The third header byte as stored (including the spare bit).
    packed: u8,
    pronunciation: &'a [u8],
}

impl<'a> LinguisticRecord<'a> {
    /// The record as stored: the 3-byte header, then the pronunciation.
    ///
    /// Mainline's `GetLinguisticTaggedValues` and `GetSigns` return records in this form.
    #[inline]
    pub fn raw_header(&self) -> [u8; HEADER_SIZE] {
        // The pronunciation was sliced using a one-byte length, so this cannot truncate.
        #[allow(clippy::cast_possible_truncation)]
        let length = self.pronunciation.len() as u8;
        [self.language, length, self.packed]
    }

    /// The raw language value.
    #[inline]
    pub const fn raw_language(&self) -> u8 {
        self.language
    }

    /// The language of the name (or of the pronunciation).
    ///
    /// # Errors
    ///
    /// Returns an error if the language value is not one Valhalla 3.9.1 defines.
    #[inline]
    pub fn language(&self) -> Result<Language, SectionDecodingError> {
        Language::try_from(self.language).map_err(|_| SectionDecodingError::UnknownValue {
            field: "linguistic language",
            value: u64::from(self.language),
        })
    }

    /// The raw phonetic alphabet value.
    #[inline]
    pub const fn raw_phonetic_alphabet(&self) -> u8 {
        self.phonetic_alphabet
    }

    /// The phonetic alphabet of the pronunciation.
    ///
    /// # Errors
    ///
    /// Returns an error if the value is not one Valhalla 3.9.1 defines.
    #[inline]
    pub fn phonetic_alphabet(&self) -> Result<PronunciationAlphabet, SectionDecodingError> {
        PronunciationAlphabet::try_from(self.phonetic_alphabet).map_err(|_| {
            SectionDecodingError::UnknownValue {
                field: "linguistic phonetic alphabet",
                value: u64::from(self.phonetic_alphabet),
            }
        })
    }

    /// The index of the name (among the edge's names, tagged ones included)
    /// or sign (among the edge's or node's signs) that this record describes.
    #[inline]
    pub const fn name_index(&self) -> u8 {
        self.name_index
    }

    /// The raw pronunciation bytes (empty when the record only gives a language).
    #[inline]
    pub const fn pronunciation(&self) -> &'a [u8] {
        self.pronunciation
    }

    /// The pronunciation as a string (lossy if the tile holds invalid UTF-8).
    #[inline]
    pub fn pronunciation_lossy(&self) -> Cow<'a, str> {
        String::from_utf8_lossy(self.pronunciation)
    }
}

/// An iterator over a run of linguistic records.
///
/// # Errors
///
/// Yields an error (and then stops) if a record runs off the end of the data,
/// or the run has no terminator.
#[derive(Clone, Debug)]
pub struct LinguisticRecords<'a> {
    bytes: &'a [u8],
    /// The text list offset of `bytes`, for error messages.
    base_offset: usize,
    position: usize,
    /// Whether the end of `bytes` counts as the terminator.
    end_is_terminator: bool,
    done: bool,
}

impl<'a> LinguisticRecords<'a> {
    /// Iterates over the records at the start of `bytes`,
    /// which starts at `base_offset` in the text list.
    #[inline]
    pub(crate) const fn new(bytes: &'a [u8], base_offset: usize) -> Self {
        Self {
            bytes,
            base_offset,
            position: 0,
            end_is_terminator: false,
            done: false,
        }
    }

    /// Iterates over the records in `bytes`, which has been sized to exclude the terminator.
    #[inline]
    pub(crate) const fn new_terminated_at_end(bytes: &'a [u8], base_offset: usize) -> Self {
        Self {
            bytes,
            base_offset,
            position: 0,
            end_is_terminator: true,
            done: false,
        }
    }

    /// The number of bytes consumed so far (not counting the terminator).
    ///
    /// Once the iterator is exhausted without error,
    /// this is the size of the whole run of records.
    #[inline]
    pub(crate) const fn consumed(&self) -> usize {
        self.position
    }

    /// Stops the iteration with an error.
    fn malformed(&mut self, reason: &'static str) -> SectionDecodingError {
        self.done = true;
        SectionDecodingError::MalformedText {
            what: "linguistic record",
            offset: self.base_offset + self.position,
            reason,
        }
    }
}

impl<'a> Iterator for LinguisticRecords<'a> {
    type Item = Result<LinguisticRecord<'a>, SectionDecodingError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // `position` never exceeds the length: it only advances past bytes that exist.
        let rest = self.bytes.get(self.position..).unwrap_or_default();
        match rest {
            [] if self.end_is_terminator => {
                self.done = true;
                None
            }
            [] => Some(Err(self.malformed("the records have no NUL terminator"))),
            [0, ..] => {
                self.done = true;
                None
            }
            [language, length, packed, tail @ ..] => {
                let length = usize::from(*length);
                let Some(pronunciation) = tail.get(..length) else {
                    return Some(Err(
                        self.malformed("the pronunciation runs past the end of the text list")
                    ));
                };
                self.position += HEADER_SIZE + length;
                Some(Ok(LinguisticRecord {
                    language: *language,
                    phonetic_alphabet: packed & 0b111,
                    name_index: (packed >> 3) & 0b1111,
                    packed: *packed,
                    pronunciation,
                }))
            }
            _ => Some(Err(
                self.malformed("the header runs past the end of the text list")
            )),
        }
    }
}

/// Pronunciations keyed by the index of the name or sign they describe,
/// as mainline's `GetLinguisticMap` and `GetSigns` build them.
///
/// When several records describe the same name,
/// mainline keeps the first, unless a later one has the same language
/// and a "higher" phonetic alphabet (other than [`PronunciationAlphabet::None`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinguisticMap<'a> {
    entries: [Option<LinguisticRecord<'a>>; MAX_NAME_INDEXES],
}

impl<'a> LinguisticMap<'a> {
    /// Adds a record, following mainline's rule for duplicates.
    pub(crate) fn insert(&mut self, record: LinguisticRecord<'a>) {
        // The name index is 4 bits, so it always has a slot.
        let Some(slot) = self.entries.get_mut(usize::from(record.name_index)) else {
            return;
        };
        match slot {
            None => *slot = Some(record),
            Some(existing) => {
                if record.phonetic_alphabet > existing.phonetic_alphabet
                    && record.phonetic_alphabet != PronunciationAlphabet::None as u8
                    && record.language == existing.language
                {
                    *slot = Some(record);
                }
            }
        }
    }

    /// Gets the record for a name (or sign) index.
    #[inline]
    pub fn get(&self, name_index: u8) -> Option<&LinguisticRecord<'a>> {
        self.entries.get(usize::from(name_index))?.as_ref()
    }

    /// Iterates over the records in name index order.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &LinguisticRecord<'a>> {
        self.entries.iter().flatten()
    }

    /// The number of names with a record.
    #[inline]
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    /// Whether there are no records.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(language: u8, length: u8, alphabet: u8, name_index: u8) -> [u8; 3] {
        [language, length, alphabet | (name_index << 3)]
    }

    #[test]
    fn test_records_and_terminator() {
        let mut bytes = Vec::new();
        bytes.extend(header(9, 0, 5, 0));
        bytes.extend(header(17, 3, 1, 2));
        bytes.extend(b"abc");
        bytes.push(0);
        bytes.extend(b"trailing");

        let mut records = LinguisticRecords::new(&bytes, 100);
        let first = records.next().unwrap().unwrap();
        assert_eq!(first.language().unwrap(), Language::Ca);
        assert_eq!(
            first.phonetic_alphabet().unwrap(),
            PronunciationAlphabet::None
        );
        assert_eq!(first.name_index(), 0);
        assert!(first.pronunciation().is_empty());
        let second = records.next().unwrap().unwrap();
        assert_eq!(second.language().unwrap().as_str(), "en");
        assert_eq!(
            second.phonetic_alphabet().unwrap(),
            PronunciationAlphabet::Ipa
        );
        assert_eq!(second.name_index(), 2);
        assert_eq!(second.pronunciation(), b"abc");
        assert!(records.next().is_none());
        assert!(records.next().is_none());
        assert_eq!(records.consumed(), 9);
    }

    #[test]
    fn test_truncated_records() {
        // Header cut short
        let bytes = [9, 0];
        let mut records = LinguisticRecords::new(&bytes, 7);
        assert_eq!(
            records.next(),
            Some(Err(SectionDecodingError::MalformedText {
                what: "linguistic record",
                offset: 7,
                reason: "the header runs past the end of the text list",
            }))
        );
        assert!(records.next().is_none());

        // Pronunciation cut short
        let mut bytes = header(9, 10, 1, 0).to_vec();
        bytes.extend(b"abc");
        let mut records = LinguisticRecords::new(&bytes, 0);
        assert!(matches!(records.next(), Some(Err(_))));
        assert!(records.next().is_none());

        // No terminator
        let bytes = header(9, 0, 5, 0);
        let mut records = LinguisticRecords::new(&bytes, 0);
        assert!(records.next().unwrap().is_ok());
        assert!(matches!(records.next(), Some(Err(_))));
        assert!(records.next().is_none());
    }

    #[test]
    fn test_unknown_values() {
        let bytes = [200, 0, 7, 0];
        let record = LinguisticRecords::new(&bytes, 0).next().unwrap().unwrap();
        assert!(record.language().is_err());
        assert!(record.phonetic_alphabet().is_err());
        assert_eq!(record.raw_language(), 200);
        assert_eq!(record.raw_phonetic_alphabet(), 7);
    }

    #[test]
    fn test_map_duplicate_rule() {
        let record = |language, alphabet, pronunciation| LinguisticRecord {
            language,
            phonetic_alphabet: alphabet,
            name_index: 1,
            packed: alphabet | 1 << 3,
            pronunciation,
        };
        let mut map = LinguisticMap::default();
        assert!(map.is_empty());
        map.insert(record(9, 1, b"first"));
        // Same alphabet: keeps the first
        map.insert(record(9, 1, b"same"));
        assert_eq!(map.get(1).unwrap().pronunciation(), b"first");
        // Higher alphabet, same language: replaces it
        map.insert(record(9, 4, b"higher"));
        assert_eq!(map.get(1).unwrap().pronunciation(), b"higher");
        // Higher alphabet, but None: keeps it
        map.insert(record(9, 5, b"none"));
        assert_eq!(map.get(1).unwrap().pronunciation(), b"higher");
        assert_eq!(map.len(), 1);
        assert!(map.get(0).is_none());
        assert!(map.get(200).is_none());

        // Higher alphabet, different language: keeps the first
        let mut other = LinguisticMap::default();
        other.insert(record(9, 1, b"first"));
        other.insert(record(17, 4, b"other"));
        assert_eq!(other.get(1).unwrap().pronunciation(), b"first");
    }

    #[test]
    fn test_language_codes() {
        assert_eq!(Language::SrLatn.as_str(), "sr-Latn");
        assert_eq!(Language::try_from(74).unwrap().as_str(), "ms");
        assert_eq!(Language::None.as_str(), "none");
        assert!(Language::try_from(75).is_err());
        assert!(Language::try_from(0).is_err());
    }
}
