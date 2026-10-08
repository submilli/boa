//! Explicit wire tags remain stable independently of engine enum layout.
use super::{Result, StorageWireError};
use boa_engine::builtins::{error::ErrorKind, typed_array::TypedArrayKind};

const ERRORS: [ErrorKind; 8] = [
    ErrorKind::Aggregate,
    ErrorKind::Error,
    ErrorKind::Eval,
    ErrorKind::Type,
    ErrorKind::Range,
    ErrorKind::Reference,
    ErrorKind::Syntax,
    ErrorKind::Uri,
];
const ARRAYS: [TypedArrayKind; 12] = [
    TypedArrayKind::Int8,
    TypedArrayKind::Uint8,
    TypedArrayKind::Uint8Clamped,
    TypedArrayKind::Int16,
    TypedArrayKind::Uint16,
    TypedArrayKind::Int32,
    TypedArrayKind::Uint32,
    TypedArrayKind::BigInt64,
    TypedArrayKind::BigUint64,
    TypedArrayKind::Float32,
    TypedArrayKind::Float64,
    TypedArrayKind::Float16,
];
pub(super) fn error_tag(kind: ErrorKind) -> Result<u8> {
    ERRORS
        .iter()
        .position(|v| *v == kind)
        .and_then(|i| u8::try_from(i).ok())
        .ok_or(StorageWireError)
}
pub(super) fn error_kind(tag: u8) -> Result<ErrorKind> {
    ERRORS
        .get(usize::from(tag))
        .copied()
        .ok_or(StorageWireError)
}
pub(super) fn array_tag(kind: TypedArrayKind) -> Result<u8> {
    ARRAYS
        .iter()
        .position(|v| *v == kind)
        .and_then(|i| u8::try_from(i).ok())
        .ok_or(StorageWireError)
}
pub(super) fn array_kind(tag: u8) -> Result<TypedArrayKind> {
    ARRAYS
        .get(usize::from(tag))
        .copied()
        .ok_or(StorageWireError)
}
pub(super) fn element_size(kind: TypedArrayKind) -> usize {
    match kind {
        TypedArrayKind::Int8 | TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => 1,
        TypedArrayKind::Int16 | TypedArrayKind::Uint16 | TypedArrayKind::Float16 => 2,
        TypedArrayKind::Int32 | TypedArrayKind::Uint32 | TypedArrayKind::Float32 => 4,
        _ => 8,
    }
}
