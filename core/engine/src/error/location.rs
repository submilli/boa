//! Typed source coordinates, independent of script-visible stack properties.

use std::path::{Path, PathBuf};

use boa_ast::Position;

use super::JsError;
use crate::vm::{shadow_stack::ShadowEntry, source_info::SourcePath};

/// The JavaScript source associated with an error. Native Rust locations are
/// never returned. Positions, when available, are one-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    path: Option<PathBuf>,
    position: Option<Position>,
    parser: bool,
}

impl SourceLocation {
    pub(super) const fn is_parser(&self) -> bool {
        self.parser
    }

    /// The source name supplied by the embedder, if any.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// One-based source coordinates, absent when the parser has no position.
    #[must_use]
    pub const fn position(&self) -> Option<Position> {
        self.position
    }

    fn from_frame(frame: &ShadowEntry) -> Option<Self> {
        let ShadowEntry::Bytecode { pc, source_info } = frame else {
            return None;
        };
        Some(Self {
            path: match source_info.map().path() {
                SourcePath::Path(path) => Some(path.to_path_buf()),
                _ => None,
            },
            position: source_info.map().find(*pc),
            parser: false,
        })
    }
}

impl JsError {
    pub(crate) fn capture_throw_location(&mut self, context: &crate::Context) {
        self.source_location = context
            .vm
            .shadow_stack
            .take(1, context.vm.frame().pc)
            .iter()
            .rev()
            .find_map(SourceLocation::from_frame);
    }

    /// Returns typed JavaScript source metadata without accessing properties or
    /// running script. Parser metadata takes precedence over a calling frame.
    #[must_use]
    pub fn source_location(&self) -> Option<SourceLocation> {
        self.source_location.clone().or_else(|| {
            self.backtrace
                .as_ref()?
                .iter()
                .rev()
                .find_map(SourceLocation::from_frame)
        })
    }

    pub(crate) fn with_source_path(mut self, path: Option<PathBuf>) -> Self {
        self.source_location
            .get_or_insert(SourceLocation {
                path: None,
                position: None,
                parser: true,
            })
            .path = path;
        self
    }
}

pub(super) fn parser_location(error: &boa_parser::Error) -> SourceLocation {
    use boa_parser::{Error, lexer::Error as LexError};
    let position = match error {
        Error::Expected { span, .. } | Error::Unexpected { span, .. } => Some(span.start()),
        Error::General { position, .. }
        | Error::Lex {
            err: LexError::Syntax(_, position),
        } => Some(*position),
        Error::AbruptEnd
        | Error::ScopeAnalysis { .. }
        | Error::Lex {
            err: LexError::IO(_),
        } => None,
    };
    SourceLocation {
        path: None,
        position,
        parser: true,
    }
}
