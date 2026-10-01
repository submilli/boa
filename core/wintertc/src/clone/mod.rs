//! Module containing all types and functions to implement `structuredClone`.
//!
//! See <https://developer.mozilla.org/en-US/docs/Web/API/Window/structuredClone>.
#![allow(clippy::needless_pass_by_value)]

use boa_engine::realm::Realm;
use boa_engine::value::TryFromJs;
use boa_engine::{Context, JsResult, JsValue, boa_module};

/// Bounded Web IDL options for `structuredClone`.
#[derive(Debug, Clone)]
pub struct StructuredCloneOptions {
    transfer: Option<Vec<JsValue>>,
}

impl TryFromJs for StructuredCloneOptions {
    fn try_from_js(value: &JsValue, context: &mut Context) -> JsResult<Self> {
        if value.is_null_or_undefined() {
            return Ok(Self { transfer: None });
        }
        let object = value.as_object().ok_or_else(
            || boa_engine::js_error!(TypeError: "Clone options must be a dictionary"),
        )?;
        let list = object.get(boa_engine::js_string!("transfer"), context)?;
        if list.is_undefined() {
            return Ok(Self { transfer: None });
        }
        let mut iterator =
            list.get_iterator(boa_engine::builtins::iterable::IteratorHint::Sync, context)?;
        let mut transfer = Vec::new();
        while let Some(value) = iterator.step_value(context)? {
            if transfer.len() >= 1024 {
                iterator.close(Err(boa_engine::js_error!(RangeError: "Structured clone transfer limit exceeded")), context)?;
                unreachable!("IteratorClose preserves an abrupt completion");
            }
            if !value.is_object() {
                iterator.close(
                    Err(boa_engine::js_error!(TypeError: "Transfer entries must be objects")),
                    context,
                )?;
                unreachable!("IteratorClose preserves an abrupt completion");
            }
            transfer.push(value);
        }
        Ok(Self {
            transfer: Some(transfer),
        })
    }
}

/// JavaScript module containing the `structuredClone` types and functions.
#[boa_module]
pub mod js_module {
    use super::StructuredCloneOptions;
    use crate::store::JsValueStore;
    use boa_engine::value::TryIntoJs;
    use boa_engine::{Context, JsResult, JsValue};

    /// The [`structuredClone()`][mdn] method of the Window interface creates a
    /// deep clone of a given value using the [structured clone algorithm][sca].
    ///
    /// # Errors
    /// Will return an error if the context cannot create objects or copy bytes, or
    /// if any unhandled case by the structured clone algorithm.
    ///
    /// [mdn]: https://developer.mozilla.org/en-US/docs/Web/API/Window/structuredClone
    /// [sca]: https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Structured_clone_algorithm
    pub fn structured_clone(
        value: JsValue,
        options: Option<StructuredCloneOptions>,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let v = JsValueStore::try_from_js(
            &value,
            context,
            options.and_then(|o| o.transfer).unwrap_or_default(),
        )?;
        v.try_into_js(context)
    }
}

/// Register the `structuredClone` function in the global context.
///
/// # Errors
/// Return an error if the function is already registered.
pub fn register(realm: Option<Realm>, context: &mut Context) -> JsResult<()> {
    js_module::boa_register(realm, context)
}
