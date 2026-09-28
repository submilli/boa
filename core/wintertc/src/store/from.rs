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
}

impl SeenMap {
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
        let bytes = match &value {
            ValueStoreInner::RegExp { source, flags } => source.len().saturating_add(flags.len()),
            _ => 0,
        };
        self.charge(bytes)?;
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
        seen.charge(buffer.data().ok_or_else(unsupported_type)?.len())?;
        let data = buffer.detach(&JsValue::undefined())?;
        let data = data.ok_or_else(unsupported_type)?;

        let node = seen.push(ValueStoreInner::ArrayBuffer(data))?;
        seen.insert(object, node);
        Ok(node)
    } else {
        Err(unsupported_type())
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
    seen.charge(length.saturating_mul(size_of::<Option<NodeId>>()))?;
    let mut inner = Vec::with_capacity(length);
    for i in 0..length {
        let v = array
            .borrow()
            .properties()
            .get(&i.into())
            .and_then(|x| x.value().cloned());
        if let Some(v) = v {
            let v = try_from_js_value(&v, transfer, seen, context)?;
            inner.push(Some(v));
        } else {
            inner.push(None);
        }
    }

    seen.nodes[dolly] = ValueStoreInner::Array(inner);
    Ok(dolly)
}

fn try_from_array_buffer_clone(
    original: &JsObject,
    buffer: &JsArrayBuffer,
    seen: &mut SeenMap,
) -> JsResult<NodeId> {
    let data = buffer.data().ok_or_else(unsupported_type)?;
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
    let kind = buffer.kind().ok_or_else(unsupported_type)?;
    let buffer = buffer.buffer(context)?;
    let buffer = try_from_js_value(&buffer, transfer, seen, context)?;
    let dolly = seen.push(ValueStoreInner::TypedArray { kind, buffer })?;
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
        .ok_or_else(unsupported_type)?;

    let stored = seen.push(ValueStoreInner::Date(ms_since_epoch))?;
    seen.insert(original, stored);
    Ok(stored)
}

fn clone_regexp(
    original: &JsObject,
    regexp: &JsRegExp,
    seen: &mut SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let source = regexp.source(context)?;
    let flags = regexp.flags(context)?;

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

    map.for_each_native(|k, v| {
        let key = try_from_js_value(&k, transfer, seen, context)?;
        let value = try_from_js_value(&v, transfer, seen, context)?;
        seen.charge(2 * size_of::<NodeId>())?;
        new_map.push((key, value));

        Ok(())
    })?;

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

    set.for_each_native(|v| {
        let value = try_from_js_value(&v, transfer, seen, context)?;
        seen.charge(size_of::<NodeId>())?;
        new_set.push(value);

        Ok(())
    })?;

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
        return try_from_shared_array_buffer(object, buffer, seen);
    } else if let Ok(ref typed_array) = JsTypedArray::from_object(object.clone()) {
        return clone_typed_array(object, typed_array, transfer, seen, context);
    } else if let Ok(ref date) = JsDate::from_object(object.clone()) {
        return clone_date(object, date, seen, context);
    } else if let Ok(_error) = object.clone().downcast::<Error>() {
        return Err(js_error!(TypeError: "Errors are not supported yet."));
    } else if let Ok(ref regexp) = JsRegExp::from_object(object.clone()) {
        return clone_regexp(object, regexp, seen, context);
    } else if let Ok(_dataview) = JsDataView::from_object(object.clone()) {
        return Err(js_error!(TypeError: "Data views are not supported yet."));
    } else if object.is_callable() {
        // Functions are invalid.
        return Err(unsupported_type());
    }

    // Create a new object and add own properties to it. This does not preserve
    // the prototype (nor do we want to).
    let dolly = seen.push(ValueStoreInner::Empty)?;
    seen.insert(object, dolly);

    let keys = object.own_property_keys(context)?;
    if keys.len() > 65536 {
        return Err(js_error!(RangeError: "Structured clone field limit exceeded"));
    }
    let mut fields: Vec<(StringStore, NodeId)> = Vec::with_capacity(keys.len());
    for k in keys {
        let value = object.get(k.clone(), context)?;
        let key = match k {
            PropertyKey::String(s) => {
                seen.charge(s.len().saturating_mul(2))?;
                StringStore::from(s)
            }
            PropertyKey::Symbol(_) => return Err(unsupported_type()),
            PropertyKey::Index(i) => JsString::from(format!("{}", i.get())).into(),
        };

        let v = try_from_js_value(&value, transfer, seen, context)?;
        seen.charge(size_of::<NodeId>())?;
        fields.push((key, v));
    }

    seen.nodes[dolly] = ValueStoreInner::Object(fields);
    Ok(dolly)
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
        JsVariant::Symbol(_) => Err(unsupported_type()),
    }
}
