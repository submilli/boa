use super::*;
use boa_engine::{NativeFunction, Source, js_string};
fn context() -> Context {
    let mut context = Context::default();
    context
        .register_global_builtin_callable(
            js_string!("clone"),
            1,
            NativeFunction::from_fn_ptr(|_, args, cx| {
                JsValueStore::try_from_js(
                    args.first().unwrap_or(&JsValue::undefined()),
                    cx,
                    vec![],
                )?
                .try_into_js(cx)
            }),
        )
        .unwrap();
    context
}
#[test]
fn cyclic_graph_releases_its_arena_and_preserves_identity() {
    let mut cx = context();
    let value = cx
        .eval(Source::from_bytes(
            "let a={};let b={a};a.self=a;a.b=b;a.alias=b;a",
        ))
        .unwrap();
    let store = JsValueStore::try_from_js(&value, &mut cx, vec![]).unwrap();
    let weak = Arc::downgrade(&store.graph);
    let clone = store.try_into_js(&mut cx).unwrap().as_object().unwrap();
    assert_eq!(
        clone.get(js_string!("self"), &mut cx).unwrap(),
        JsValue::from(clone.clone())
    );
    let b = clone.get(js_string!("b"), &mut cx).unwrap();
    assert_eq!(b, clone.get(js_string!("alias"), &mut cx).unwrap());
    assert_eq!(
        b.as_object()
            .unwrap()
            .get(js_string!("a"), &mut cx)
            .unwrap(),
        JsValue::from(clone)
    );
    drop(store);
    assert!(weak.upgrade().is_none());
}
#[test]
fn depth_sparse_arrays_and_reentrant_clones_are_bounded() {
    let mut cx = context();
    for code in [
        "let a={};for(let i=0;i<20000;i++)a={child:a};clone(a)",
        "clone(new Array(0xffffffff))",
        "function chain(n){let a={get child(){return clone(chain(40))}};while(n--)a={child:a};return a}clone(chain(40))",
    ] {
        assert!(cx.eval(Source::from_bytes(code)).is_err());
        assert!(
            cx.eval(Source::from_bytes("clone({ok:1}).ok===1"))
                .unwrap()
                .to_boolean()
        );
    }
}
#[test]
fn clone_amplification_is_bounded() {
    let mut cx = context();
    assert!(
        cx.eval(Source::from_bytes(
            "let s='x'.repeat(1024*1024);clone(Array(100).fill(s))"
        ))
        .is_err()
    );
}

#[test]
fn bigint_and_transfer_payloads_obey_byte_budget() {
    let mut cx = context();
    assert!(
        cx.eval(Source::from_bytes("clone(Array(200).fill(1n << 1000000n))"))
            .is_err()
    );
    let buffer = cx
        .eval(Source::from_bytes("new ArrayBuffer(17*1024*1024)"))
        .unwrap();
    assert!(JsValueStore::try_from_js(&buffer, &mut cx, vec![buffer.clone()]).is_err());
    let buffer =
        boa_engine::object::builtins::JsArrayBuffer::from_object(buffer.as_object().unwrap())
            .unwrap();
    assert_eq!(buffer.byte_length(), 17 * 1024 * 1024);
}
#[test]
fn transferred_buffer_aliases_share_one_node() {
    let mut cx = context();
    let value=cx.eval(Source::from_bytes("var transferred=new ArrayBuffer(8);[transferred,transferred,new Uint8Array(transferred,2,2)]")).unwrap();
    let buffer = cx.eval(Source::from_bytes("transferred")).unwrap();
    let store = JsValueStore::try_from_js(&value, &mut cx, vec![buffer]).unwrap();
    let clone = store.try_into_js(&mut cx).unwrap().as_object().unwrap();
    let view = clone.get(2, &mut cx).unwrap().as_object().unwrap();
    assert_eq!(
        view.get(js_string!("byteOffset"), &mut cx)
            .unwrap()
            .as_number(),
        Some(2.0)
    );
    assert_eq!(
        view.get(js_string!("length"), &mut cx).unwrap().as_number(),
        Some(2.0)
    );
    let first = clone.get(0, &mut cx).unwrap();
    assert_eq!(first, clone.get(1, &mut cx).unwrap());
    assert_eq!(
        first,
        clone
            .get(2, &mut cx)
            .unwrap()
            .as_object()
            .unwrap()
            .get(js_string!("buffer"), &mut cx)
            .unwrap()
    );
}

#[test]
fn storage_copies_enumerable_fields_without_prototype_callbacks() {
    let mut cx = context();
    let value = cx
        .eval(Source::from_bytes(
            r#"
        let source = {a:{n:1}, get b(){return this.a}};
        Object.defineProperty(source, 'hidden', {get(){throw 1}});
        source[Symbol('ignored')] = () => {};
        Object.defineProperty(source, '__proto__', {value:{safe:1}, enumerable:true});
        source
    "#,
        ))
        .unwrap();
    let store = JsValueStore::for_storage(&value, &mut cx).unwrap();
    assert!(store.retained_bytes() > 0);
    cx.eval(Source::from_bytes(
        "Object.defineProperty(Object.prototype,'a',{set(){throw 1}, configurable:true})",
    ))
    .unwrap();
    let clone = store.try_into_js(&mut cx).unwrap().as_object().unwrap();
    assert_eq!(
        clone.get(js_string!("a"), &mut cx).unwrap(),
        clone.get(js_string!("b"), &mut cx).unwrap()
    );
    assert!(
        clone
            .has_own_property(js_string!("__proto__"), &mut cx)
            .unwrap()
    );
    assert!(
        !clone
            .has_own_property(js_string!("hidden"), &mut cx)
            .unwrap()
    );
}

#[test]
fn storage_rejects_unserializable_objects_and_preserves_getter_exceptions() {
    let mut cx = context();
    for script in [
        "()=>{}",
        "Symbol()",
        "new Proxy({}, {})",
        "new WeakMap()",
        "new SharedArrayBuffer(8)",
    ] {
        let value = cx.eval(Source::from_bytes(script)).unwrap();
        let error = JsValueStore::for_storage(&value, &mut cx).unwrap_err();
        assert!(StorageDataCloneError::is_error(&error), "{script}");
    }
    let value = cx
        .eval(Source::from_bytes(
            "({get field(){throw new Error('DataCloneError: from getter')}})",
        ))
        .unwrap();
    let error = JsValueStore::for_storage(&value, &mut cx).unwrap_err();
    assert!(!StorageDataCloneError::is_error(&error));
}

#[test]
fn arrays_views_and_builtin_snapshots_keep_their_shape() {
    let mut cx = context();
    for script in [
        "let a=Array(4);a[1]={n:1};a.extra=a[1];let b=clone(a);return b.length===4 && !(0 in b) && !(3 in b) && b.extra===b[1]",
        "let b=new ArrayBuffer(8);let v=new Uint8Array(b,2,2);let c=clone([b,v,new DataView(b,3,1)]);return c[1].buffer===c[0] && c[1].byteOffset===2 && c[1].length===2 && c[2].buffer===c[0] && c[2].byteOffset===3 && c[2].byteLength===1",
        "let r=/a/gi;Object.defineProperty(r,'global',{get(){throw 1}});return clone(r).flags==='gi'",
        "let m=new Map();let first={get n(){m.clear();return 1}};m.set(1,first);m.set(2,2);let c=clone(m);return c.size===2 && c.get(1).n===1 && c.get(2)===2",
    ] {
        assert!(
            cx.eval(Source::from_bytes(&format!("(()=>{{{script}}})()")))
                .unwrap()
                .to_boolean(),
            "{script}"
        );
    }
}

#[test]
fn ignored_keys_do_not_reserve_retained_field_capacity() {
    let mut cx = context();
    let value = cx
        .eval(Source::from_bytes(
            "Object.defineProperties({}, {one:{value:1},two:{value:2},three:{value:3}})",
        ))
        .unwrap();
    let store = JsValueStore::for_storage(&value, &mut cx).unwrap();
    let ValueStoreInner::Object(fields) = &store.graph[store.root] else {
        panic!("ordinary object");
    };
    assert_eq!(fields.capacity(), 0);
    assert!(store.retained_bytes() >= store.graph.capacity() * size_of::<ValueStoreInner>());
}

#[test]
fn storage_rejects_invalid_views_before_reconstruction() {
    let mut cx = context();
    for code in [
        "let b=new ArrayBuffer(8,{maxByteLength:16});let v=new Uint8Array(b,4,4);b.resize(2);v",
        "let b=new ArrayBuffer(8,{maxByteLength:16});let v=new DataView(b,4,4);b.resize(2);v",
    ] {
        let value = cx
            .eval(Source::from_bytes(&format!("(()=>{{{code};return v}})()")))
            .unwrap();
        assert!(StorageDataCloneError::is_error(
            &JsValueStore::for_storage(&value, &mut cx).unwrap_err()
        ));
    }
}

#[test]
fn detached_data_view_is_a_storage_rejection() {
    use boa_engine::object::builtins::JsArrayBuffer;
    let mut cx = context();
    let value = cx
        .eval(Source::from_bytes(
            "var detachedBuffer=new ArrayBuffer(8);new DataView(detachedBuffer)",
        ))
        .unwrap();
    let buffer = cx
        .eval(Source::from_bytes("detachedBuffer"))
        .unwrap()
        .as_object()
        .unwrap();
    JsArrayBuffer::from_object(buffer)
        .unwrap()
        .detach(&JsValue::undefined())
        .unwrap();
    assert!(StorageDataCloneError::is_error(
        &JsValueStore::for_storage(&value, &mut cx).unwrap_err()
    ));
}

#[test]
fn native_collection_snapshots_skip_deleted_entries() {
    use boa_engine::object::builtins::{JsMap, JsSet};
    let mut cx = context();
    let object = cx
        .eval(Source::from_bytes(
            "var nativeMap=new Map([[1,1],[2,2],[3,3]]);nativeMap",
        ))
        .unwrap()
        .as_object()
        .unwrap();
    let map = JsMap::from_object(object).unwrap();
    let mut keys = Vec::new();
    map.for_each_native(|key, _| {
        keys.push(key.as_number().unwrap());
        cx.eval(Source::from_bytes("nativeMap.delete(2)"))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(keys, [1.0, 3.0]);
    let object = cx
        .eval(Source::from_bytes(
            "var nativeSet=new Set([1,2,3]);nativeSet",
        ))
        .unwrap()
        .as_object()
        .unwrap();
    let set = JsSet::from_object(object).unwrap();
    let mut values = Vec::new();
    set.for_each_native(|value| {
        values.push(value.as_number().unwrap());
        cx.eval(Source::from_bytes("nativeSet.delete(2)"))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(values, [1.0, 3.0]);
}

#[test]
fn transfer_validation_and_serialization_failures_do_not_detach() {
    let mut cx = context();
    for source in [
        "var buffer = new ArrayBuffer(8); ({buffer, invalid:()=>{}})",
        "var buffer = new ArrayBuffer(8); ({buffer, get failure(){throw 42}})",
    ] {
        let value = cx.eval(Source::from_bytes(source)).unwrap();
        let buffer = cx.eval(Source::from_bytes("buffer")).unwrap();
        assert!(JsValueStore::try_from_js(&value, &mut cx, vec![buffer]).is_err());
        assert_eq!(
            cx.eval(Source::from_bytes("buffer.byteLength")).unwrap(),
            8.into()
        );
    }
    let buffer = cx.eval(Source::from_bytes("new ArrayBuffer(8)")).unwrap();
    assert!(
        JsValueStore::try_from_js(&buffer, &mut cx, vec![buffer.clone(), buffer.clone()]).is_err()
    );
    let wrapped =
        boa_engine::object::builtins::JsArrayBuffer::from_object(buffer.as_object().unwrap())
            .unwrap();
    assert_eq!(wrapped.byte_length(), 8);
}

#[test]
fn unused_transfers_detach_and_dataview_aliases_survive() {
    let mut cx = context();
    let value = cx
        .eval(Source::from_bytes(
            "var buffer=new ArrayBuffer(8); [buffer,new DataView(buffer,2,3)]",
        ))
        .unwrap();
    let buffer = cx.eval(Source::from_bytes("buffer")).unwrap();
    let store = JsValueStore::try_from_js(&value, &mut cx, vec![buffer]).unwrap();
    let clone = store.try_into_js(&mut cx).unwrap().as_object().unwrap();
    let view = clone.get(1, &mut cx).unwrap().as_object().unwrap();
    assert_eq!(
        view.get(js_string!("buffer"), &mut cx).unwrap(),
        clone.get(0, &mut cx).unwrap()
    );
    assert_eq!(
        view.get(js_string!("byteOffset"), &mut cx).unwrap(),
        2.into()
    );
    assert_eq!(
        view.get(js_string!("byteLength"), &mut cx).unwrap(),
        3.into()
    );
    let unused = cx
        .eval(Source::from_bytes("var unused=new ArrayBuffer(8);unused"))
        .unwrap();
    JsValueStore::try_from_js(&JsValue::null(), &mut cx, vec![unused]).unwrap();
    assert_eq!(
        cx.eval(Source::from_bytes("unused.byteLength")).unwrap(),
        0.into()
    );
}

#[test]
fn boxed_values_error_causes_and_resizable_views_preserve_internal_slots() {
    let mut cx = context();
    for code in [
        "const values=[new Number(3),new Boolean(true),new String('a'),Object(4n)];return values.every(v=>{const c=clone(v);return c!==v && Object.prototype.toString.call(c)===Object.prototype.toString.call(v) && c.valueOf()===v.valueOf()})",
        "const e=new TypeError('bad',{cause:{x:1}});e.extra=3;const c=clone(e);return c instanceof TypeError && c.message==='bad' && c.cause.x===1 && !('extra' in c) && typeof c.stack==='string'",
        "const e=new Error('self');e.cause=e;const c=clone(e);return c!==e && c.cause===c",
        "const b=new ArrayBuffer(8,{maxByteLength:16});const c=clone([b,new Uint8Array(b,2),new Uint8Array(b,2,2),new DataView(b,2),new DataView(b,2,2)]);c[0].resize(12);return c[0].resizable && c[0].maxByteLength===16 && c[1].length===10 && c[2].length===2 && c[3].byteLength===10 && c[4].byteLength===2 && c[1].buffer===c[0]",
    ] {
        assert!(
            cx.eval(Source::from_bytes(&format!("(()=>{{{code}}})()")))
                .unwrap()
                .to_boolean(),
            "{code}"
        );
    }
}

#[test]
fn transfer_admission_counts_retained_capacity_before_detaching() {
    use boa_engine::builtins::array_buffer::{AlignedVec, ArrayBuffer};
    use boa_engine::object::builtins::JsArrayBuffer;
    let mut cx = context();
    let mut data = AlignedVec::with_capacity(0, 64);
    data.resize(32, 0);
    let buffer = JsArrayBuffer::from_byte_block(data, &mut cx)
        .unwrap()
        .with_max_byte_length(64);
    let object: boa_engine::JsObject = buffer.clone().into();
    object
        .downcast_mut::<ArrayBuffer>()
        .unwrap()
        .resize(1)
        .unwrap();
    let mut seen = from::SeenMap::new(false);
    seen.reserve_transfers(std::slice::from_ref(&object))
        .unwrap();
    // Leave enough admission for the visible byte, but not the retained allocation.
    // A tiny allocation tests the boundary without exhausting process memory.
    seen.charge(16 * 1024 * 1024 - 8).unwrap();
    assert!(seen.finish_transfers(&[object]).is_err());
    assert_eq!(buffer.byte_length(), 1);
}

#[test]
fn transferred_views_keep_captured_slots_after_getter_shrink() {
    let mut cx = context();
    let value = cx.eval(Source::from_bytes("var backing=new ArrayBuffer(8,{maxByteLength:16});({typed:new Uint8Array(backing,4,4),view:new DataView(backing,4,4),get after(){backing.resize(2);return 1}})")).unwrap();
    let buffer = cx.eval(Source::from_bytes("backing")).unwrap();
    let store = JsValueStore::try_from_js(&value, &mut cx, vec![buffer]).unwrap();
    let value = store.try_into_js(&mut cx).unwrap();
    cx.register_global_property(
        js_string!("restored"),
        value,
        boa_engine::property::Attribute::all(),
    )
    .unwrap();
    assert!(
        cx.eval(Source::from_bytes("restored.typed.length===0"))
            .unwrap()
            .to_boolean()
    );
    assert!(cx.eval(Source::from_bytes("(()=>{try{restored.view.byteLength;return false}catch(e){return e instanceof TypeError}})()")).unwrap().to_boolean());
    assert!(cx.eval(Source::from_bytes("restored.typed.buffer.resize(8);restored.typed.byteOffset===4 && restored.typed.length===4 && restored.view.byteOffset===4 && restored.view.byteLength===4")).unwrap().to_boolean());
}
