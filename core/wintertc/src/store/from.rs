//! All methods for serializing a [`JsValue`] into a [`NodeId`].

use crate::store::{NodeId, StringStore, ValueStoreInner, unsupported_type};
use boa_engine::builtins::array_buffer::{AlignedVec, ArrayBuffer};
use boa_engine::builtins::error::Error;
use boa_engine::object::builtins::{
    JsArray, JsArrayBuffer, JsDataView, JsDate, JsMap, JsRegExp, JsSet, JsSharedArrayBuffer,
    JsTypedArray,
};
use boa_engine::property::PropertyKey;
use boa_engine::{Context, JsError, JsObject, JsResult, JsString, JsValue, JsVariant, js_error};
use rustc_hash::{FxHashMap, FxHashSet};

/// A Map of seen objects when walking through the value. We use the address
/// of the inner object as it is unique per JavaScript value.
#[derive(Default)]
pub(super) struct SeenMap {
    objects: FxHashMap<JsObject, NodeId>,
    pub(super) nodes: Vec<ValueStoreInner>,
    bytes: usize,
    storage: bool,
}

impl SeenMap {
    fn unsupported(&self) -> JsError {
        if self.storage {
            super::StorageDataCloneError::error()
        } else {
            unsupported_type()
        }
    }
    pub(super) fn new(storage: bool) -> Self {
        Self {
            storage,
            ..Self::default()
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.nodes.capacity() * size_of::<ValueStoreInner>()
            + self
                .nodes
                .iter()
                .map(ValueStoreInner::retained_payload_bytes)
                .sum::<usize>()
            + 64
    }
    fn get(&self, object: &JsObject) -> Option<NodeId> {
        self.objects.get(object).copied()
    }
    fn insert(&mut self, original: &JsObject, object: NodeId) {
        self.objects.insert(original.clone(), object);
    }
    fn charge(&mut self, bytes: usize) -> JsResult<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= 16 * 1024 * 1024)
            .ok_or_else(|| js_error!(RangeError: "Structured clone byte limit exceeded"))?;
        Ok(())
    }
    fn push(&mut self, value: ValueStoreInner) -> JsResult<NodeId> {
        if self.nodes.len() >= 65536 {
            return Err(js_error!(RangeError: "Structured clone node limit exceeded"));
        }
        let id = self.nodes.len();
        self.nodes.push(value);
        Ok(id)
    }
}

/// Return true if an object is transferable.
pub(super) fn is_transferable(object: &JsObject) -> bool {
    // The only transferable object supported for now is ArrayBuffer.
    object.downcast_mut::<ArrayBuffer>().is_some()
}

/// The core logic of the [`super::JsValueStore::try_from_js`] function.
fn try_from_js_object(
    value: &JsObject,
    transfer: &FxHashSet<JsObject>,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    // Have we seen this object? If so, return its clone.
    if let Some(o2) = seen.get(value) {
        return Ok(o2);
    }

    // Is it a transferable object?
    let new_value = if transfer.contains(value) {
        try_from_js_object_transfer(value, seen, context)?
    } else {
        try_from_js_object_clone(value, transfer, seen, context)?
    };

    Ok(new_value)
}

/// Transfer an object into a store instead of cloning it. See [mdn].
///
/// Only [transferable objects][to] can be transferred. Anything else will return an
/// error. Since any object t
///
/// [mdn]: https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Transferable_objects
/// [to]: https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Transferable_objects#supported_objects
fn try_from_js_object_transfer(
    object: &JsObject,
    seen: &mut SeenMap,
    _context: &mut Context,
) -> JsResult<NodeId> {
    if let Some(mut buffer) = object.downcast_mut::<ArrayBuffer>() {
        seen.charge(buffer.data().ok_or_else(|| seen.unsupported())?.len())?;
        let data = buffer.detach(&JsValue::undefined())?;
        let data = data.ok_or_else(|| seen.unsupported())?;

        let node = seen.push(ValueStoreInner::ArrayBuffer(data))?;
        seen.insert(object, node);
        Ok(node)
    } else {
        Err(seen.unsupported())
    }
}

fn try_from_array_clone(
    array: &JsArray,
    transfer: &FxHashSet<JsObject>,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    // Create an empty clone, we will replace its inner values after we gather them.
    // To stop the recursion, we need to add the right value to the seen map prior,
    // though.
    let dolly = seen.push(ValueStoreInner::Empty)?;
    seen.insert(&JsObject::from(array.clone()), dolly);

    let length = array.length(context)?;
    let length = usize::try_from(length).map_err(JsError::from_rust)?;
    if length > 65536 {
        return Err(js_error!(RangeError: "Structured clone array limit exceeded"));
    }
    let fields = own_fields(&JsObject::from(array.clone()), transfer, seen, context)?;
    seen.nodes[dolly] = ValueStoreInner::Array {
        length: length as u64,
        fields,
    };
    Ok(dolly)
}

fn try_from_array_buffer_clone(
    original: &JsObject,
    buffer: &JsArrayBuffer,
    seen: &mut SeenMap,
) -> JsResult<NodeId> {
    let data = buffer.data().ok_or_else(|| seen.unsupported())?;
    seen.charge(data.len())?;
    let data = AlignedVec::from_slice(0, &data);
    let new_value = seen.push(ValueStoreInner::ArrayBuffer(data))?;
    seen.insert(original, new_value);

    Ok(new_value)
}

fn try_from_shared_array_buffer(
    original: &JsObject,
    buffer: &JsSharedArrayBuffer,
    seen: &mut SeenMap,
) -> JsResult<NodeId> {
    let new_value = seen.push(ValueStoreInner::SharedArrayBuffer(buffer.inner()))?;
    seen.insert(original, new_value);
    Ok(new_value)
}

fn clone_typed_array(
    original: &JsObject,
    buffer: &JsTypedArray,
    transfer: &FxHashSet<JsObject>,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let backing = buffer.buffer(context)?;
    let transferred_length = backing
        .as_object()
        .filter(|object| transfer.contains(object))
        .and_then(|object| seen.get(&object))
        .and_then(|node| match &seen.nodes[node] {
            ValueStoreInner::ArrayBuffer(data) => Some(data.len()),
            _ => None,
        });
    let (byte_offset, length) = if let Some(bytes) = transferred_length {
        buffer
            .shape_for_buffer_length(bytes)
            .map_err(|_| seen.unsupported())?
    } else {
        buffer.validate_view().map_err(|_| seen.unsupported())?;
        (buffer.byte_offset(context)?, buffer.length(context)?)
    };
    let kind = buffer.kind().ok_or_else(|| seen.unsupported())?;
    let buffer = backing;
    let buffer = try_from_js_value(&buffer, transfer, seen, context)?;
    let dolly = seen.push(ValueStoreInner::TypedArray {
        kind,
        buffer,
        byte_offset,
        length,
    })?;
    seen.insert(original, dolly);
    Ok(dolly)
}

fn clone_date(
    original: &JsObject,
    date: &JsDate,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let ms_since_epoch = date
        .get_time(context)?
        .as_number()
        .ok_or_else(|| seen.unsupported())?;

    let stored = seen.push(ValueStoreInner::Date(ms_since_epoch))?;
    seen.insert(original, stored);
    Ok(stored)
}

fn clone_regexp(
    original: &JsObject,
    regexp: &JsRegExp,
    seen: &mut SeenMap,
    _context: &mut Context,
) -> JsResult<NodeId> {
    let (source, flags) = regexp.pattern_and_flags();
    seen.charge(source.len().saturating_add(flags.len()).saturating_mul(2))?;
    let (source, flags) = (source.into(), flags.into());

    let stored = seen.push(ValueStoreInner::RegExp { source, flags })?;
    seen.insert(original, stored);
    Ok(stored)
}

fn try_from_map(
    original: &JsObject,
    map: &JsMap,
    transfer: &FxHashSet<JsObject>,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let mut new_map = Vec::new();
    let store = seen.push(ValueStoreInner::Empty)?;
    seen.insert(original, store);

    let mut entries = Vec::new();
    map.for_each_native(|key, value| {
        if entries.len() >= 65536 {
            return Err(js_error!(RangeError: "Structured clone map limit exceeded"));
        }
        seen.charge(2 * size_of::<NodeId>())?;
        entries.push((key, value));
        Ok(())
    })?;
    for (k, v) in entries {
        let key = try_from_js_value(&k, transfer, seen, context)?;
        let value = try_from_js_value(&v, transfer, seen, context)?;
        new_map.push((key, value));
    }

    seen.nodes[store] = ValueStoreInner::Map(new_map);

    Ok(store)
}

fn try_from_set(
    original: &JsObject,
    set: &JsSet,
    transfer: &FxHashSet<JsObject>,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let mut new_set = Vec::new();
    let store = seen.push(ValueStoreInner::Empty)?;
    seen.insert(original, store);

    let mut entries = Vec::new();
    set.for_each_native(|value| {
        if entries.len() >= 65536 {
            return Err(js_error!(RangeError: "Structured clone set limit exceeded"));
        }
        seen.charge(size_of::<NodeId>())?;
        entries.push(value);
        Ok(())
    })?;
    for v in entries {
        let value = try_from_js_value(&v, transfer, seen, context)?;
        new_set.push(value);
    }

    seen.nodes[store] = ValueStoreInner::Set(new_set);

    Ok(store)
}

fn try_from_js_object_clone(
    object: &JsObject,
    transfer: &FxHashSet<JsObject>,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    // If this is a special type of object, apply some special rules to it.
    // Described in
    // https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Structured_clone_algorithm#supported_types

    if let Ok(array) = JsArray::from_object(object.clone()) {
        return try_from_array_clone(&array, transfer, seen, context);
    } else if let Ok(map) = JsMap::from_object(object.clone()) {
        return try_from_map(object, &map, transfer, seen, context);
    } else if let Ok(set) = JsSet::from_object(object.clone()) {
        return try_from_set(object, &set, transfer, seen, context);
    } else if let Ok(ref buffer) = JsArrayBuffer::from_object(object.clone()) {
        return try_from_array_buffer_clone(object, buffer, seen);
    } else if let Ok(ref buffer) = JsSharedArrayBuffer::from_object(object.clone()) {
        if seen.storage {
            return Err(seen.unsupported());
        }
        return try_from_shared_array_buffer(object, buffer, seen);
    } else if let Ok(ref typed_array) = JsTypedArray::from_object(object.clone()) {
        return clone_typed_array(object, typed_array, transfer, seen, context);
    } else if let Ok(ref date) = JsDate::from_object(object.clone()) {
        return clone_date(object, date, seen, context);
    } else if let Ok(_error) = object.clone().downcast::<Error>() {
        return Err(seen.unsupported());
    } else if let Ok(ref regexp) = JsRegExp::from_object(object.clone()) {
        return clone_regexp(object, regexp, seen, context);
    } else if let Ok(view) = JsDataView::from_object(object.clone()) {
        let byte_length = view.byte_length(context).map_err(|_| seen.unsupported())?;
        let byte_offset = view.byte_offset(context).map_err(|_| seen.unsupported())?;
        let buffer = view.buffer(context)?;
        let buffer = try_from_js_value(&buffer, transfer, seen, context)?;
        let node = seen.push(ValueStoreInner::DataView {
            buffer,
            byte_length,
            byte_offset,
        })?;
        seen.insert(object, node);
        return Ok(node);
    } else if object.is_callable() {
        // Functions are invalid.
        return Err(seen.unsupported());
    }

    if seen.storage && !object.is_ordinary() {
        return Err(seen.unsupported());
    }

    // Create a new object and add own properties to it. This does not preserve
    // the prototype (nor do we want to).
    let dolly = seen.push(ValueStoreInner::Empty)?;
    seen.insert(object, dolly);

    let fields = own_fields(object, transfer, seen, context)?;
    seen.nodes[dolly] = ValueStoreInner::Object(fields);
    Ok(dolly)
}

fn own_fields(
    object: &JsObject,
    transfer: &FxHashSet<JsObject>,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<Vec<(StringStore, NodeId)>> {
    let keys = object.own_property_keys(context)?;
    if keys.len() > 65536 {
        return Err(js_error!(RangeError: "Structured clone field limit exceeded"));
    }
    let mut fields: Vec<(StringStore, NodeId)> = Vec::new();
    for k in keys {
        if matches!(k, PropertyKey::Symbol(_)) {
            continue;
        }
        let enumerable = object
            .borrow()
            .properties()
            .get(&k)
            .is_some_and(|descriptor| descriptor.enumerable() == Some(true));
        if !enumerable {
            continue;
        }
        let value = object.get(k.clone(), context)?;
        let key = match k {
            PropertyKey::String(s) => {
                seen.charge(s.len().saturating_mul(2))?;
                StringStore::from(s)
            }
            PropertyKey::Symbol(_) => return Err(seen.unsupported()),
            PropertyKey::Index(i) => {
                let key = JsString::from(format!("{}", i.get()));
                seen.charge(key.len() * 2)?;
                key.into()
            }
        };

        let v = try_from_js_value(&value, transfer, seen, context)?;
        seen.charge(size_of::<(StringStore, NodeId)>())?;
        fields.push((key, v));
    }

    Ok(fields)
}

pub(super) fn try_from_js_value(
    value: &JsValue,
    transfer: &FxHashSet<JsObject>,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let _depth = super::Traversal::enter()?;
    match value.variant() {
        JsVariant::Null => seen.push(ValueStoreInner::Null),
        JsVariant::Undefined => seen.push(ValueStoreInner::Undefined),
        JsVariant::Boolean(b) => seen.push(ValueStoreInner::Boolean(b)),
        JsVariant::String(s) => {
            seen.charge(s.len().saturating_mul(2))?;
            seen.push(ValueStoreInner::String(s.into()))
        }
        JsVariant::Float64(f) => seen.push(ValueStoreInner::Float(f)),
        JsVariant::Integer32(i) => seen.push(ValueStoreInner::Float(f64::from(i))),
        JsVariant::BigInt(b) => {
            let bytes =
                usize::try_from(b.as_inner().bits().div_ceil(8)).map_err(JsError::from_rust)?;
            seen.charge(bytes)?;
            seen.push(ValueStoreInner::BigInt(b.as_inner().clone()))
        }
        JsVariant::Object(ref o) => try_from_js_object(o, transfer, seen, context),

        // Symbols cannot be transferred/cloned.
        JsVariant::Symbol(_) => Err(seen.unsupported()),
    }
}
