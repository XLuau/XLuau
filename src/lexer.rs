use std::fmt;

use crate::compiler::{CompilerError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Identifier,
    Number,
    String,
    Keyword(Keyword),
    Symbol(Symbol),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    And,
    Break,
    Case,
    Catch,
    Comptime,
    Const,
    Continue,
    Default,
    Do,
    Else,
    ElseIf,
    End,
    Enum,
    Extends,
    Fallthrough,
    False,
    Fire,
    For,
    Function,
    If,
    In,
    Local,
    Match,
    Nil,
    Object,
    On,
    Not,
    Once,
    Or,
    Repeat,
    Return,
    Signal,
    Spawn,
    State,
    Switch,
    Task,
    Then,
    True,
    Type,
    Until,
    Watch,
    Yield,
    While,
    Export,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Colon,
    DoubleColon,
    Semi,
    Assign,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Plus,
    Minus,
    Star,
    Slash,
    DoubleSlash,
    Percent,
    Caret,
    Pipe,
    Ampersand,
    Hash,
    DoubleDot,
    Ellipsis,
    Arrow,
    PipeGreater,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    DoubleSlashEqual,
    PercentEqual,
    CaretEqual,
    DoubleDotEqual,
    Question,
    DoubleQuestion,
    DoubleQuestionEqual,
}

#[derive(Debug, Clone)]
pub struct Lexer<'src> {
    source: &'src str,
    chars: Vec<char>,
    index: usize,
    line: usize,
    column: usize,
}

impl<'src> Lexer<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            chars: source.chars().collect(),
            index: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        while let Some(ch) = self.peek_char(0) {
            if ch.is_whitespace() {
                self.bump_char();
                continue;
            }

            if ch == '-' && self.peek_char(1) == Some('-') {
                self.skip_comment()?;
                continue;
            }

            let span_start = self.current_span_start();
            let token = if is_ident_start(ch) {
                self.lex_identifier_or_keyword(span_start)
            } else if ch.is_ascii_digit() {
                self.lex_number(span_start)
            } else {
                match ch {
                    '"' | '\'' | '`' => self.lex_quoted_string(span_start, ch)?,
                    '[' if self.is_long_bracket_start() => self.lex_long_string(span_start)?,
                    _ => self.lex_symbol(span_start)?,
                }
            };

            tokens.push(token);
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            lexeme: String::new(),
            span: self.current_span_start(),
        });

        Ok(tokens)
    }

    fn skip_comment(&mut self) -> Result<()> {
        self.bump_char();
        self.bump_char();
        if self.is_long_bracket_start() {
            self.consume_long_bracket()?;
            return Ok(());
        }

        while let Some(ch) = self.peek_char(0) {
            self.bump_char();
            if ch == '\n' {
                break;
            }
        }
        Ok(())
    }

    fn lex_identifier_or_keyword(&mut self, span_start: Span) -> Token {
        let mut text = String::new();
        while let Some(ch) = self.peek_char(0) {
            if is_ident_continue(ch) {
                text.push(ch);
                self.bump_char();
            } else {
                break;
            }
        }

        let kind = match text.as_str() {
            "and" => TokenKind::Keyword(Keyword::And),
            "break" => TokenKind::Keyword(Keyword::Break),
            "case" => TokenKind::Keyword(Keyword::Case),
            "catch" => TokenKind::Keyword(Keyword::Catch),
            "comptime" => TokenKind::Keyword(Keyword::Comptime),
            "const" => TokenKind::Keyword(Keyword::Const),
            "continue" => TokenKind::Keyword(Keyword::Continue),
            "default" => TokenKind::Keyword(Keyword::Default),
            "do" => TokenKind::Keyword(Keyword::Do),
            "else" => TokenKind::Keyword(Keyword::Else),
            "elseif" => TokenKind::Keyword(Keyword::ElseIf),
            "end" => TokenKind::Keyword(Keyword::End),
            "enum" => TokenKind::Keyword(Keyword::Enum),
            "extends" => TokenKind::Keyword(Keyword::Extends),
            "fallthrough" => TokenKind::Keyword(Keyword::Fallthrough),
            "false" => TokenKind::Keyword(Keyword::False),
            "fire" => TokenKind::Keyword(Keyword::Fire),
            "for" => TokenKind::Keyword(Keyword::For),
            "function" => TokenKind::Keyword(Keyword::Function),
            "if" => TokenKind::Keyword(Keyword::If),
            "in" => TokenKind::Keyword(Keyword::In),
            "local" => TokenKind::Keyword(Keyword::Local),
            "match" => TokenKind::Keyword(Keyword::Match),
            "nil" => TokenKind::Keyword(Keyword::Nil),
            "object" => TokenKind::Keyword(Keyword::Object),
            "on" => TokenKind::Keyword(Keyword::On),
            "not" => TokenKind::Keyword(Keyword::Not),
            "once" => TokenKind::Keyword(Keyword::Once),
            "or" => TokenKind::Keyword(Keyword::Or),
            "repeat" => TokenKind::Keyword(Keyword::Repeat),
            "return" => TokenKind::Keyword(Keyword::Return),
            "signal" => TokenKind::Keyword(Keyword::Signal),
            "spawn" => TokenKind::Keyword(Keyword::Spawn),
            "state" => TokenKind::Keyword(Keyword::State),
            "switch" => TokenKind::Keyword(Keyword::Switch),
            "task" => TokenKind::Keyword(Keyword::Task),
            "then" => TokenKind::Keyword(Keyword::Then),
            "true" => TokenKind::Keyword(Keyword::True),
            "type" => TokenKind::Keyword(Keyword::Type),
            "until" => TokenKind::Keyword(Keyword::Until),
            "watch" => TokenKind::Keyword(Keyword::Watch),
            "yield" => TokenKind::Keyword(Keyword::Yield),
            "while" => TokenKind::Keyword(Keyword::While),
            "export" => TokenKind::Keyword(Keyword::Export),
            _ => TokenKind::Identifier,
        };

        Token {
            kind,
            lexeme: text,
            span: self.finish_span(span_start),
        }
    }

    fn lex_number(&mut self, span_start: Span) -> Token {
        let mut text = String::new();
        while let Some(ch) = self.peek_char(0) {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '+' | '-') {
                text.push(ch);
                self.bump_char();
            } else {
                break;
            }
        }

        Token {
            kind: TokenKind::Number,
            lexeme: text,
            span: self.finish_span(span_start),
        }
    }

    fn lex_quoted_string(&mut self, span_start: Span, quote: char) -> Result<Token> {
        let mut text = String::new();
        text.push(quote);
        self.bump_char();

        let mut escaped = false;
        while let Some(ch) = self.peek_char(0) {
            text.push(ch);
            self.bump_char();
            if escaped {
                escaped = false;
                continue;
            }

            if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                return Ok(Token {
                    kind: TokenKind::String,
                    lexeme: text,
                    span: self.finish_span(span_start),
                });
            }
        }

        Err(CompilerError::Lex {
            message: format!(
                "unterminated string starting at {}:{}",
                span_start.line, span_start.column
            ),
        })
    }

    fn lex_long_string(&mut self, span_start: Span) -> Result<Token> {
        let text = self.consume_long_bracket()?;
        Ok(Token {
            kind: TokenKind::String,
            lexeme: text,
            span: self.finish_span(span_start),
        })
    }

    fn consume_long_bracket(&mut self) -> Result<String> {
        let mut text = String::new();
        let equals_count = self.long_bracket_equals();
        text.push('[');
        self.bump_char();
        for _ in 0..equals_count {
            text.push('=');
            self.bump_char();
        }
        text.push('[');
        self.bump_char();

        loop {
            match self.peek_char(0) {
                Some(']') if self.long_bracket_equals_with_prefix(0, equals_count) => {
                    text.push(']');
                    self.bump_char();
                    for _ in 0..equals_count {
                        text.push('=');
                        self.bump_char();
                    }
                    text.push(']');
                    self.bump_char();
                    return Ok(text);
                }
                Some(ch) => {
                    text.push(ch);
                    self.bump_char();
                }
                None => {
                    return Err(CompilerError::Lex {
                        message: "unterminated long bracket literal".to_string(),
                    });
                }
            }
        }
    }

    fn lex_symbol(&mut self, span_start: Span) -> Result<Token> {
        let (kind, lexeme) = match (self.peek_char(0), self.peek_char(1), self.peek_char(2)) {
            (Some('?'), Some('?'), Some('=')) => (
                TokenKind::Symbol(Symbol::DoubleQuestionEqual),
                "??=".to_string(),
            ),
            (Some('+'), Some('='), _) => (TokenKind::Symbol(Symbol::PlusEqual), "+=".to_string()),
            (Some('-'), Some('='), _) => (TokenKind::Symbol(Symbol::MinusEqual), "-=".to_string()),
            (Some('*'), Some('='), _) => (TokenKind::Symbol(Symbol::StarEqual), "*=".to_string()),
            (Some('/'), Some('/'), Some('=')) => (
                TokenKind::Symbol(Symbol::DoubleSlashEqual),
                "//=".to_string(),
            ),
            (Some('/'), Some('='), _) => (TokenKind::Symbol(Symbol::SlashEqual), "/=".to_string()),
            (Some('%'), Some('='), _) => {
                (TokenKind::Symbol(Symbol::PercentEqual), "%=".to_string())
            }
            (Some('^'), Some('='), _) => (TokenKind::Symbol(Symbol::CaretEqual), "^=".to_string()),
            (Some('.'), Some('.'), Some('=')) => {
                (TokenKind::Symbol(Symbol::DoubleDotEqual), "..=".to_string())
            }
            (Some('?'), Some('?'), _) => {
                (TokenKind::Symbol(Symbol::DoubleQuestion), "??".to_string())
            }
            (Some('|'), Some('>'), _) => (TokenKind::Symbol(Symbol::PipeGreater), "|>".to_string()),
            (Some(':'), Some(':'), _) => (TokenKind::Symbol(Symbol::DoubleColon), "::".to_string()),
            (Some('.'), Some('.'), Some('.')) => {
                (TokenKind::Symbol(Symbol::Ellipsis), "...".to_string())
            }
            (Some('.'), Some('.'), _) => (TokenKind::Symbol(Symbol::DoubleDot), "..".to_string()),
            (Some('='), Some('='), _) => (TokenKind::Symbol(Symbol::Equal), "==".to_string()),
            (Some('~'), Some('='), _) => (TokenKind::Symbol(Symbol::NotEqual), "~=".to_string()),
            (Some('<'), Some('='), _) => (TokenKind::Symbol(Symbol::LessEqual), "<=".to_string()),
            (Some('>'), Some('='), _) => {
                (TokenKind::Symbol(Symbol::GreaterEqual), ">=".to_string())
            }
            (Some('/'), Some('/'), _) => (TokenKind::Symbol(Symbol::DoubleSlash), "//".to_string()),
            (Some('-'), Some('>'), _) => (TokenKind::Symbol(Symbol::Arrow), "->".to_string()),
            (Some('('), _, _) => (TokenKind::Symbol(Symbol::LParen), "(".to_string()),
            (Some(')'), _, _) => (TokenKind::Symbol(Symbol::RParen), ")".to_string()),
            (Some('{'), _, _) => (TokenKind::Symbol(Symbol::LBrace), "{".to_string()),
            (Some('}'), _, _) => (TokenKind::Symbol(Symbol::RBrace), "}".to_string()),
            (Some('['), _, _) => (TokenKind::Symbol(Symbol::LBracket), "[".to_string()),
            (Some(']'), _, _) => (TokenKind::Symbol(Symbol::RBracket), "]".to_string()),
            (Some(','), _, _) => (TokenKind::Symbol(Symbol::Comma), ",".to_string()),
            (Some('.'), _, _) => (TokenKind::Symbol(Symbol::Dot), ".".to_string()),
            (Some(':'), _, _) => (TokenKind::Symbol(Symbol::Colon), ":".to_string()),
            (Some(';'), _, _) => (TokenKind::Symbol(Symbol::Semi), ";".to_string()),
            (Some('='), _, _) => (TokenKind::Symbol(Symbol::Assign), "=".to_string()),
            (Some('<'), _, _) => (TokenKind::Symbol(Symbol::Less), "<".to_string()),
            (Some('>'), _, _) => (TokenKind::Symbol(Symbol::Greater), ">".to_string()),
            (Some('+'), _, _) => (TokenKind::Symbol(Symbol::Plus), "+".to_string()),
            (Some('-'), _, _) => (TokenKind::Symbol(Symbol::Minus), "-".to_string()),
            (Some('*'), _, _) => (TokenKind::Symbol(Symbol::Star), "*".to_string()),
            (Some('/'), _, _) => (TokenKind::Symbol(Symbol::Slash), "/".to_string()),
            (Some('%'), _, _) => (TokenKind::Symbol(Symbol::Percent), "%".to_string()),
            (Some('^'), _, _) => (TokenKind::Symbol(Symbol::Caret), "^".to_string()),
            (Some('|'), _, _) => (TokenKind::Symbol(Symbol::Pipe), "|".to_string()),
            (Some('&'), _, _) => (TokenKind::Symbol(Symbol::Ampersand), "&".to_string()),
            (Some('#'), _, _) => (TokenKind::Symbol(Symbol::Hash), "#".to_string()),
            (Some('?'), _, _) => (TokenKind::Symbol(Symbol::Question), "?".to_string()),
            _ => {
                return Err(CompilerError::Lex {
                    message: format!(
                        "unexpected character '{}' at {}:{}",
                        self.peek_char(0).unwrap_or('\0'),
                        span_start.line,
                        span_start.column
                    ),
                });
            }
        };

        for _ in 0..lexeme.chars().count() {
            self.bump_char();
        }

        Ok(Token {
            kind,
            lexeme,
            span: self.finish_span(span_start),
        })
    }

    fn current_span_start(&self) -> Span {
        Span {
            start: self.byte_index(),
            end: self.byte_index(),
            line: self.line,
            column: self.column,
        }
    }

    fn finish_span(&self, start: Span) -> Span {
        Span {
            start: start.start,
            end: self.byte_index(),
            line: start.line,
            column: start.column,
        }
    }

    fn byte_index(&self) -> usize {
        self.source
            .char_indices()
            .nth(self.index)
            .map(|(idx, _)| idx)
            .unwrap_or(self.source.len())
    }

    fn peek_char(&self, offset: usize) -> Option<char> {
        self.chars.get(self.index + offset).copied()
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.chars.get(self.index).copied()?;
        self.index += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn is_long_bracket_start(&self) -> bool {
        self.peek_char(0) == Some('[')
            && self.long_bracket_equals_with_prefix(0, self.long_bracket_equals())
    }

    fn long_bracket_equals(&self) -> usize {
        let mut count = 0;
        while self.peek_char(1 + count) == Some('=') {
            count += 1;
        }
        count
    }

    fn long_bracket_equals_with_prefix(&self, prefix: usize, equals_count: usize) -> bool {
        if self.peek_char(prefix) != Some('[') && self.peek_char(prefix) != Some(']') {
            return false;
        }
        for idx in 0..equals_count {
            if self.peek_char(prefix + 1 + idx) != Some('=') {
                return false;
            }
        }
        self.peek_char(prefix + 1 + equals_count)
            == Some(if self.peek_char(prefix) == Some('[') {
                '['
            } else {
                ']'
            })
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

impl fmt::Display for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Keyword::And => "and",
            Keyword::Break => "break",
            Keyword::Case => "case",
            Keyword::Catch => "catch",
            Keyword::Comptime => "comptime",
            Keyword::Const => "const",
            Keyword::Continue => "continue",
            Keyword::Default => "default",
            Keyword::Do => "do",
            Keyword::Else => "else",
            Keyword::ElseIf => "elseif",
            Keyword::End => "end",
            Keyword::Enum => "enum",
            Keyword::Extends => "extends",
            Keyword::Fallthrough => "fallthrough",
            Keyword::False => "false",
            Keyword::Fire => "fire",
            Keyword::For => "for",
            Keyword::Function => "function",
            Keyword::If => "if",
            Keyword::In => "in",
            Keyword::Local => "local",
            Keyword::Match => "match",
            Keyword::Nil => "nil",
            Keyword::Object => "object",
            Keyword::On => "on",
            Keyword::Not => "not",
            Keyword::Once => "once",
            Keyword::Or => "or",
            Keyword::Repeat => "repeat",
            Keyword::Return => "return",
            Keyword::Signal => "signal",
            Keyword::Spawn => "spawn",
            Keyword::State => "state",
            Keyword::Switch => "switch",
            Keyword::Task => "task",
            Keyword::Then => "then",
            Keyword::True => "true",
            Keyword::Type => "type",
            Keyword::Until => "until",
            Keyword::Watch => "watch",
            Keyword::Yield => "yield",
            Keyword::While => "while",
            Keyword::Export => "export",
        };
        write!(f, "{text}")
    }
}
