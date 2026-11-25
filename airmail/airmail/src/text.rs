//! Text as the index has it: see [`airmail_tagger::text`], which the
//! tagger shares so that it reads queries as the index does, and the
//! tokenizer that splits folded text into its [`words`].

pub use airmail_tagger::text::*;
use tantivy::tokenizer::{Token, TokenStream};

/// The name `Tokenizer` is registered under.
pub const TOKENIZER: &str = "airmail";

/// Splits text into [`words`], lowercased. Text should be folded first,
/// which `fold` does for both indexing and queries.
#[derive(Debug, Clone, Default)]
pub struct Tokenizer;

impl tantivy::tokenizer::Tokenizer for Tokenizer {
    type TokenStream<'a> = WordStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> WordStream<'a> {
        let words = if text.is_ascii() {
            Words::Ascii(AsciiWords::new(text))
        } else {
            Words::Other(words(text).into_iter())
        };
        WordStream {
            words,
            token: Token::default(),
            next: 0,
        }
    }
}

enum Words<'a> {
    /// Most text: read as it's tokenized, without a list.
    Ascii(AsciiWords<'a>),
    Other(std::vec::IntoIter<(usize, &'a str)>),
}

/// [`Tokenizer`]'s words, each lowercased into one token as it's read.
pub struct WordStream<'a> {
    words: Words<'a>,
    token: Token,
    /// The next word's position.
    next: usize,
}

impl TokenStream for WordStream<'_> {
    fn advance(&mut self) -> bool {
        let word = match &mut self.words {
            Words::Ascii(words) => words.next(),
            Words::Other(words) => words.next(),
        };
        let Some((offset, word)) = word else {
            return false;
        };
        let token = &mut self.token;
        token.offset_from = offset;
        token.offset_to = offset + word.len();
        token.position = self.next;
        token.position_length = 1;
        token.text.clear();
        if word.is_ascii() {
            token
                .text
                .extend(word.bytes().map(|b| char::from(b.to_ascii_lowercase())));
        } else {
            token.text.push_str(&word.to_lowercase());
        }
        self.next += 1;
        true
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use tantivy::tokenizer::Tokenizer as _;

    /// The tokens as the tokenizer made them before it streamed them.
    fn listed(text: &str) -> Vec<Token> {
        words(text)
            .into_iter()
            .enumerate()
            .map(|(position, (offset, word))| Token {
                offset_from: offset,
                offset_to: offset + word.len(),
                position,
                text: word.to_lowercase(),
                position_length: 1,
            })
            .collect()
    }

    #[test]
    fn tokens_are_the_words_lowercased() {
        let long = "x".repeat(65);
        let texts = [
            "fremont ave n".to_string(),
            "McDonald's  RD\r\n 54TH".to_string(),
            String::new(),
            "  ".to_string(),
            "a".to_string(),
            format!("{long} b {long} c"),
            "渋谷区 Shibuya 2-1".to_string(),
            "Ленинский Проспект".to_string(),
            "Straße".to_string(),
            "İstanbul".to_string(),
        ];
        for text in &texts {
            let mut tokenizer = Tokenizer;
            let mut stream = tokenizer.token_stream(text);
            let mut tokens = Vec::new();
            while stream.advance() {
                tokens.push(stream.token().clone());
            }
            assert_eq!(
                format!("{tokens:?}"),
                format!("{:?}", listed(text)),
                "{text:?}"
            );
        }
    }
}
