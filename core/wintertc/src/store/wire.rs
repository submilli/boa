//! Versioned storage transport for context-free graphs. Decoding never runs JS.
use super::{JsValueStore, StringStore, ValueStoreInner as Node};
use boa_engine::{bigint::RawBigInt, builtins::array_buffer::AlignedVec};
use std::sync::Arc;

mod kinds;
mod validate;

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_NODES: usize = 65_536;
const MAGIC: &[u8] = b"BVS\x01";

/// Invalid, unsupported or over-budget persisted storage data. Contains no page data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageWireError;
impl std::fmt::Display for StorageWireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid persisted storage data")
    }
}
impl std::error::Error for StorageWireError {}

type Result<T> = std::result::Result<T, StorageWireError>;
impl JsValueStore {
    /// Encode without getters, transfers or shared memory. The format is versioned.
    ///
    /// # Errors
    /// Rejects non-storage graphs or an encoded payload larger than 16 mebibytes.
    pub fn to_storage_bytes(&self) -> Result<Vec<u8>> {
        let mut writer = Writer(Vec::new());
        writer.raw(MAGIC)?;
        writer.number(self.root as u64)?;
        writer.number(self.graph.len() as u64)?;
        for node in self.graph.iter() {
            writer.node(node)?;
        }
        Ok(writer.0)
    }

    /// Admit and reconstruct a bounded context-free storage graph.
    ///
    /// # Errors
    /// Rejects unknown versions/tags, malformed references, unsupported shared
    /// memory, excessive allocations/depth and invalid binary-view geometry.
    pub fn from_storage_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES || !bytes.starts_with(MAGIC) {
            return Err(StorageWireError);
        }
        let mut reader = Reader {
            bytes: &bytes[MAGIC.len()..],
            retained: 0,
        };
        let root = reader.index()?;
        let count = reader.length(size_of::<Node>(), MAX_NODES)?;
        if count == 0 || count > MAX_NODES || root >= count {
            return Err(StorageWireError);
        }
        let mut graph = Vec::with_capacity(count);
        for _ in 0..count {
            graph.push(reader.node()?);
        }
        if !reader.bytes.is_empty() {
            return Err(StorageWireError);
        }
        validate::graph(&graph, root)?;
        let retained_bytes = graph.capacity() * size_of::<Node>()
            + graph
                .iter()
                .map(Node::retained_payload_bytes)
                .sum::<usize>();
        if retained_bytes > MAX_BYTES {
            return Err(StorageWireError);
        }
        Ok(Self {
            graph: Arc::new(graph),
            root,
            retained_bytes,
        })
    }
}

struct Writer(Vec<u8>);
impl Writer {
    fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        if self.0.len().saturating_add(bytes.len()) > MAX_BYTES {
            return Err(StorageWireError);
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn tag(&mut self, tag: u8) -> Result<()> {
        self.raw(&[tag])
    }
    fn number(&mut self, value: u64) -> Result<()> {
        self.raw(&value.to_le_bytes())
    }
    fn optional(&mut self, value: Option<u64>) -> Result<()> {
        self.tag(u8::from(value.is_some()))?;
        if let Some(value) = value {
            self.number(value)?;
        }
        Ok(())
    }
    fn string(&mut self, text: &StringStore) -> Result<()> {
        self.number(text.0.len() as u64)?;
        for unit in &text.0 {
            self.raw(&unit.to_le_bytes())?;
        }
        Ok(())
    }
    fn optional_string(&mut self, text: Option<&StringStore>) -> Result<()> {
        self.tag(u8::from(text.is_some()))?;
        if let Some(text) = text {
            self.string(text)?;
        }
        Ok(())
    }
    fn fields(&mut self, fields: &[(StringStore, usize)]) -> Result<()> {
        self.number(fields.len() as u64)?;
        for (name, value) in fields {
            self.string(name)?;
            self.number(*value as u64)?;
        }
        Ok(())
    }
    fn map_entries(&mut self, entries: &[(usize, usize)]) -> Result<()> {
        self.tag(8)?;
        self.number(entries.len() as u64)?;
        for (key, value) in entries {
            self.number(*key as u64)?;
            self.number(*value as u64)?;
        }
        Ok(())
    }
    fn node(&mut self, node: &Node) -> Result<()> {
        match node {
            Node::Empty | Node::SharedArrayBuffer(_) => return Err(StorageWireError),
            Node::Null => self.tag(0)?,
            Node::Undefined => self.tag(1)?,
            Node::Boolean(value) => {
                self.tag(2)?;
                self.tag(u8::from(*value))?;
            }
            Node::Float(value) => {
                self.tag(3)?;
                self.number(value.to_bits())?;
            }
            Node::String(value) => {
                self.tag(4)?;
                self.string(value)?;
            }
            Node::BigInt(value) => {
                self.tag(5)?;
                let bytes = value.to_signed_bytes_le();
                self.number(bytes.len() as u64)?;
                self.raw(&bytes)?;
            }
            Node::Boxed(value) => {
                self.tag(6)?;
                self.number(*value as u64)?;
            }
            Node::Object(fields) => {
                self.tag(7)?;
                self.fields(fields)?;
            }
            Node::Map(entries) => self.map_entries(entries)?,
            Node::Set(entries) => {
                self.tag(9)?;
                self.number(entries.len() as u64)?;
                for value in entries {
                    self.number(*value as u64)?;
                }
            }
            Node::Array { length, fields } => {
                self.tag(10)?;
                self.number(*length)?;
                self.fields(fields)?;
            }
            Node::Date(value) => {
                self.tag(11)?;
                self.number(value.to_bits())?;
            }
            Node::Error {
                kind,
                message,
                stack,
                cause,
            } => {
                self.tag(12)?;
                self.tag(kinds::error_tag(*kind)?)?;
                self.optional_string(message.as_ref())?;
                self.optional_string(stack.as_ref())?;
                self.optional(cause.map(|v| v as u64))?;
            }
            Node::RegExp { source, flags } => {
                self.tag(13)?;
                self.string(source)?;
                self.string(flags)?;
            }
            Node::ArrayBuffer {
                data,
                max_byte_length,
            } => {
                self.tag(14)?;
                self.number(data.len() as u64)?;
                self.raw(data)?;
                self.optional(*max_byte_length)?;
            }
            Node::DataView {
                buffer,
                byte_length,
                byte_offset,
            } => {
                self.tag(15)?;
                self.number(*buffer as u64)?;
                self.optional(*byte_length)?;
                self.number(*byte_offset)?;
            }
            Node::TypedArray {
                kind,
                buffer,
                byte_offset,
                length,
            } => {
                self.tag(16)?;
                self.tag(kinds::array_tag(*kind)?)?;
                self.number(*buffer as u64)?;
                self.number(*byte_offset as u64)?;
                self.optional(length.map(|v| v as u64))?;
            }
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    retained: usize,
}
impl<'a> Reader<'a> {
    fn raw(&mut self, count: usize) -> Result<&'a [u8]> {
        let (value, rest) = self.bytes.split_at_checked(count).ok_or(StorageWireError)?;
        self.bytes = rest;
        Ok(value)
    }
    fn tag(&mut self) -> Result<u8> {
        Ok(self.raw(1)?[0])
    }
    fn boolean(&mut self) -> Result<bool> {
        match self.tag()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StorageWireError),
        }
    }
    fn number(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.raw(8)?.try_into().map_err(|_| StorageWireError)?,
        ))
    }
    fn index(&mut self) -> Result<usize> {
        usize::try_from(self.number()?).map_err(|_| StorageWireError)
    }
    fn optional(&mut self) -> Result<Option<u64>> {
        if self.boolean()? {
            self.number().map(Some)
        } else {
            Ok(None)
        }
    }
    fn length(&mut self, unit: usize, maximum: usize) -> Result<usize> {
        let count = self.index()?;
        self.retained = self
            .retained
            .checked_add(count.checked_mul(unit).ok_or(StorageWireError)?)
            .ok_or(StorageWireError)?;
        if self.retained > MAX_BYTES || count > maximum {
            return Err(StorageWireError);
        }
        Ok(count)
    }
    fn string(&mut self) -> Result<StringStore> {
        let count = self.length(2, MAX_BYTES / 2)?;
        let bytes = self.raw(count.checked_mul(2).ok_or(StorageWireError)?)?;
        Ok(StringStore(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect(),
        ))
    }
    fn optional_string(&mut self) -> Result<Option<StringStore>> {
        if self.boolean()? {
            self.string().map(Some)
        } else {
            Ok(None)
        }
    }
    fn fields(&mut self) -> Result<Vec<(StringStore, usize)>> {
        let count = self.length(size_of::<(StringStore, usize)>(), MAX_NODES)?;
        // Every field needs at least its string length and reference.
        if count > self.bytes.len() / 16 {
            return Err(StorageWireError);
        }
        (0..count)
            .map(|_| Ok((self.string()?, self.index()?)))
            .collect()
    }
    fn node(&mut self) -> Result<Node> {
        Ok(match self.tag()? {
            0 => Node::Null,
            1 => Node::Undefined,
            2 => Node::Boolean(self.boolean()?),
            3 => Node::Float(f64::from_bits(self.number()?)),
            4 => Node::String(self.string()?),
            5 => {
                let count = self.length(2, MAX_BYTES / 2)?;
                Node::BigInt(RawBigInt::from_signed_bytes_le(self.raw(count)?))
            }
            6 => Node::Boxed(self.index()?),
            7 => Node::Object(self.fields()?),
            8 => {
                let count = self.length(size_of::<(usize, usize)>(), MAX_NODES)?;
                if count > self.bytes.len() / 16 {
                    return Err(StorageWireError);
                }
                Node::Map(
                    (0..count)
                        .map(|_| Ok((self.index()?, self.index()?)))
                        .collect::<Result<_>>()?,
                )
            }
            9 => {
                let count = self.length(size_of::<usize>(), MAX_NODES)?;
                if count > self.bytes.len() / 8 {
                    return Err(StorageWireError);
                }
                Node::Set((0..count).map(|_| self.index()).collect::<Result<_>>()?)
            }
            10 => Node::Array {
                length: self.number()?,
                fields: self.fields()?,
            },
            11 => Node::Date(f64::from_bits(self.number()?)),
            12 => Node::Error {
                kind: kinds::error_kind(self.tag()?)?,
                message: self.optional_string()?,
                stack: self.optional_string()?,
                cause: self
                    .optional()?
                    .map(usize::try_from)
                    .transpose()
                    .map_err(|_| StorageWireError)?,
            },
            13 => Node::RegExp {
                source: self.string()?,
                flags: self.string()?,
            },
            14 => {
                let count = self.length(1, MAX_BYTES)?;
                let data = AlignedVec::from_slice(0, self.raw(count)?);
                Node::ArrayBuffer {
                    data,
                    max_byte_length: self.optional()?,
                }
            }
            15 => Node::DataView {
                buffer: self.index()?,
                byte_length: self.optional()?,
                byte_offset: self.number()?,
            },
            16 => Node::TypedArray {
                kind: kinds::array_kind(self.tag()?)?,
                buffer: self.index()?,
                byte_offset: self.index()?,
                length: self
                    .optional()?
                    .map(usize::try_from)
                    .transpose()
                    .map_err(|_| StorageWireError)?,
            },
            _ => return Err(StorageWireError),
        })
    }
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
