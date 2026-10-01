//! Exercise the public embedder API from outside the engine crate.
// Integration coverage uses only a subset of the engine package dependencies.
#![allow(unused_crate_dependencies)]
use boa_engine::object::native_exotic::{
    NativeExotic, NativeExoticObject, NativeKeyEnumeration, ordinary,
};
use boa_engine::property::{Attribute, PropertyDescriptor, PropertyKey};
use boa_engine::{Context, Finalize, JsObject, JsResult, JsValue, Source, Trace, js_string};
use boa_gc::GcRefCell;

#[derive(Trace, Finalize)]
struct LiveMap {
    value: GcRefCell<JsValue>,
}

impl NativeExotic for LiveMap {
    fn get_own_property(
        object: &JsObject,
        key: &PropertyKey,
        context: &mut Context,
    ) -> JsResult<Option<PropertyDescriptor>> {
        if key == &PropertyKey::from(js_string!("live")) {
            let value = object
                .downcast_ref::<NativeExoticObject<Self>>()
                .unwrap()
                .0
                .value
                .borrow()
                .clone();
            return Ok(Some(
                PropertyDescriptor::builder()
                    .value(value)
                    .writable(true)
                    .enumerable(true)
                    .configurable(true)
                    .build(),
            ));
        }
        ordinary::get_own_property(object, key, context)
    }

    fn define_own_property(
        object: &JsObject,
        key: &PropertyKey,
        descriptor: PropertyDescriptor,
        context: &mut Context,
    ) -> JsResult<bool> {
        if key == &PropertyKey::from(js_string!("live")) {
            let value = descriptor.value().cloned().unwrap_or_default();
            // Conversion may reenter this object's hooks.
            let value = value.to_string(context)?;
            *object
                .downcast_ref::<NativeExoticObject<Self>>()
                .unwrap()
                .0
                .value
                .borrow_mut() = value.into();
            return Ok(true);
        }
        ordinary::define_own_property(object, key, descriptor, context)
    }

    fn own_property_keys(object: &JsObject, context: &mut Context) -> JsResult<Vec<PropertyKey>> {
        let mut keys = vec![js_string!("live").into()];
        keys.extend(ordinary::own_property_keys(object, context)?);
        Ok(keys)
    }

    fn prevent_extensions(_: &JsObject, _: &mut Context) -> JsResult<bool> {
        Ok(false)
    }
}

fn check(code: &str) {
    let mut context = Context::default();
    let object = JsObject::from_proto_and_data(
        Some(JsObject::with_object_proto(context.intrinsics())),
        NativeExoticObject(LiveMap {
            value: GcRefCell::new(js_string!("initial").into()),
        }),
    );
    context
        .register_global_property(js_string!("map"), object, Attribute::all())
        .unwrap();
    assert_eq!(context.eval(Source::from_bytes(code)).unwrap(), true.into());
}

#[test]
fn native_definitions_do_not_use_proxy_invariants() {
    check(
        r"
        Object.defineProperty(map, 'live', {value:'changed', configurable:false});
        const descriptor = Object.getOwnPropertyDescriptor(map, 'live');
        map.live === 'changed' && descriptor.configurable && descriptor.writable &&
            !Reflect.preventExtensions(map) && Object.isExtensible(map) &&
            typeof map === 'object' && Object.keys(map).join() === 'live'
    ",
    );
}

#[test]
fn dynamic_descriptors_and_prototypes_cannot_supply_inline_cache_slots() {
    check(
        r"
        Object.getPrototypeOf(map).fallback = 7;
        function read(o) { return o.live + ':' + o.fallback; }
        let ok = true;
        const child = Object.create(map);
        for (let i=0;i<200;i++) {
            map.live = String(i);
            ok &&= read(map) === i+':7' && read(child) === i+':7';
        }
        child.live = 'child';
        ok && child.live === 'child' && map.live === '199'
    ",
    );
}

#[test]
fn conversion_can_reenter_native_property_hooks() {
    check(
        r"
        let observed;
        map.live = {toString() { map.live='inner'; observed=map.live; return 'outer'; }};
        let failed=false;
        try {map.live={toString(){throw new Error('stop')}}} catch {failed=true}
        observed === 'inner' && map.live === 'outer' && failed
    ",
    );
}

#[test]
fn ordinary_symbols_and_wrapping_proxies_retain_invariants() {
    check(
        r"
        const symbol=Symbol();
        Object.defineProperty(map,symbol,{value:12,configurable:false});
        let failed=false;
        try {Object.defineProperty(new Proxy({}, {defineProperty(){return true}}),
            'x', {value:1,configurable:false})} catch(e) {failed=e instanceof TypeError}
        const proxy=new Proxy(map,{});
        failed && proxy[symbol]===12 && !Reflect.deleteProperty(map,symbol) &&
            Reflect.ownKeys(map)[1]===symbol && proxy.live==='initial'
    ",
    );
}

#[test]
fn shared_shape_constructors_isolate_native_hooks_from_ordinary_caches() {
    use boa_engine::builtins::object::OrdinaryObject;
    let mut context = Context::default();
    let prototype = JsObject::with_object_proto(context.intrinsics());
    prototype
        .create_data_property(js_string!("live"), 7, &mut context)
        .unwrap();
    let ordinary = JsObject::new(context.root_shape(), prototype.clone(), OrdinaryObject).upcast();
    let native = JsObject::new(
        context.root_shape(),
        prototype,
        NativeExoticObject(LiveMap {
            value: GcRefCell::new(js_string!("initial").into()),
        }),
    )
    .upcast();
    context
        .register_global_property(js_string!("ordinary"), ordinary, Attribute::all())
        .unwrap();
    context
        .register_global_property(js_string!("native"), native, Attribute::all())
        .unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes(
                r"
        function read(o) {return o.live}
        let ok=true;
        for(let i=0;i<200;i++) {ok &&= read(ordinary)===7 && read(native)==='initial'}
        ok
    "
            ))
            .unwrap(),
        true.into()
    );
}

#[test]
fn recursive_conversion_throws_and_releases_native_budget() {
    check(
        r"
        const value = {[Symbol.toPrimitive](){map.live=value;return 'never'}};
        let caught=false;
        try {map.live=value} catch(e) {caught=e instanceof RangeError}
        map.live='recovered'; caught && map.live==='recovered'
    ",
    );
}

#[derive(Trace, Finalize)]
struct EnumeratedNames;

impl NativeExotic for EnumeratedNames {
    fn own_property_keys(object: &JsObject, context: &mut Context) -> JsResult<Vec<PropertyKey>> {
        let mut keys = vec![js_string!("hidden").into()];
        keys.extend(ordinary::own_property_keys(object, context)?);
        Ok(keys)
    }

    fn is_enumerable_own_property(
        _: &JsObject,
        key: &PropertyKey,
        kind: NativeKeyEnumeration,
        _: &mut Context,
    ) -> JsResult<Option<bool>> {
        if key == &PropertyKey::from(js_string!("hidden")) {
            return Ok(Some(true));
        }
        if key == &PropertyKey::from(js_string!("reserved")) && kind == NativeKeyEnumeration::Own {
            return Ok(Some(false));
        }
        if key == &PropertyKey::from(js_string!("suppressed")) {
            return Ok(Some(false));
        }
        Ok(None)
    }
}

#[test]
fn host_key_enumeration_does_not_change_descriptors_values_or_proxies() {
    let mut context = Context::default();
    let object = JsObject::from_proto_and_data(None, NativeExoticObject(EnumeratedNames));
    object
        .create_data_property(js_string!("visible"), 7, &mut context)
        .unwrap();
    object
        .create_data_property(boa_engine::JsSymbol::new(None).unwrap(), 8, &mut context)
        .unwrap();
    context
        .register_global_property(js_string!("names"), object, Attribute::all())
        .unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes(
                r"
        Object.keys(names).join() === 'hidden,visible' &&
        Object.getOwnPropertyDescriptor(names,'hidden') === undefined &&
        Object.values(names).join() === '7' &&
        Object.entries(names).length === 1 && Object.entries(names)[0].join() === 'visible,7' &&
        Object.keys(new Proxy(names,{})).join() === 'visible' &&
        Reflect.ownKeys(names).length === 3 &&
        (() => {let keys=[];for (const key in names) keys.push(key);
            return keys.join() === 'hidden,visible'})() &&
        JSON.stringify(names) === JSON.stringify({visible:7}) &&
        Object.keys({...names}).join() === 'visible'
    "
            ))
            .unwrap(),
        true.into()
    );
}

#[derive(Trace, Finalize)]
struct RecursiveEnumeration;

impl NativeExotic for RecursiveEnumeration {
    fn own_property_keys(_: &JsObject, _: &mut Context) -> JsResult<Vec<PropertyKey>> {
        Ok(vec![js_string!("key").into()])
    }
    fn is_enumerable_own_property(
        _: &JsObject,
        _: &PropertyKey,
        _: NativeKeyEnumeration,
        context: &mut Context,
    ) -> JsResult<Option<bool>> {
        context.eval(Source::from_bytes("Object.keys(recursive)"))?;
        Ok(Some(true))
    }
}

#[test]
fn recursive_enumeration_is_catchable_and_releases_native_budget() {
    let mut context = Context::default();
    let recursive = JsObject::from_proto_and_data(None, NativeExoticObject(RecursiveEnumeration));
    let names = JsObject::from_proto_and_data(None, NativeExoticObject(EnumeratedNames));
    context
        .register_global_property(js_string!("recursive"), recursive, Attribute::all())
        .unwrap();
    context
        .register_global_property(js_string!("names"), names, Attribute::all())
        .unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes(
                r"
        let caught=false;
        try {Object.keys(recursive)} catch(e) {caught=e instanceof RangeError}
        let iterationCaught=false;
        try {for(const key in recursive) {}} catch(e) {iterationCaught=e instanceof RangeError}
        caught && iterationCaught && Object.keys(names).join() === 'hidden'
    "
            ))
            .unwrap(),
        true.into()
    );
}

#[test]
fn host_names_shadow_non_enumerable_prototype_names_during_iteration() {
    let mut context = Context::default();
    let prototype = JsObject::with_null_proto();
    prototype
        .define_property_or_throw(
            js_string!("hidden"),
            PropertyDescriptor::builder()
                .value(42)
                .enumerable(false)
                .configurable(true),
            &mut context,
        )
        .unwrap();
    let object =
        JsObject::from_proto_and_data(Some(prototype), NativeExoticObject(EnumeratedNames));
    context
        .register_global_property(js_string!("names"), object, Attribute::all())
        .unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes(
                r"
        let keys=[];for(const key in names) keys.push(key);
        keys.join() === 'hidden' && JSON.stringify(names) === JSON.stringify({hidden:42}) &&
        Object.values(names).length === 0 && Object.keys(Object.assign({},names)).length === 0
    "
            ))
            .unwrap(),
        true.into()
    );
}

#[test]
fn explicit_false_host_enumeration_suppresses_own_and_inherited_names() {
    let mut context = Context::default();
    let prototype = JsObject::with_null_proto();
    prototype
        .create_data_property(js_string!("suppressed"), 42, &mut context)
        .unwrap();
    let object =
        JsObject::from_proto_and_data(Some(prototype), NativeExoticObject(EnumeratedNames));
    object
        .create_data_property(js_string!("suppressed"), 7, &mut context)
        .unwrap();
    context
        .register_global_property(js_string!("names"), object, Attribute::all())
        .unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes(
                r"
        let keys=[];for(const key in names) keys.push(key);
        let ok=keys.join() === 'hidden' && Object.keys(names).join() === 'hidden';
        delete names.suppressed;
        keys=[];for(const key in names) keys.push(key);
        ok && keys.join() === 'hidden,suppressed'
    "
            ))
            .unwrap(),
        true.into()
    );
}

#[test]
fn host_enumeration_can_distinguish_own_keys_from_iteration() {
    let mut context = Context::default();
    let object = JsObject::from_proto_and_data(None, NativeExoticObject(EnumeratedNames));
    object
        .create_data_property(js_string!("reserved"), 7, &mut context)
        .unwrap();
    context
        .register_global_property(js_string!("names"), object, Attribute::all())
        .unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes(
                r"
        let keys=[];for(const key in names) keys.push(key);
        keys.join() === 'hidden,reserved' && Object.keys(names).join() === 'hidden' &&
        JSON.stringify(names) === '{}' && Object.values(names).join() === '7'
    "
            ))
            .unwrap(),
        true.into()
    );
}
