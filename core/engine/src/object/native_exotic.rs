//! Property-only exotic objects for native embedders.
//!
//! Call/construct dispatch and VM cache slots remain engine-owned. Hooks receive
//! an object, not a borrowed payload: release any payload borrow before calling
//! JavaScript, which can reenter the same object. Implementations must honor their
//! host specification's property invariants and bound host-side work.

use super::internal_methods::{
    self, InternalMethodPropertyContext, InternalObjectMethods, ORDINARY_INTERNAL_METHODS,
};
use super::shape::slot::SlotAttributes;
use crate::property::{PropertyDescriptor, PropertyKey};
use crate::{Context, Finalize, JsData, JsObject, JsResult, JsValue, Trace};

/// Key enumeration operations that browser host objects may distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeKeyEnumeration {
    /// Own enumerable keys for `Object.keys` and JSON serialization.
    Own,
    /// Keys used by `for...in`, including prototype traversal.
    ForIn,
}

/// An optional native read override. `None` delegates to the ordinary read
/// algorithm, retaining its distinction between an absent property and undefined.
pub type NativeGet =
    fn(&JsObject, &PropertyKey, JsValue, &mut Context) -> JsResult<Option<JsValue>>;

/// A native prototype query, called without an outstanding payload borrow.
pub type NativeGetPrototypeOf = fn(&JsObject, &mut Context) -> JsResult<Option<JsObject>>;

/// Native property hooks. Defaults use ordinary algorithms; get/has follow the
/// overridden own descriptors, including through prototype chains.
///
/// Construct with `JsObject::from_proto_and_data(proto, NativeExoticObject(data))`.
/// Callbacks can downcast the object to `NativeExoticObject<Self>` to access data.
/// A callback must release that borrow before invoking script or another hook.
pub trait NativeExotic: Trace + Sized + 'static {
    /// Optional `[[GetPrototypeOf]]` override for dynamic host prototypes.
    /// `None` keeps the ordinary internal method itself, so ordinary prototype
    /// cycle detection can continue through native objects using the default.
    const GET_PROTOTYPE_OF: Option<NativeGetPrototypeOf> = None;
    /// Override selected `[[Get]]` reads independently of the own descriptor.
    /// The engine applies this hook to both get and try-get and prevents caching.
    const GET: Option<NativeGet> = None;
    /// Implements `[[SetPrototypeOf]]`; the embedder enforces its host invariants.
    fn set_prototype_of(
        object: &JsObject,
        prototype: Option<JsObject>,
        context: &mut Context,
    ) -> JsResult<bool> {
        ordinary::set_prototype_of(object, prototype, context)
    }
    /// Implements `[[IsExtensible]]`; pair with `prevent_extensions` consistently.
    fn is_extensible(object: &JsObject, context: &mut Context) -> JsResult<bool> {
        ordinary::is_extensible(object, context)
    }
    /// Implements `[[GetOwnProperty]]`; see the ordinary helper for fallback behavior.
    fn get_own_property(
        object: &JsObject,
        key: &PropertyKey,
        context: &mut Context,
    ) -> JsResult<Option<PropertyDescriptor>> {
        ordinary::get_own_property(object, key, context)
    }
    /// Implements `[[DefineOwnProperty]]`; see the ordinary helper for fallback behavior.
    fn define_own_property(
        object: &JsObject,
        key: &PropertyKey,
        descriptor: PropertyDescriptor,
        context: &mut Context,
    ) -> JsResult<bool> {
        ordinary::define_own_property(object, key, descriptor, context)
    }
    /// Implements `[[Set]]`; see the ordinary helper for fallback behavior.
    fn set(
        object: &JsObject,
        key: PropertyKey,
        value: JsValue,
        receiver: JsValue,
        context: &mut Context,
    ) -> JsResult<bool> {
        ordinary::set(object, key, value, receiver, context)
    }
    /// Implements `[[Delete]]`; see the ordinary helper for fallback behavior.
    fn delete(object: &JsObject, key: &PropertyKey, context: &mut Context) -> JsResult<bool> {
        ordinary::delete(object, key, context)
    }
    /// Implements `[[OwnPropertyKeys]]`; see the ordinary helper for fallback behavior.
    fn own_property_keys(object: &JsObject, context: &mut Context) -> JsResult<Vec<PropertyKey>> {
        ordinary::own_property_keys(object, context)
    }
    /// Selects a string key for `Object.keys`, JSON serialization, and `for...in`.
    ///
    /// Return `None` to use the object's own descriptor (the default).
    /// `Some(true)` emits the name and marks it visited during `for...in`.
    /// `Some(false)` suppresses the name, including a same-named prototype
    /// property during `for...in`. Some browser host objects expose supported names in enumeration even when prototype visibility hides their
    /// own descriptor. This hook is called per key after `[[OwnPropertyKeys]]`,
    /// keeping each eligibility check live rather than caching host answers.
    /// `Object.values`, `Object.entries`, and wrapping Proxies keep their
    /// ECMAScript descriptor filtering.
    fn is_enumerable_own_property(
        object: &JsObject,
        key: &PropertyKey,
        kind: NativeKeyEnumeration,
        context: &mut Context,
    ) -> JsResult<Option<bool>> {
        let _ = (object, key, kind, context);
        Ok(None)
    }
    /// Implements `[[PreventExtensions]]`; see the ordinary helper for fallback behavior.
    fn prevent_extensions(object: &JsObject, context: &mut Context) -> JsResult<bool> {
        ordinary::prevent_extensions(object, context)
    }
}

/// Traced payload wrapper selecting the native exotic property dispatch table.
#[derive(Debug, Trace, Finalize)]
pub struct NativeExoticObject<T: NativeExotic>(pub T);

impl<T: NativeExotic> JsData for NativeExoticObject<T> {
    fn requires_unique_shape(&self) -> bool {
        true
    }

    fn internal_methods(&self) -> &'static InternalObjectMethods {
        &Self::METHODS
    }
}

impl<T: NativeExotic> NativeExoticObject<T> {
    const METHODS: InternalObjectMethods = InternalObjectMethods {
        __get_prototype_of__: if T::GET_PROTOTYPE_OF.is_some() {
            Self::get_prototype_of
        } else {
            internal_methods::ordinary_get_prototype_of
        },
        __set_prototype_of__: Self::set_prototype_of,
        __is_extensible__: Self::is_extensible,
        __get_own_property__: Self::get_own_property,
        __define_own_property__: Self::define_own_property,
        __set__: Self::set,
        __delete__: Self::delete,
        __own_property_keys__: Self::own_property_keys,
        is_enumerable_own_property: Some(Self::is_enumerable_own_property),
        __prevent_extensions__: Self::prevent_extensions,
        __get__: Self::get,
        __try_get__: Self::try_get,
        __has_property__: Self::has_property,
        ..ORDINARY_INTERNAL_METHODS
    };
    fn get_prototype_of(object: &JsObject, context: &mut Context) -> JsResult<Option<JsObject>> {
        let _recursion = context.enter_native_recursion()?;
        T::GET_PROTOTYPE_OF.unwrap_or(ordinary::get_prototype_of)(object, context)
    }
    fn set_prototype_of(
        object: &JsObject,
        prototype: Option<JsObject>,
        context: &mut Context,
    ) -> JsResult<bool> {
        let _recursion = context.enter_native_recursion()?;
        T::set_prototype_of(object, prototype, context)
    }
    fn is_extensible(object: &JsObject, context: &mut Context) -> JsResult<bool> {
        let _recursion = context.enter_native_recursion()?;
        T::is_extensible(object, context)
    }
    fn get(
        object: &JsObject,
        key: &PropertyKey,
        receiver: JsValue,
        context: &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<JsValue> {
        let _recursion = context.enter_native_recursion()?;
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        if let Some(get) = T::GET
            && let Some(value) = get(object, key, receiver.clone(), context)?
        {
            return Ok(value);
        }
        let result = internal_methods::ordinary_get(object, key, receiver, context);
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        result
    }
    fn try_get(
        object: &JsObject,
        key: &PropertyKey,
        receiver: JsValue,
        context: &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<Option<JsValue>> {
        let _recursion = context.enter_native_recursion()?;
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        if let Some(get) = T::GET
            && let Some(value) = get(object, key, receiver.clone(), context)?
        {
            return Ok(Some(value));
        }
        let result = internal_methods::ordinary_try_get(object, key, receiver, context);
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        result
    }
    fn has_property(
        object: &JsObject,
        key: &PropertyKey,
        context: &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<bool> {
        let _recursion = context.enter_native_recursion()?;
        let result = internal_methods::ordinary_has_property(object, key, context);
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        result
    }
    fn get_own_property(
        object: &JsObject,
        key: &PropertyKey,
        context: &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<Option<PropertyDescriptor>> {
        // Never cache a native answer or a prototype reached through this object.
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        let _recursion = context.enter_native_recursion()?;
        T::get_own_property(object, key, context)
    }
    fn define_own_property(
        object: &JsObject,
        key: &PropertyKey,
        descriptor: PropertyDescriptor,
        context: &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<bool> {
        // Never cache a native answer or a prototype reached through this object.
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        let _recursion = context.enter_native_recursion()?;
        T::define_own_property(object, key, descriptor, context)
    }
    fn set(
        object: &JsObject,
        key: PropertyKey,
        value: JsValue,
        receiver: JsValue,
        context: &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<bool> {
        // Never cache a native answer or a prototype reached through this object.
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        let _recursion = context.enter_native_recursion()?;
        T::set(object, key, value, receiver, context)
    }
    fn delete(
        object: &JsObject,
        key: &PropertyKey,
        context: &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<bool> {
        // Never cache a native answer or a prototype reached through this object.
        context.slot().attributes |= SlotAttributes::NOT_CACHEABLE;
        let _recursion = context.enter_native_recursion()?;
        T::delete(object, key, context)
    }
    fn own_property_keys(object: &JsObject, context: &mut Context) -> JsResult<Vec<PropertyKey>> {
        let _recursion = context.enter_native_recursion()?;
        T::own_property_keys(object, context)
    }
    fn is_enumerable_own_property(
        object: &JsObject,
        key: &PropertyKey,
        kind: NativeKeyEnumeration,
        context: &mut Context,
    ) -> JsResult<Option<bool>> {
        let _recursion = context.enter_native_recursion()?;
        T::is_enumerable_own_property(object, key, kind, context)
    }
    fn prevent_extensions(object: &JsObject, context: &mut Context) -> JsResult<bool> {
        let _recursion = context.enter_native_recursion()?;
        T::prevent_extensions(object, context)
    }
}

/// Ordinary fallbacks that bypass only the named hook. Other abstract operations
/// still dispatch normally (e.g. define consults the exotic own descriptor).
/// These helpers cannot expose VM slots or alter callable state.
pub mod ordinary {
    use super::{
        Context, JsObject, JsResult, JsValue, PropertyDescriptor, PropertyKey, internal_methods,
    };
    /// Runs the ordinary `get_prototype_of` algorithm on the object.
    pub fn get_prototype_of(
        object: &JsObject,
        context: &mut Context,
    ) -> JsResult<Option<JsObject>> {
        internal_methods::ordinary_get_prototype_of(object, context)
    }
    /// Runs the ordinary `set_prototype_of` algorithm on the object.
    pub fn set_prototype_of(
        object: &JsObject,
        prototype: Option<JsObject>,
        context: &mut Context,
    ) -> JsResult<bool> {
        internal_methods::ordinary_set_prototype_of(object, prototype, context)
    }
    /// Runs the ordinary `is_extensible` algorithm on the object.
    pub fn is_extensible(object: &JsObject, context: &mut Context) -> JsResult<bool> {
        internal_methods::ordinary_is_extensible(object, context)
    }
    /// Runs the ordinary `get_own_property` algorithm on the object.
    pub fn get_own_property(
        object: &JsObject,
        key: &PropertyKey,
        context: &mut Context,
    ) -> JsResult<Option<PropertyDescriptor>> {
        internal_methods::ordinary_get_own_property(object, key, &mut context.into())
    }
    /// Runs the ordinary `define_own_property` algorithm on the object.
    pub fn define_own_property(
        object: &JsObject,
        key: &PropertyKey,
        descriptor: PropertyDescriptor,
        context: &mut Context,
    ) -> JsResult<bool> {
        internal_methods::ordinary_define_own_property(object, key, descriptor, &mut context.into())
    }
    /// Runs the ordinary `set` algorithm on the object.
    pub fn set(
        object: &JsObject,
        key: PropertyKey,
        value: JsValue,
        receiver: JsValue,
        context: &mut Context,
    ) -> JsResult<bool> {
        internal_methods::ordinary_set(object, key, value, receiver, &mut context.into())
    }
    /// Runs the ordinary `delete` algorithm on the object.
    pub fn delete(object: &JsObject, key: &PropertyKey, context: &mut Context) -> JsResult<bool> {
        internal_methods::ordinary_delete(object, key, &mut context.into())
    }
    /// Runs the ordinary `own_property_keys` algorithm on the object.
    pub fn own_property_keys(
        object: &JsObject,
        context: &mut Context,
    ) -> JsResult<Vec<PropertyKey>> {
        internal_methods::ordinary_own_property_keys(object, context)
    }
    /// Runs the ordinary `prevent_extensions` algorithm on the object.
    pub fn prevent_extensions(object: &JsObject, context: &mut Context) -> JsResult<bool> {
        internal_methods::ordinary_prevent_extensions(object, context)
    }
}
