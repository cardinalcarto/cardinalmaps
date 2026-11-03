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
    type TokenStream<'a> = WordStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> WordStream {
        let tokens = words(text)
            .into_iter()
            .enumerate()
            .map(|(position, (offset, word))| Token {
                offset_from: offset,
                offset_to: offset + word.len(),
                position,
                text: word.to_lowercase(),
                position_length: 1,
            })
            .collect();
        WordStream { tokens, next: 0 }
    }
}

/// [`Tokenizer`]'s words.
pub struct WordStream {
    tokens: Vec<Token>,
    next: usize,
}

impl TokenStream for WordStream {
    fn advance(&mut self) -> bool {
        self.next += 1;
        self.next <= self.tokens.len()
    }

    fn token(&self) -> &Token {
        &self.tokens[self.next - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.next - 1]
    }
}

