//! What the tagger labels each word as.

/// A word's role in a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tag {
    /// What's sought: a place's name or a kind of place (`starbucks`,
    /// `vegan pizza`), which search matches against names and the category
    /// model alike.
    Venue,
    /// A word relating what's sought to where: `near`, `in`, `bei`.
    Near,
    HouseNumber,
    Road,
    /// A unit, level, entrance or staircase.
    Unit,
    Postcode,
    Neighbourhood,
    Locality,
    County,
    Region,
    Country,
    /// Anything else: noise, punctuation words.
    Other,
}

impl Tag {
    pub const ALL: [Tag; 12] = [
        Tag::Venue,
        Tag::Near,
        Tag::HouseNumber,
        Tag::Road,
        Tag::Unit,
        Tag::Postcode,
        Tag::Neighbourhood,
        Tag::Locality,
        Tag::County,
        Tag::Region,
        Tag::Country,
        Tag::Other,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Tag::Venue => "venue",
            Tag::Near => "near",
            Tag::HouseNumber => "house_number",
            Tag::Road => "road",
            Tag::Unit => "unit",
            Tag::Postcode => "postcode",
            Tag::Neighbourhood => "neighbourhood",
            Tag::Locality => "locality",
            Tag::County => "county",
            Tag::Region => "region",
            Tag::Country => "country",
            Tag::Other => "other",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }

    /// The administrative band it names, if it's an area.
    #[must_use]
    pub fn band(self) -> Option<&'static str> {
        match self {
            Tag::Neighbourhood | Tag::Locality | Tag::County | Tag::Region | Tag::Country => Some(self.as_str()),
            _ => None,
        }
    }

    /// Its index among the model's labels.
    #[must_use]
    pub fn label(self) -> crate::crf::Label {
        Self::ALL.iter().position(|t| *t == self).expect("a tag") as crate::crf::Label
    }

    /// The tag of a model label.
    #[must_use]
    pub fn of_label(label: crate::crf::Label) -> Self {
        Self::ALL[usize::from(label)]
    }

    /// Every tag's name, in label order, for a model's labels.
    #[must_use]
    pub fn names() -> Vec<String> {
        Self::ALL.iter().map(|t| t.as_str().to_string()).collect()
    }
}
