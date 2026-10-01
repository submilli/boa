//! Module containing the types related to the [`JsValueStore`].
use boa_engine::bigint::RawBigInt;
use boa_engine::builtins::array_buffer::{AlignedVec, SharedArrayBuffer};
use boa_engine::builtins::error::ErrorKind;
use boa_engine::builtins::typed_array::TypedArrayKind;
use boa_engine::value::TryIntoJs;
use boa_engine::{Context, JsError, JsResult, JsString, JsValue, JsVariant, js_error};
use rustc_hash::FxHashSet;
use std::sync::Arc;

mod from;
mod to;

/// Convenience method to avoid copy-pasting the same message.
#[inline]
fn unsupported_type() -> JsError {
    js_error!(Error: "DataCloneError: unsupported type for structured data")
}

#[inline]
fn unsupported_transfer() -> JsError {
    js_error!(TypeError: "Found an invalid value in transferList")
}

/// Native marker for storage serialization rejection. It contains no page data.
#[derive(Debug, boa_engine::Trace, boa_engine::Finalize, boa_engine::JsData)]
pub struct StorageDataCloneError;

impl StorageDataCloneError {
    /// Distinguish an engine rejection from an exception thrown by a getter.
    #[must_use]
    pub fn is_error(error: &JsError) -> bool {
        error
            .as_opaque()
            .and_then(JsValue::as_object)
            .is_some_and(|object| object.is::<Self>())
    }
    fn error() -> JsError {
        JsError::from_opaque(boa_engine::JsObject::from_proto_and_data(None, Self).into())
    }
}

/// A type to help store [`JsString`]. Because [`JsString`] relies on [`std::rc::Rc`],
/// it cannot be `Send`, which is a necessary contract for the Store. The [`StringStore`]
/// can be transformed from and into `JsString`, but owns its data. It is _not_ copy-on-
/// write.
#[derive(Debug, Eq, PartialEq, Hash)]
struct StringStore(Vec<u16>);

impl StringStore {
    fn to_js_string(&self) -> JsString {
        JsString::from(self.0.as_slice())
    }
}

impl From<JsString> for StringStore {
    fn from(value: JsString) -> Self {
        Self(value.to_vec())
    }
}

impl From<StringStore> for JsString {
    fn from(value: StringStore) -> Self {
        JsString::from(value.0.as_slice())
    }
}

/// Inner value for [`JsValueStore`].
#[derive(Debug)]
enum ValueStoreInner {
    /// An Empty value that will be filled later. This is only used during
    /// construction, and if encountered at other points will result
    /// in an error.
    Empty,

    /// Primitive values - `null`.
    Null,

    /// Primitive values - `undefined`.
    Undefined,

    /// Primitive values - `Boolean`.
    Boolean(bool),

    /// Primitive values - `float64`. No need to store integers separately,
    /// they'll be checked when recreating the `JsValue`.
    Float(f64),

    /// [`JsString`]s are context-free, but not `Send`. Since we want to be
    /// `Send`, we'll have to make a copy of the data.
    String(StringStore),

    /// [`boa_engine::JsBigInt`]s are context-free but not `Send`. The Raw version
    /// of it is, though.
    BigInt(RawBigInt),

    /// A dictionary of strings to values which should be reconstructed into
    /// a `JsObject`. Note: the prototype and constructor are not maintained,
    /// and during reconstruction the default `Object` prototype will be used.
    Object(Vec<(StringStore, NodeId)>),

    /// A `Map()` object in JavaScript.
    Map(Vec<(NodeId, NodeId)>),

    /// A `Set()` object in JavaScript. The elements are already unique at
    /// construction.
    Set(Vec<NodeId>),

    /// An `Array` object in JavaScript.
    Array {
        length: u64,
        fields: Vec<(StringStore, NodeId)>,
    },

    /// A `Date` object in JavaScript. Although this can be marshaled, it uses
    /// the system's datetime library to be reconstructed and may diverge.
    Date(f64),

    /// Allowed error types (see the structured clone algorithm page).
    #[expect(unused)]
    Error {
        kind: ErrorKind,
        name: StringStore,
        message: StringStore,
        stack: StringStore,
        cause: StringStore,
    },

    /// Regular expression. We store the expression and its flags. Everything else
    /// is reset. These are extracted as `String`, so we don't need to use the
    /// [`StringStore`] type.
    RegExp {
        source: StringStore,
        flags: StringStore,
    },

    /// Array Buffer.
    ArrayBuffer(AlignedVec<u8>),

    /// Shared Array Buffer.
    SharedArrayBuffer(SharedArrayBuffer),

    /// Dataview.
    DataView {
        buffer: NodeId,
        byte_length: u64,
        byte_offset: u64,
    },

    /// Typed Array, including its kind and data.
    TypedArray {
        kind: TypedArrayKind,
        buffer: NodeId,
        byte_offset: usize,
        length: usize,
    },
}

impl ValueStoreInner {
    fn retained_payload_bytes(&self) -> usize {
        match self {
            Self::String(value) => value.0.capacity() * 2,
            Self::BigInt(value) => {
                usize::try_from(value.bits().div_ceil(8))
                    .expect("serialized BigInt passed the byte limit")
                    * 2
                    + 32
            }
            Self::Object(fields) | Self::Array { fields, .. } => {
                fields.capacity() * size_of::<(StringStore, NodeId)>()
                    + fields
                        .iter()
                        .map(|(key, _)| key.0.capacity() * 2)
                        .sum::<usize>()
            }
            Self::Map(entries) => entries.capacity() * size_of::<(NodeId, NodeId)>(),
            Self::Set(entries) => entries.capacity() * size_of::<NodeId>(),
            Self::RegExp { source, flags } => (source.0.capacity() + flags.0.capacity()) * 2,
            Self::ArrayBuffer(data) => data.capacity(),
            Self::Error {
                name,
                message,
                stack,
                cause,
                ..
            } => [name, message, stack, cause]
                .iter()
                .map(|s| s.0.capacity() * 2)
                .sum(),
            Self::Empty
            | Self::Null
            | Self::Undefined
            | Self::Boolean(_)
            | Self::Float(_)
            | Self::Date(_)
            | Self::SharedArrayBuffer(_)
            | Self::DataView { .. }
            | Self::TypedArray { .. } => 0,
        }
    }
}

/// A [`JsValue`]-like structure that can rebuild its value given any [`Context`].
/// It essentially stores the value itself and its original type. During
/// reconstruction, the constructors of the new [`Context`] will be used.
///
/// This follows the rules of the [structured clone algorithm][sca], but does not
/// require a [`Context`] to copy/move, and can be [`Send`] between threads.
///
/// It is not serializable as it allows recursive values.
///
/// To transform a [`JsValue`] into a [`JsValueStore`], the application MUST
/// pass in the context of the initial value. To transform it back to a
/// [`JsValue`], the application MUST pass the context that will contain
/// all prototypes for the new types (e.g. Object).
///
/// [sca]: https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Structured_clone_algorithm
#[derive(Debug, Clone)]
pub struct JsValueStore {
    graph: Arc<Vec<ValueStoreInner>>,
    root: NodeId,
    retained_bytes: usize,
}

type NodeId = usize;

impl TryIntoJs for JsValueStore {
    fn try_into_js(&self, context: &mut Context) -> JsResult<JsValue> {
        let mut seen = to::ReverseSeenMap::new(self.graph.clone());
        to::try_value_into_js(self.root, &mut seen, context)
    }
}

impl JsValueStore {
    /// Create a context-free [`JsValue`] equivalent from an existing `JsValue` and the
    /// [`Context`] that was used to create it. The `transfer` argument allows for
    /// transferring ownership of the inner data to the context-free value instead of
    /// cloning it. By default, if a value isn't in the transfer vector, it is cloned.
    ///
    /// # Errors
    /// Any errors related to transferring or cloning a value's inner data.
    pub fn try_from_js(
        value: &JsValue,
        context: &mut Context,
        transfer: Vec<JsValue>,
    ) -> JsResult<Self> {
        Self::serialize(value, context, transfer, false)
    }

    /// Serialize session history state without shared memory or transfers.
    ///
    /// # Errors
    /// Returns a [`StorageDataCloneError`] marker for unsupported values, the
    /// original exception from a getter, or a range error at a resource limit.
    pub fn for_storage(value: &JsValue, context: &mut Context) -> JsResult<Self> {
        Self::serialize(value, context, Vec::new(), true)
    }

    /// Conservative retained graph and payload size for aggregate admission.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    fn serialize(
        value: &JsValue,
        context: &mut Context,
        transfer: Vec<JsValue>,
        storage: bool,
    ) -> JsResult<Self> {
        let mut seen = from::SeenMap::new(storage);
        // Verify the validity of the transfer list and make it a set.
        let transfer = transfer
            .into_iter()
            .map(|v| match v.variant() {
                JsVariant::Object(o) if from::is_transferable(&o) => Ok(o),
                _ => Err(unsupported_transfer()),
            })
            .collect::<Result<FxHashSet<_>, _>>()?;

        let v = from::try_from_js_value(value, &transfer, &mut seen, context)?;
        Ok(Self {
            root: v,
            retained_bytes: seen.retained_bytes(),
            graph: Arc::new(seen.nodes),
        })
    }
}

// Shared across nested structuredClone calls made by getters, including calls
// crossing contexts on the same VM thread. Drop restores the budget on errors.
thread_local! { static ACTIVE_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
struct Traversal;
impl Traversal {
    fn enter() -> JsResult<Self> {
        ACTIVE_DEPTH.with(|depth| {
            if depth.get() >= 64 {
                return Err(js_error!(RangeError: "Structured clone depth exceeded"));
            }
            depth.set(depth.get() + 1);
            Ok(Self)
        })
    }
}
impl Drop for Traversal {
    fn drop(&mut self) {
        ACTIVE_DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}

#[cfg(test)]
mod tests;
