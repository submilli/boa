//! All methods for deserializing a [`NodeId`] into a [`JsValue`].
use crate::store::{NodeId, StringStore, ValueStoreInner, unsupported_type};
use boa_engine::builtins::array_buffer::{AlignedVec, SharedArrayBuffer};
use boa_engine::builtins::typed_array::TypedArrayKind;
use boa_engine::object::builtins::{
    JsArray, JsArrayBuffer, JsDataView, JsDate, JsMap, JsRegExp, JsSet, JsSharedArrayBuffer,
    js_typed_array_from_kind,
};
use boa_engine::{Context, JsBigInt, JsObject, JsResult, JsString, JsValue, js_error};
use rustc_hash::FxHashMap;

pub(super) struct ReverseSeenMap {
    objects: FxHashMap<NodeId, JsObject>,
    graph: std::sync::Arc<Vec<ValueStoreInner>>,
}

impl ReverseSeenMap {
    pub(super) fn new(graph: std::sync::Arc<Vec<ValueStoreInner>>) -> Self {
        Self {
            graph,
            objects: FxHashMap::default(),
        }
    }
    fn get(&self, object: NodeId) -> Option<JsObject> {
        self.objects.get(&object).cloned()
    }
    fn insert(&mut self, original: NodeId, object: JsObject) {
        self.objects.insert(original, object);
    }
}

fn try_fields_into_js_object(
    store: NodeId,
    fields: &Vec<(StringStore, NodeId)>,
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let dolly = JsObject::with_object_proto(context.intrinsics());
    seen.insert(store, dolly.clone());

    for (k, v) in fields {
        let k = k.to_js_string();
        let value = try_value_into_js(*v, seen, context)?;
        dolly.set(k, value, true, context)?;
    }
    Ok(JsValue::from(dolly))
}

fn try_items_into_js_array(
    store: NodeId,
    items: &[Option<NodeId>],
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let dolly = JsArray::new(context)?;
    seen.insert(store, dolly.clone().into());

    for (k, v) in items
        .iter()
        .enumerate()
        .filter_map(|(k, v)| v.as_ref().map(|v| (k, v)))
    {
        let value = try_value_into_js(*v, seen, context)?;
        dolly.set(k, value, true, context)?;
    }
    Ok(JsValue::from(dolly))
}

fn try_into_js_array_buffer(
    store: NodeId,
    data: &[u8],
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let buffer = JsArrayBuffer::from_byte_block(AlignedVec::from_slice(0, data), context)?;
    let obj = JsObject::from(buffer);
    seen.insert(store, obj.clone());
    Ok(JsValue::from(obj))
}

fn try_into_js_shared_array_buffer(
    store: NodeId,
    inner: &SharedArrayBuffer,
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsValue {
    let buffer = JsSharedArrayBuffer::from_buffer(inner.clone(), context);
    let obj = JsObject::from(buffer);
    seen.insert(store, obj.clone());
    JsValue::from(obj)
}

fn try_into_js_typed_array(
    store: NodeId,
    kind: TypedArrayKind,
    buffer: NodeId,
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let buffer = try_value_into_js(buffer, seen, context)?;
    let Some(buffer) = buffer.as_object() else {
        return Err(unsupported_type());
    };
    let buffer = JsArrayBuffer::from_object(buffer)?;
    let array = js_typed_array_from_kind(kind, buffer, context)?;
    if let Some(o) = array.as_object() {
        seen.insert(store, o);
    }
    Ok(array)
}

fn try_into_js_map(
    store: NodeId,
    key_values: &[(NodeId, NodeId)],
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let map = JsMap::new(context);
    seen.insert(store, map.clone().into());
    for (k, v) in key_values {
        let k = try_value_into_js(*k, seen, context)?;
        let v = try_value_into_js(*v, seen, context)?;
        map.set(k, v, context)?;
    }

    Ok(JsValue::from(map))
}

fn try_into_js_set(
    store: NodeId,
    values: &[NodeId],
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let set = JsSet::new(context);
    seen.insert(store, set.clone().into());
    for v in values {
        let v = try_value_into_js(*v, seen, context)?;
        set.add(v, context)?;
    }

    Ok(JsValue::from(set))
}

fn try_into_js_date(
    store: NodeId,
    ms_since_epoch: f64,
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let date = JsDate::new(context);
    date.set_time(ms_since_epoch, context)?;
    seen.insert(store, date.clone().into());

    Ok(JsValue::from(date))
}

fn try_into_regexp(
    store: NodeId,
    source: &str,
    flags: &str,
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let re = JsRegExp::new(JsString::from(source), JsString::from(flags), context)?;
    seen.insert(store, re.clone().into());
    Ok(JsValue::from(re))
}

fn try_into_data_view(
    store: NodeId,
    buffer: NodeId,
    byte_length: u64,
    byte_offset: u64,
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let buffer = try_value_into_js(buffer, seen, context)?;
    let data_view = JsDataView::from_js_array_buffer(
        JsArrayBuffer::from_object(buffer.as_object().ok_or_else(unsupported_type)?)?,
        Some(byte_offset),
        Some(byte_length),
        context,
    )?;

    seen.insert(store, data_view.clone().into());
    Ok(JsValue::from(data_view))
}

pub(super) fn try_value_into_js(
    store: NodeId,
    seen: &mut ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    if let Some(v) = seen.get(store) {
        return Ok(JsValue::from(v));
    }

    let _depth = super::Traversal::enter()?;
    let graph = seen.graph.clone();
    match &graph[store] {
        ValueStoreInner::Empty => {
            unreachable!("ValueStoreInner::Empty should not exist after storage.");
        }
        ValueStoreInner::Null => Ok(JsValue::null()),
        ValueStoreInner::Undefined => Ok(JsValue::undefined()),
        ValueStoreInner::Boolean(b) => Ok(JsValue::from(*b)),
        ValueStoreInner::Float(f) => Ok(JsValue::from(*f)),
        ValueStoreInner::String(s) => Ok(JsValue::from(s.to_js_string())),
        ValueStoreInner::BigInt(b) => Ok(JsValue::from(JsBigInt::new(b.clone()))),
        ValueStoreInner::Object(fields) => try_fields_into_js_object(store, fields, seen, context),
        ValueStoreInner::Map(key_values) => try_into_js_map(store, key_values, seen, context),
        ValueStoreInner::Set(values) => try_into_js_set(store, values, seen, context),
        ValueStoreInner::Array(items) => try_items_into_js_array(store, items, seen, context),
        ValueStoreInner::Date(msec) => try_into_js_date(store, *msec, seen, context),
        ValueStoreInner::Error { .. } => Err(js_error!("Not yet implemented.")),
        ValueStoreInner::RegExp { source, flags } => {
            try_into_regexp(store, source, flags, seen, context)
        }
        ValueStoreInner::ArrayBuffer(data) => try_into_js_array_buffer(store, data, seen, context),
        ValueStoreInner::SharedArrayBuffer(inner) => {
            Ok(try_into_js_shared_array_buffer(store, inner, seen, context))
        }
        ValueStoreInner::DataView {
            buffer,
            byte_length,
            byte_offset,
        } => try_into_data_view(store, *buffer, *byte_length, *byte_offset, seen, context),
        ValueStoreInner::TypedArray { kind, buffer } => {
            try_into_js_typed_array(store, *kind, *buffer, seen, context)
        }
    }
}
