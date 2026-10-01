//! Error snapshots preserve standard data fields without copying custom properties.
use super::{NodeId, StringStore, ValueStoreInner, from, to};
use boa_engine::builtins::error::{Error, ErrorKind};
use boa_engine::property::PropertyDescriptor;
use boa_engine::{Context, JsObject, JsResult, JsValue, js_string};

pub(super) fn serialize(
    object: &JsObject,
    seen: &mut from::SeenMap,
    context: &mut Context,
) -> JsResult<NodeId> {
    let node = seen.push(ValueStoreInner::Empty)?;
    seen.insert(object, node);
    let name = object.get(js_string!("name"), context)?;
    let name = name
        .as_string()
        .filter(|s| s.len() <= 14)
        .map(|s| s.to_std_string_escaped());
    let kind = match name.as_deref() {
        Some("EvalError") => ErrorKind::Eval,
        Some("RangeError") => ErrorKind::Range,
        Some("ReferenceError") => ErrorKind::Reference,
        Some("SyntaxError") => ErrorKind::Syntax,
        Some("TypeError") => ErrorKind::Type,
        Some("URIError") => ErrorKind::Uri,
        _ => ErrorKind::Error,
    };
    let message = own_data(object, "message")
        .as_ref()
        .and_then(JsValue::as_string);
    let message = capture_string(message, seen)?;
    let stack = object.get(js_string!("stack"), context)?.as_string();
    let stack = capture_string(stack, seen)?;
    let cause = own_data(object, "cause")
        .map(|value| from::try_from_js_value(&value, seen, context))
        .transpose()?;
    seen.nodes[node] = ValueStoreInner::Error {
        kind,
        message,
        stack,
        cause,
    };
    Ok(node)
}

fn own_data(object: &JsObject, name: &str) -> Option<JsValue> {
    object
        .borrow()
        .properties()
        .get(&js_string!(name).into())
        .and_then(|descriptor| descriptor.value().cloned())
}

fn capture_string(
    value: Option<boa_engine::JsString>,
    seen: &mut from::SeenMap,
) -> JsResult<Option<StringStore>> {
    value
        .map(|value| {
            seen.charge(value.len().saturating_mul(2))?;
            Ok(value.into())
        })
        .transpose()
}

pub(super) fn deserialize(
    node: NodeId,
    kind: ErrorKind,
    message: Option<&StringStore>,
    stack: Option<&StringStore>,
    cause: Option<NodeId>,
    seen: &mut to::ReverseSeenMap,
    context: &mut Context,
) -> JsResult<JsValue> {
    let constructors = context.intrinsics().constructors();
    let prototype = match kind {
        ErrorKind::Eval => constructors.eval_error(),
        ErrorKind::Range => constructors.range_error(),
        ErrorKind::Reference => constructors.reference_error(),
        ErrorKind::Syntax => constructors.syntax_error(),
        ErrorKind::Type => constructors.type_error(),
        ErrorKind::Uri => constructors.uri_error(),
        _ => constructors.error(),
    }
    .prototype();
    let object = JsObject::from_proto_and_data(Some(prototype), Error::new(kind));
    seen.insert(node, object.clone());
    for (name, value) in [("message", message), ("stack", stack)] {
        if let Some(value) = value {
            define(&object, name, value.to_js_string().into(), context)?;
        }
    }
    if let Some(cause) = cause {
        let cause = to::try_value_into_js(cause, seen, context)?;
        define(&object, "cause", cause, context)?;
    }
    Ok(object.into())
}

fn define(object: &JsObject, name: &str, value: JsValue, context: &mut Context) -> JsResult<()> {
    object.define_property_or_throw(
        js_string!(name),
        PropertyDescriptor::builder()
            .value(value)
            .writable(true)
            .enumerable(false)
            .configurable(true),
        context,
    )?;
    Ok(())
}
