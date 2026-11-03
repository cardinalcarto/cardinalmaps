//! Tokenization (upstream `tokenization/*.js`).
//!
//! The input is split into **sections** at field delimiters (comma, tab,
//! newline, quotes), each section into **words** at whitespace and then
//! again at hyphens and slashes (`"Saint-Germain"` yields `Saint-Germain`,
//! `Saint` and `Germain`), and every run of up to ten adjacent words of a
//! section becomes a **phrase**. Classifiers label words and phrases;
//! solvers then pick non-overlapping combinations of them.
//!
//! Spans live in an arena owned by [`Tokenizer`] and refer to each other by
//! [`SpanId`], standing in for upstream's object graph (`child`, `phrase`,
//! `prev`, `next`, `child:first` and `child:last` edges). Offsets are in
//! UTF-16 code units, like upstream's JavaScript string indices, so scores
//! match upstream exactly; [`crate::Parse`] converts them to byte and char
//! offsets.

use crate::classification::{Classification, Kind};
use crate::js;
use crate::solver::Solution;

/// Index of a span in [`Tokenizer::spans`].
pub(crate) type SpanId = u32;

/// Upstream `MAX_SPAN_LENGTH`: span bodies are truncated to this many
/// UTF-16 units.
const MAX_SPAN_LENGTH: usize = 140;

/// Upstream permutation window: phrases are 1 to 10 words long.
const PHRASE_WINDOW_MAX: usize = 10;

/// A piece of the input: a section, word or phrase.
#[derive(Clone, Debug, Default)]
pub(crate) struct Span {
    /// The text, for phrases the words joined by single spaces.
    pub body: String,
    /// Lowercased body.
    pub norm: String,
    /// Start offset in the input, in UTF-16 units.
    pub start: usize,
    /// End offset (exclusive) in the input, in UTF-16 units.
    pub end: usize,
    /// The body contains an ASCII digit.
    pub numerals: bool,
    /// The body ends with a period.
    pub final_period: bool,
    /// Classifications, in the order they were first added.
    pub classifications: Vec<Classification>,
    /// Sections: their words. Phrases: their words, in order.
    pub children: Vec<SpanId>,
    /// Sections: their phrases.
    pub phrases: Vec<SpanId>,
    pub prev: Vec<SpanId>,
    pub next: Vec<SpanId>,
    /// Phrases: the first and last word.
    pub child_first: Option<SpanId>,
    pub child_last: Option<SpanId>,
}

impl Span {
    /// Upstream `new Span(body, start)`.
    pub fn new(body: &str, start: usize) -> Self {
        let mut span = Span {
            start,
            ..Default::default()
        };
        span.set_body(body);
        span
    }

    /// Upstream `Span.setBody`.
    fn set_body(&mut self, body: &str) {
        let body = js::truncate(body, MAX_SPAN_LENGTH);
        self.body = body.to_string();
        self.norm = body.to_lowercase();
        self.end = self.start + js::len(body);
        self.numerals = body.bytes().any(|b| b.is_ascii_digit());
        self.final_period = body.ends_with('.');
    }

    /// Whether the spans' ranges overlap.
    pub fn intersects(&self, other: &Span) -> bool {
        self.start < other.end && self.end > other.start
    }

    /// Whether this span's range contains the other's.
    pub fn covers(&self, other: &Span) -> bool {
        self.start <= other.start && self.end >= other.end
    }

    /// Characters between the spans (0 if they overlap).
    pub fn distance(&self, other: &Span) -> usize {
        if self.intersects(other) {
            0
        } else if self.end < other.start {
            other.start - self.end
        } else {
            self.start.saturating_sub(other.end)
        }
    }

    pub fn has(&self, kind: Kind) -> bool {
        self.classifications.iter().any(|c| c.kind == kind)
    }

    pub fn has_any(&self, kinds: &[Kind]) -> bool {
        kinds.iter().any(|k| self.has(*k))
    }

    /// Upstream `Span.classify`: adds a classification, or replaces one of
    /// the same kind with lower confidence (keeping its position).
    pub fn classify(&mut self, classification: Classification) {
        match self
            .classifications
            .iter_mut()
            .find(|c| c.kind == classification.kind)
        {
            Some(existing) if existing.confidence >= classification.confidence => {}
            Some(existing) => *existing = classification,
            None => self.classifications.push(classification),
        }
    }
}

fn add_edge(edges: &mut Vec<SpanId>, id: SpanId) {
    if !edges.contains(&id) {
        edges.push(id);
    }
}

/// Field delimiters between sections (upstream `fieldsFuncBoundary`).
fn is_boundary(c: char) -> bool {
    const QUOTES: &str = "\"«»‘’‚‛“”„‟‹›⹂「」『』〝〞〟﹁﹂﹃﹄＂＇｢｣";
    matches!(c, '\n' | '\t' | ',') || QUOTES.contains(c)
}

fn is_hyphen_or_whitespace(c: char) -> bool {
    c == '-' || c == '/' || js::is_whitespace(c)
}

/// A tokenized query: the span arena, sections, phrases and (after solving)
/// the solutions.
#[derive(Clone, Debug)]
pub(crate) struct Tokenizer {
    pub spans: Vec<Span>,
    /// The whole input.
    pub root: SpanId,
    pub sections: Vec<SpanId>,
    /// The whitespace-separated words of all sections, in order (before
    /// hyphen splitting).
    pub words: Vec<SpanId>,
    /// Total UTF-16 units covered by words (delimiters excluded).
    pub coverage: usize,
    pub solutions: Vec<Solution>,
}

impl Tokenizer {
    /// Upstream `new Tokenizer(s)`.
    pub fn new(input: &str) -> Self {
        let mut t = Tokenizer {
            spans: Vec::new(),
            root: 0,
            sections: Vec::new(),
            words: Vec::new(),
            coverage: 0,
            solutions: Vec::new(),
        };
        t.root = t.push(Span::new(input, 0));
        t.sections = t.split(t.root, is_boundary);
        for i in 0..t.sections.len() {
            let section = t.sections[i];
            let words = t.split(section, js::is_whitespace);
            t.set_children(section, &words);
            t.words.extend(&words);
            let words = t.split(section, is_hyphen_or_whitespace);
            t.set_children(section, &words);
        }
        t.compute_coverage();
        for i in 0..t.sections.len() {
            let section = t.sections[i];
            for phrase in t.permutate(section) {
                add_edge(&mut t.span_mut(section).phrases, phrase);
            }
        }
        t
    }

    pub fn push(&mut self, span: Span) -> SpanId {
        self.spans.push(span);
        SpanId::try_from(self.spans.len() - 1).expect("span count fits u32")
    }

    pub fn span(&self, id: SpanId) -> &Span {
        &self.spans[id as usize]
    }

    pub fn span_mut(&mut self, id: SpanId) -> &mut Span {
        &mut self.spans[id as usize]
    }

    fn set_children(&mut self, parent: SpanId, children: &[SpanId]) {
        for &c in children {
            add_edge(&mut self.span_mut(parent).children, c);
        }
    }

    /// Upstream `split(span, f)`: the fields of `parent`'s body between
    /// characters matching `is_delim`, reusing existing children with the
    /// same start and body, linked as siblings.
    fn split(&mut self, parent: SpanId, is_delim: fn(char) -> bool) -> Vec<SpanId> {
        let body = self.span(parent).body.clone();
        let parent_start = self.span(parent).start;
        let mut fields: Vec<(usize, &str)> = Vec::new();
        let mut field: Option<(usize, usize)> = None; // (utf16 offset, byte offset)
        let mut units = 0;
        for (byte, c) in body.char_indices() {
            if is_delim(c) {
                if let Some((u, b)) = field.take() {
                    fields.push((u, &body[b..byte]));
                }
            } else if field.is_none() {
                field = Some((units, byte));
            }
            units += c.len_utf16();
        }
        if let Some((u, b)) = field {
            fields.push((u, &body[b..]));
        }

        let mut spans = Vec::with_capacity(fields.len());
        for (offset, text) in fields {
            let start = parent_start + offset;
            let existing = self
                .span(parent)
                .children
                .iter()
                .copied()
                .find(|&c| self.span(c).start == start && self.span(c).body == text);
            spans.push(existing.unwrap_or_else(|| self.push(Span::new(text, start))));
        }
        self.connect_siblings(&spans);
        spans
    }

    /// Upstream `Span.connectSiblings`.
    fn connect_siblings(&mut self, spans: &[SpanId]) {
        for (i, &s) in spans.iter().enumerate() {
            if i > 0 {
                add_edge(&mut self.span_mut(s).prev, spans[i - 1]);
            }
            if let Some(&next) = spans.get(i + 1) {
                add_edge(&mut self.span_mut(s).next, next);
            }
        }
    }

    /// Upstream `computeCoverage`: follows the first `next` edge from each
    /// section's first word, summing word lengths.
    fn compute_coverage(&mut self) {
        let mut coverage = 0;
        for &section in &self.sections {
            let mut cur = self.span(section).children.first().copied();
            while let Some(id) = cur {
                let span = self.span(id);
                coverage += span.end - span.start;
                cur = span.next.first().copied();
            }
        }
        self.coverage = coverage;
    }

    /// Upstream `permutate(children, 0, 10)`: every path of 1–10 words along
    /// `next` edges, in upstream's (post-order) sequence.
    fn permutate(&mut self, section: SpanId) -> Vec<SpanId> {
        let mut out = Vec::new();
        let children = self.span(section).children.clone();
        for child in children {
            self.permutate_rec(None, child, 1, &mut out);
        }
        out
    }

    fn permutate_rec(
        &mut self,
        prev: Option<SpanId>,
        s: SpanId,
        window: usize,
        out: &mut Vec<SpanId>,
    ) {
        if window > PHRASE_WINDOW_MAX {
            return;
        }
        let s_span = self.span(s);
        let (s_start, s_end) = (s_span.start, s_span.end);
        let mut span = match prev {
            Some(p) => {
                let p = self.span(p);
                Span::new(&format!("{} {}", p.body, s_span.body), p.start)
            }
            None => Span::new(&s_span.body, 0),
        };
        if let Some(p) = prev {
            let p = self.span(p);
            span.children = p.children.clone();
            span.child_first = p.child_first;
        } else {
            span.child_first = Some(s);
        }
        add_edge(&mut span.children, s);
        span.child_last = Some(s);
        if prev.is_none() {
            span.start = s_start;
            span.end = s_end;
        } else {
            span.start = span.start.min(s_start);
            span.end = span.end.max(s_end);
        }
        let id = self.push(span);
        let nexts = self.span(s).next.clone();
        for next in nexts {
            self.permutate_rec(Some(id), next, window + 1, out);
        }
        out.push(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bodies(t: &Tokenizer, ids: &[SpanId]) -> Vec<String> {
        ids.iter().map(|&i| t.span(i).body.clone()).collect()
    }

    #[test]
    fn sections_and_words() {
        let t = Tokenizer::new("100 West 26th Street, NYC, 10010 NY, USA");
        assert_eq!(
            bodies(&t, &t.sections),
            ["100 West 26th Street", " NYC", " 10010 NY", " USA"]
        );
        assert_eq!(
            bodies(&t, &t.span(t.sections[0]).children),
            ["100", "West", "26th", "Street"]
        );
        assert_eq!(bodies(&t, &t.span(t.sections[2]).children), ["10010", "NY"]);
        assert_eq!(t.span(t.span(t.sections[1]).children[0]).start, 22);
    }

    #[test]
    fn hyphens_add_words() {
        let t = Tokenizer::new("20 Boulevard Saint-Germain, Paris, France");
        assert_eq!(
            bodies(&t, &t.span(t.sections[0]).children),
            ["20", "Boulevard", "Saint-Germain", "Saint", "Germain"]
        );
    }

    #[test]
    fn permutations() {
        let t = Tokenizer::new("soho new york usa");
        assert_eq!(
            bodies(&t, &t.span(t.sections[0]).phrases),
            [
                "soho new york usa",
                "soho new york",
                "soho new",
                "soho",
                "new york usa",
                "new york",
                "new",
                "york usa",
                "york",
                "usa"
            ]
        );
    }

    #[test]
    fn coverage_skips_delimiters() {
        assert_eq!(Tokenizer::new("a  bc, d-e").coverage, 6);
    }
}
