//! Exercise the public embedder API from outside the engine crate.
// Integration coverage uses only a subset of the engine package dependencies.
#![allow(unused_crate_dependencies)]
use boa_engine::object::native_exotic::{NativeExotic, NativeExoticObject, ordinary};
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
