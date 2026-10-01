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
use rustc_hash::FxHashMap;

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
    pub(super) fn reserve_transfers(&mut self, objects: &[JsObject]) -> JsResult<()> {
        for object in objects {
            let node = self.push(ValueStoreInner::Empty)?;
            self.insert(object, node);
        }
        Ok(())
    }

    pub(super) fn finish_transfers(&mut self, objects: &[JsObject]) -> JsResult<()> {
        // All validation and budget checks precede the first detach. No callback
        // can run between validation and detachment.
        for object in objects {
            let buffer = object
                .downcast_ref::<ArrayBuffer>()
                .ok_or_else(|| self.unsupported())?;
            if !buffer.is_transferable() {
                return Err(self.unsupported());
            }
            self.charge(buffer.allocated_bytes())?;
        }
        for object in objects {
            let mut buffer = object
                .downcast_mut::<ArrayBuffer>()
                .ok_or_else(|| self.unsupported())?;
            let max_byte_length = buffer.max_byte_length();
            let data = buffer
                .detach(&JsValue::undefined())?
                .ok_or_else(|| self.unsupported())?;
            let node = self.get(object).expect("transfer identities were reserved");
            self.nodes[node] = ValueStoreInner::ArrayBuffer {
                data,
                max_byte_length,
            };
        }
        Ok(())
    }

    pub(super) fn get(&self, object: &JsObject) -> Option<NodeId> {
        self.objects.get(object).copied()
    }
    pub(super) fn insert(&mut self, original: &JsObject, object: NodeId) {
        self.objects.insert(original.clone(), object);
    }
    pub(super) fn charge(&mut self, bytes: usize) -> JsResult<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= 16 * 1024 * 1024)
            .ok_or_else(|| js_error!(RangeError: "Structured clone byte limit exceeded"))?;
        Ok(())
    }
    pub(super) fn push(&mut self, value: ValueStoreInner) -> JsResult<NodeId> {
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
    object
        .downcast_ref::<ArrayBuffer>()
        .is_some_and(|buffer| buffer.is_transferable())
}

/// The core logic of the [`super::JsValueStore::try_from_js`] function.
fn try_from_js_object(
    value: &JsObject,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    // Have we seen this object? If so, return its clone.
    if let Some(o2) = seen.get(value) {
        return Ok(o2);
    }

    try_from_js_object_clone(value, seen, context)
}

fn try_from_array_clone(
    array: &JsArray,
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
    let fields = own_fields(&JsObject::from(array.clone()), seen, context)?;
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
    let max_byte_length = original
        .downcast_ref::<ArrayBuffer>()
        .expect("JsArrayBuffer is branded")
        .max_byte_length();
    let new_value = seen.push(ValueStoreInner::ArrayBuffer {
        data,
        max_byte_length,
    })?;
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
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let backing = buffer.buffer(context)?;
    buffer.validate_view().map_err(|_| seen.unsupported())?;
    let (byte_offset, length) = (buffer.byte_offset(context)?, buffer.length(context)?);
    let buffer_is_tracking = buffer.is_length_tracking();
    let kind = buffer.kind().ok_or_else(|| seen.unsupported())?;
    let buffer = backing;
    let buffer = try_from_js_value(&buffer, seen, context)?;
    let dolly = seen.push(ValueStoreInner::TypedArray {
        kind,
        buffer,
        byte_offset,
        length: (!buffer_is_tracking).then_some(length),
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
        let key = try_from_js_value(&k, seen, context)?;
        let value = try_from_js_value(&v, seen, context)?;
        new_map.push((key, value));
    }

    seen.nodes[store] = ValueStoreInner::Map(new_map);

    Ok(store)
}

fn try_from_set(
    original: &JsObject,
    set: &JsSet,
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
        let value = try_from_js_value(&v, seen, context)?;
        new_set.push(value);
    }

    seen.nodes[store] = ValueStoreInner::Set(new_set);

    Ok(store)
}

fn try_from_js_object_clone(
    object: &JsObject,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let primitive = if let Some(value) = object.downcast_ref::<bool>() {
        Some(JsValue::from(*value))
    } else if let Some(value) = object.downcast_ref::<f64>() {
        Some(JsValue::from(*value))
    } else if let Some(value) = object.downcast_ref::<JsString>() {
        Some(JsValue::from(value.clone()))
    } else {
        object
            .downcast_ref::<boa_engine::JsBigInt>()
            .map(|value| JsValue::from(value.clone()))
    };
    if let Some(primitive) = primitive {
        let value = try_from_js_value(&primitive, seen, context)?;
        let node = seen.push(ValueStoreInner::Boxed(value))?;
        seen.insert(object, node);
        return Ok(node);
    }
    // If this is a special type of object, apply some special rules to it.
    // Described in
    // https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Structured_clone_algorithm#supported_types

    if let Ok(array) = JsArray::from_object(object.clone()) {
        return try_from_array_clone(&array, seen, context);
    } else if let Ok(map) = JsMap::from_object(object.clone()) {
        return try_from_map(object, &map, seen, context);
    } else if let Ok(set) = JsSet::from_object(object.clone()) {
        return try_from_set(object, &set, seen, context);
    } else if let Ok(ref buffer) = JsArrayBuffer::from_object(object.clone()) {
        return try_from_array_buffer_clone(object, buffer, seen);
    } else if let Ok(ref buffer) = JsSharedArrayBuffer::from_object(object.clone()) {
        if seen.storage {
            return Err(seen.unsupported());
        }
        return try_from_shared_array_buffer(object, buffer, seen);
    } else if let Ok(ref typed_array) = JsTypedArray::from_object(object.clone()) {
        return clone_typed_array(object, typed_array, seen, context);
    } else if let Ok(ref date) = JsDate::from_object(object.clone()) {
        return clone_date(object, date, seen, context);
    } else if object.is::<Error>() {
        return super::errors::serialize(object, seen, context);
    } else if let Ok(ref regexp) = JsRegExp::from_object(object.clone()) {
        return clone_regexp(object, regexp, seen, context);
    } else if let Ok(view) = JsDataView::from_object(object.clone()) {
        let byte_length = view.byte_length(context).map_err(|_| seen.unsupported())?;
        let byte_offset = view.byte_offset(context).map_err(|_| seen.unsupported())?;
        let buffer = view.buffer(context)?;
        let buffer = try_from_js_value(&buffer, seen, context)?;
        let node = seen.push(ValueStoreInner::DataView {
            buffer,
            byte_length: (!view.is_length_tracking()).then_some(byte_length),
            byte_offset,
        })?;
        seen.insert(object, node);
        return Ok(node);
    } else if object.is_callable() {
        // Functions are invalid.
        return Err(seen.unsupported());
    }

    if !object.is_ordinary() {
        return Err(seen.unsupported());
    }

    // Create a new object and add own properties to it. This does not preserve
    // the prototype (nor do we want to).
    let dolly = seen.push(ValueStoreInner::Empty)?;
    seen.insert(object, dolly);

    let fields = own_fields(object, seen, context)?;
    seen.nodes[dolly] = ValueStoreInner::Object(fields);
    Ok(dolly)
}

fn own_fields(
    object: &JsObject,
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

        let v = try_from_js_value(&value, seen, context)?;
        seen.charge(size_of::<(StringStore, NodeId)>())?;
        fields.push((key, v));
    }

    Ok(fields)
}

pub(super) fn try_from_js_value(
    value: &JsValue,
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
        JsVariant::Object(ref o) => try_from_js_object(o, seen, context),

        // Symbols cannot be transferred/cloned.
        JsVariant::Symbol(_) => Err(seen.unsupported()),
    }
}
